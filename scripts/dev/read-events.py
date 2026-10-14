# SPDX-License-Identifier: MPL-2.0

"""Read this machine's event journal, without disturbing it.

The events worker owns `events.db`; this opens the same file **read-only**
(SQLite URI `mode=ro`), so it is safe to run against a live stack and cannot
be the reason something broke. It is the fastest answer to "did that event
actually fire", which a log line only tells you if you were watching.

    python scripts/dev/read-events.py                  # the last 20 events
    python scripts/dev/read-events.py --tail 100       # more of them
    python scripts/dev/read-events.py --counts         # how many of each, and when last
    python scripts/dev/read-events.py --name document. # only names with this prefix
    python scripts/dev/read-events.py --status         # journal counters and schema
    python scripts/dev/read-events.py --channel beta   # another channel's journal
    python scripts/dev/read-events.py --root D:\\stack  # a --root stack's journal

Channels never share a journal, so `--channel` is how you look at one build
while running another. A debug build is always `internal-debug`.
"""

import argparse
import json
import os
import pathlib
import sqlite3
import sys

# The one derivation rule, mirrored from `koru_channel::events_root_beside`:
# the sibling of a conventional `store/` directory. Mirrored rather than
# imported because this is a Python script and that is Rust; if the layout
# ever moves, both change.
def journal_path(channel: str, root: str | None) -> pathlib.Path:
    if root:
        base = pathlib.Path(root)
        return (base.parent / "events" if base.name == "store" else base / "events") / "events.db"
    local = os.environ.get("LOCALAPPDATA")
    if not local:
        sys.exit("LOCALAPPDATA is not set; pass --root explicitly")
    return pathlib.Path(local) / "Koru" / channel / "events" / "events.db"


def open_read_only(path: pathlib.Path) -> sqlite3.Connection:
    if not path.exists():
        sys.exit(
            f"no journal at {path}\n"
            "The events worker creates one on its first run; check the channel "
            "(--channel) or pass --root."
        )
    return sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)


def attrs_summary(raw: str) -> str:
    """Attribute values are typed on the wire (`{"t": "token", "v": "manual"}`);
    print the value alone, because the kind is the registry's business and
    what a person reading this wants is the answer."""
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return raw
    if not parsed:
        return ""
    return " ".join(f"{key}={value.get('v')}" for key, value in sorted(parsed.items()))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--channel", default="internal-debug", help="stable, beta, nightly, internal-debug (default), internal-release")
    parser.add_argument("--root", help="an explicit stack root (the store dir, or its parent)")
    parser.add_argument("--tail", type=int, default=20, help="how many events to show (default 20)")
    parser.add_argument("--name", help="only events whose name starts with this")
    parser.add_argument("--counts", action="store_true", help="one line per event name, with its last occurrence")
    parser.add_argument("--status", action="store_true", help="journal counters, consent and schema version")
    args = parser.parse_args()

    path = journal_path(args.channel, args.root)
    conn = open_read_only(path)
    print(f"# {path}")

    if args.status:
        counts = dict(conn.execute("SELECT class, COUNT(*) FROM events GROUP BY class"))
        meta = dict(conn.execute("SELECT key, value FROM meta"))
        print(f"schema           v{conn.execute('PRAGMA user_version').fetchone()[0]}")
        print(f"events           {sum(counts.values())} (domain {counts.get('domain', 0)}, instrumentation {counts.get('instrumentation', 0)})")
        print(f"last seq         {conn.execute('SELECT COALESCE(MAX(seq), 0) FROM events').fetchone()[0]}")
        print(f"documents known  {conn.execute('SELECT COUNT(*) FROM doc_state').fetchone()[0]}")
        try:
            print(f"workspaces known {conn.execute('SELECT COUNT(*) FROM workspace_state').fetchone()[0]}")
        except sqlite3.OperationalError:
            print("workspaces known -  (journal predates the v2 migration)")
        # The outbox is what *would* be uploaded. Empty is the correct
        # resting state: consent is off until somebody turns it on.
        print(f"telemetry outbox {conn.execute('SELECT COUNT(*) FROM telemetry_outbox').fetchone()[0]} pending")
        # The opt-in record (2026-10-11). `telemetry_consent` is the legacy
        # key ADR-176's builds read; it is mirrored, never trusted.
        answer = {'1': 'on', '0': 'off'}.get(meta.get('telemetry_opt_in'), 'unanswered (off)')
        print(f"telemetry consent {answer}")
        print(f"baseline seeded  {'yes' if meta.get('baseline_seeded') == '1' else 'no'}")
        print(f"install recorded {'yes' if meta.get('installed_recorded') == '1' else 'no'}")
        print(f"pruned so far    {meta.get('pruned_total', '0')}")
        return

    if args.counts:
        where, params = ("WHERE name LIKE ?", [f"{args.name}%"]) if args.name else ("", [])
        rows = conn.execute(
            f"SELECT name, COUNT(*), MAX(datetime(recorded_at_ms / 1000, 'unixepoch', 'localtime')) "
            f"FROM events {where} GROUP BY name ORDER BY name",
            params,
        )
        for name, count, last in rows:
            print(f"{name:<28} {count:>5}   last {last}")
        return

    where, params = ("WHERE name LIKE ?", [f"{args.name}%"]) if args.name else ("", [])
    params.append(args.tail)
    rows = conn.execute(
        f"SELECT seq, datetime(recorded_at_ms / 1000, 'unixepoch', 'localtime'), name, "
        f"COALESCE(subject_id, ''), attrs FROM events {where} ORDER BY seq DESC LIMIT ?",
        params,
    ).fetchall()
    for seq, when, name, subject, attrs in reversed(rows):
        detail = " ".join(part for part in (subject[:8], attrs_summary(attrs)) if part)
        print(f"{seq:>6}  {when}  {name:<28} {detail}")


if __name__ == "__main__":
    main()
