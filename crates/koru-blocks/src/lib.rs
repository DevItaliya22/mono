// SPDX-License-Identifier: MPL-2.0

//! Block behaviour isolated from the editor: syntax highlighting for code
//! blocks, the markdown-fence parser, and the grammar catalog the language
//! picker lists.
//!
//! This crate is the only place `syntect` is linked — the pure-Rust
//! (`default-fancy`, no C) syntax engine — so the choice stays reversible
//! behind the [`Highlighter`] trait. The editor calls [`highlight`] at layout
//! time and [`parse_fence`] when Enter lands on a fence line; both are free
//! functions over a process-global `SyntaxSet`/`ThemeSet` loaded once.
//!
//! Syntax colour is **derived here and never stored**: it is a function of the
//! text and the language, would bloat every sync, and would be stale one
//! keystroke later. The only thing the document persists is `NodeMeta::language`
//! (see `koru-core`), which names the grammar.

pub mod table;

use std::ops::Range;
use std::sync::OnceLock;

use peniko::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, Theme, ThemeSet};
use syntect::parsing::{SyntaxDefinition, SyntaxReference, SyntaxSet};

/// Maps a code block's text to colour ranges, in UTF-8 byte offsets.
///
/// The one integration point the editor needs. Returning byte ranges (not
/// unicode scalars) matches the coordinate system the rest of the editor uses;
/// the caller pushes each as a parley `StyleProperty::Brush` the way a mark
/// would.
pub trait Highlighter {
    fn highlight(
        &self,
        text: &str,
        language: Option<&str>,
        dark: bool,
    ) -> Vec<(Range<usize>, Color)>;
}

/// The default [`Highlighter`], backed by `syntect`. A unit struct: the
/// expensive state lives in process globals loaded on first use.
pub struct SyntectHighlighter;

impl Highlighter for SyntectHighlighter {
    fn highlight(
        &self,
        text: &str,
        language: Option<&str>,
        dark: bool,
    ) -> Vec<(Range<usize>, Color)> {
        let ss = syntax_set();
        // No reliable content-based detection in syntect: a code block with no
        // language renders uncoloured, and the user picks one with the chip.
        let Some(syntax) = language.and_then(|l| resolve(ss, l)) else {
            return Vec::new();
        };
        let theme = theme(dark);
        let mut h = HighlightLines::new(syntax, theme);

        let mut out: Vec<(Range<usize>, Color)> = Vec::new();
        let mut offset = 0usize;
        for line in text.split('\n') {
            if let Ok(segments) = h.highlight_line(line, ss) {
                for (style, piece) in segments {
                    let len = piece.len();
                    if len > 0 {
                        out.push((offset..offset + len, to_peniko(style)));
                    }
                    offset += len;
                }
            }
            // The `'\n'` `split` removed; account for it so byte offsets stay
            // anchored in the original text. The final iteration overcounts by
            // one (no trailing newline), but no range consumes it.
            offset += 1;
        }
        coalesce(&mut out);
        out
    }
}

/// Highlight with the default [`SyntectHighlighter`]. The seam the editor
/// calls from its layout pass.
pub fn highlight(text: &str, language: Option<&str>, dark: bool) -> Vec<(Range<usize>, Color)> {
    SyntectHighlighter.highlight(text, language, dark)
}

/// A grammar the language picker can list: a display name and the token that
/// `set_language` stores and [`resolve]] understands back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Language {
    /// Human-readable, as syntect reports it — "Rust", "Python", "JSON".
    pub name: String,
    /// What the document stores: the lowercased name. Round-trips through
    /// [`resolve]] case-insensitively, so "rust" → Rust and back.
    pub token: String,
}

/// Every grammar the bundled syntax set knows, in name order. The picker
/// filters this list as the user types.
pub fn grammar_catalog() -> Vec<Language> {
    let ss = syntax_set();
    let mut out: Vec<Language> = ss
        .syntaxes()
        .iter()
        .map(|s| Language {
            name: s.name.clone(),
            token: s.name.to_lowercase(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse a markdown fence line — `` ``` `` optionally followed by a language
/// info string — into the grammar to pin, if any.
///
/// Returns `Some(Some(lang))` for `` ```rust ``, `Some(None)` for a bare
/// `` ``` `` (fence, auto-detect), and `None` for anything that is not a fence
/// (so a backtick typed mid-sentence is left alone). The whole line must be the
/// fence: an info string with more than one token is treated as not-a-fence so
/// typing prose after the opening backticks does not trigger.
pub fn parse_fence(line: &str) -> Option<Option<String>> {
    let trimmed = line.trim_start();
    let backticks = trimmed.bytes().take_while(|&b| b == b'`').count();
    if backticks < 3 {
        return None;
    }
    let info = trimmed[backticks..].trim();
    let lang = if info.is_empty() {
        None
    } else {
        let mut parts = info.split_whitespace();
        let first = parts.next()?;
        if parts.next().is_some() || first.contains('`') {
            return None;
        }
        Some(first.to_string())
    };
    Some(lang)
}

// --- internals --------------------------------------------------------------

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        // **TypeScript is not in syntect's bundled set** — `resolve("typescript")`
        // answered None, which rendered every TypeScript block plain. The
        // grammar is vendored beside this file (`assets/README.md` has the
        // provenance and licence) and added on top of the defaults. It is
        // self-contained: every include names a local context.
        let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
        match SyntaxDefinition::load_from_str(TYPESCRIPT_GRAMMAR, true, Some("\n")) {
            Ok(syntax) => builder.add(syntax),
            Err(error) => warn(&format!("TypeScript grammar did not parse: {error}")),
        }
        builder.build()
    })
}

/// The vendored TypeScript grammar (`assets/TypeScript.sublime-syntax`),
/// compiled into the binary so a missing asset file is a build error rather
/// than a silently unhighlighted language.
const TYPESCRIPT_GRAMMAR: &str = include_str!("../assets/TypeScript.sublime-syntax");

/// A failure worth one line on stderr: highlighting degrades to plain text,
/// which is quiet by nature. Never a panic — a broken grammar must not take
/// the editor down.
fn warn(message: &str) {
    eprintln!("[koru][blocks] {message}");
}

fn theme_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

fn theme(dark: bool) -> &'static Theme {
    let ts = theme_set();
    // Both ship with `default-themes`. Fall back to whatever the set holds so a
    // build that customises the themes never panics here.
    let name = if dark {
        "base16-eighties.dark"
    } else {
        "InspiredGitHub"
    };
    ts.themes
        .get(name)
        .or_else(|| ts.themes.values().next())
        .expect("default theme set is never empty")
}

/// Resolve a stored language token to a syntax. Tries the exact name, the
/// extension list, then a case-insensitive name match — which is what makes
/// `` ```rust `` (lowercase) resolve to the "Rust" grammar, and "py" to Python
/// via its extension.
fn resolve<'a>(ss: &'a SyntaxSet, lang: &str) -> Option<&'a SyntaxReference> {
    if let Some(s) = ss.find_syntax_by_name(lang) {
        return Some(s);
    }
    if let Some(s) = ss.find_syntax_by_extension(lang) {
        return Some(s);
    }
    ss.syntaxes()
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(lang))
}

fn to_peniko(style: Style) -> Color {
    let c = style.foreground;
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

/// Merge adjacent ranges carrying the same colour, so the layout pass pushes
/// the fewest possible styles.
fn coalesce(ranges: &mut Vec<(Range<usize>, Color)>) {
    let mut out: Vec<(Range<usize>, Color)> = Vec::with_capacity(ranges.len());
    for (range, color) in ranges.drain(..) {
        match out.last_mut() {
            Some((r, c)) if *c == color && r.end == range.start => r.end = range.end,
            _ => out.push((range, color)),
        }
    }
    *ranges = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_fence_is_auto_detect() {
        assert_eq!(parse_fence("```"), Some(None));
        assert_eq!(parse_fence("   ```"), Some(None));
        assert_eq!(parse_fence("````"), Some(None)); // 4 backticks is still a fence
    }

    #[test]
    fn a_fenced_language_is_captured() {
        assert_eq!(parse_fence("```rust"), Some(Some("rust".into())));
        assert_eq!(parse_fence("```rust\n"), Some(Some("rust".into()))); // trailing newline trimmed
        assert_eq!(parse_fence("```ts"), Some(Some("ts".into())));
    }

    #[test]
    fn not_a_fence_when_there_is_prose_after_the_info() {
        // Two tokens after the backticks is not a fence open — it is text.
        assert_eq!(parse_fence("```rust hello"), None);
        // A backtick typed mid-sentence is not a fence.
        assert_eq!(parse_fence("hello `world`"), None);
        // Fewer than three backticks is not a fence.
        assert_eq!(parse_fence("``rust"), None);
    }

    #[test]
    fn highlight_colours_a_known_language_and_stays_in_bounds() {
        let text = "fn main() {\n    println!(\"hi\")\n}";
        let ranges = highlight(text, Some("rust"), false);
        assert!(!ranges.is_empty(), "rust should highlight something");
        for (range, _) in &ranges {
            assert!(range.start <= range.end);
            assert!(
                range.end <= text.len(),
                "range {range:?} escapes the text (len {})",
                text.len()
            );
        }
    }

    #[test]
    fn highlight_clips_to_plain_for_an_unknown_language() {
        // Unknown grammar → no ranges → default brush applies, nothing breaks.
        let ranges = highlight("anything", Some("not-a-real-language"), true);
        assert!(ranges.is_empty());
    }

    #[test]
    fn the_grammar_catalog_is_non_empty_and_round_trips() {
        let catalog = grammar_catalog();
        assert!(
            catalog.len() > 10,
            "default syntax set has dozens of grammars"
        );
        // The token the catalog offers resolves back to a real syntax.
        let rust = catalog
            .iter()
            .find(|l| l.name == "Rust")
            .expect("Rust ships by default");
        assert!(resolve(syntax_set(), &rust.token).is_some());
    }

    #[test]
    fn highlight_resolves_lowercased_and_extension_tokens() {
        // "rust" (lowercase name), "py" (extension), "js" (extension) all resolve.
        for lang in ["rust", "Rust", "py", "js"] {
            assert!(
                resolve(syntax_set(), lang).is_some(),
                "{lang} should resolve to a grammar"
            );
        }
    }

    /// **TypeScript colours.** The grammar is not in syntect's bundled dump —
    /// it is vendored in `assets/` and added on load — and a failure to parse
    /// or add it degrades quietly to plain text, so this pin is the alarm.
    #[test]
    fn typescript_resolves_and_colours() {
        let syntax = resolve(syntax_set(), "typescript").expect("typescript resolves");
        assert_eq!(syntax.name, "TypeScript");

        let text = "interface User {\n  name: string;\n}\nconst user: User = { id: 1 };";
        let ranges = highlight(text, Some("typescript"), false);
        assert!(!ranges.is_empty(), "a TypeScript block is not plain text");
        let mut distinct = ranges.iter().map(|(_, c)| *c).collect::<Vec<_>>();
        distinct.dedup();
        assert!(distinct.len() > 1, "more than one colour: {distinct:?}");
        for (range, _) in &ranges {
            assert!(
                range.end <= text.len(),
                "range {range:?} escapes the text (len {})",
                text.len()
            );
        }
    }
}
