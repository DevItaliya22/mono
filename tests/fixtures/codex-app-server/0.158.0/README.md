# Codex 0.158.0 evidence

Admitted on Windows on 2026-09-29 with the normally installed `codex`, on the
the maintainers' instruction after **Codex (Embedded)** read *Needs a supported version*
in the chat's Local agents card again: the installation had moved on from
0.156.1. The generated v2 schema SHA-256 is
`5742a9a7dd41a8b44dca3138f506e013620d4a93573c792b1e5881c053f169a7`
(generated twice, same digest). Reproduce it with
`scripts/dev/probe-codex-app-server.ps1`.

**What was checked.** The schema carries every method, notification, approval
request and input shape the adapter depends on (the `required_*` lists in
`schema-evidence.json`), and `model/rerouted` is still there. Its server
requests are the 0.156.1 set plus the legacy v1 `applyPatchApproval` and
`execCommandApproval`, which a v2 thread does not send. The opt-in no-prompt
factory handshake (`live_codex_no_prompt_handshake_lists_models_when_opted_in`,
under `KORU_LIVE_PROVIDER_SMOKE`) completed `initialize` and paged
`model/list` against the installed CLI: 7 models in 1.6 s, no thread, no
prompt. `codex features list` still names `multi_agent_v2` (stable, `false` by
default) and accepts `-c features.multi_agent_v2=false`, so delegated launches
on 0.158.0 set both child-agent switches off, as on 0.153.4 through 0.156.1.

**What was not.** There are no replay fixtures for this version: the 0.154.0
set remains the adapter's boundary fixture, and no model turn, approval,
interruption or image turn has been recorded under 0.158.0. The five server
requests 0.156.1 added (`item/tool/call`, `item/tool/requestUserInput`,
`mcpServer/elicitation/request`, `attestation/generate`,
`account/chatgptAuthTokens/refresh`) are still unhandled and would reach a
person as a generic approval. The first real turn is the acceptance's.
