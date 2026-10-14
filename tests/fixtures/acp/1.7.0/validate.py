"""Maintenance validation; jsonschema is tooling, never a Koru dependency."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
from jsonschema import Draft202012Validator

root = Path(__file__).resolve().parent
parser = argparse.ArgumentParser()
parser.add_argument("--regenerate", action="store_true")
parser.add_argument("--check-generated", action="store_true")
args = parser.parse_args()
if args.regenerate or args.check_generated:
    generated = subprocess.check_output([
        "cargo", "run", "--locked", "--quiet", "--manifest-path",
        str(root / "generate" / "Cargo.toml")
    ]).decode("utf-8").replace("\r\n", "\n")
    if args.regenerate:
        (root / "schema.json").write_bytes(generated.encode("utf-8"))
    elif generated != (root / "schema.json").read_text(encoding="utf-8-sig"):
        raise SystemExit("Vendored schema differs from the pinned generator.")
schema_bytes = (root / "schema.json").read_bytes()
schema = json.loads(schema_bytes.decode("utf-8-sig"))
payloads = json.loads((root / "payloads.json").read_text(encoding="utf-8"))
Draft202012Validator.check_schema(schema)
validator = Draft202012Validator(schema)
validator.validate(payloads)
# Permission alternatives and all stop reasons use the same generated schema.
for kind in ["allow_once", "allow_always", "reject_once", "reject_always"]:
    payloads["permission_request"]["options"] = [{
        "optionId": "<choice>", "name": "<label>", "kind": kind
    }]
    validator.validate(payloads)
for outcome in [{"outcome": "cancelled"}, {"outcome": "selected", "optionId": "<choice>"}]:
    payloads["permission_response"]["outcome"] = outcome
    validator.validate(payloads)
for stop in ["end_turn", "cancelled", "refusal", "max_tokens", "max_turn_requests"]:
    payloads["prompt_response"]["stopReason"] = stop
    validator.validate(payloads)
print("ACP payloads, permission alternatives and stop reasons validate against 1.7.0.")
print("schema SHA-256:", hashlib.sha256(schema_bytes).hexdigest())
