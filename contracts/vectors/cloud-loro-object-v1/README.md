# Cloud Loro object v1 — frozen complete engineering artifact

Status: engineering artifact freeze approved by the lead on 2026-09-13. All
eleven families below form one immutable reviewed commit. This does not approve
production implementation or the capacity/default value. Wire suffix v1 names
the signed envelope version. Later corrections require additive reviewed
artifacts; do not rewrite these captured fixture bytes or expectations.

Authority: final object contract,
final catalog, and the scoped
execution artifact.

| Final family | Exact artifact / checks | Evidence boundary |
| --- | --- | --- |
| 1 — segment/checkpoint every unit | `objects.json` O-1 through O-5, S/C; actual `loro.json` chains | Five real Koru shapes, not a text payload relabeled five ways |
| 2 — bytes/digests/keys | Golden unit-ref/header/envelope/payload bytes, SHA-256 and request domain in `objects.json`; the independent decoder (`oracle.py`) was removed 2026-10-10 | Exact untouched captured bytes, not universal semantic VV hashes |
| 3 — staging | F03 names for five kinds/both objects, exact prefix, stale diagnostic fields, held/unheld, other dataset, malformed/unparseable/link flags, all-cursor/failed listing | Grammar/reference cleanup; no filesystem or OS handle proof |
| 4 — empty/base/chain | `seeds.json`, O-BASE, labeled O-EMPTY-MECHANICS, O-INITIAL-NONEMPTY and five two-segment chains | Real nonempty source paragraph initializes a headless Cloud unit with full checkpoint seq1/coverage0; wholly empty Loro is not Koru-valid |
| 5 — malformed | F05 magic/version/tag/region2, short/extra/padding/overflow, optional shape, key and every receipt identity/commitment mismatch | Independent byte/reference checks, not a production decoder |
| 6 — integrity/causal | F06 checksums; real Rust invalid payload/VV/frontier, pending ancestor Ok/no application, predecessor/partial-end/result separately, wrong frontier, schema and Profile scope | Actual pinned Loro and existing Koru readers/staging; no SQL authority |
| 7 — bounds | F07 exact/over all eight limits, every SQL-backed u64 class, checked full-object sum and bounded-reader short/extra | Declarations and tiny readers only, not at-limit valid objects or 1 GiB measurements |
| 8 — receipts/state | F08 reserve/put/read-back/publish/crash/lost ACK/exact retry, changed request, every stale axis, create-only collision, every listed/unlisted unit/object transition | Explicit Python reference state, never SQL/disk/crash proof |
| 9 — checkpoints | F09 199/200, 99999/100000, oversized admitted-segment trigger, overlap/envelope excluded, first rollover retires through old-current coverage, later writes/counters | Reference publication/counters plus real Loro captured checkpoint |
| 10 — fallback | F10 exact promotion, incomplete head preservation, later exact repair returns active; corrupt sole required segment quarantines only that segment and keeps healthy checkpoint pointer | Reference transaction plus real Loro exact/incomplete reconstruction |
| 11 — coverage | F11 exact dataset in input/output, separate Workspace/Profile aggregates, all six full DTOs, equality/ahead/behind/incomparable, dataset/lineage/replica/unit substitution, completeness/order | Reference DTO/state; actual Loro inclusion separately; no public query |

`check.py`, which held the explicit case inputs, expected outcomes and ids and
drove the real Loro oracle, was removed on 2026-10-10: the oracle and evidence
it read had already been deleted with the rename, so it no longer ran.
`oracle.py`, which held the independent byte/layout and reference state
functions `check.py` called, was removed the same day: nothing imported it
once `check.py` was gone. The fixture JSONs (`objects.json`, `loro.json`,
`seeds.json`) are kept unchanged. The Python decoder deliberately left opaque
Loro bytes for the real Rust verifier; its success alone was never a
valid-Loro or SQL-row claim. F08's `crash` models persisted
state only and is not a process-kill experiment. Transition guards and symlink
flags are explicitly injected, never described as observed production state.

Coverage reference counters use named fake peer counters while digest fields
are SHA-256 of captured VV bytes; there is no claim those fake counter maps are
the decoded captured Loro VV. Actual VV inclusion/equality is separately proved
by the Rust oracle. The hidden task-list is an ordinary Workspace document
identity, not a sixth unit kind; blobs/LegacyPrivate are never admitted units.

## Provenance and reproduction

The nested Rust tool (removed 2026-10-10 with the rename, together with its
`prepare.py` author tooling and the catalog inventory) was not a root workspace member. It uses the exact workspace-locked Loro family,
plus existing `koru-store-core` and `koru-core` path dependencies. Those model
crates add no Store service, browser or telemetry sink and need no bypass flags.
No production module is changed. Tests live beside their artifact module and
consume fixture bytes with compile-time inclusion; they do not read runtime
files or discover an installation.

`seeds.json` contains untouched snapshots produced once by the existing
Manifest, WorkspaceDoc, IdentityManifest, PrivateManifest and PersonalManifest
constructors. Constructor initial peers are random and pinned, never rewritten.
Extensions use peer ids 1001–1005 and 2001–2005; the two concurrent branches use
3001–3005/4001–4005. Metadata creation time is explicitly 1234 where supported.
No clock-recording option is enabled and mutation exports are untouched.
The tool captures two real deltas, an exact captured-frontier checkpoint, later
checkpoint and concurrent two-frontier snapshot for each kind. Profile writes
use the actual dictionary and Personal attribute APIs; ordinary name registers
use their unchanged established Loro schema. Model readers validate scope and
compatibility; Profile staged imports separately reject wrong scope.
The initial nonempty upload additionally uses the actual `Node::paragraph` and
`WorkspaceDoc::insert` APIs with a fixed node id and peer 5002. Reopening the full
snapshot must recover the exact paragraph text; no blank intermediate Cloud
checkpoint is created.

Fresh-process raw-byte reproducibility is measured in
provenance.json,
not assumed. Three observed extensions from pinned seeds produced equal raw
outputs; this does not promise that separately constructing a new unit or
re-encoding an equal decoded VV always produces equal bytes. The separate
encoding probe demonstrates the latter counterexample. Original payloads and
header VV/frontier bytes remain the immutable commitments.

Verification was run by `execution/plan20-object-artifact/verify.py`, which ran
the offline/locked build/tests, focused fmt, all reference cases and the real
Loro oracle into a fresh caller-owned output directory. It and `check.py` were
removed on 2026-10-10 because the Rust oracle and evidence they read had been
deleted with the rename; there is no longer a command that reproduces this
artifact's verification.
`prepare.py --seed-once` was an author-only one-time command that refused to replace
existing seeds; it was proposed-artifact author tooling and is removed. Verification only reads fixtures.

The verification JSON (evidence removed 2026-10-10 with the rename)
listed exact executed ids and false SQL/disk/runtime/capacity flags. The separate
lead run was preserved in its own archive (evidence removed 2026-10-10 with the rename);
the freeze record
distinguishes historical proposed-source hashes from final status documentation.
The tool's retained `status: proposed` is not an approval mechanism. The separate
1 GiB checkpoint and runtime capacity gates remain open. No production source,
worker receipt, durable volume, migration runner, real PostgreSQL constraint,
desktop/mobile transfer or end-to-end completion is claimed.
