#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Check that every commit in a range is signed off (DCO). CI runs this on
every pull request.

    python scripts/checks/dco-check.py <base>..<head>
    python scripts/checks/dco-check.py                 # origin/main..HEAD

Every commit that is not a merge must carry a trailer

    Signed-off-by: Name <email>

whose name and email match the commit author's (case-insensitively). That is the rule
CONTRIBUTING.md states: the sign-off is the author certifying the Developer
Certificate of Origin (DCO, at the repository root), so someone else's
sign-off does not count. Merge commits are skipped. Bot-looking author
addresses are not exempt: Git author metadata is supplied by the contributor,
not authenticated by GitHub. Dependabot's pull requests are exempt in ci.yml
instead, by the GitHub accounts behind the pull request, so this script never
sees them.

Exit 0 when every commit passes, 1 when any fails (each one is listed with how
to fix it), 2 when the range cannot be read.
"""

import re
import subprocess
import sys

SIGN_OFF = re.compile(r"^Signed-off-by:\s*(?P<name>.+?)\s*<(?P<email>[^<>\s]+)>\s*$", re.M)

# Unit separator between fields, record separator between commits: neither can
# appear in a name, an email or (in practice) a commit message.
FIELD = "\x1f"
RECORD = "\x1e"


def commits(rev_range: str, cwd: str | None = None) -> list[dict]:
    """Every non-merge commit in `rev_range`, oldest first."""
    # Git parses the trailer block; a quoted sign-off in the message body is
    # not a certification. Do not implement a second trailer parser here.
    fmt = FIELD.join(["%H", "%an", "%ae", "%s", "%(trailers:key=Signed-off-by,only)"]) + RECORD
    out = subprocess.run(
        ["git", "log", "--no-merges", "--reverse", f"--format={fmt}", rev_range],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout
    found = []
    for record in out.split(RECORD):
        record = record.strip("\n")
        if not record:
            continue
        sha, name, email, subject, body = record.split(FIELD, 4)
        found.append(
            {"sha": sha, "name": name, "email": email, "subject": subject, "body": body}
        )
    return found


def problem(commit: dict) -> str | None:
    """Why `commit` fails the DCO rule, or None when it passes."""
    sign_offs = [(m.group("name"), m.group("email")) for m in SIGN_OFF.finditer(commit["body"])]
    if not sign_offs:
        return "no Signed-off-by line"
    author = commit["email"].strip().lower()
    name = " ".join(commit["name"].split()).casefold()
    if (name, author) not in (
        (" ".join(signer.split()).casefold(), email.strip().lower())
        for signer, email in sign_offs
    ):
        return (
            f"signed off by {', '.join(f'{n} <{e}>' for n, e in sign_offs)}, "
            f"but the author is {commit['name']} <{commit['email']}>"
        )
    return None


def main(argv: list[str]) -> int:
    rev_range = argv[0] if argv else "origin/main..HEAD"
    try:
        checked = commits(rev_range)
    except subprocess.CalledProcessError as error:
        print(f"dco-check: cannot read {rev_range}: {error.stderr.strip()}", file=sys.stderr)
        return 2
    failures = [(c, why) for c in checked if (why := problem(c)) is not None]
    for commit, why in failures:
        print(f"::error::{commit['sha'][:12]} {commit['subject']!r}: {why}")
    if failures:
        print(
            f"\n{len(failures)} of {len(checked)} commit(s) are not signed off.\n"
            "Every commit needs 'Signed-off-by: Your Name <you@example.com>' matching\n"
            "its author (CONTRIBUTING.md, 'Sign off every commit'). To fix:\n\n"
            "    git commit --amend -s --no-edit      # the last commit\n"
            "    git rebase --signoff <base>          # every commit on the branch\n"
            "    git push --force-with-lease\n"
        )
        return 1
    print(f"dco-check: {len(checked)} commit(s) in {rev_range}, all signed off")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
