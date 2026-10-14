# ACP 1.7.0 wire fixture

Status: test-only. This fixture proves wire shapes and failure machinery; it
qualifies no installed agent and registers no managed profile.

Koru links exactly agent-client-protocol-schema =1.7.0 with
default-features = false, under Apache-2.0. Its registry archive SHA-256 is
ca98360c7bb8cc97d7acd49e2a8a851c3f7bee6b2f0535036d8ab86b5fcd223d,
enforced by the root Cargo.lock. The crate records upstream commit
272bf799f35a258c6a4107a0410ed361e83683d3, path agent-client-protocol-schema,
in .cargo_vcs_info.json. Source:
https://github.com/agentclientprotocol/agent-client-protocol/tree/272bf799f35a258c6a4107a0410ed361e83683d3/agent-client-protocol-schema.

schema.json is generated from the ten v1 request, response and notification
types used by the adapter. Its root groups those payload families and includes
their complete generated definitions. The separate generate/ workspace and
lockfile enable schemars only for maintenance. It is not a workspace member,
a build script, an ACP SDK, or a dependency of the session worker. Koru's new
dependency resolves to exactly one additional package in the root lockfile.

Run from the repository root (Python jsonschema 4.24.0 was used):

    python tests/fixtures/acp/1.7.0/validate.py --regenerate
    python tests/fixtures/acp/1.7.0/validate.py --check-generated
    cargo test -p koru-session-runtime --lib acp_fake

The first command regenerates with cargo run --locked and writes UTF-8 without
a BOM. The second independently regenerates and compares, then uses JSON Schema
2020-12 to validate payloads.json, every permission kind, both permission
outcomes, and every stable stop reason. Ordinary Rust tests parse all ten
payloads with the pinned types and drive the fake through production JSONL
framing. No ordinary test regenerates files or launches a provider.

All payloads are synthetic. Native session/tool/option ids, approval labels,
prompt, response, path and bearer are replacement markers. None was captured
from an authenticated session. Fake peer/state Debug prints only counters.
sent() is an explicit private test inspection seam and must never be logged.

Two wire details matter:

- The crate's wire types are under v1; the independent negotiated initialize
  protocolVersion is 1. A crate version is not a protocol integer.
- Permission results have an extra outcome level:
  {"outcome":{"outcome":"cancelled"}} or
  {"outcome":{"outcome":"selected","optionId":"<choice>"}}.
  The shorthand in GENERICAGENT.final sections 4.5/4.6 omits that outer level.
- The schema crate defaults or skips some malformed optional fields during
  deserialization. Readiness must test the actual mcpCapabilities.http boolean,
  never count successful deserialization as HTTP support.

The fake is providers::acp_fake, compiled only under cfg(test), implementing the
existing JsonRpcTransport. It can fail every ProviderStage; the discovery,
version and spawn hooks are called before transport construction. It can hold
arbitrary option sets pending, refuse unknown/replayed answers, ignore cancel,
generate endless late updates without allocating an endless queue, queue
extension traffic and terminate unexpectedly. State observes accepted prompts,
cancel notifications, private answers and shutdown calls. Its receive returns
immediately: adapter deadline and process-tree cleanup are G7's tests over this
seam and the existing process transport tests. AgentControl retains the caller's
turn identity; ACP has no Koru turn-id field and this fake mints none.

Human-only evidence still owed: each actual provider's authentication, HTTP MCP
bearer forwarding, native-child controls, cancellation and registered result.

Vendored schema SHA-256 (UTF-8, LF):
9c6e22ba5b302781468e508a11dc3f0f4ef8aba61e839c0387343dbd1d947d88.

G7 implements the adapter against this unchanged pinned fixture. Run
`cargo test -p koru-session-runtime --lib providers::acp` for offline adapter,
fake-peer and failure-normalization coverage. The adapter additionally tests
independent protocol/HTTP gates, exact private bearer headers, caller-owned
turns, asynchronous prompts, all stop reasons, native images, unknown requests,
malformed frames, EOF, approval expiration and silent/flooding/full-queue
cancellation deadlines. No fixture here qualifies a real provider.