// SPDX-License-Identifier: MPL-2.0

//! Rich text: formatting marks, pill anchors, and the runs a renderer needs.
//!
//! Formatting is a **mark on a range**, not a delimiter in the text. The old
//! model stored `**bold**` literally and parsed it back out, which meant the
//! delimiters were visible, every byte index had to be adjusted by hand, and
//! nesting needed its own parser. `LoroText::mark` carries it as a span that
//! Loro moves for us — including across a concurrent remote edit.
//!
//! # Flags and values
//!
//! A Loro mark can carry any value, not just `true`. Two kinds live here and
//! they are deliberately different types, because they are different things to
//! a person as well as to the code:
//!
//! - [`Mark`] is a **flag** — bold, italic, underline, strikethrough. It
//!   toggles, several can apply at once, and a toolbar draws it as a pressed
//!   button. [`MarkSet`] answers "which of these cover the selection".
//! - [`Script`], size and highlight colour are **values**. They are chosen
//!   rather than toggled, only one can apply at a time, and a toolbar draws
//!   them as a picker. [`TextStyle`] answers "what value covers the selection".
//!
//! # Coordinates
//!
//! Loro addresses text in unicode scalars by default; parley works in UTF-8
//! bytes. Every call here uses the `_utf8` variants and every offset in this
//! module is a byte offset, so nothing has to be converted at the boundary.
//! Mixing the two silently produces off-by-a-few bugs in non-ASCII text, which
//! is why [`Utf8Pos`] exists.

use std::fmt;

use crate::ids::AttrId;

/// A byte offset into a node's text.
///
/// A newtype rather than a bare `usize` because the other plausible unit —
/// unicode scalars, which is Loro's default — is indistinguishable at the type
/// level and wrong in exactly the cases (Korean, Cyrillic, emoji) that are
/// easiest to forget to test.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Utf8Pos(pub usize);

impl Utf8Pos {
    pub fn get(self) -> usize {
        self.0
    }
}

impl fmt::Debug for Utf8Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", self.0)
    }
}

impl From<usize> for Utf8Pos {
    fn from(v: usize) -> Self {
        Self(v)
    }
}

/// The formatting flags a run can carry. Each toggles independently.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    /// Inline code — mono with a background, the intra-line counterpart of a
    /// code block. Configured with `ExpandType::None` (typing after an inline
    /// code span exits it, matching the markdown intuition) rather than the
    /// rightwards expansion the other flags use.
    Code,
}

impl Mark {
    /// The key stored in Loro. Stable — changing it would orphan existing marks.
    pub fn key(self) -> &'static str {
        match self {
            Self::Bold => "b",
            Self::Italic => "i",
            Self::Underline => "u",
            Self::Strike => "s",
            Self::Code => "code",
        }
    }

    pub const ALL: [Mark; 5] = [
        Mark::Bold,
        Mark::Italic,
        Mark::Underline,
        Mark::Strike,
        Mark::Code,
    ];
}

/// Superscript or subscript. A value rather than two flags, because a run
/// cannot sensibly be both — the type says so instead of a rule saying so.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Script {
    Super,
    Sub,
}

impl Script {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Super => "sup",
            Self::Sub => "sub",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sup" => Some(Self::Super),
            "sub" => Some(Self::Sub),
            _ => None,
        }
    }

    /// The OpenType feature that renders it. parley has no baseline-shift
    /// property, so the font's own designed glyphs do the work — which is
    /// better typography than scaling and nudging anyway.
    pub fn feature(self) -> &'static str {
        match self {
            Self::Super => "sups",
            Self::Sub => "subs",
        }
    }
}

/// Storage keys for the value marks. Stable, like [`Mark::key`].
pub const SCRIPT_KEY: &str = "sc";
pub const SIZE_KEY: &str = "sz";
pub const HIGHLIGHT_KEY: &str = "hl";

/// The key a pill anchor is marked with. Its value is the [`AttrId`] of the
/// attribute the pill renders.
pub const PILL_KEY: &str = "pill";

/// Every style key the document configures expansion for.
pub const VALUE_KEYS: [&str; 3] = [SCRIPT_KEY, SIZE_KEY, HIGHLIGHT_KEY];

/// The character standing in for a pill in the text.
///
/// `U+2060 WORD JOINER` is zero-advance, non-breaking and a
/// `Default_Ignorable_Code_Point`, so the shaper hides it and the pill's whole
/// visible extent is the inline box drawn over it.
///
/// It has to be a *real character* rather than a bare position: that is what
/// makes a pill one cluster, so a single arrow key steps over it and a single
/// backspace deletes it. Loro has no embed API — `LoroText` is
/// insert/delete/mark only — so an anchor is the mechanism, and the mark on it
/// is what Loro keeps in place through edits.
pub const ANCHOR: char = '\u{2060}';
pub const ANCHOR_STR: &str = "\u{2060}";
pub const ANCHOR_LEN: usize = 3;

/// The chosen-rather-than-toggled part of a run's formatting.
///
/// `None` everywhere means "inherit the default", which is why these are
/// `Option` rather than carrying a default value: a run with no size mark
/// follows the theme, and a run with `Some(24.0)` does not.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct TextStyle {
    pub script: Option<Script>,
    pub size: Option<f32>,
    /// A colour *name*, resolved by the theme — so a highlight stays legible
    /// when the user switches to dark mode. Storing RGB would freeze it.
    pub highlight: Option<String>,
}

impl TextStyle {
    pub fn is_empty(&self) -> bool {
        self.script.is_none() && self.size.is_none() && self.highlight.is_none()
    }

    /// The values shared by *every* run of a selection; anything that differs
    /// between runs reports `None`.
    ///
    /// The same intersection rule [`MarkSet::covering`] uses, and for the same
    /// reason: a toolbar should show "mixed" rather than picking one run's
    /// answer and pretending it speaks for the rest.
    pub fn covering<'a>(runs: impl IntoIterator<Item = &'a TextRun>) -> Self {
        let mut iter = runs.into_iter().filter(|r| !r.text.is_empty());
        let Some(first) = iter.next() else {
            return Self::default();
        };
        let mut out = first.style.clone();
        for run in iter {
            if run.style.script != out.script {
                out.script = None;
            }
            if run.style.size != out.size {
                out.size = None;
            }
            if run.style.highlight != out.highlight {
                out.highlight = None;
            }
        }
        out
    }
}

/// A run of text sharing one set of marks — the unit a renderer consumes.
#[derive(Clone, PartialEq, Debug)]
pub struct TextRun {
    /// Byte range within the node's text.
    pub start: Utf8Pos,
    pub end: Utf8Pos,
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Inline code: render mono with a background.
    pub code: bool,
    /// The value marks: script, size, highlight colour.
    pub style: TextStyle,
    /// Set when this run is a pill anchor; names the attribute it draws.
    pub pill: Option<AttrId>,
}

impl TextRun {
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start.0..self.end.0
    }

    pub fn is_pill(&self) -> bool {
        self.pill.is_some()
    }

    pub fn has(&self, mark: Mark) -> bool {
        match mark {
            Mark::Bold => self.bold,
            Mark::Italic => self.italic,
            Mark::Underline => self.underline,
            Mark::Strike => self.strike,
            Mark::Code => self.code,
        }
    }

    /// Whether two runs are formatted identically — the test `normalise` uses
    /// to decide whether neighbours can be merged.
    fn formatting_matches(&self, other: &Self) -> bool {
        self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.strike == other.strike
            && self.code == other.code
            && self.style == other.style
    }
}

/// Which flags cover a byte range — what a toolbar shows for the selection.
#[derive(Copy, Clone, PartialEq, Eq, Default, Debug)]
pub struct MarkSet {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
}

impl MarkSet {
    pub fn has(&self, mark: Mark) -> bool {
        match mark {
            Mark::Bold => self.bold,
            Mark::Italic => self.italic,
            Mark::Underline => self.underline,
            Mark::Strike => self.strike,
            Mark::Code => self.code,
        }
    }

    pub fn toggle(&mut self, mark: Mark) {
        let on = self.has(mark);
        self.set(mark, !on);
    }

    pub fn is_empty(&self) -> bool {
        !self.bold && !self.italic && !self.underline && !self.strike && !self.code
    }

    pub fn set(&mut self, mark: Mark, on: bool) {
        match mark {
            Mark::Bold => self.bold = on,
            Mark::Italic => self.italic = on,
            Mark::Underline => self.underline = on,
            Mark::Strike => self.strike = on,
            Mark::Code => self.code = on,
        }
    }

    /// The marks present on *every* run of a selection.
    ///
    /// Intersection, not union: with a partly-bold selection, Ctrl+B should
    /// make all of it bold rather than clearing the bold part.
    pub fn covering<'a>(runs: impl IntoIterator<Item = &'a TextRun>) -> Self {
        let mut iter = runs.into_iter().filter(|r| !r.text.is_empty()).peekable();
        if iter.peek().is_none() {
            return Self::default();
        }
        let mut out = Self {
            bold: true,
            italic: true,
            underline: true,
            strike: true,
            code: true,
        };
        for run in iter {
            for mark in Mark::ALL {
                if !run.has(mark) {
                    out.set(mark, false);
                }
            }
        }
        out
    }
}

/// Split `runs` so each pill anchor is on its own, and merge neighbours that
/// share formatting. Keeps the run list minimal, which keeps the style pushes a
/// renderer makes minimal too.
pub fn normalise(runs: Vec<TextRun>) -> Vec<TextRun> {
    let mut out: Vec<TextRun> = Vec::with_capacity(runs.len());
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(prev)
                if prev.pill.is_none()
                    && run.pill.is_none()
                    && prev.end == run.start
                    && prev.formatting_matches(&run) =>
            {
                prev.text.push_str(&run.text);
                prev.end = run.end;
            }
            _ => out.push(run),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(start: usize, text: &str, bold: bool) -> TextRun {
        TextRun {
            start: Utf8Pos(start),
            end: Utf8Pos(start + text.len()),
            text: text.into(),
            bold,
            italic: false,
            underline: false,
            strike: false,
            code: false,
            style: TextStyle::default(),
            pill: None,
        }
    }

    #[test]
    fn the_anchor_is_three_bytes() {
        assert_eq!(ANCHOR.len_utf8(), ANCHOR_LEN);
        assert_eq!(ANCHOR_STR.len(), ANCHOR_LEN);
    }

    #[test]
    fn mark_keys_are_stable() {
        // Changing these orphans every mark already stored.
        assert_eq!(Mark::Bold.key(), "b");
        assert_eq!(Mark::Italic.key(), "i");
        assert_eq!(Mark::Underline.key(), "u");
        assert_eq!(Mark::Strike.key(), "s");
        assert_eq!(Mark::Code.key(), "code");
        assert_eq!(SCRIPT_KEY, "sc");
        assert_eq!(SIZE_KEY, "sz");
        assert_eq!(HIGHLIGHT_KEY, "hl");
        assert_eq!(PILL_KEY, "pill");
    }

    /// The keys have to be distinct, or one style would silently clear another.
    #[test]
    fn no_two_style_keys_collide() {
        let mut keys: Vec<&str> = Mark::ALL.iter().map(|m| m.key()).collect();
        keys.extend(VALUE_KEYS);
        keys.push(PILL_KEY);
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "duplicate style key in {keys:?}");
    }

    #[test]
    fn script_round_trips_through_storage() {
        for s in [Script::Super, Script::Sub] {
            assert_eq!(Script::parse(s.as_str()), Some(s));
        }
        assert_eq!(Script::parse("nonsense"), None);
        // parley has no baseline shift, so these are OpenType features.
        assert_eq!(Script::Super.feature(), "sups");
        assert_eq!(Script::Sub.feature(), "subs");
    }

    #[test]
    fn adjacent_runs_with_the_same_marks_merge() {
        let merged = normalise(vec![run(0, "one ", false), run(4, "two", false)]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "one two");
        assert_eq!(merged[0].range(), 0..7);
    }

    #[test]
    fn runs_with_different_marks_stay_apart() {
        let kept = normalise(vec![run(0, "plain ", false), run(6, "bold", true)]);
        assert_eq!(kept.len(), 2);
    }

    /// A value mark splits a run just as a flag does — otherwise two
    /// differently-highlighted words would merge into one and lose a colour.
    #[test]
    fn runs_with_different_values_stay_apart() {
        let yellow = TextRun {
            style: TextStyle {
                highlight: Some("yellow".into()),
                ..Default::default()
            },
            ..run(0, "warn", false)
        };
        let plain = run(4, " and", false);
        assert_eq!(normalise(vec![yellow, plain]).len(), 2);

        let big = TextRun {
            style: TextStyle {
                size: Some(24.0),
                ..Default::default()
            },
            ..run(0, "big", false)
        };
        let small = run(3, "small", false);
        assert_eq!(normalise(vec![big, small]).len(), 2);
    }

    #[test]
    fn runs_sharing_a_value_still_merge() {
        let style = TextStyle {
            highlight: Some("yellow".into()),
            ..Default::default()
        };
        let a = TextRun {
            style: style.clone(),
            ..run(0, "one ", false)
        };
        let b = TextRun {
            style,
            ..run(4, "two", false)
        };
        let merged = normalise(vec![a, b]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "one two");
    }

    #[test]
    fn a_pill_never_merges_into_its_neighbours() {
        let pill = TextRun {
            pill: Some(AttrId::new()),
            ..run(4, ANCHOR_STR, false)
        };
        let runs = normalise(vec![run(0, "for ", false), pill, run(7, " today", false)]);
        assert_eq!(runs.len(), 3, "the anchor must stay addressable on its own");
        assert!(runs[1].is_pill());
    }

    #[test]
    fn empty_runs_are_dropped() {
        assert!(normalise(vec![run(0, "", false)]).is_empty());
    }

    /// Intersection, so Ctrl+B on a part-bold selection bolds all of it.
    #[test]
    fn covering_marks_are_the_intersection() {
        let runs = vec![run(0, "bold", true), run(4, " plain", false)];
        let set = MarkSet::covering(&runs);
        assert!(!set.bold, "not every run is bold");

        let all_bold = vec![run(0, "bold", true), run(4, " too", true)];
        assert!(MarkSet::covering(&all_bold).bold);
    }

    #[test]
    fn covering_an_empty_selection_is_empty() {
        assert_eq!(MarkSet::covering(&[]), MarkSet::default());
        assert!(TextStyle::covering(&[]).is_empty());
    }

    /// A value that differs between runs reports `None` — "mixed" — rather than
    /// letting one run speak for the whole selection.
    #[test]
    fn covering_values_report_mixed_as_none() {
        let yellow = TextRun {
            style: TextStyle {
                highlight: Some("yellow".into()),
                size: Some(18.0),
                ..Default::default()
            },
            ..run(0, "one ", false)
        };
        let green = TextRun {
            style: TextStyle {
                highlight: Some("green".into()),
                size: Some(18.0),
                ..Default::default()
            },
            ..run(4, "two", false)
        };
        let style = TextStyle::covering(&[yellow, green]);
        assert_eq!(style.highlight, None, "two colours means no single answer");
        assert_eq!(style.size, Some(18.0), "but the shared size survives");
    }

    /// Byte offsets, not character counts — the reason `Utf8Pos` exists.
    #[test]
    fn offsets_are_bytes_in_non_ascii_text() {
        let text = "보상 약관";
        let r = run(0, text, false);
        assert_eq!(r.end.get(), text.len());
        assert_ne!(r.end.get(), text.chars().count(), "bytes, not chars");
    }
}
