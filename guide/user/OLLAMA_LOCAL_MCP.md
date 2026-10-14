# Connect a local Ollama model to Koru

Ollama runs models and supports tool calling, but `ollama run` is not itself an
MCP host. Use an MCP-capable client such as OpenCode, with Ollama as that
client's model backend.

## Prepare the local model and Koru

Install Ollama and pull a tool-capable model. Large MCP sessions benefit from a
context window of at least 64K tokens.

```powershell
ollama pull qwen3.5
```

Build Koru and leave Store running:

```bat
koru.bat run-all --mcp
```

## Configure OpenCode

Install OpenCode, then create or merge `opencode.json` in the project (or its
global configuration location):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "koru": {
      "type": "local",
      "command": [
        "C:\\absolute\\path\\to\\repo\\target\\debug\\koru-mcp.exe",
        "serve",
        "--stdio",
        "--root",
        "C:\\absolute\\path\\to\\repo\\.dev-data\\history-smoke"
      ],
      "enabled": true,
      "timeout": 30000
    }
  }
}
```

Launch OpenCode through Ollama:

```powershell
ollama launch opencode
```

OpenCode owns the stdio bridge. Do not manually start another MCP process.

## Smoke test

> Use only the Koru tools. List workspaces and documents, read the History smoke
> document, insert a /document reference to an existing document at the end of
> its first paragraph, then read it again and verify the routed reference.

If the model invents `ordinal`, raw CRDT operations, or nested document nodes,
ensure it loaded the latest tool schemas and add this instruction:

> Koru accepts semantic operations only. Read immediately before editing, use
> parent and sibling UUIDs, and use insert_inline for /document references.

`store_unreachable` means Store is missing or uses a different root; start it
and the next call connects. `storage_failed` means Store is there and its
storage failed. A revision
conflict means another writer changed the document; read again and retry.

References: [Ollama tool calling](https://docs.ollama.com/capabilities/tool-calling), [Ollama with OpenCode](https://docs.ollama.com/integrations/opencode), [OpenCode MCP servers](https://dev.opencode.ai/docs/mcp-servers/)
