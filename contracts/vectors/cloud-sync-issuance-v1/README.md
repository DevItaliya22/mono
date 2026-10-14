# Frozen Cloud sync issuance v1 reference subset

Additive Plan 13 preparation against source baseline `c28cf31c`. This is an
immutable input/expected-result artifact for review and later Rust consumption,
not an implementation, an enabled endpoint or full Plan 13 acceptance.

Authority is Cloud admission, especially
Device sync credential and Issuance decision order, amended by
TECHNICAL final section 7.
Documents retains MS1 Plan 13 and the
published [Plan 28 denial table](../../document-sync-denials-v1.md). No new
schema, credential purpose, rate enum, environment setting or mode policy is
proposed here. Source work waits for the lead's Plan 12 integration and this
artifact's independent review.

## Files and run

The Rust tests that read these files are the check:
`crates/koru-account-session/src/cloud/issuance/tests.rs`,
`crates/koru-account-session/src/lib_tests/cloud_rate.rs` and
`crates/koru-account-wire/src/cloud_sync/tests.rs`
(`cargo test -p koru-account-session -p koru-account-wire`).

- `issuance.json`: exact request JSON; response construction/mutation recipes;
  independent expected decisions and effect counts; integer expiry inputs;
  fixed rate-bucket fixture and explicitly unclosed denial obligations.

The existing `cloud-sync-admission-v1/foundation.json` supplies real, already
frozen credential bytes and exact datasets. Its SHA-256 is pinned. No old vector
or expected result is edited. Its README's historic statement that issuance is
pending remains true for production evidence. This checker compares the decoded
fixture payload and reads the exact signed token unchanged; it does not sign a
token or verify Ed25519 itself. Existing Rust verifier vectors remain mandatory.

## Case families

| Cases | Frozen observation |
| --- | --- |
| I-R01-R33 | Strict version/Profile request and path UUID; no caller-supplied authority, endpoint, worker, lease or expiry; duplicate/unknown/null/type/version/trailing-value refusal |
| I-O01-O40 | Nine ordered issuer stages, exact Profile admission, current native account/session/device results, revocation before rate/signing, no implicit owner or scope union, lifecycle/assignment refusal, partial routes and no fallback to an unassigned ready worker |
| I-T01-T16 | Whole-second expiry; production 600 seconds, optional shorter internal duration, explicit unbounded/capped entitlement input, equality/refusal and checked signed-64-bit arithmetic |
| I-V01-V04 | Generic credential lifetime rule, including 900 accepted and 901 refused; not an issuer configuration |
| I-S01-S31 | Strict response and dataset-result unions; exact dataset/expiry correlation with the existing token, partial availability, trusted relay versus isolated null, endpoint/address shape, required nested fields and retry bounds |
| I-B01-B04 | Existing canonical subject and keyed digest; identity/device/Workspace and purpose isolation; existing DeviceOperation enum; compiled 30 per 60,000 ms policy and count boundary |
| I-C01-C02 | Existing token payload matches its frozen claims and contains no worker/lease/route facts |

The order cases are internal test projections, not deserializable production
requests, signed claims, repository commands or permission proofs. Unspecified
fields inherit the checker's documented valid fixture. Flags such as
`another_profile_sync_scope`, `profile_has_bytes`, `previous_token_exists` and
`old_snapshot_valid` intentionally cannot rescue a failed current exact check.
Native-verifier and assignment error inputs are injected results; they do not
exercise the cryptographic verifier or database joins. Counts name the required
future observation, not actual API/SQL/signing instrumentation. In particular,
`protected_writes=0` excludes the permitted rate bucket and existing native
session machinery; it means no credential/cache/assignment/lifecycle/payload
publication or bilateral trust creation.

Issuer membership cases require the explicitly signed `workspace:sync` scope
on the chosen Profile's own membership. TECHNICAL section 7.1's separate
selection transaction requires all three discovery scopes. This artifact does
not silently add a new issuer scope policy and does not assert approval or
success for optional read-scope combinations. Those combinations are not tested.
`provisioned` here means admitted, not bytes possessed; the all-reader audit and
real selection/P2P transaction-order tests are still required by section 7.2.

## Encoding and refusal boundaries

The rate subject reuses `service.rs::rate_decision`'s field encoding: each
canonical UUID's ASCII bytes prefixed with an unsigned 64-bit big-endian length,
in identity/device/Workspace order. `runtime.rs::digest` computes HMAC-SHA256 of
`ASCII(purpose) || 00 || subject`. Fixture key is public `33` repeated 32 times.
The golden digest was computed with the standard-library Python implementation
and independently confirmed by PowerShell/.NET HMACSHA256:

`8ffa350b0937df51f1069ce3f6e904f534a35d5d98e81d364215efd259b62063`

The separate purpose is `rate-cloud-sync-token`; the existing PostgreSQL enum
variant is `DeviceOperation`. There is no rate migration. Actual SQL counting,
window rollover, concurrent requests and HTTP Retry-After remain unverified.

Response recipes are deterministic transformations of the pinned foundation,
with Workspace/Profile sessions at indexes 0/1. Available routes use the
foundation's exact worker endpoint and numeric loopback ports 7777/7778; these
are inert fixtures, not a deployment. Production fixture relay is
`https://relay.example.invalid`; an explicitly isolated fixture has empty trusted
relay configuration and required null. The checker checks exact fixture endpoint
bytes, not general Ed25519 point validity or current registry readiness. It does
not replace the existing strict regional route adapter or freeze a new route.

Large integer strings in expiry cases are artifact-only encodings to avoid
JSON-tool precision loss; wire `iat` and `exp` remain integer Unix seconds.
An `exp:null` expected result means the internal expiry computation refuses to
sign, not that null is a legal wire expiry or that a new HTTP code is approved.
No entitlement-denial code, absent-lifecycle issuer code, or HTTP request/body
cap absent from the exact issuance contract is invented by this subset. The
public producer mapping for those refused inputs must be resolved before its
affected source acceptance. Selection's 1 KiB cap and the handshake's 32 KiB
cap are not transplanted onto the issuance HTTP body.

## Evidence still owed

All mapped DS-D rows remain open. An injected result under a denial id is not
the row's producer proof. Plan 13 must consume this artifact in real Rust tests,
pin actual API codes and headers, instrument zero signing/protected effects on
refusal, and prove exact-profile/current-registration/current-assignment reads
under real concurrent PostgreSQL writers. The generic Plan 11 snapshot unions
qualifying Profile scopes and is insufficient as that issuer read by itself.

Still owed: key rotation/foreign-purpose API handling, actual cryptographic
signing, entropy/time injection, database clock and race boundaries, no positive
authority cache, failure/Retry-After response bytes, real worker registry routes,
full workspace verification, independent review, Windows/Android adapter tests,
capacity and desktop-off phone retrieval. The source should fail unavailable
without real route composition, never install these fixtures as ready workers.

No shared mode-storage representation, Android UI, physical acceptance,
durability or permission to deploy is implied by a reference pass.
