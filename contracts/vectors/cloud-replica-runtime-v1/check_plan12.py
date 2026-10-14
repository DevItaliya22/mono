"""Read-only independent lease reference oracle; not PostgreSQL/runtime evidence."""
import copy
import hashlib
import json
from pathlib import Path
import uuid

MAX = 2**63 - 1


def integer(value):
    number = int(value)
    if not 0 <= number <= MAX:
        raise ValueError("SQL integer domain")
    return number


def canonical(vector, ids):
    kind = {"place": 1, "acquire": 2, "renew": 3, "release": 4}[vector["kind"]]
    raw = bytearray(b"koru/cloud/lease-operation/v1\0")
    raw.append(kind)
    raw.extend(uuid.UUID(ids["replica"]).bytes)
    raw.append({"workspace": 1, "profile": 2}[vector["dataset"]])
    for key in ("authority", "lineage", "worker"):
        raw.extend(uuid.UUID(ids[key]).bytes)
    epoch = integer(vector["expected_epoch"])
    raw.extend(epoch.to_bytes(8, "big"))
    present = vector["expected_lease"] is not None
    if present != (kind in (3, 4)):
        raise ValueError("lease shape")
    raw.append(int(present))
    if present:
        expiry = integer(vector["expected_expiry_millis"])
        if not epoch or not expiry:
            raise ValueError("positive lease fence")
        raw.extend(uuid.UUID(vector["expected_lease"]).bytes)
        raw.extend(expiry.to_bytes(8, "big"))
    elif vector["expected_expiry_millis"] is not None:
        raise ValueError("unexpected expiry")
    raw.extend(b"\0\0")
    return bytes(raw)


def transition(initial, request):
    row = copy.deepcopy(initial)
    kind = request["kind"]
    if request["authority"] != "ok":
        return request["authority"], row
    if row is not None:
        last = row["last"]
        if last and last["id"] == request["id"]:
            result = "exact_retry" if last["digest"] == request["digest"] else "operation_conflict"
            return result, row
    if kind == "place":
        if row is not None:
            return "replica_exists", row
        if not request["ready"]:
            return "sync_unavailable", row
        row = dict(epoch=0, lease=None, start=None, expiry=None, worker=request["worker"])
        result = "placed"
    else:
        if row is None:
            return "replica_fenced", row
        epoch = integer(row["epoch"])
        if kind == "acquire" and request["worker"] != row["worker"]:
            return "storage_move_required", row
        if kind in ("acquire", "renew") and not request["ready"]:
            return "sync_unavailable", row
        live = row["lease"] is not None and row["expiry"] > request["now"]
        if kind == "settle":
            if row["lease"] is None or live:
                return "unchanged", row
        else:
            if integer(request["epoch"]) != epoch or request["worker"] != row["worker"]:
                return "replica_fenced", row
            if kind in ("renew", "release") and any(
                row[key] != request[key] for key in ("lease", "expiry")
            ):
                return "replica_fenced", row
            if kind in ("renew", "release") and row["lease"] is None:
                return "replica_fenced", row
            if kind == "renew" and not live:
                return "replica_fenced", row
            if kind == "acquire" and live:
                return "lease_held", row
        if kind != "renew" and epoch == MAX:
            return "replica_fenced", row
        if kind in ("acquire", "renew"):
            if request["now"] > MAX - 30000:
                return "replica_fenced", row
            row["expiry"] = request["now"] + 30000
            if kind == "acquire":
                row.update(epoch=epoch + 1, lease="generated-lease", start=request["now"])
            result = "acquired" if kind == "acquire" else "renewed"
        else:
            row.update(epoch=epoch + 1, lease=None, start=None, expiry=None)
            if kind == "settle":
                row["last"] = None
                return "settled", row
            result = "released"
    row["last"] = dict(id=request["id"], kind=kind, digest=request["digest"], at=request["now"])
    return result, row


def main():
    vectors = json.loads(Path(__file__).with_name("plan12.json").read_text(encoding="utf-8"))
    for vector in vectors["digest_vectors"]:
        raw = canonical(vector, vectors["ids"])
        assert raw.hex() == vector["canonical_hex"], vector["id"]
        assert hashlib.sha256(raw).hexdigest() == vector["sha256"], vector["id"]
    for vector in vectors["lease_vectors"]:
        result, row = transition(vector["initial"], vector["command"])
        expected = vector["expected"]
        assert result == expected["result"], (vector["id"], result)
        if expected.get("unchanged"):
            assert row == vector["initial"], vector["id"]
        for key in ("epoch", "expiry", "last"):
            if key in expected:
                assert row[key] == expected[key], (vector["id"], key)
        if row is not None:
            assert row["worker"] == (vector["initial"] or vector["command"])["worker"]
    for invalid in (-1, 2**63):
        try:
            integer(invalid)
        except ValueError:
            pass
        else:
            raise AssertionError("accepted invalid integer")
    renew = vectors["digest_vectors"][4]
    for changes in (
        {"kind": "move"},
        {"expected_epoch": 2**63},
        {"expected_expiry_millis": 2**63},
        {"expected_epoch": 0},
        {"expected_expiry_millis": 0},
        {"expected_lease": None},
        {"kind": "place"},
    ):
        try:
            canonical(dict(renew, **changes), vectors["ids"])
        except (ValueError, KeyError):
            pass
        else:
            raise AssertionError(("accepted invalid command", changes))
    print("PASS: 8 canonical byte/digest vectors, 44 lease reference cases, 2 integer refusals")
    print("PASS: 7 reserved-kind, integer-domain and fence-shape refusals")
    print("NOT RUN: production repository, PostgreSQL, migration runner, worker or capacity evidence")


if __name__ == "__main__":
    main()
