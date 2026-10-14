# Cloud transfer v1 — production carrier vectors

Frozen before implementation from maintainer-approved TECHNICAL.final.md §§3–4.
The 32 cases in cases.json pin strict control decoding, framing header limits
and monotonic budget arithmetic. Exact JSON strings are inputs, not schematic
objects: duplicates must survive until the real decoder rejects them.

T12 is a shape-only six-byte payload descriptor. It does NOT prove a valid Loro
import. Production tests must additionally export real Loro documents and import
only after complete length/hash checking through the existing Store boundary.
No old amendment JSON, SQL, checker or golden bytes are rewritten or inherited
as production proof. The four-message handshake and its existing vectors remain.

Supplement.json freezes T33–T52 before source acceptance: decision controls,
all three Profile unit arms, strict PeerFrame and nested unknown/duplicate
properties. Both files are consumed by production Rust decoder tests.

The first production run exposed T50's authoring error: existing ProfileUnitRef
uses type, not the new Cloud transfer unit's kind. errata.json explicitly changes
that expected result to invalid while preserving the frozen input bytes.
profile-request.json adds T53–54 for the real existing shape and escaped
duplicate. Rust consumes all three input files and checks the erratum's original
expected value before applying the documented correction.

Stateful producer tests additionally cover policy generation 1 initialization,
identical replay, skipped/changed/backward policy, automatic 200000 versus
explicit 200001, zero pause, defer, stale/future start distinction, independent
directions/streams, complete/header/index/length/hash validation, bounded memory,
EOF/cancel, deadline/lifetime, policy versus durable-handoff ordering, and real
causally missing Loro ancestors. Cases here alone do not close those obligations.

Implementation/evidence: ../../../execution/PLAN10-TRANSFER.md.

sequences.json freezes T55–59 as executable directional/race sequences. Each
step is [operation, integer argument, exact result code]; start_a/start_b begin
and deliver a start from that side, accept_a/defer_a decide the A-originated
transfer at B and deliver its decision, policy_a delivers the named generation
to A, drain_b replaces B, and send_a verifies no payload remains after defer.
The Rust sequence interpreter consumes every step and asserts each result.
