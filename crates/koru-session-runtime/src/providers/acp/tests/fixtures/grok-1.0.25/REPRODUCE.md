# Installed Grok child-exclusion reproduction

This opt-in Windows probe runs the exact Grok 1.0.25/f7e67d6988e2 binary
against a loopback-only synthetic model. It never reads or copies credentials.
It creates a fresh target/grok-child-refusal-* directory, redirects provider
home and temp there, supplies a synthetic key, and keeps only closed evidence.
No provider package or global settings are modified.

From the repository root, with the exact installed grok and Node on PATH:

    $env:KORU_LIVE_PROVIDER_SMOKE='1'
    node crates/koru-session-runtime/src/providers/acp/tests/fixtures/grok-1.0.25/local-refusal.mjs
    node crates/koru-session-runtime/src/providers/acp/tests/fixtures/grok-1.0.25/local-refusal.mjs --persisted-enable

The second run sets conflicting persisted subagents.enabled=true. Both must
exit zero and report qualified=true. Each of the three injected aliases must
be refused for its exact synthetic tool-call ID, have a failed ACP update and
end its turn. All model request inventories must omit the three aliases while
retaining nonchild tools. The process tree is terminated in finally with a
separate overall deadline. Sanitized JSON remains in each fresh target folder.

The mock rejects non-loopback proxy requests and CONNECT, but this is observed
network evidence rather than an OS firewall guarantee. Local discovery and
background model requests are counted separately from the tested turns.
This proof uses advertised xai.api_key with synthetic credentials; it does not
qualify API-key-only production authentication, the cached-token route, or a
real Koru endpoint/cancellation run. macOS/Linux need their own process cleanup
and installed-binary qualification.
