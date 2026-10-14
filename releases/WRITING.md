# Release-writing instructions

Write a release descriptor for the requested Koru release channel.

## Gather the release context

Fetch the published Windows manifests for stable, beta, and nightly from
`https://updates.koru.do/channels/<channel>/windows-x86_64.json`, adding a fresh
`cb` query parameter to bypass the channel cache (ADR-68). Read each manifest's
`version`, `build_number`, and `commit`. A missing manifest means the channel
has not been published; do not mistake a network failure for an empty history.

Read the current git HEAD, UTC date, and working-tree status using the tools
available on this platform. Gather this context yourself if it was not already
provided with the request.

## Choose the exact descriptor

If the publisher supplied an exact descriptor path, use it. Do not infer a new
version or change the filename, even if the date or published manifests have
changed since publication began.

Otherwise, read `crates/koru-release/src/version.rs` and apply its version rules
to the published manifests. Do not guess the scheme. Stable advances the
published stable patch (or starts at `0.1.0`); beta uses `beta.N` off the stable
line; nightly uses the patch after the higher of beta/stable, plus
`-nightly.<UTC date>+<HEAD>`. Write `releases/<full-version>.toml`.

## Read what changed

Use the requested channel's published commit as the baseline for
`git log <published-commit>..HEAD --no-merges`, then inspect the diffs carrying
user-visible changes. Ignore pure refactors and anything else a user could
not notice. If this channel has no published baseline,
inspect the available history and describe only features supported by evidence.

## Write the descriptor

Use TOML with all three required fields: `title` (string), `summary` (array of
strings), and `body` (Markdown string). None may be blank. For example:

```toml
title = "A faster, more reliable Koru"
summary = ["Documents now open faster."]
body = """
## Documents

Explain the user-visible change here.
"""
```

- `title`: one short phrase naming the release's theme, not a version number.
- `summary`: two to four short sentences about what a user can now do or see.
  This is the in-app update prompt. No commit hashes, crate names, or internal
  types.
- `body`: group the notes under `##` headings by area. It becomes an immutable
  page at `https://updates.koru.do/releases/<version>/index.html`. Include useful
  detail for someone using Koru, rather than someone maintaining it.

If nothing user-visible changed, say so instead of inventing features or padding
the notes. An honest one-line nightly is enough. Follow `releases/README.md`
for the descriptor's publishing context.

Only write the selected descriptor and preserve other working-tree changes.
Do not commit, tag, build, or publish. Print the filename when finished.
