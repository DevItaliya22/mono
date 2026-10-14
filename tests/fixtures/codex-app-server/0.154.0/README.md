# Codex 0.154.0 evidence

Reviewed on Windows on 2026-09-10 with the normally installed `codex`.
The generated v2 schema SHA-256 is
`f3487938786b729cb6773dbc9e83a7efab9c78c845db7094e8f539f373cbacc9`.
Reproduce it with `scripts/dev/probe-codex-app-server.ps1`.

`initialize.jsonl` is a sanitized projection of a real no-prompt initialize
and model/list exchange: model names, account paths and platform identity are
replaced, and optional model fields are omitted. Exactly one default model
advertised text and image input. No thread or model turn was started.

All other JSONL files are **schema-derived boundary fixtures**, adapted from
0.153.4, and are not claimed as 0.154.0 turn recordings. They exercise the
unchanged parser fields for thread start, text, tools, approval, usage, images,
rerouting and interruption. Additive fields ignored by the adapter remain in
the replay. The registered live gateway tests separately establish actual
model-turn behavior; see the provider note for their results.

No-prompt CLI checks on 0.153.4 and 0.154.0 confirmed that explicit
`features.multi_agent=false` and `features.multi_agent_v2=false` overrides
report both switches off. A separate 0.154.0 probe reported the switches as
false/true when configured that way, so no dependency between them is assumed.
Delegated launches explicitly disable both on these reviewed versions.
The 0.150.1 launch remains unchanged because its v2 switch is unverified.
These probes neither changed the user's config nor started a child agent.
