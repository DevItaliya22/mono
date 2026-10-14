// SPDX-License-Identifier: MPL-2.0

//! **The index mappings, written out once.**
//!
//! One sentence, and what it excludes: *this module is the arithmetic between
//! the five ways of counting one string, and nothing else.* It builds no
//! nodes, holds no cache, reads no layout and knows nothing about a row's place
//! in a window — [`super::projection`] and [`super::geometry`] own those. What
//! is here is the conversion table the roadmap requires to exist explicitly
//! rather than as a habit at each call site:
//!
//! | Space | Unit | Owner |
//! | --- | --- | --- |
//! | Loro | unicode scalars — but every call site uses the `_utf8` variants, so what Koru stores is bytes | `koru_core::schema` |
//! | Koru / [`Paragraph`](crate::Paragraph) | **UTF-8 bytes**, stepped in **extended grapheme clusters** | `koru-editor` |
//! | parley | UTF-8 bytes into the *shaped* string, stepped in **shaping clusters** | parley |
//! | AccessKit | a **character index** into one text run, with `character_lengths` giving each character's UTF-8 length | `accesskit` |
//! | UIA | UTF-16 code units | `accesskit_windows` |
//!
//! Two of the five are traps, and both corrections live here:
//! [`merge_to_graphemes`] folds parley's clusters into the unit Koru's arrow
//! keys move by, and [`trim_strut`] takes the layout strut back out of the
//! text a screen reader is given. Everything else is a [`RunSpan`] and a sum.
//!
//! **Nothing here allocates per publish except where it must.** A [`RunSpan`]
//! is built once per *shaping*, not once per update, because the whole point of
//! [`super::projection`]'s cache is that an unchanged row is not walked again.

use std::collections::HashSet;

use accesskit::{Node, NodeId, TextPosition};

use crate::rowlayout::LAYOUT_STRUT;

/// The largest character index `accesskit::Node::word_starts` can express.
///
/// **A real limit of the pinned adapter, not a Koru choice.** `word_starts` is
/// a `Box<[u8]>`, and `accesskit_consumer` looks a position up in it with
/// `binary_search(&(character_index as u8))` — so past 255 characters in one
/// run a word start cannot be recorded and the lookup wraps. parley caps a run
/// at 256 characters for exactly this reason, which is the mitigation Koru
/// inherits by using its projection instead of writing one. Reading, character
/// motion and line motion are unaffected; only word motion inside a very long
/// unbroken run would be — and parley's cap means there is no such run.
pub const MAX_WORD_STARTS_INDEX: usize = u8::MAX as usize;

/// One published run's place in `row.text`.
///
/// The map is per run and is arithmetic all the way down: a character index is
/// a prefix sum over [`lengths`](Self::lengths), and a byte offset is the
/// search for the prefix that contains it. Deliberately **not**
/// `parley::Cursor::to_access_position`, which answers in parley's shaping
/// clusters while this tree is published in Koru's grapheme clusters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSpan {
    pub id: NodeId,
    /// Byte offset of this run's first character within `row.text`.
    pub start: usize,
    /// Each character's length in UTF-8 bytes — Koru characters, after
    /// [`merge_to_graphemes`].
    pub lengths: Vec<u8>,
}

impl RunSpan {
    /// The byte offset of a character index inside this run, clamped to its
    /// end. An index equal to the character count is the run's end position,
    /// which is a legal AccessKit position.
    pub fn byte_of(&self, character: usize) -> usize {
        self.start
            + self
                .lengths
                .iter()
                .take(character)
                .copied()
                .map(usize::from)
                .sum::<usize>()
    }

    /// One past the last byte this run covers.
    pub fn end(&self) -> usize {
        self.byte_of(self.lengths.len())
    }
}

/// The spans for a freshly built set of runs, in reading order.
///
/// Built once per shaping, beside the runs themselves. The returned byte total
/// is what the caller checks against `row.text.len()` — the assertion that says
/// the published runs cover the row exactly, at the boundary rather than in a
/// comment.
pub fn spans_of(runs: &[(NodeId, Node)]) -> (Vec<RunSpan>, usize) {
    let mut spans = Vec::with_capacity(runs.len());
    let mut at = 0usize;
    for (id, node) in runs {
        let lengths = node.character_lengths().to_vec();
        let len: usize = lengths.iter().copied().map(usize::from).sum();
        spans.push(RunSpan {
            id: *id,
            start: at,
            lengths,
        });
        at += len;
    }
    (spans, at)
}

/// Whether a parley affinity means "the following run", at a boundary.
///
/// A soft wrap is one byte offset and two places on screen, and the editor
/// already knows which one it means — `Affinity::Downstream` is the start of
/// the next line, `Upstream` the end of the previous one. Publishing without it
/// makes a screen reader announce "end of line" where the caret is visibly at
/// the beginning of the next, because `UIA_CaretPositionAttributeId` is
/// computed from exactly this.
pub fn downstream(affinity: parley::Affinity) -> bool {
    matches!(affinity, parley::Affinity::Downstream)
}

/// Which run and character a byte offset falls in.
///
/// `prefer_next` is the affinity: at a boundary between two runs, `true` means
/// the start of the following run and `false` the end of the preceding one.
/// Both name the same byte, so the round trip is unaffected either way; what
/// changes is which of the two places on screen a client is told about.
pub fn position_of(spans: &[RunSpan], at: usize, prefer_next: bool) -> TextPosition {
    for span in spans {
        // `<` when the caret leans forward, so a boundary falls through to the
        // run that *starts* there; `<=` when it leans back, because a run's end
        // offset is a legal position in it. The last run is reached by the
        // fall-through below in either case, which is the only way to say "the
        // end of the text".
        let contains = if prefer_next {
            at < span.end()
        } else {
            at <= span.end()
        };
        if contains {
            let mut character = 0usize;
            let mut byte = span.start;
            while byte < at {
                let Some(length) = span.lengths.get(character) else {
                    break;
                };
                byte += usize::from(*length);
                character += 1;
            }
            return TextPosition {
                node: span.id,
                character_index: character,
            };
        }
    }
    // No runs at all cannot happen — every row publishes at least the emptied
    // strut run — but answering with a placeholder would be worse than
    // answering with the document's end.
    match spans.last() {
        Some(span) => TextPosition {
            node: span.id,
            character_index: span.lengths.len(),
        },
        None => TextPosition {
            node: NodeId(1),
            character_index: 0,
        },
    }
}

/// The byte offset an AccessKit position names, or `None` if it names a run
/// this paragraph did not publish.
pub fn byte_of(spans: &[RunSpan], position: &TextPosition) -> Option<usize> {
    let span = spans.iter().find(|span| span.id == position.node)?;
    Some(span.byte_of(position.character_index))
}

/// Take the layout strut back out of the last run.
///
/// The strut is the only difference between `row.text` and the string parley
/// shaped, it is always the last character of the last run, and `build_row`
/// pushes it with a transparent brush — so parley gives it a run of its own and
/// this normally empties that run rather than shortening a real one.
///
/// **Emptied, never removed.** A run with no characters is legal and load
/// bearing twice over: it keeps the end-of-text position addressable, and it is
/// the *only* run an empty paragraph has —
/// `accesskit_consumer::Node::supports_text_ranges` refuses a text pattern to a
/// container with no runs at all, so removing it would leave an empty paragraph
/// unreadable rather than merely empty.
///
/// Returns whether the run it emptied was the strut's *own* — which is the
/// ordinary case, because the strut is pushed with a brush of its own — so the
/// caller can give that run an id parley cannot supply. See
/// [`super::projection::STRUT_RUN`].
pub fn trim_strut(node: &mut Node) -> bool {
    let Some(value) = node.value().map(str::to_owned) else {
        return false;
    };
    if !value.ends_with(LAYOUT_STRUT) {
        return false;
    }
    let Some((last, head)) = node.character_lengths().split_last() else {
        return false;
    };
    // Defence in depth: if the strut ever stopped being one character of its
    // own, shortening by three bytes would corrupt every index in the run.
    // Leaving it alone shows a visible extra character; getting this wrong is a
    // silent off-by-three at the end of every paragraph.
    if usize::from(*last) != LAYOUT_STRUT.len() {
        return false;
    }
    let keep = head.len();
    let head = head.to_vec();
    let positions = node.character_positions().map(|p| p[..keep].to_vec());
    let widths = node.character_widths().map(|w| w[..keep].to_vec());
    let word_starts: Vec<u8> = node
        .word_starts()
        .iter()
        .copied()
        .filter(|start| usize::from(*start) < keep)
        .collect();

    node.set_value(value[..value.len() - LAYOUT_STRUT.len()].to_owned());
    node.set_character_lengths(head);
    if let Some(positions) = positions {
        node.set_character_positions(positions);
    }
    if let Some(widths) = widths {
        node.set_character_widths(widths);
    }
    node.set_word_starts(word_starts);
    keep == 0
}

/// Fold parley's shaping clusters into the unit Koru's caret moves by.
///
/// AccessKit defines a character as "the smallest unit of text that can be
/// selected" and says the lengths must come from the text editor. Koru's is the
/// **extended grapheme cluster** — `Paragraph::before`/`after` step by it on
/// purpose — and parley's is the shaping cluster, which splits a skin-tone
/// emoji into base and modifier and can split a letter from its combining mark.
/// Left alone, an AT would report and request caret positions the arrow keys
/// can never produce, and reading a paragraph character by character would
/// announce half an emoji.
///
/// Merging is the only direction that is safe: a grapheme boundary is always
/// also a cluster boundary, so this only ever *removes* stops. The merged
/// character keeps the first cluster's position and the sum of the widths, and
/// a word start that fell inside a merged group is dropped — it was not a place
/// the caret could stop.
pub fn merge_to_graphemes(node: &mut Node) {
    use unicode_segmentation::UnicodeSegmentation;

    let Some(value) = node.value().map(str::to_owned) else {
        return;
    };
    if value.is_empty() {
        return;
    }
    let stops: HashSet<usize> = value
        .grapheme_indices(true)
        .map(|(at, _)| at)
        .chain(std::iter::once(value.len()))
        .collect();

    let lengths = node.character_lengths().to_vec();
    let positions = node.character_positions().map(<[f32]>::to_vec);
    let widths = node.character_widths().map(<[f32]>::to_vec);

    let mut merged_lengths: Vec<u8> = Vec::with_capacity(lengths.len());
    let mut merged_positions: Vec<f32> = Vec::with_capacity(lengths.len());
    let mut merged_widths: Vec<f32> = Vec::with_capacity(lengths.len());
    // parley character index -> merged character index, for the word starts.
    let mut reindex: Vec<Option<usize>> = Vec::with_capacity(lengths.len());

    let mut byte = 0usize;
    for (i, length) in lengths.iter().copied().map(usize::from).enumerate() {
        if stops.contains(&byte) {
            reindex.push(Some(merged_lengths.len()));
            merged_lengths.push(0);
            merged_positions.push(
                positions
                    .as_ref()
                    .and_then(|p| p.get(i).copied())
                    .unwrap_or(0.0),
            );
            merged_widths.push(0.0);
        } else {
            reindex.push(None);
        }
        if let Some(last) = merged_lengths.last_mut() {
            // A grapheme longer than 255 bytes cannot be expressed at all —
            // `character_lengths` is a `Box<[u8]>`. Saturating keeps the
            // invariant "lengths sum to the value" false rather than panicking,
            // so the debug assertion in `projection` is what reports it; no
            // real cluster comes close.
            *last = last.saturating_add(u8::try_from(length).unwrap_or(u8::MAX));
        }
        if let Some(last) = merged_widths.last_mut() {
            *last += widths
                .as_ref()
                .and_then(|w| w.get(i).copied())
                .unwrap_or(0.0);
        }
        byte += length;
    }

    if merged_lengths.len() == lengths.len() {
        return;
    }

    let word_starts: Vec<u8> = node
        .word_starts()
        .iter()
        .copied()
        .filter_map(|start| reindex.get(usize::from(start)).copied().flatten())
        .filter_map(|start| u8::try_from(start).ok())
        .collect();

    node.set_character_lengths(merged_lengths);
    if positions.is_some() {
        node.set_character_positions(merged_positions);
    }
    if widths.is_some() {
        node.set_character_widths(merged_widths);
    }
    node.set_word_starts(word_starts);
}
