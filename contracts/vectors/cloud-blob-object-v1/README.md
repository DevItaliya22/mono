# `cloud-blob-object-v1` vectors

Read by the test suite, not only by people. The contract is
[`../../cloud-blob-object-v1.md`](../../../docs/cloud-platform/contracts/cloud-blob-object-v1.md); these files
are what makes it enforceable rather than aspirational.

## `cases.json` — one request, one answer

Each case is a single admission decision. `code` is the expected stable code
from the contract's §7, or `ok`.

| Prefix | What it exercises |
| --- | --- |
| `C` | capability admission — region, purpose, audience, expiry, signature, bindings |
| `R` | `Content-Range` admission — contiguity, idempotent replay, bounds |
| `A` | allocation — the ceiling, idempotence by `(workspace, incarnation, hash)` |
| `V` | verification — what publishes and what quarantines |
| `G` | reads — and the difference between "not yours" and "not there" |
| `P` | the report seam — idempotence, and what a moved epoch does |

Fields beyond `claims` describe the *world* the request meets rather than the
request itself: `current_eph` and `current_inc` are what the database now says,
`route_sub` is the id in the URL when it differs from the capability's, and
`identical` says whether a replayed range carries the same bytes.

Two pairs are the ones worth reading before writing any code:

- **C05/C06** — a read capability used to write, and the reverse. Purpose is
  compared exactly, because at rung 7 one of these tokens goes to a third
  party.
- **G02/G03** — a locator that exists but is not yours, and one that does not
  exist. **Both answer `wrong_object`.** A denial must not teach an
  unauthorised caller which locators are real.

## `sequences.json` — orderings that must survive a crash

Each sequence is a run with a kill in it. The assertions that matter are not
"it finished" but the invariants beside them:

- `mapping_before_bytes: false` — **SQL never claims durability before the
  immutable object exists.** S01, S04 and S09 each assert it from a different
  angle.
- `association_before_bytes: false` — the client-side twin. S10 and S11.
- `resends: 0` — a resume re-sends nothing. S02, S05, S10.
- `sessions: 1` / `locators: 1` — two devices racing on one picture converge
  (S08). This is the one-creator invariant, applied where it actually binds:
  a blob's identity is its content, but a session and a locator are minted.

**S04 is the sequence to run first when anything changes**: killed between the
rename and the report, the object exists and the database does not know. Get
that one wrong in the safe direction and you leak an object; get it wrong in
the unsafe direction and a mapping points at nothing.

**S06 is the new one D252 created.** The API is down mid-upload and bytes keep
arriving, because `PATCH` needs no control plane. It is the property the split
was for, and it is easy to lose by accident the first time somebody adds a
database read to the byte path.

## Regenerating

These are hand-written, not generated: they encode intent, and a generator
would only ever reproduce the implementation's own assumptions. Adding a case
is adding a sentence to the contract, so both move together.
