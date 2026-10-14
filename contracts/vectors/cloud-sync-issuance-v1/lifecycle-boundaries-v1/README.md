# Issuance lifecycle and common-boundary supplement

Additive resolution of the three deliberately unmodeled boundaries in the
[base issuance artifact](../README.md), immutable commit
`94a1117d556b23f3197fe98c2c986311ac61a1f2`. Its four files and all original
130 reference cases remain unchanged. This supplement requires no new product
decision, schema, HTTP denial vocabulary or entitlement feature.

## Binding sources

- Workspace lifecycle contract,
  authorization-snapshot section and ordered decision table (lines 423-446):
  a missing Workspace row returns `workspace_not_authorized`; after membership,
  binding and scope checks, a retained row whose lifecycle is `absent` returns
  `cloud_absent` for Cloud data admission. Lifecycle denial precedes stale
  access-epoch checks. `cloud_incarnation_stale` instead concerns a supplied
  incarnation mismatch, not the token issuer's missing caller-supplied incarnation.
- Current account API decoder,
  `MAX_REQUEST_BODY_BYTES` at line 30 and `decode_request` at line 1419:
  ordinary Body routes already use `64 * 1024` bytes before typed JSON decoding,
  with `invalid_request` on excess. Issuance inherits this common boundary;
  selection's 1 KiB and handshake's 32 KiB are not new issuer limits.
- Retained MS1 Plan 13
  requires an explicit, unbounded production entitlement input. A synthetic
  finite boundary tests expiry shortening, not a shipping entitlement producer.
  The lifecycle contract explicitly keeps negative entitlement/quota reasons
  dormant and excludes their codes from shipping DTOs/UI in this rung.

Thus the original README's unresolved-source-mapping notes are historical
questions resolved by these existing authorities, not additional maintainer gates.

## Cases and verification

`crates/koru-account-session/src/cloud/issuance/tests.rs` reads `cases.json`
and checks every case (`cargo test -p koru-account-session`).

| Cases | Observation |
| --- | --- |
| I-L01-L12 | `cloud_absent` on an authorized retained row, before epoch/assignment failure; missing/unauthorized row, membership/binding, rate, revoked device, native authentication and malformed request retain their earlier denials |
| I-H01-H05 | Valid JSON padded with ASCII space to 1,024, 32,769, 65,535 and 65,536 bytes remains within the existing HTTP-body boundary; 65,537 bytes is `invalid_request` before any rate/signing call |
| I-E01-E04 | Production unbounded input yields 600 seconds; synthetic finite input shortens expiry; equality/past input returns internal no-lifetime with no public denial code approved or exposed |

The body cases stop at decode, so their zero rate/sign counters do not say that
a later successful issuer skips its rate bucket. Lifetime refusal is an
internal typed result; it is not a successful nullable expiry on the wire.

Author verification: original 130 and additional 21 reference checks passed,
both processes exited 0. No Rust/API/SQL integration, migrations, actual request
body streaming, entitlement enforcement, live route resolution or physical
device tests ran. No DS-D row is closed by these reference checks. Production
source must consume the fixtures and prove the corresponding effects.
