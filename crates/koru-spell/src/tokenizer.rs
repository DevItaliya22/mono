// SPDX-License-Identifier: MPL-2.0

//! **How a slice of prose becomes checkable words without losing its bytes.**
//!
//! The engine checks words; it deliberately does not tokenize prose (spec §11).
//! So this module owns the cut, under one discipline borrowed from
//! `koru-search`'s normalizer: **the source range and the checked value are
//! kept separate**, and an editor range is never recomputed from a normalized
//! word — normalization may change byte lengths, and a decoration drawn from a
//! normalized offset lands under the wrong glyphs.
//!
//! ## What is and is not a word here
//!
//! - UAX #29 word boundaries (`unicode-segmentation`, the workspace's one pin
//!   for this), which already keep `isn't` and `Steven's` whole — including
//!   the typographic apostrophe U+2019.
//! - `test-file` splits at the hyphen and each side is checked alone, which is
//!   what Hunspell dictionaries expect: they list `test` and `file`, not every
//!   hyphenation.
//! - A token carrying an underscore or a digit (`foo_bar`, `4f7c2a9e`,
//!   `IPv6`) is an identifier, not prose, and is skipped (spec §12).
//! - The pill anchor `U+2060 WORD JOINER` is a Format character UAX #29 would
//!   glue across, welding a pill to its neighbouring word — so the text is
//!   split at anchors *first* and each stretch tokenized alone (plan §P1).
//! - Anything overlapping a caller-supplied excluded range (inline code, a raw
//!   URL) is dropped; the caller knows what is prose, this module does not.
//!
//! ## Normalization, exactly one rule
//!
//! The checked value differs from the source only when the source uses
//! U+2019/U+02BC as an apostrophe: dictionaries spell `isn't` with U+0027.
//! Everything else — case, accents, combining marks — is the dictionary's
//! business, not ours: Hunspell affix files declare their own casing rules,
//! and a tokenizer that lowercased would erase the difference between `paris`
//! and `Paris` that some dictionaries encode.

use std::borrow::Cow;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// The pill anchor: `koru-core` stores a pill as one `U+2060 WORD JOINER` in
/// the text. Restated here as a char constant rather than borrowed, because
/// this crate must not depend on `koru-core`; a test in the editor's spelling
/// controller is what keeps the two spellings from drifting.
pub const PILL_ANCHOR: char = '\u{2060}';

/// One checkable word: where it is in the source, and what to hand the
/// dictionary.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WordToken<'a> {
    /// Byte range over the *source* text — the range a decoration draws from
    /// and a replacement edits. Never derived from `check`.
    pub range: Range<usize>,
    /// The source bytes themselves, exactly as written.
    pub text: &'a str,
    /// What the dictionary is asked about: the source, with typographic
    /// apostrophes folded to U+0027. Borrowed when nothing needed folding.
    pub check: Cow<'a, str>,
}

/// The words worth checking in `text`, skipping everything that overlaps one
/// of `exclusions` (byte ranges over `text`, in any order).
///
/// The caller decides what the exclusions are — inline code, raw URLs, table
/// syntax; this function only promises not to look inside them.
pub fn words<'a>(text: &'a str, exclusions: &[Range<usize>]) -> Vec<WordToken<'a>> {
    let mut tokens = Vec::new();
    // Split at pill anchors first: U+2060 is a Format character UAX #29
    // ignores, so without this cut "word⁠" and its pill would tokenize as one.
    let mut stretch_start = 0;
    for (offset, part) in text.split(PILL_ANCHOR).map(|part| {
        let at = stretch_start;
        stretch_start += part.len() + PILL_ANCHOR.len_utf8();
        (at, part)
    }) {
        for (word_at, word) in part.unicode_word_indices() {
            let mut start = offset + word_at;
            let mut word = word;
            // Romance elision: UAX #29 joins `l'école` into one word, but the
            // dictionaries store `école` — the elided article is grammar, not
            // spelling, so it is dropped and only the tail is checked. The
            // prefix set is closed and short, which is what keeps `isn't`
            // (prefix `isn`, not in the set) whole.
            if let Some((prefix, tail)) = word.split_once(['\'', '\u{2019}', '\u{02BC}'])
                && !tail.is_empty()
                && is_elision_prefix(prefix)
            {
                start += word.len() - tail.len();
                word = tail;
            }
            let range = start..start + word.len();
            if !is_prose_word(word) {
                continue;
            }
            if exclusions
                .iter()
                .any(|excluded| range.start < excluded.end && excluded.start < range.end)
            {
                continue;
            }
            tokens.push(WordToken {
                text: word,
                check: fold_apostrophes(word),
                range,
            });
        }
    }
    tokens
}

/// Whether a UAX #29 word is prose rather than an identifier or a number.
///
/// One alphabetic character somewhere is required (so `42` and `--` never
/// reach a dictionary). Three shapes disqualify, because they are lexically
/// ordinary words the exclusion pass cannot see:
///
/// - a digit or underscore anywhere — `foo_bar`, `4f7c2a9e`, `IPv6`;
/// - a `.` inside — UAX #29 joins `api.koru.do` (and `e.g`) into one word via
///   MidNumLet, and both domains and dotted abbreviations are classic false
///   positives, so a dotted word never reaches a dictionary;
/// - a lowercase→uppercase transition — `getDocumentById` is camelCase, not a
///   misspelling. `McDonald` is skipped by the same rule, which is the right
///   failure direction: not flagging a name beats flagging every identifier.
fn is_prose_word(word: &str) -> bool {
    let mut alphabetic = false;
    let mut previous_lower = false;
    for c in word.chars() {
        if c == '_' || c == '.' || c.is_ascii_digit() {
            return false;
        }
        if c.is_uppercase() && previous_lower {
            return false;
        }
        previous_lower = c.is_lowercase();
        alphabetic |= c.is_alphabetic();
    }
    alphabetic
}

/// The elided articles and conjunctions of the Romance languages (`l'école`,
/// `qu'il`, `dell'arte`), dropped before the dictionary is asked. Matched
/// case-insensitively — `L'école` elides like `l'école`.
fn is_elision_prefix(prefix: &str) -> bool {
    const ELISIONS: &[&str] = &[
        "l", "d", "j", "n", "m", "t", "s", "c", "qu", "jusqu", "lorsqu", "puisqu", "quoiqu",
        "quelqu", "presqu", "un", "dell", "all", "sull", "nell",
    ];
    prefix.len() <= 6
        && ELISIONS
            .iter()
            .any(|elision| prefix.eq_ignore_ascii_case(elision))
}

/// The one normalization: typographic apostrophes become the ASCII one the
/// dictionaries spell contractions with. Borrow when untouched, so the hot
/// path allocates nothing for ordinary words.
fn fold_apostrophes(word: &str) -> Cow<'_, str> {
    if word.contains(['\u{2019}', '\u{02BC}']) {
        Cow::Owned(word.replace(['\u{2019}', '\u{02BC}'], "'"))
    } else {
        Cow::Borrowed(word)
    }
}

#[cfg(test)]
mod tests;
