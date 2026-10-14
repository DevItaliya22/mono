# syntect

Syntax highlighting for code blocks: turning a language + text into coloured
byte ranges. `syntect` 5, configured pure-Rust.

New to Rust syntax? [../developers/RUST.md](../RUST.md) first.

---

## Why this and not something else

`syntect` with `default-fancy` swaps Oniguruma for `fancy-regex`, so there is
**no C dependency** — the only other C in the tree is inside wgpu. The
alternatives (`inkjet`, `tree-sitter-highlight`) highlight better via tree-sitter
grammars but bring C build dependencies and per-language crates.

It is linked in **one place only**: the `koru-blocks` crate
(ADR-45),
behind a `Highlighter` trait, so the choice stays reversible.

Its documented weakness is being "absurdly slow in debug mode" because the
engine is Rust rather than always-optimised C; the workspace's
`[profile.dev.package."*"] opt-level = 3` already removes that.

## The one integration point

```rust
// crates/koru-blocks/src/lib.rs
pub trait Highlighter {
    fn highlight(&self, text: &str, language: Option<&str>, dark: bool)
        -> Vec<(std::ops::Range<usize>, peniko::Color)>;
}
```

The editor calls the free function `koru_blocks::highlight(..)` from
`rowlayout::build_row`, pushing each range as a parley `StyleProperty::Brush`
— exactly the path a mark takes, without touching the CRDT. Ranges are **UTF-8
bytes** (the coordinate system the rest of the editor uses), accumulated across
`'\n'` so a multi-line block highlights as one piece.

## Derived, never stored

Syntax colour is a function of the text and the language. It would bloat every
sync and be stale one keystroke later, so it is **computed at layout time and
never stored**. The only thing the document persists is `NodeMeta::language`,
which names the grammar. User highlight colour (`hl`) *is* stored, because it is
intent rather than derivation.

## No content-based auto-detect

syntect has no reliable content-based language detection, so a code block with
no `language` renders uncoloured. The markdown fence (```` ```rust ````) sets
the language at creation; the hover chip is the always-available override.

## Resolution

A stored token ("rust", "py", "js") resolves to a grammar by exact name, then
extension, then a case-insensitive name match — which is what makes a lowercased
fence token like `rust` resolve to the "Rust" grammar, and `py` to Python via
its extension.

## Where to look

- `crates/koru-blocks/src/lib.rs` — the trait, the syntect impl, the fence
  parser, the grammar catalog
- `crates/koru-editor/src/rowlayout.rs` — where the ranges become parley styles
- [syntect docs](https://docs.rs/syntect) — upstream
