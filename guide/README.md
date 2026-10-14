# Koru guides

These guides are part of the public source repository and are available in a
normal clone. Start with the section for what you want to do.

| Task | Read |
| --- | --- |
| Use Koru, connect devices, or connect an MCP client | [User guides](user/README.md) |
| Build, test, debug, or change Koru | [Contributor guides](contributing/README.md) |
| Submit a contribution and sign off a commit | [CONTRIBUTING.md](../CONTRIBUTING.md) |
| Find an environment variable, shortcut, event, or API contract | [Generated references](../reference/README.md) |
| Understand document operations over MCP | [MCP model and tools](../crates/koru-mcp-server/MCP.md) |
| Check licensing and third-party attribution | [Licensing](../licensing/README.md) |

## Keeping documentation current

Update these pages in the same change as the behavior they describe. User
instructions belong under `user/`; building and implementation guidance belongs
under `contributing/`. Keep contribution policy in the root `CONTRIBUTING.md`.
Generated references stay under `reference/` and are updated through their
generators.

Internal plans, ADRs, company notes, and hosted-service operations stay in the
private `docs/` submodule. Public guides must stand on their own. Link to source
or another public page when a reader needs more detail. A maintainer records
internal decisions for outside contributions, as described in
[governance](../GOVERNANCE.md#proposals-and-decisions).

After moving a page, update links and source comments throughout the repository
and run `python scripts/checks/oss-links.py .` from the repository root.
