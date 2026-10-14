# Antigravity ACP server 1.2.1 fixtures

Recorded 2026-09-26 on Windows x64 from Google's official server,
`agy-acp-server-1.2.1-windows-x86_64.zip` from dl.google.com (archive SHA-256
`9b82493819bc14613baa76264d55ad307ddd8ab4a8d6e110edb32da35498c07b`), which
`scripts/setup/fetch-antigravity-acp.ps1` places and verifies. Both executables in the
archive carry a valid Authenticode signature from Google LLC.

- `digest.txt` is the SHA-256 of `agy_acp_server.exe`. The server has no
  `--version` flag (absl rejects it), so this digest is the route's pin, and
  `initialize`'s `agentInfo.version` (`1.2.1`) is checked against it.
- `initialize.json` keeps the capabilities exactly as observed and
  `agentInfo.name`/`version`, the two facts the route reads. The four
  authentication methods are replaced by placeholders; the route calls none of
  them.
- `new-session.json` is the observed shape after a cached Google sign-in: the
  `model` and `mode` select options and the legacy `modes`/`models` fields. The
  session id, every label and the eleven model ids are replaced; three
  placeholder models keep the shape.
- `auth-required.json` is the observed signed-out refusal of `session/new`:
  ACP `-32000` with a `data.message`. Message and detail are replaced.

Recorded by a disposable handshake driver under the scratch directory,
reducing in memory to these fields. No raw frame, OAuth URL, account, project,
path or session id is retained. Refresh the same way against a newly pinned
build, then replay through `routes.rs`.
