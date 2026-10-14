# Cloud admission v1 foundation vectors

This is the approved, separable Plan 10 foundation, not full contract acceptance.
`foundation.json` freezes exact compact JWS bytes, both dataset principals,
four-message Workspace/Profile exchanges, u32-BE frames, 244-byte transcripts,
Ed25519 signatures, provenance bytes and version gates. Test keys are deliberately
public deterministic fixtures. `generate.py` prints them using independent
Python cryptography; Rust tests consume the committed bytes without rewriting.

**Regenerated for D231 (2026-09-15), the route-attestation withdrawal.** The
round that produced the current bytes removed `route_attestation` from
`WorkerChallengeV1`, removed the trailing route digest from the transcript
(276 → 244 bytes), and replaced each session's `route_claims`/`route` pair with
an unsigned `assignment` block — the worker's own dataset, endpoint, worker id,
lease id and epoch, which now reach admission only through its live authority
read. The withdrawn bytes remain in git history at the commit before this one.

The D201 admission final approves these wire shapes. The D199 transfer digest
carrier is still an explicit maintainer gate: no transfer envelope, prelude, changed
PeerFrame or private Loro extension is defined here. Issuance, routing response,
worker composition and platform acceptance remain their own evidence.

## Exact boundary and hostile vector derivations

For each `sessions[*].messages[*].json`, the following byte recipes are frozen
alongside the literal success bytes. They avoid storing repeated spaces:

- At the JSON cap: remove the final `}`, append ASCII space repeated exactly
  `32768 - original_byte_length`, then append `}`. Prefix with `00008000`.
  Expected: the same decoded message.
- Above the cap: the same construction with one additional ASCII space and
  prefix `00008001`. Expected: `handshake_malformed` before body allocation.
- Zero-length prefix `00000000`; expected `handshake_malformed`.
- Insert `,"version":1` before the closing brace: duplicate-field refusal.
- Insert `,"unexpected":true` before the closing brace: unknown-field refusal.
- Replace the message with ASCII `null`, append a second top-level `{}`, or
  replace the first byte with `ff`: malformed refusal.
- Replace each required field independently with JSON `null`; insert a second
  `logical_replica_id` key in the nested dataset, including a repeated equal
  value: malformed refusal. These apply at every nested scope, not just roots.
- Replace each required credential field with null, or add an unknown one.
  Re-sign each mutated claim's exact JSON with the fixture regional key before
  verification: `credential_malformed`. Re-sign a duplicate claim key the same
  way.
- Append `=` to a nonce or signature's base64url spelling: malformed refusal.
- Expire the same credential at `1800000600000` milliseconds: `credential_expired`;
  evaluate it at `1799999999999`: `credential_not_yet_valid`.
- Accept `previous_key_credential` with regional+previous key configuration;
  remove `previous`: `credential_unknown_key`.
- Change the accepted expected header type: `wrong_purpose`; never inspect a
  foreign token's audience by pooling signing keys.
- A 10,241-byte ASCII compact input is `credential_malformed` before shared
  verification. Padding a segment or adding a fourth segment is malformed.

Changing one role-ordered endpoint, nonce or dataset field in the frozen
transcript while retaining its signature must refuse. A completed challenge
cannot be reused. These are admission evidence, not device-trust creation.

## Producer mapping

| Row | Foundation observation | Later evidence still owed |
| --- | --- | --- |
| DS-D01 | `wrong_audience` with valid purpose/type/key; real foreign type gives `wrong_purpose` | API integration |
| DS-D03 | `wrong_region` before issuer mismatch | No EU deployment claimed |
| DS-D05 | `credential_expired` exactly at expiry | Worker frame close |
| DS-D07 | injected current device revocation gives `device_revoked` | Hosted reconciliation |
| DS-D08 / DS-D10 | changed client/worker transport endpoint gives `endpoint_mismatch` | Real Iroh adapters |
| DS-D09 | changed transcript or consumed challenge gives `handshake_replay` | Worker composition |
| DS-D11 | different logical replica gives `replica_mismatch` | Current catalog adapter |
| DS-D12 | Cloud principal on device path gives `principal_not_allowed` | Provisioning integration |
| DS-D13 / DS-D14 | changed membership/access epoch gives exact row code | Worker freshness |
| DS-D15 | a lease replaced between challenge and finish gives `replica_fenced` from the worker's own authority | Lease repository |
| DS-D19 | different lineage gives `cloud_incarnation_stale` | Delete/re-enable |
| DS-D20 / DS-D21 / DS-D22 / DS-D27 | injected current authority refusal preserved unchanged | Worker lifecycle |
| DS-D33 | pending actual peer-below-5 `profile_units_required` observation; control5→6 is separate compatibility evidence | Physical old-client UI |
| DS-D36 | no conversion from Cloud principal to bilateral device identity | Cloud completion/restart |

Malformed message code is `handshake_malformed`; deadline is `handshake_timeout`.
Credential structural/crypto errors retain the existing Plan 6 exact codes.
These code additions have concrete foundation producers, not new UI text.

## Persistence and compatibility

D172 writes only `kind=cloud` and the 16-byte logical replica id. SQL NULL
retains historical from-client/Store interpretation without rewriting the row;
malformed, null JSON and unknown non-null tags are corruption. Workspace schema
12 has no structural SQL, solely an old-writer refusal fence. Device records
and peer5 are unchanged. Control6 retains frame1 and rejects mixed local builds.

The approved generic catalog uses `lineage_id`, not the older tentative
`cloud_dataset_incarnation_id` column name in admission prose. The frozen
mapping records the real catalog column without changing either wire field.
