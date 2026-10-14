# Claude wrapper 0.76.0 sanitized evidence

Pinned source: v0.76.0, c2e4815029ef3962787ecaefe208b0b6f8b81302.
https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.76.0/src/auth-status.ts
https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.76.0/src/acp-agent.ts

auth-none.json and auth-account.json preserve the notification carrier and
closed kind observed on Windows x64 on 2026-09-10, respectively with isolated
unsigned config and normal cached CLI authentication. Labels and account fields
are synthetic sentinels, not captured content. auth-future.json is an adversarial
future-kind fixture. The empty capability marker contains no status.

The wrapper's 0.3.257 SDK bundled native runtime was 2.1.257. Startup with Node
absolute and PATH excluding raw claude succeeded with HTTP=true/protocol=1;
session/new still succeeded in the unsigned case. Raw claude presence with
wrapper absent was independently checked. Missing-bundle and invalid-override
errors are under acp_failure/fixtures. Those files retain only fixed diagnostic
signatures, with a replacement path for the override.

Refresh by installing exactly @agentclientprotocol/claude-agent-acp@0.76.0
under a task target directory, running bounded initialize/session-new with no
model prompt, and retaining only these closed structured facts. Never copy
credentials, raw account fields or provider frames into the fixtures.
