# SPDX-License-Identifier: MPL-2.0

"""Tests for scripts/checks/dco-check.py, against throwaway git repositories.

    python -m unittest discover -s scripts/checks -p "test_dco*.py"
"""

import importlib.util
import os
import shutil
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("dco_check", SCRIPTS / "dco-check.py")
dco = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(dco)


class DcoCheck(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="dco-"))
        self.git("init", "-q", "-b", "main")
        # A signing key or hook on the developer's machine must not leak in.
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", os.devnull)
        self.commit("base", signed=True)
        self.git("checkout", "-q", "-b", "topic")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def git(self, *args, author=("Ada Lovelace", "ada@example.com")):
        env = dict(
            os.environ,
            GIT_AUTHOR_NAME=author[0],
            GIT_AUTHOR_EMAIL=author[1],
            GIT_COMMITTER_NAME=author[0],
            GIT_COMMITTER_EMAIL=author[1],
        )
        return subprocess.run(
            ["git", *args], cwd=self.root, env=env, check=True, capture_output=True, text=True
        ).stdout

    def commit(self, name, signed, author=("Ada Lovelace", "ada@example.com"), trailer=None):
        (self.root / name).write_text(name)
        self.git("add", name, author=author)
        args = ["commit", "-q", "-m", f"Add {name}"]
        if signed:
            args.append("-s")
        if trailer:
            args += ["-m", trailer]
        self.git(*args, author=author)

    def run_check(self):
        cwd = os.getcwd()
        os.chdir(self.root)
        try:
            out = StringIO()
            with redirect_stdout(out):
                code = dco.main(["main..topic"])
            return code, out.getvalue()
        finally:
            os.chdir(cwd)

    def test_signed_commits_pass(self):
        self.commit("a", signed=True)
        self.commit("b", signed=True)
        code, out = self.run_check()
        self.assertEqual(code, 0, out)
        self.assertIn("2 commit(s)", out)

    def test_an_unsigned_commit_fails_and_says_how_to_fix_it(self):
        self.commit("a", signed=True)
        self.commit("b", signed=False)
        code, out = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("no Signed-off-by line", out)
        self.assertIn("git commit --amend -s", out)

    def test_amending_with_sign_off_fixes_it(self):
        # What a contributor does when the check refuses a fork's pull request.
        self.commit("a", signed=False)
        self.assertEqual(self.run_check()[0], 1)
        self.git("commit", "-q", "--amend", "-s", "--no-edit")
        self.assertEqual(self.run_check()[0], 0)

    def test_someone_elses_sign_off_does_not_count(self):
        self.commit("a", signed=False, trailer="Signed-off-by: Bob <bob@example.com>")
        code, out = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("but the author is Ada Lovelace <ada@example.com>", out)

    def test_email_case_does_not_matter(self):
        self.commit("a", signed=False, trailer="Signed-off-by: Ada Lovelace <ADA@Example.com>")
        self.assertEqual(self.run_check()[0], 0)

    def test_unsigned_bot_identity_cannot_bypass_sign_off(self):
        bot = ("dependabot[bot]", "49699333+dependabot[bot]@users.noreply.github.com")
        self.commit("a", signed=False, author=bot)
        self.assertEqual(self.run_check()[0], 1)

    def test_matching_email_with_another_name_is_not_the_authors_sign_off(self):
        self.commit("a", signed=False, trailer="Signed-off-by: Bob <ada@example.com>")
        self.assertEqual(self.run_check()[0], 1)

    def test_sign_off_quoted_in_the_body_is_not_a_trailer(self):
        self.commit("a", signed=False, trailer=(
            "Example:\nSigned-off-by: Ada Lovelace <ada@example.com>\n\n"
            "This paragraph is not a sign-off."
        ))
        self.assertEqual(self.run_check()[0], 1)

    def test_merge_commits_are_skipped(self):
        self.commit("a", signed=True)
        self.git("checkout", "-q", "main")
        self.commit("m", signed=True)
        self.git("checkout", "-q", "topic")
        self.git("merge", "-q", "--no-ff", "--no-edit", "main")
        code, out = self.run_check()
        self.assertEqual(code, 0, out)

    def test_an_unreadable_range_is_an_error_not_a_pass(self):
        cwd = os.getcwd()
        os.chdir(self.root)
        try:
            with redirect_stdout(StringIO()):
                self.assertEqual(dco.main(["nope..topic"]), 2)
        finally:
            os.chdir(cwd)


if __name__ == "__main__":
    unittest.main()
