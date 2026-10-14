# Host control v1 — the principal union and control protocol 7

Frozen from maintainer-approved
HOST-CONTROL.final.md §§2–3, which
Steven Liechti signed off on 2026-09-15. These vectors pin the *plumbing*: the
identity union at the network edge and bridge, and the seven Cloud operations
of the Agent→gateway control protocol. They pin no Cloud dial, no Reconciler
run and no Store write — those are later increments and owe their own evidence.

Exact JSON objects are inputs, not schematic sketches. A duplicate or unknown
property must survive in the file until the real decoder refuses it, which is
what `principals.json` H3, H9 and `control.json` C9, C13 exist to prove.

**No case contains a credential.** A permit names a credential *context*
(`credential_id` is the signed `jti`); `endpoint_id` values are public Ed25519
keys, derived from fixed all-one-byte secrets so the file is reproducible and
so a reader cannot mistake them for secrets. There is no name, title, path or
document byte anywhere in this directory.

## The files

| File | What it pins | Consumed by |
| --- | --- | --- |
| `principals.json` | H1–H6, the `NetworkPrincipal` arms; H7–H11, the `NetworkTarget` arms | `koru-network-edge`'s `tests.rs`, through `include_str!` |
| `control.json` | C1–C13, one valid case per control-7 operation plus unknown field, unknown variant, unknown enum label and a withdrawn-field case | `koru-sync-control`'s `tests.rs` |
| `sequences.json` | S1–S12, executable `ControlHost` scripts: sequence retry, conflict and staleness, a foreign permit, install replay, install against an unknown permit, generation rollover, the three fence shapes, decision staleness, bootstrap shape, a foreign session and a restart | `koru-sync-control`'s `tests.rs` |

A sequence step is one of four things: a `request` with an `expect` (the exact
response JSON), a `request` with an `expect_code` (the exact stable refusal
code), a `permit_ids` assertion over the live status snapshot in `permit_id`
order, or `restart: true`, which ends the control session and begins a new one
under `restart_session`. Each sequence runs against its own fresh
`ControlHost`, because a session is the only scope any of this state has.

## What is proved in Rust rather than here

Three bounds are exercised programmatically in `koru-sync-control`'s tests
instead of being spelled out as JSON, because writing thirty-three permits out
by hand would add length without adding a single new refusal:

- a page at the 32-permit bound, and a 33rd permit that takes it over;
- a second account whose permit would take the **host** over 32 live permits,
  refused whole rather than applied in part;
- the largest legal body — thirty-two Workspace permits, each with its
  descriptor, with `u64::MAX` counters — measured against the extra 64 KiB
  Cloud frame cap, so the cap can never start refusing correct messages.

## What these vectors do not establish

Approval of a design is not evidence, and neither is a decoder test. Exact
binary bridge framing across a real socket, the Windows and Android
constructors, the origin-off two-device test, and every capacity, durability
and activation gate remain owed. `CloudStatus` in particular is current
in-memory state: `settled` is a stage, never durable Cloud coverage, and it may
never feed the coverage projection.

Implementation and evidence: EXECUTION.md.
