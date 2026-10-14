# Frozen Cloud replica runtime v1 — Plan 12 subset

Command format v1, amended by TECHNICAL final §5.
This directory freezes the lease subset before Rust source; it is not a claim
that all runtime vectors or implementation acceptance gates are complete.

`plan12.json` fixes eight Workspace/Profile canonical byte and SHA-256 vectors
and 44 explicit lease input/state/result cases. `check_plan12.py` independently
reconstructs bytes and checks the lease reference transitions, read-only and
without dependencies or network. SHA-256 values were independently computed
with .NET before freezing. Run `python check_plan12.py` from this directory.
Maximum-domain epochs are decimal strings to avoid JSON tooling's 53-bit limit.
Symbolic lease/worker/operation labels in transition cases are references to
distinct injected test identities, not production UUID encodings. Digests in
transition cases are equality labels; canonical digest bytes are separately
frozen above. Current authority/readiness results are injected oracle inputs,
not evidence that a real authority query or readiness check ran.

The lease-state oracle's command rows are internal transition projections,
not canonical repository requests: their shared record contains irrelevant
fields (for example a Place row's epoch/lease) that the transition ignores.
They must not be deserialized as valid production commands or used to claim
strict command parity. Production typed Place/Acquire constructors do not
contain those fields; Renew/Release require their complete expected fence.
The separate eight digest vectors are exact valid canonical commands.
Settlement is an internal locked-row operation, never a caller command/id.

Production tests must consume these vectors and add the actual guarded SQL
proofs: Workspace/Profile contenders, each actual authority epoch and
kind/lineage/region substitution, rollback, restart, lock order, constraints,
fresh creation and predecessor upgrades. The eight byte vectors plus changed
expiry must pin the production digest, not only this Python oracle.

Not yet frozen here: Plan 19 registration/readiness/route, supervision and
capacity vectors. Not yet measured: connection/session limits and actor queue
delay while verifying the maximum checkpoint through the actual object path.
The runtime contract retains these as Plan 12/19 acceptance gates. There is no
approved relaxation or real-capacity evidence in this artifact.
