#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Check the relative links in this repository's Markdown before it is published.

    python scripts/checks/oss-links.py .                      # this checkout
    python scripts/checks/oss-links.py <dir> [--repo <checkout>] [--anchors]

A page must not link to a path the repository does not carry. Every relative
link and image in every `*.md` file under <dir> (normally `.`, the checkout
itself) has to resolve to a file or directory inside <dir>. A link that does
not is reported as:

- `private`  only when <dir> is a copy of part of a checkout: the target
             exists in the full checkout (`--repo`, default: the repository
             this script lives in) but not in <dir>, so a page points into a
             path the copy leaves out;
- `dead`     the target does not exist: a broken link;
- `outside`  the link climbs out of <dir>.

What counts as a link: inline links and images `[t](dest)`, `![a](dest "t")`,
`<dest>` forms, reference definitions `[id]: dest`, and HTML `href`/`src`
attributes. Absolute URLs (`https:`, `mailto:`, any scheme) and bare
`#anchors` are not paths and are skipped. A destination is URL-decoded
(`%20`), and its `?query` and `#fragment` are dropped before it is resolved;
a leading `/` resolves from <dir>, as GitHub does from the repository root. Fenced code
blocks (``` and ~~~) and inline code spans are not prose and are skipped.

`--anchors` also checks each `#fragment` into a Markdown file against that
file's headings (GitHub's slug rules) and explicit `id`/`name` anchors. Those
are reported as `anchor` warnings and never change the exit status: slugs
are a heuristic, a broken anchor still lands on the right page.

Markdown under `third-party/` is vendored upstream text whose relative links
name upstream's tree; it is skipped unless `--vendored` is given.

Exit status 1 when any `private`, `dead` or `outside` link is found.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import unquote

# Build output and VCS metadata inside a checkout after a build; never content.
SKIP_DIRS = {".git", "target", "node_modules", ".gradle"}
# By path from the root: installed into a checkout, never committed, or another
# repository's pages (docs/ is the koru-docs submodule; oss-scan.py has the same
# list, and says why).
SKIP_PATHS = {"third-party/crates", ".agents/skills", ".claude/skills", "docs"}
# Vendored upstream Markdown keeps upstream's relative links, which name
# upstream's tree (sherpa-onnx's README links its flutter-examples/). Its
# links are upstream's to fix; links *into* it from our pages are still checked.
VENDORED = ("third-party/",)

_FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})")
_CODE_SPAN = re.compile(r"(`+)(?:.+?)\1")
# [text](dest) and ![alt](dest); dest may be <...> and may carry a title.
_INLINE = re.compile(r"!?\[(?:\\.|[^\[\]\\]|\[(?:\\.|[^\[\]\\])*\])*\]\(\s*(<[^>]*>|[^\s()]*(?:\([^\s()]*\)[^\s()]*)*)(?:\s+(?:\"[^\"]*\"|'[^']*'|\([^)]*\)))?\s*\)")
_REFDEF = re.compile(r"^ {0,3}\[(?:\\.|[^\]\\])+\]:\s*(<[^>]*>|\S+)")
_HTML = re.compile(r"""<(?:a|img|source|link)\b[^>]*?\b(?:href|src)\s*=\s*(?:"([^"]*)"|'([^']*)')""", re.I)
_SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")
_HEADING = re.compile(r"^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$")
_ID_ANCHOR = re.compile(r"""<[^>]*\b(?:id|name)\s*=\s*["']([^"']+)["']""", re.I)


@dataclass(frozen=True)
class Finding:
    kind: str  # private | dead | outside | anchor
    source: str  # path of the Markdown file, relative to the checked directory
    line: int
    dest: str  # the destination as written
    target: str  # what it resolved to

    def __str__(self) -> str:
        return f"{self.source}:{self.line}: [{self.kind}] {self.dest} -> {self.target}"


def prose_lines(text: str):
    """(line number, line) for every line outside fenced code, with inline
    code spans blanked out."""
    fence: str | None = None
    for number, line in enumerate(text.splitlines(), 1):
        m = _FENCE.match(line)
        if fence is None:
            if m:
                fence = m.group(1)[0] * len(m.group(1))
                continue
            yield number, _CODE_SPAN.sub(lambda s: " " * len(s.group()), line)
        elif m and m.group(1).startswith(fence) and not line.strip()[len(m.group(1)):].strip():
            fence = None


def destinations(text: str):
    """(line number, destination) for every link-like destination in prose."""
    for number, line in prose_lines(text):
        for m in _INLINE.finditer(line):
            yield number, m.group(1)
        m = _REFDEF.match(line)
        if m:
            yield number, m.group(1)
        for m in _HTML.finditer(line):
            yield number, m.group(1) if m.group(1) is not None else m.group(2)


def split_dest(dest: str) -> tuple[str, str] | None:
    """(path, fragment) of a relative destination, or None when it is not a
    path at all (a URL with a scheme, an empty or bare-anchor link)."""
    dest = dest.strip()
    if dest.startswith("<") and dest.endswith(">"):
        dest = dest[1:-1].strip()
    if not dest or dest.startswith("#") or _SCHEME.match(dest) or dest.startswith("//"):
        return None
    path, _, fragment = dest.partition("#")
    path = path.split("?", 1)[0]
    if not path:
        return None
    return unquote(path), unquote(fragment)


def slug(heading: str, seen: dict[str, int]) -> str:
    """GitHub's heading anchor: lowercase, punctuation dropped, spaces to
    hyphens, and a `-n` suffix for repeats."""
    text = re.sub(r"<[^>]+>", "", heading)
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)  # links keep their text
    text = text.replace("`", "").strip().lower()
    text = re.sub(r"[^\w\- ]", "", text)
    base = text.replace(" ", "-")
    n = seen.get(base, 0)
    seen[base] = n + 1
    return base if n == 0 else f"{base}-{n}"


def anchors_of(text: str) -> set[str]:
    out: set[str] = set()
    seen: dict[str, int] = {}
    for _, line in prose_lines(text):
        m = _HEADING.match(line)
        if m:
            out.add(slug(m.group(2), seen))
        out.update(a for a in _ID_ANCHOR.findall(line))
    return out


def markdown_files(root: Path) -> list[Path]:
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        here = Path(dirpath).relative_to(root).as_posix()
        dirnames[:] = sorted(
            d for d in dirnames
            if d not in SKIP_DIRS and (d if here == "." else f"{here}/{d}") not in SKIP_PATHS
        )
        for name in sorted(filenames):
            if name.lower().endswith(".md"):
                out.append(Path(dirpath) / name)
    return out


def check(root: Path, repo: Path | None, anchors: bool = False,
          vendored: bool = False) -> list[Finding]:
    root = root.resolve()
    findings: list[Finding] = []
    anchor_cache: dict[Path, set[str]] = {}
    for md in markdown_files(root):
        source = md.relative_to(root).as_posix()
        if not vendored and source.startswith(VENDORED):
            continue
        text = md.read_text(encoding="utf-8", errors="replace")
        for line, dest in destinations(text):
            parts = split_dest(dest)
            if parts is None:
                continue
            path, fragment = parts
            base = root if path.startswith("/") else md.parent
            target = Path(os.path.normpath(base / path.lstrip("/")))
            try:
                rel = target.relative_to(root).as_posix()
            except ValueError:
                findings.append(Finding("outside", source, line, dest, path))
                continue
            if rel == ".":
                rel = ""
            # A link into docs/ can only be checked where the submodule is
            # checked out; a public clone has an empty docs/.
            if (rel == "docs" or rel.startswith("docs/")) and not (root / "docs" / "README.md").exists():
                continue
            if target.exists():
                if anchors and fragment and target.is_file() and target.suffix.lower() == ".md":
                    if target not in anchor_cache:
                        anchor_cache[target] = anchors_of(target.read_text(encoding="utf-8", errors="replace"))
                    if fragment.lower() not in {a.lower() for a in anchor_cache[target]}:
                        findings.append(Finding("anchor", source, line, dest, f"{rel}#{fragment}"))
                continue
            in_repo = repo is not None and (repo / rel).exists()
            findings.append(Finding("private" if in_repo else "dead", source, line, dest, rel))
    return findings


def default_repo() -> Path | None:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"], cwd=Path(__file__).resolve().parent,
            check=True, capture_output=True, text=True,
        ).stdout.strip()
        return Path(out)
    except (OSError, subprocess.CalledProcessError):
        return None


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("root", metavar="dir", type=Path, help="the directory to check, normally .")
    ap.add_argument("--repo", type=Path, help="the full checkout, to tell private from dead links")
    ap.add_argument("--anchors", action="store_true", help="also check #fragments (warnings only)")
    ap.add_argument("--vendored", action="store_true",
                    help="also check links inside vendored upstream docs (third-party/)")
    args = ap.parse_args(argv)
    if not args.root.is_dir():
        print(f"oss-links: {args.root} is not a directory", file=sys.stderr)
        return 2
    repo = args.repo.resolve() if args.repo else default_repo()
    if repo is not None and repo.resolve() == args.root.resolve():
        repo = None
    findings = check(args.root, repo, args.anchors, args.vendored)
    kinds = ("private", "dead", "outside", "anchor")
    for kind in kinds:
        for f in (f for f in findings if f.kind == kind):
            print(f)
    counts = {k: sum(1 for f in findings if f.kind == k) for k in kinds}
    failing = counts["private"] + counts["dead"] + counts["outside"]
    by_file: dict[str, int] = {}
    for f in findings:
        if f.kind != "anchor":
            by_file[f.source] = by_file.get(f.source, 0) + 1
    top = sorted(by_file.items(), key=lambda kv: (-kv[1], kv[0]))[:10]
    if top:
        print("most affected: " + ", ".join(f"{p} ({n})" for p, n in top))
    summary = ", ".join(f"{k} {counts[k]}" for k in kinds if k != "anchor" or args.anchors)
    files = len(markdown_files(args.root.resolve()))
    print(f"oss-links: {failing} broken link(s) in {files} Markdown file(s) ({summary})")
    return 1 if failing else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
