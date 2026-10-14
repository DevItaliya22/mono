# Koru patch: onenote_parser

`onenote_parser` **2.0.0** from crates.io (© Markus Siemens and contributors,
MPL-2.0), with Koru's change in [`onenote_parser.patch`](onenote_parser.patch).
`scripts/setup/fetch-third-party.*` fetches the crate, checks it against its pin in
[`../fetch.manifest`](../fetch.manifest), applies the patch and places the
result in `third-party/crates/onenote_parser/`, which the workspace names by
path. It is a patch rather than a dependency on the release until the same
focused API change is published from Koru's onenote.rs fork.

## What changed

- `src/onenote/outline.rs`: outlines, outline groups and outline elements keep
  the OneNote object id that internal hyperlinks point at, and expose it as
  `link_target_id()`.
- `src/onenote/section.rs`: a section exposes the filename it was read from.
- `Cargo.toml`: an empty `[workspace]` table, so the crate is never inferred as
  a member of an enclosing workspace when a worktree sits beneath a checkout.

No parser behavior is changed. The patch is MPL-2.0, as the code it changes.

## Updating

Edit `third-party/crates/onenote_parser/`, then run
`scripts\setup\fetch-third-party.ps1 -UpdatePatch onenote_parser` (or
`bash scripts/setup/fetch-third-party.sh --update-patch onenote_parser`) and put the
printed sha256 in `fetch.manifest`. To move to a new release, change its
version and crates.io sha256 there too, fetch, and resolve whatever no longer
applies. [`../README.md`](../README.md) has the whole procedure.
