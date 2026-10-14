// SPDX-License-Identifier: MPL-2.0

//! Tag **names**: the fold key that decides identity, the `/` path grammar that
//! gives the flat table a hierarchy, and what counts as a `#tag` in a line of
//! text.
//!
//! **A leaf that links nothing but `unicode-normalization`**, which is the
//! whole reason it is a crate rather than a module of `koru-core`. Four things
//! have to agree about what a tag name *is*: the editor's chooser narrows a
//! list with it, the store's query parser lifts `#fun` out of a search with it,
//! the search box completes a half-typed one with it, and the model decides
//! identity with it. Two of those four have no node model and must not acquire
//! one to answer a question about a string — the same reason `koru-link` holds
//! the `koru://` grammar and deliberately does not depend on `koru-core`.
//!
//! `koru_core::tags` re-exports everything here, so a caller that already has
//! the node model sees no change; the pieces that name a `TagId` — `TagLanding`
//! and `resolve_copied_tag` — stay there, because an id is not a name.
//!
//! Two rules, and everything here is one of them.
//!
//! **Identity is case-insensitive; display is case-preserving.** `#HOME` and
//! `#home` are one tag; the name is stored as first typed and shown that way.
//! The comparison unit is [`fold_key`] — never `eq_ignore_ascii_case`, which is
//! what the picker used to use and which let `#STRASSE`/`#straße` through as
//! two records in a product committed to eight languages. Every tag question
//! folds through this one function: duplicate detection on create, the picker's
//! filter and its create offer, hierarchy segment comparison, parent creation,
//! and group-by folding.
//!
//! **Hierarchy is the name, parsed at the edges** ([ADR-185]). `#fun/home` is a
//! tag *named* `fun/home`; `#fun` is its ancestor because the folded segments
//! of one are a prefix of the other's, not because a record points anywhere.
//! There is deliberately no stored `parent`: it would move every reader and
//! every wire type, and two peers could then build a cycle that needs a second
//! bounded walk and a second tie-break. A `parent` key stays additive on top of
//! a working naming convention; the reverse is not true.
//!
//! # The fold, precisely
//!
//! For each `/`-separated segment: **trim, NFKD-decompose, drop combining
//! marks, full-case-fold, collapse `i` + COMBINING DOT ABOVE back to `i`, then
//! recompose NFC.**
//!
//! **The first three steps are `koru-search`'s normalizer, deliberately**
//! (`crates/koru-search/src/normalize.rs`: NFKD, drop combining marks,
//! lowercase). The model and the index have to agree about what one word is, or
//! we recreate for accents exactly the split we just closed for capitals — a
//! table holding `#café` and `#cafe` as two tags that search, and the person
//! who typed them, consider one. So the tag fold is **at least as coarse as
//! search's**: anything search calls one word, the tag table calls one tag.
//!
//! Four notes, each a thing that had to be checked rather than assumed:
//!
//! - **NFKD, not NFD**, because that is what search uses: the *compatibility*
//!   decompositions are part of the agreement. `ﬁ` folds to `fi`, `²` to `2`,
//!   half-width kana `ｱ` to `ア`. Canonical decomposition alone would leave all
//!   three apart from their ordinary spellings on one side of the seam only.
//! - **Dropping combining marks is what folds diacritics.** `#Café`, `#café`
//!   and `#cafe` are one tag; so are `#résumé` and `#resume`. Arabic
//!   harakat and Japanese voiced marks go the same way, again matching search.
//! - The case fold is `to_uppercase().to_lowercase()`, **not** `to_lowercase()`
//!   alone. Rust's `to_lowercase` is the Unicode *lowercase* mapping, and
//!   `ß` is already lowercase — so `"STRASSE"` and `"straße"` lower to
//!   `"strasse"` and `"straße"` and would stay two tags. Round-tripping through
//!   uppercase performs the `ß → ss` expansion that Unicode's *full case
//!   folding* specifies, and agrees with full folding on the other expansions
//!   (final sigma `ς → σ`, the titlecase digraphs).
//! - The dot-above collapse is what makes Turkish `İ` fold with `i`. NFKD +
//!   mark-stripping already achieves that (U+0130 decomposes to `I` + U+0307),
//!   so this step is now a **guard rather than the mechanism** — it is kept so
//!   that removing either half alone cannot quietly bring `#İş`/`#iş` back as
//!   two tags. It removes a combining dot **only** directly after an `i`, never
//!   a mark anywhere else.
//!
//! # Where it is still coarser than search, and why
//!
//! Three places, all in the same direction — the tag fold folds *more*, never
//! less, so the invariant above holds and no tag can split a search result.
//! They are listed rather than left implicit, and pinned by
//! `the_tag_fold_never_splits_what_search_joins`:
//!
//! | | tag fold | search |
//! | --- | --- | --- |
//! | `ß` / `ss` | one (`STRASSE` = `straße`) | two — `to_lowercase` leaves `ß` alone |
//! | final sigma `ς` / `σ` | one | two — same reason |
//! | dotless `ı` / `i` | one | two |
//!
//! The first is a stated product requirement, and the third is the accepted
//! cost of folding Turkish `İ` with `i`: under strict Turkish rules `ı` and `i`
//! are two letters, and Koru treats every spelling of the Latin i as one. All
//! three would be closed by giving search full case folding, which is a change
//! to an index and its rebuild rather than to a name.
//!
//! [ADR-185]: ../../../docs/decisions/ADR-201-225.md#adr-208-tagnames--a-tag-name-folds-for-identity-and-parses-for-hierarchy

use std::fmt;

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// What separates one path segment from the next.
pub const SEPARATOR: char = '/';

/// How deep a tag path may go, counted in segments: `a/b/c/d/e` is legal and
/// `a/b/c/d/e/f` is not.
///
/// Five is a *guardrail on typing*, not a model limit — the model stores a
/// string and could hold any depth. It is here so a pasted URL-shaped word does
/// not silently become a five-level branch nobody meant, and it is generous
/// enough that no real filing scheme reaches it: Bear ships three levels of
/// nesting in its own examples.
pub const MAX_DEPTH: usize = 5;

/// U+0307. See the module docs — the Turkish dotted-`I` rule.
const COMBINING_DOT_ABOVE: char = '\u{0307}';

/// The path's segments, trimmed, with empty ones dropped — so `"fun / home"`,
/// `"fun//home"` and `"fun/home"` all read as `["fun", "home"]`.
pub fn segments(name: &str) -> impl DoubleEndedIterator<Item = &str> {
    name.split(SEPARATOR)
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
}

/// **The comparison unit for every tag question.** Two names name the same tag
/// exactly when their fold keys are equal.
///
/// The key is itself a `/`-joined path, so a prefix test on the *segments* of
/// two keys is the ancestor test ([`is_ancestor`]). It is derived, never
/// stored: nothing in the system document is keyed by it, which is why
/// hierarchy and case folding cost no schema version.
pub fn fold_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, segment) in segments(name).enumerate() {
        if i > 0 {
            out.push(SEPARATOR);
        }
        out.push_str(&fold_segment(segment));
    }
    out
}

/// One segment's fold key. See the module docs for why it is spelled this way,
/// and for the three places it stays coarser than `koru-search`'s normalizer.
pub fn fold_segment(segment: &str) -> String {
    // NFKD, minus the combining marks — char by char, which is
    // `koru_search::normalize::normalize`'s own loop. Keep the two in step.
    let mut stripped = String::with_capacity(segment.len());
    for ch in segment.trim().chars() {
        for decomposed in ch.to_string().nfkd() {
            if is_combining_mark(decomposed) {
                continue;
            }
            stripped.push(decomposed);
        }
    }
    let folded = stripped.to_uppercase().to_lowercase();
    // The Turkish guard. Redundant after the strip above, and kept so that
    // removing either half alone cannot bring `#İş`/`#iş` back as two tags.
    let mut out = String::with_capacity(folded.len());
    let mut after_i = false;
    for ch in folded.chars() {
        if after_i && ch == COMBINING_DOT_ABOVE {
            continue;
        }
        after_i = ch == 'i';
        out.push(ch);
    }
    out.nfc().collect()
}

/// Each segment's fold key, which is what hierarchy compares.
pub fn fold_segments(name: &str) -> Vec<String> {
    segments(name).map(fold_segment).collect()
}

/// The last segment — what a chip renders. `"fun/home"` is drawn `home`; the
/// path belongs in a hover, not on the row.
pub fn leaf(name: &str) -> &str {
    segments(name).next_back().unwrap_or("")
}

/// The parent path, or `None` for a root tag: `"a/b/c"` → `"a/b"`.
pub fn parent(name: &str) -> Option<String> {
    let segments: Vec<&str> = segments(name).collect();
    (segments.len() > 1).then(|| segments[..segments.len() - 1].join("/"))
}

/// Every proper ancestor path, outermost first: `"a/b/c"` → `["a", "a/b"]`.
/// These are the records `SystemDoc::create_tag` brings into being
/// alongside the leaf.
pub fn ancestors(name: &str) -> Vec<String> {
    let segments: Vec<&str> = segments(name).collect();
    (1..segments.len())
        .map(|n| segments[..n].join("/"))
        .collect()
}

/// Whether `ancestor` is a **strict** ancestor of `name`, comparing segment by
/// folded segment — so `#Fun` is an ancestor of `#fun/home`.
pub fn is_ancestor(ancestor: &str, name: &str) -> bool {
    let ancestor = fold_segments(ancestor);
    let name = fold_segments(name);
    !ancestor.is_empty() && name.len() > ancestor.len() && name.starts_with(&ancestor[..])
}

/// Whether `ancestor` matches `name` **or** anything under it — the predicate a
/// filter or a group heading wants, since matching a parent matches its
/// descendants.
pub fn covers(ancestor: &str, name: &str) -> bool {
    let ancestor = fold_segments(ancestor);
    let name = fold_segments(name);
    !ancestor.is_empty() && name.len() >= ancestor.len() && name.starts_with(&ancestor[..])
}

/// Why an entry surface refused a typed path.
///
/// **These are the entry surface's rules, never the table's** (D5: warn, never
/// block). `SystemDoc::create_tag` takes any string at all, because the
/// model does not get to police prose and
/// a document written by another build must still read back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TagPathError {
    /// Nothing but whitespace and separators.
    Empty,
    /// A leading or trailing `/`. Interior `//` is collapsed rather than
    /// refused — an extra keystroke mid-path is a slip, while `/fun` and `fun/`
    /// are a different intention that has no meaning here.
    EdgeSeparator,
    /// More than [`MAX_DEPTH`] segments.
    TooDeep { depth: usize, max: usize },
}

impl fmt::Display for TagPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "a tag needs a name"),
            Self::EdgeSeparator => write!(f, "a tag path cannot start or end with '/'"),
            Self::TooDeep { depth, max } => {
                write!(f, "a tag path goes at most {max} levels deep, not {depth}")
            }
        }
    }
}

/// A tag name that has passed the entry surface's guardrails: trimmed segments,
/// no edge separator, no empty segment, at most [`MAX_DEPTH`] deep.
///
/// Holding one is the proof the guardrails ran. The *display* form is what it
/// carries — case exactly as typed — and [`fold_key`](Self::fold_key) is what
/// decides whether it names a tag that already exists.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TagName {
    display: String,
}

impl TagName {
    /// Apply the guardrails. Trims each segment and collapses interior `//`;
    /// refuses an empty name, an edge `/`, and a path deeper than
    /// [`MAX_DEPTH`].
    pub fn parse(input: &str) -> Result<Self, TagPathError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(TagPathError::Empty);
        }
        if trimmed.starts_with(SEPARATOR) || trimmed.ends_with(SEPARATOR) {
            return Err(TagPathError::EdgeSeparator);
        }
        let segments: Vec<&str> = segments(trimmed).collect();
        if segments.is_empty() {
            return Err(TagPathError::Empty);
        }
        if segments.len() > MAX_DEPTH {
            return Err(TagPathError::TooDeep {
                depth: segments.len(),
                max: MAX_DEPTH,
            });
        }
        Ok(Self {
            display: segments.join("/"),
        })
    }

    /// The name as it will be stored and shown — the user's own casing.
    pub fn as_str(&self) -> &str {
        &self.display
    }

    pub fn into_string(self) -> String {
        self.display
    }

    pub fn fold_key(&self) -> String {
        fold_key(&self.display)
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        segments(&self.display)
    }

    pub fn depth(&self) -> usize {
        self.segments().count()
    }

    pub fn leaf(&self) -> &str {
        leaf(&self.display)
    }

    pub fn ancestors(&self) -> Vec<String> {
        ancestors(&self.display)
    }

    pub fn is_ancestor_of(&self, other: &str) -> bool {
        is_ancestor(&self.display, other)
    }

    pub fn covers(&self, other: &str) -> bool {
        covers(&self.display, other)
    }
}

impl fmt::Display for TagName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display)
    }
}

// ---------------------------------------------------------------- matching

/// Whether `name`'s folded segments match a folded `query`'s — the predicate
/// behind every tag list that narrows as you type.
///
/// **One definition, because two surfaces narrow with it**: the editor's tag
/// chooser and the search box's completion popover. They draw differently and
/// must not disagree about what `fun/h` means.
///
/// Two shapes of query, because a `/` in one is a statement about structure:
///
/// - **No separator** — `fun` — matches any tag *one of whose segments*
///   contains it. So it finds `#fun`, `#fun/home` and `#fun/outside`, and
///   `home` finds `#fun/home` without the person knowing where it is filed.
/// - **With a separator** — `fun/h` — matches a contiguous run of segments:
///   every segment but the last must match whole, and the last is the prefix
///   still being typed. That is what makes `fun/h` narrow to the children of
///   `fun` rather than reading as one long string.
///
/// Both sides arrive already folded ([`fold_segments`]), so a caller holding a
/// table folds it **once** when it arrives rather than once per keystroke.
pub fn matches_query(query: &[String], name: &[String]) -> bool {
    match query.len() {
        0 => true,
        1 => name.iter().any(|segment| segment.contains(&query[0])),
        n => name
            .windows(n)
            .any(|run| run[..n - 1] == query[..n - 1] && run[n - 1].starts_with(&query[n - 1])),
    }
}

/// [`matches_query`], folding both sides for a caller that has not.
///
/// A trailing `/` folds away, so `fun/` mid-typing reads as `fun` and lists the
/// whole branch — which is the useful answer while a hand is still moving.
pub fn matches(query: &str, name: &str) -> bool {
    matches_query(&fold_segments(query), &fold_segments(name))
}

// ------------------------------------------------------------ sigil tokens

/// What may follow a sigil. Alphanumerics, the path separator, and the two
/// punctuation marks a name is commonly written with.
pub fn is_tag_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, SEPARATOR | '-' | '_')
}

/// The **sigil-led token** containing `caret`, as a byte range over `text`, or
/// `None` when the caret is not in one.
///
/// **A sigil counts only where a word starts** — the beginning of the text or
/// after whitespace — so `a#b` is the text it has always been and `##a` is
/// literal text too: the first `#` names nothing and the second does not start
/// a word. For `#` this is the same rule the store's query parser applies when
/// it lifts filters out of a query, and it is here so the two cannot drift: a
/// box that completes a token the store will not treat as a filter is worse
/// than one that completes nothing.
///
/// The range **includes** the sigil, so replacing it writes the whole token.
/// A caret directly after the sigil is inside the token, which is what makes a
/// popover open on the first character typed rather than the second.
///
/// **Why the sigil is a parameter and the grammar above it is not.** The search
/// box leads a typed attribute filter with `/`, and the *token* question —
/// where does this word begin and end, and is the caret in it — is the same
/// question `#` asks. Answering it twice is how `a#b` and `10/28` would end up
/// disagreeing about what a word start is. The `key:op value` grammar that
/// follows a `/` has exactly one consumer and stays there
/// ([ADR-218](../../../docs/decisions/ADR-201-225.md#adr-218-filterchooser--the-search-box-parses-its-own-key-filters-and-the-chooser-is-built-from-the-definitions)).
///
/// Note that [`is_tag_char`] admits [`SEPARATOR`], so a second `/` inside a
/// token is a body character rather than a fresh sigil. That is what makes
/// `#fun/home` one token — and, for the `/` sigil, what makes `10/28` and
/// `/home/user` answer `None`: the nearest `/` before the caret is preceded by
/// a letter or a digit, so it starts no word.
pub fn token_at(text: &str, caret: usize, sigil: char) -> Option<std::ops::Range<usize>> {
    let caret = caret.min(text.len());
    let start = text[..caret]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_tag_char(*ch) || *ch == sigil)
        .find(|(_, ch)| *ch == sigil)
        .map(|(at, _)| at)?;
    // Only at a word start.
    let starts_word = text[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    if !starts_word {
        return None;
    }
    let body = start + sigil.len_utf8();
    // Nothing between the sigil and the caret may be a non-tag character.
    if text[body..caret].chars().any(|ch| !is_tag_char(ch)) {
        return None;
    }
    let end = body
        + text[body..]
            .char_indices()
            .take_while(|(_, ch)| is_tag_char(*ch))
            .map(|(at, ch)| at + ch.len_utf8())
            .last()
            .unwrap_or(0);
    Some(start..end)
}

/// [`token_at`] with the `#` sigil — what counts as a `#tag` in a line of text.
pub fn hash_token_at(text: &str, caret: usize) -> Option<std::ops::Range<usize>> {
    token_at(text, caret, '#')
}

#[cfg(test)]
mod tests;
