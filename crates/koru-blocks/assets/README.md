# Fetched syntax assets

## `TypeScript.sublime-syntax`

[bat]'s `assets/syntaxes/02_Extra/TypeScript.sublime-syntax`, verbatim, from
bat v0.24.0. It is not committed: `scripts/setup/fetch-third-party.*` fetches it into
this directory, checked against its pin in `third-party/fetch.manifest`, and
it is gitignored ([third-party/](../../../third-party/README.md)).
bat produced it by machine-converting `TypeScript.tmLanguage` from Microsoft's
[TypeScript-Sublime-Plugin] (bat's commit "Do machine-conversion from
.tmLanguage to .sublime-syntax", PR #1834); the grammar itself is Microsoft's. It is compiled into
`koru-blocks` with `include_str!` and added on top of syntect's bundled
syntax set at load time.

**Why it is needed:** syntect's bundled dump has no TypeScript grammar —
`resolve("typescript")` answered `None`, so TypeScript code blocks rendered
plain. The file is self-contained: every `include:` names a local context,
so nothing else needs to be loaded beside it.

**License:** Apache-2.0, Copyright (c) Microsoft Corporation. The full text is
`crates/koru-release/licences/bundled/typescript-grammar.txt`, and every package
carries it in `THIRD-PARTY-NOTICES.txt`.
This README records the conversion, which is the modification notice Apache-2.0
§4(b) asks for. An earlier version of this README said "MIT, Sublime HQ"; that
was wrong. To refresh it, change its URL and sha256 in the manifest; take the
file whole, because the parser rejects leading comments before the `%YAML`
header.

[bat]: https://github.com/sharkdp/bat
[TypeScript-Sublime-Plugin]: https://github.com/microsoft/TypeScript-Sublime-Plugin
