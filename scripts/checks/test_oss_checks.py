# SPDX-License-Identifier: MPL-2.0

"""Tests for the checks run on this repository before it is published:
scripts/checks/oss-links.py and scripts/checks/oss-scan.py.

    python -m unittest discover -s scripts/checks -p "test_*.py"

Secret-shaped samples are assembled at run time, so this file never carries
a string the scanner (or a push-protection hook) would flag.
"""

import contextlib
import importlib.util
import io
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent


def load(name: str):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses look their module up here
    spec.loader.exec_module(module)
    return module


links = load("oss-links")
scan = load("oss-scan")


def quiet(main, argv: list[str]) -> int:
    """A script's exit status, with its report swallowed."""
    with contextlib.redirect_stdout(io.StringIO()):
        return main(argv)


class Tree:
    def __init__(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="oss-checks-")).resolve()

    def write(self, path: str, text: str = "x\n") -> Path:
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text, encoding="utf-8", newline="\n")
        return file

    def close(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)


class Links(unittest.TestCase):
    def setUp(self) -> None:
        self.repo = Tree()  # the full checkout
        self.export = Tree()  # a copy of part of it, the directory checked
        for t in (self.repo, self.export):
            t.write("guide/public/README.md", "# Public\n\n## Two words\n")
            t.write("guide/public/img/logo.png")
            t.write("guide/public/My Page.md", "# Mine\n")
        self.repo.write("guide/private/PLAN.md")

    def tearDown(self) -> None:
        self.repo.close()
        self.export.close()

    def run_on(self, text: str, anchors: bool = False):
        self.export.write("guide/public/GUIDE.md", text)
        return links.check(self.export.root, self.repo.root, anchors)

    def kinds(self, findings):
        return [(f.kind, f.target) for f in findings]

    def test_good_links_pass(self):
        found = self.run_on(
            "[a](README.md) [b](./README.md#two-words) ![logo](img/logo.png \"Logo\")\n"
            "[c](My%20Page.md) [d](<My Page.md>) [e](/guide/public/README.md)\n"
            "[f](../public/) [g](README.md?plain=1)\n"
            "[ref]: README.md\n"
            '<img src="img/logo.png"> <a href="README.md">x</a>\n'
        )
        self.assertEqual(found, [])

    def test_link_into_a_private_path(self):
        found = self.run_on("See [the plan](../private/PLAN.md).\n")
        self.assertEqual(self.kinds(found), [("private", "guide/private/PLAN.md")])
        self.assertEqual(found[0].line, 1)

    def test_dead_and_outside_links(self):
        found = self.run_on("[gone](nope.md)\n\n![missing](img/none.svg)\n[up](../../../../etc/passwd)\n")
        self.assertEqual([f.kind for f in found], ["dead", "dead", "outside"])
        self.assertEqual([f.line for f in found], [1, 3, 4])

    def test_reference_definition_and_html(self):
        found = self.run_on('[x][p]\n\n[p]: ../private/PLAN.md "Plan"\n<a href="../private/PLAN.md">p</a>\n')
        self.assertEqual([f.kind for f in found], ["private", "private"])

    def test_urls_anchors_and_code_are_skipped(self):
        found = self.run_on(
            "[w](https://example.com/x.md) [m](mailto:a@example.com) [s](#local)\n"
            "`[code](../private/PLAN.md)`\n"
            "```md\n[fenced](../private/PLAN.md)\n```\n"
            "~~~~\n[tilde](nope.md)\n```\nstill code\n~~~~\n"
            "[after](../private/PLAN.md)\n"
        )
        self.assertEqual([(f.kind, f.line) for f in found], [("private", 11)])

    def test_escaped_brackets_in_labels(self):
        found = self.run_on(
            "[\\[MS-ONE\\]: Format] and more\n\n"
            "[\\[MS-ONE\\]: Format]: https://example.com/spec\n"
            "[\\[x\\] y](../private/PLAN.md)\n"
        )
        self.assertEqual([(f.kind, f.line) for f in found], [("private", 4)])

    def test_vendored_upstream_docs_are_skipped_unless_asked(self):
        self.export.write("third-party/lib/README.md", "[ex](./examples/a)\n")
        self.assertEqual(self.run_on("[ok](README.md)\n"), [])
        found = links.check(self.export.root, self.repo.root, vendored=True)
        self.assertEqual(self.kinds(found), [("dead", "third-party/lib/examples/a")])

    def test_anchors_are_warnings_only(self):
        found = self.run_on("[ok](README.md#two-words) [bad](README.md#nowhere)\n", anchors=True)
        self.assertEqual(self.kinds(found), [("anchor", "guide/public/README.md#nowhere")])
        self.assertEqual(self.run_on("[bad](README.md#nowhere)\n", anchors=False), [])

    def test_slugs_follow_github(self):
        seen = {}
        self.assertEqual(links.slug("Part 1 — the `approved` plan!", seen), "part-1--the-approved-plan")
        self.assertEqual(links.slug("Part 1 — the `approved` plan!", seen), "part-1--the-approved-plan-1")

    def test_cli_exit_status(self):
        self.export.write("guide/public/GUIDE.md", "[ok](README.md)\n")
        self.assertEqual(quiet(links.main, [str(self.export.root), "--repo", str(self.repo.root)]), 0)
        self.export.write("guide/public/GUIDE.md", "[no](../private/PLAN.md)\n")
        self.assertEqual(quiet(links.main, [str(self.export.root), "--repo", str(self.repo.root)]), 1)


class Scan(unittest.TestCase):
    def setUp(self) -> None:
        self.tree = Tree()

    def tearDown(self) -> None:
        self.tree.close()

    def found(self, text: str):
        return [(f.category, f.match) for f in scan.scan_text("f.txt", text)]

    def test_ipv4(self):
        self.assertEqual(self.found("host 203.0.114.9 and 45.77.1.2\n"),
                         [("ipv4", "203.0.114.9"), ("ipv4", "45.77.1.2")])
        quiet = (
            "127.0.0.1 0.0.0.0 10.1.2.3 192.168.1.20 172.20.0.1 169.254.1.1 "
            "192.0.2.10 198.51.100.7 203.0.113.99 8.8.8.8 224.0.0.251\n"
            "v1.2.3.4 1.2.3.4.5 libreoffice-24.8.2.1 999.1.1.1 1.2.3\n"
            'version = "4.5.6.7"\n"2.5.29.37={text}"\n/// See [\\[MS-ONE\\] 2.2.1.3].\n'
        )
        self.assertEqual(self.found(quiet), [])

    def test_home_paths(self):
        found = self.found(
            r"C:\Users\jdoe\x /Users/jdoe/x /home/jdoe/x C:\\Users\\jdoe D:/projects/koru D:\projects" "\n"
        )
        self.assertEqual([c for c, _ in found], ["home-path"] * 6)
        quiet = r"C:\Users\<name>\x %USERPROFILE% C:\Users\%USERNAME% /home/runner/x /Users/you $HOME /home/$USER" "\n"
        self.assertEqual(self.found(quiet), [])

    def test_emails(self):
        found = self.found("mail jdoe@corp.io or ops@koru.do\n")
        self.assertEqual(found, [("email", "jdoe@corp.io"), ("email", "ops@koru.do")])
        quiet = (
            "sliechti@gmail.com security@koru.do noreply@anthropic.com a@example.com "
            "b@mail.example.org 1+x@users.noreply.github.com git@github.com:x/y c@site.test\n"
        )
        self.assertEqual(self.found(quiet), [])

    def test_secrets(self):
        samples = {
            "pem-private-key": "-----BEGIN " + "RSA PRIVATE KEY-----",
            "age-secret-key": "AGE-SECRET-KEY-" + "1" + "Q" * 58,
            "sops-value": "ENC[" + "AES256_GCM,data:abc123==",
            "github-token": "gh" + "p_" + "a" * 36,
            "aws-access-key": "AK" + "IA" + "B" * 16,
            "google-oauth-secret": "GOC" + "SPX-" + "c" * 28,
            "google-client-id": "123456789-" + "d" * 32 + ".apps.googleusercontent.com",
            "posthog-key": "ph" + "c_" + "e" * 40,
        }
        for name, sample in samples.items():
            with self.subTest(name):
                found = scan.scan_text("f.txt", f"x = '{sample}'\n")
                self.assertEqual([(f.category, f.detail) for f in found], [("secret", name)])
        public = "-----BEGIN " + "PUBLIC KEY-----\n"
        self.assertEqual(self.found(public), [])

    def test_sops_metadata_blocks(self):
        json_doc = '{\n  "data": 1,\n  "sops": {\n    "age": []\n  }\n}\n'
        yaml_doc = "data: 1\nsops:\n    lastmodified: x\n"
        for doc in (json_doc, yaml_doc):
            found = scan.scan_text("f", doc)
            self.assertEqual([f.detail for f in found], ["sops-metadata"])

    def test_cto_phrase(self):
        phrase = "the " + "CTO"
        self.assertEqual(self.found(f"Ask {phrase}. The CTO's call.\n"),
                         [("cto", phrase), ("cto", "The CTO's")])
        self.assertEqual(self.found("CTOs and a CTO\n"), [])

    def test_exceptions_files_and_cli(self):
        t = self.tree
        t.write("guide/a.md", "jdoe@corp.io\nother@corp.io\n")
        t.write("crates/x/src/tests.rs", 'let p = "/home/jdoe/x";\n')
        t.write("assets/font.ttf", "binary\0jdoe@corp.io")
        t.write("target/debug/out.txt", "jdoe@corp.io\n")
        result = scan.scan(t.root)
        self.assertEqual(sorted((f.category, f.path) for f in result.findings), [
            ("email", "guide/a.md"), ("email", "guide/a.md"), ("home-path", "crates/x/src/tests.rs"),
        ])
        self.assertEqual(result.skipped, 1)
        self.assertEqual(quiet(scan.main, [str(t.root), "--quiet"]), 1)

        t.write(scan.ALLOW_FILE,
                "# reviewed\n"
                "guide/*.md email jdoe@corp.io # a sample address\n"
                "guide/*.md email other@corp.io # another\n"
                "crates/**/tests.rs home-path * # fixtures\n")
        result = scan.scan(t.root)
        self.assertEqual((result.findings, result.errors, result.excepted), ([], [], 3))
        self.assertEqual(quiet(scan.main, [str(t.root), "--quiet"]), 0)

        t.write(scan.ALLOW_FILE,
                "guide/*.md email * # all\n"
                "crates/**/tests.rs home-path * # fixtures\n"
                "nowhere/** cto * # stale\n"
                "guide/a.md email\n")
        result = scan.scan(t.root)
        self.assertEqual(result.findings, [])
        self.assertEqual(len(result.errors), 2, result.errors)  # malformed + stale
        self.assertEqual(quiet(scan.main, [str(t.root), "--quiet"]), 1)

    def test_glob(self):
        self.assertTrue(scan.glob_match("crates/**/tests.rs", "crates/a/b/tests.rs"))
        self.assertTrue(scan.glob_match("crates/**/tests.rs", "crates/tests.rs"))
        self.assertTrue(scan.glob_match("guide/*.md", "guide/a.md"))
        self.assertFalse(scan.glob_match("guide/*.md", "guide/x/a.md"))

    def test_the_scripts_scan_clean(self):
        # The scanner and these tests must not trip the scanner themselves.
        # test_oss_checks.py carries samples on purpose; oss.scan-allow excepts it.
        for name in ("oss-scan.py", "oss-links.py"):
            text = (SCRIPTS / name).read_text(encoding="utf-8")
            self.assertEqual(scan.scan_text(name, text), [], name)


if __name__ == "__main__":
    unittest.main()
