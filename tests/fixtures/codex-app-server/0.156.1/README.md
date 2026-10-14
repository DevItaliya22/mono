# Codex 0.156.1 evidence

Admitted on Windows on 2026-09-25 with the normally installed `codex`, on the
the maintainers' instruction after **Codex (Embedded)** read *Needs a supported version*
in the chat's Local agents card (AGENTUX acceptance). The generated v2 schema
SHA-256 is
`995fc3b8f8c469f6787e8fc5be4038c4f31359025edd8480b862e83355f3bf3b`.
Reproduce it with `scripts/dev/probe-codex-app-server.ps1`.

**What was checked.** The schema carries every method, notification, approval
request and input shape the adapter depends on (the `required_*` lists in
`schema-evidence.json`). The opt-in no-prompt factory handshake
(`live_codex_no_prompt_handshake_lists_models_when_opted_in`, under
`KORU_LIVE_PROVIDER_SMOKE`) completed `initialize` and paged `model/list`
against the installed CLI with exactly one text-capable default; no thread
was started and no prompt was sent.

**What was not.** There are no replay fixtures for this version: the 0.154.0
set remains the adapter's boundary fixture, and no model turn, approval,
interruption or image turn has been recorded under 0.156.1. The schema adds
five server requests the adapter does not know (`item/tool/call`,
`item/tool/requestUserInput`, `mcpServer/elicitation/request`,
`attestation/generate`, `account/chatgptAuthTokens/refresh`); the adapter
surfaces an unknown server request as a generic approval, so one of these
arriving mid-turn shows up as a permission row rather than a hang, but that
path has not been exercised. The `features.multi_agent_v2` override that
delegated launches set on 0.153.4 and 0.154.0 is set on 0.156.1 too, since
2026-09-25: `codex features list` on the installed CLI names
`multi_agent_v2` (stable, `false` by default) and accepts the override. What
follows is the earlier state of this note, kept for the record — the probe
was not repeated at admission;
delegated launches on 0.156.1 set only `features.multi_agent=false`, as
0.150.1 does. Manual chat launches carry no override.
