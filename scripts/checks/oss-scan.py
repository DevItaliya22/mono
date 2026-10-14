#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Scan this repository for content that must not be published.

    python scripts/checks/oss-scan.py . [--allow <file>] [--quiet]

The checkout is scanned for what must never be published in it; CI runs this
on every push and pull request. Every text file under the directory given
(normally `.`) is read (binary files, `.git`, build output, fetched
third-party code and the private docs/ submodule are skipped) and each match
is reported under one category:

- `ipv4`       an IPv4 address, except loopback, 0.0.0.0, broadcast, link-local,
               the RFC 1918 private ranges, the RFC 5737 documentation ranges
               and the well-known public resolvers (1.1.1.1, 8.8.8.8, ...). A
               dotted run with more than four parts, one that follows a `v` or
               a letter, an X.509 OID, one followed by `=`, or one on a line
               that says `version`/`ref`/`tag`/`release`/`oid` is a version or
               an OID, not an address.
- `home-path`  a personal path: `C:\\Users\\<name>`, `/Users/<name>`,
               `/home/<name>` with a real name (placeholders such as `<name>`,
               `%USERNAME%`, `$USER`, `you`, `runner` are fine), and the
               maintainer's checkout root (`projects` directly under drive D),
               in either slash form.
- `email`      an address other than the approved ones: the author identity
               `sliechti@gmail.com`, `security@koru.do`, `noreply@anthropic.com`
               (commit trailers), anything at example.com/.org/.net or a
               `.example`/`.test`/`.invalid` domain, GitHub's
               users.noreply.github.com, and `git@` SSH remotes.
- `secret`     private-key and token shapes: PEM private-key headers, age
               secret keys, SOPS-encrypted values, GitHub, AWS, Google
               (API key, OAuth client secret and client ID), Slack and Stripe
               tokens, and PostHog project keys.
- `cto`        the phrase "the" followed by "CTO": internal process language
               that a public page should not carry.

Exceptions are reviewed: `scripts/checks/oss.scan-allow` under the scanned
directory (or `--allow <file>`), one per line,

    <path glob> <category> <match> # reason

where `<path glob>` is a path relative to the scanned directory (`*` and `**` work, `/`
separated), `<category>` one of the above, and `<match>` the matched text
exactly, or `*` for any match of that category in those paths. A reason is
required, and an entry that matches nothing is itself reported, so the list
cannot rot.

Prints every unexcepted finding, a count per category with the most affected
paths, and exits 1 when anything unexcepted (or a stale exception) remains.
"""

from __future__ import annotations

import argparse
import ipaddress
import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

ALLOW_FILE = "scripts/checks/oss.scan-allow"
CATEGORIES = ("ipv4", "home-path", "email", "secret", "cto")
SKIP_DIRS = {".git", "target", "node_modules", ".gradle", "build", "dist"}
# By path from the root, not by name: what is installed into a checkout but
# never committed, so the published repository never holds it. The crates fetched from
# crates.io by scripts/setup/fetch-third-party.* (only their patches are published),
# and the agent skills scripts/setup/setup-skills.sh installs from koruworks/skills.
# A directory name as common as `crates` or `skills` cannot be skipped by name.
# And docs/, the koru-docs submodule: another, private repository, whose pages
# this repository never publishes.
SKIP_PATHS = {"third-party/crates", ".agents/skills", ".claude/skills", "docs"}
MAX_BYTES = 8 * 1024 * 1024

# --- ipv4 --------------------------------------------------------------------
_IPV4 = re.compile(r"(?<![\w.-])(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(?![\w-]|\.\d)")
_ALLOWED_NETS = [
    ipaddress.ip_network(n)
    for n in (
        "0.0.0.0/32", "127.0.0.0/8", "255.255.255.255/32", "169.254.0.0/16",
        "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16",
        "192.0.2.0/24", "198.51.100.0/24", "203.0.113.0/24",  # RFC 5737
        "224.0.0.0/4",  # multicast (mDNS and friends)
    )
]
# Well-known public addresses: example.com's, and the public DNS resolvers.
_PUBLIC_RESOLVERS = {"93.184.216.34", "1.1.1.1", "1.0.0.1", "8.8.8.8", "8.8.4.4", "9.9.9.9", "149.112.112.112"}
# X.509/PKIX object identifiers, and the 1.2.3.x placeholder version.
_OID_PREFIXES = ("2.5.29.", "2.5.4.", "1.2.840.", "1.3.6.", "1.2.3.")
# A line about a version, or a Microsoft open-spec section ("[MS-ONE] 2.2.1.3").
_VERSION_CONTEXT = re.compile(r"(?i)\b(?:version|ver|rev|ref|tag|release|oid|section)\b|\bMS-[A-Z]+\b|§")

# --- home-path ---------------------------------------------------------------
_HOME = re.compile(
    r"(?:\b[A-Za-z]:(?:\\\\|\\|/)Users(?:\\\\|\\|/)|(?<![\w.])/Users/|(?<![\w.])/home/)"
    r"(?P<name>[^\\/\s\"'`<>()\[\]{}:;,|*?]+)"
)
_PROJECTS = re.compile(r"\b[Dd]:(?:\\\\|\\|/)projects\b")
_PLACEHOLDER_USERS = {
    "user", "username", "you", "your", "yourname", "your-name", "your_name", "name",
    "me", "runner", "example", "alice", "bob", "dev", "developer", "public", "default",
    "all users", "foo", "someone", "x", "ubuntu", "admin", "builder", "jane", "john",
    "shared", "koru", "test", "tester", "sam", "ada", "private", "first",
}

# --- email -------------------------------------------------------------------
_EMAIL = re.compile(r"(?<![\w.+-])([A-Za-z0-9._%+-]+)@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,})\b")
_APPROVED_EMAILS = {"sliechti@gmail.com", "security@koru.do", "noreply@anthropic.com"}
_EXAMPLE_DOMAINS = ("example.com", "example.org", "example.net", "users.noreply.github.com")
_EXAMPLE_TLDS = (".example", ".test", ".invalid", ".localhost")

# --- secret ------------------------------------------------------------------
# No pattern below can match its own source text: each needs a run of key
# characters right where the source has a `[` or `{`.
SECRETS = [
    ("pem-private-key", re.compile(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----")),
    ("age-secret-key", re.compile(r"AGE-SECRET-KEY-1[0-9A-Z]{58}")),
    ("sops-value", re.compile(r"ENC\[AES256_GCM,data:[A-Za-z0-9+/=]+")),
    ("sops-metadata", re.compile(r"[\"']?sops[\"']?\s*:\s*\{?\s*\n?\s*[\"']?(?:kms|age|pgp|mac|lastmodified)[\"']?\s*:")),
    ("github-token", re.compile(r"\bgh[pousr]_[A-Za-z0-9]{36,}\b")),
    ("github-pat", re.compile(r"\bgithub_pat_[A-Za-z0-9_]{22,}\b")),
    ("aws-access-key", re.compile(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b")),
    ("google-api-key", re.compile(r"\bAIza[0-9A-Za-z_-]{35}\b")),
    ("google-oauth-secret", re.compile(r"\bGOCSPX-[A-Za-z0-9_-]{20,}")),
    ("google-client-id", re.compile(r"\b\d{6,}-[a-z0-9]{32}\.apps\.googleusercontent\.com\b")),
    ("slack-token", re.compile(r"\bxox[abposr]-[A-Za-z0-9-]{10,}")),
    ("stripe-key", re.compile(r"\b[rs]k_live_[A-Za-z0-9]{16,}")),
    ("posthog-key", re.compile(r"\bphc_[A-Za-z0-9]{20,}")),
]

_SOPS_JSON = re.compile(r"\"sops\"\s*:\s*\{\s*\"(?:kms|age|pgp|mac|lastmodified)\"")
_SOPS_YAML = re.compile(r"(?m)^sops:[ \t]*\r?\n(?:[ \t]+.*\r?\n){0,40}?[ \t]+(?:kms|age|pgp|mac|lastmodified):")

# --- cto ---------------------------------------------------------------------
_CTO = re.compile(r"\b[Tt]he\s+CTO(?:'s)?\b")


@dataclass(frozen=True)
class Finding:
    category: str
    path: str  # relative to the scanned directory, posix
    line: int
    match: str
    detail: str = ""

    def __str__(self) -> str:
        what = f" ({self.detail})" if self.detail else ""
        return f"{self.path}:{self.line}: [{self.category}{what}] {self.match}"


def _ipv4(line: str):
    for m in _IPV4.finditer(line):
        octets = [int(g) for g in m.groups()]
        if any(o > 255 for o in octets) or any(len(g) > 1 and g.startswith("0") for g in m.groups()):
            continue
        text = m.group()
        if text in _PUBLIC_RESOLVERS or text.startswith(_OID_PREFIXES):
            continue
        # An OID assignment (`2.5.29.37={text}...`) or a four-part version on
        # a line that says so (`$Ref = "24.8.2.1"`, `version: 1.2.3.4`).
        if line[m.end() : m.end() + 1] == "=" or _VERSION_CONTEXT.search(line):
            continue
        addr = ipaddress.ip_address(text)
        if any(addr in n for n in _ALLOWED_NETS):
            continue
        yield text, ""


def _home(line: str):
    for m in _HOME.finditer(line):
        name = m.group("name")
        bare = name.strip().lower().split("@", 1)[0]
        if bare in _PLACEHOLDER_USERS or name[0] in "<%$.{" or name.isupper():
            continue
        yield m.group(), ""
    for m in _PROJECTS.finditer(line):
        yield m.group(), ""


def _email(line: str):
    for m in _EMAIL.finditer(line):
        local, domain = m.group(1), m.group(2).lower()
        address = f"{local}@{domain}".lower()
        if address in _APPROVED_EMAILS or local.lower() == "git":
            continue
        if domain in _EXAMPLE_DOMAINS or any(domain.endswith("." + d) for d in _EXAMPLE_DOMAINS):
            continue
        if domain.endswith(_EXAMPLE_TLDS) or domain in ("example", "localhost"):
            continue
        yield m.group(), ""


def _secret(line: str):
    for name, pattern in SECRETS:
        for m in pattern.finditer(line):
            yield m.group(), name


def _cto(line: str):
    for m in _CTO.finditer(line):
        yield m.group(), ""


DETECTORS = {"ipv4": _ipv4, "home-path": _home, "email": _email, "secret": _secret, "cto": _cto}


def scan_text(path: str, text: str) -> list[Finding]:
    out = []
    for number, line in enumerate(text.splitlines(), 1):
        for category, detect in DETECTORS.items():
            for match, detail in detect(line):
                out.append(Finding(category, path, number, match, detail))
    # SOPS metadata spans lines; the per-line pass sees its key only when the
    # file is on one line. Look at the whole text once for the block form.
    if not any(f.detail == "sops-metadata" for f in out):
        m = _SOPS_JSON.search(text) or _SOPS_YAML.search(text)
        if m:
            out.append(Finding("secret", path, text.count("\n", 0, m.start()) + 1, m.group(), "sops-metadata"))
    return out


def text_files(root: Path):
    for dirpath, dirnames, filenames in os.walk(root):
        here = Path(dirpath).relative_to(root).as_posix()
        dirnames[:] = sorted(
            d for d in dirnames
            if d not in SKIP_DIRS and (d if here == "." else f"{here}/{d}") not in SKIP_PATHS
        )
        for name in sorted(filenames):
            file = Path(dirpath) / name
            if file.is_symlink():
                continue
            yield file


def read_text(file: Path) -> str | None:
    """The file as text, or None when it is binary or too large to be prose."""
    try:
        if file.stat().st_size > MAX_BYTES:
            return None
        data = file.read_bytes()
    except OSError:
        return None
    if b"\0" in data[:8192]:
        return None
    return data.decode("utf-8", errors="replace")


@dataclass
class Exception_:
    glob: str
    category: str
    match: str
    line: int
    used: bool = False

    def covers(self, f: Finding) -> bool:
        return (
            self.category == f.category
            and (self.match == "*" or self.match == f.match)
            and glob_match(self.glob, f.path)
        )


def glob_match(pattern: str, path: str) -> bool:
    """`*` within a path segment, `**` across segments."""
    regex = ""
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            regex += "(?:.*/)?"
            i += 3
        elif pattern.startswith("**", i):
            regex += ".*"
            i += 2
        elif pattern[i] == "*":
            regex += "[^/]*"
            i += 1
        elif pattern[i] == "?":
            regex += "[^/]"
            i += 1
        else:
            regex += re.escape(pattern[i])
            i += 1
    return re.fullmatch(regex, path) is not None


def load_exceptions(path: Path) -> tuple[list[Exception_], list[str]]:
    rules: list[Exception_] = []
    errors: list[str] = []
    if not path.exists():
        return rules, errors
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        body, hashmark, why = raw.partition(" #")
        if raw.lstrip().startswith("#"):
            continue
        body = body.strip()
        if not body:
            continue
        parts = body.split(None, 2)
        if len(parts) != 3 or not hashmark or not why.strip():
            errors.append(f"{path.name}:{number}: expected '<path glob> <category> <match> # reason'")
            continue
        glob, category, match = parts
        if category not in CATEGORIES:
            errors.append(f"{path.name}:{number}: unknown category {category!r} (one of {', '.join(CATEGORIES)})")
            continue
        rules.append(Exception_(glob, category, match.strip(), number))
    return rules, errors


@dataclass
class Result:
    findings: list[Finding] = field(default_factory=list)  # unexcepted
    excepted: int = 0
    files: int = 0
    skipped: int = 0
    errors: list[str] = field(default_factory=list)  # exceptions file problems, stale entries


def scan(root: Path, allow: Path | None = None) -> Result:
    root = root.resolve()
    allow = allow or (root / ALLOW_FILE)
    rules, errors = load_exceptions(allow)
    result = Result(errors=list(errors))
    for file in text_files(root):
        if file.resolve() == allow.resolve():
            continue  # it quotes every excepted match, by design
        text = read_text(file)
        if text is None:
            result.skipped += 1
            continue
        result.files += 1
        rel = file.relative_to(root).as_posix()
        for f in scan_text(rel, text):
            rule = next((r for r in rules if r.covers(f)), None)
            if rule:
                rule.used = True
                result.excepted += 1
            else:
                result.findings.append(f)
    for r in rules:
        if not r.used:
            result.errors.append(f"{allow.name}:{r.line}: '{r.glob} {r.category} {r.match}' matches nothing; remove it")
    return result


def summary(result: Result, top: int = 5) -> list[str]:
    lines = []
    for category in CATEGORIES:
        mine = [f for f in result.findings if f.category == category]
        by_path: dict[str, int] = {}
        for f in mine:
            by_path[f.path] = by_path.get(f.path, 0) + 1
        ranked = sorted(by_path.items(), key=lambda kv: (-kv[1], kv[0]))[:top]
        where = ", ".join(f"{p} ({n})" for p, n in ranked)
        lines.append(f"{category}: {len(mine)} in {len(by_path)} file(s)" + (f"; top: {where}" if where else ""))
    return lines


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("root", metavar="dir", type=Path, help="the directory to scan, normally .")
    ap.add_argument("--allow", type=Path, help=f"exceptions file (default: <dir>/{ALLOW_FILE})")
    ap.add_argument("--quiet", action="store_true", help="print the summary only")
    args = ap.parse_args(argv)
    if not args.root.is_dir():
        print(f"oss-scan: {args.root} is not a directory", file=sys.stderr)
        return 2
    result = scan(args.root, args.allow)
    if not args.quiet:
        for f in sorted(result.findings, key=lambda f: (CATEGORIES.index(f.category), f.path, f.line)):
            print(f)
    for e in result.errors:
        print(e)
    for line in summary(result):
        print(line)
    counts = ", ".join(f"{c} {sum(1 for f in result.findings if f.category == c)}" for c in CATEGORIES)
    total = len(result.findings) + len(result.errors)
    print(
        f"oss-scan: {len(result.findings)} unexcepted finding(s) in {result.files} text file(s) "
        f"({counts}; {result.excepted} excepted, {len(result.errors)} exception-file problem(s))"
    )
    return 1 if total else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
