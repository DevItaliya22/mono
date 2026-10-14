// SPDX-License-Identifier: MPL-2.0

//! The index mapping, against a **real** parley layout.
//!
//! Every assertion here shapes text for real, because the whole subject is the
//! correspondence between four ways of counting the same string and a default
//! `Layout` makes all four agree on nothing.

use std::collections::HashMap;

use accesskit::NodeId;
use koru_core::node::Node as CoreNode;
use koru_draw::text::TextEngine;
use unicode_segmentation::UnicodeSegmentation;

use super::projection::run_id;
use super::text_positions::MAX_WORD_STARTS_INDEX;
use super::*;
use crate::paragraph::Paragraph;
use crate::rowlayout::{self, LAYOUT_STRUT};

/// A laid-out paragraph, at a width that fits.
fn para(text: &str) -> Paragraph {
    laid_out(text, 600.0)
}

/// A laid-out paragraph at a chosen width, so wrapping can be forced.
fn laid_out(text: &str, width: f32) -> Paragraph {
    let node = CoreNode::paragraph(text).unwrap();
    let mut row = Paragraph::from_node(&node, 0, Vec::new(), HashMap::new(), None, 0);
    let mut engine = TextEngine::new();
    rowlayout::build_row(&mut engine, &mut row, width);
    row
}

fn base() -> NodeId {
    NodeId(0x1234_5678_9abc_def0)
}

/// Everything a screen reader would read, concatenated.
fn published_text(text: &ParagraphText) -> String {
    text.runs
        .iter()
        .filter_map(|(_, node)| node.value())
        .collect()
}

/// Put the caret at a byte offset and settle it, the way a reflow does.
fn caret_at(row: &mut Paragraph, at: usize) {
    row.place_caret(at);
    row.apply_pending();
}

#[test]
fn the_published_text_is_exactly_the_row_text() {
    // The layout is three bytes longer than `row.text` — the strut — and this
    // is the assertion that says so at the boundary rather than in a comment.
    let row = para("Reward payout flow");
    let mut doc = DocumentAccessibility::default();
    let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
    assert_eq!(published_text(&text), row.text);
    assert!(
        !published_text(&text).contains(LAYOUT_STRUT),
        "the layout strut reached a screen reader"
    );
}

#[test]
fn a_wrapped_paragraph_publishes_one_run_per_visual_line() {
    let row = laid_out(
        "This paragraph is long enough to wrap onto several visual lines at \
         this width, which is the case the caret has to keep straight.",
        180.0,
    );
    assert!(row.line_count() > 1, "the fixture did not actually wrap");
    let mut doc = DocumentAccessibility::default();
    let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
    // One run per line at minimum; a line may split further on formatting, so
    // this is a floor rather than an equality.
    assert!(
        text.run_count() >= row.line_count(),
        "{} runs for {} lines",
        text.run_count(),
        row.line_count()
    );
    assert_eq!(published_text(&text), row.text);
}

#[test]
fn an_empty_paragraph_publishes_one_empty_run_and_no_placeholder() {
    let row = para("");
    let mut doc = DocumentAccessibility::default();
    let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
    assert_eq!(text.run_count(), 1, "a text pattern needs at least one run");
    let (_, node) = &text.runs[0];
    assert_eq!(node.value(), Some(""));
    assert!(node.character_lengths().is_empty());
    assert_eq!(published_text(&text), "");
}

/// AccessKit's own invariant: the character lengths sum to the value's length
/// in bytes. Everything a client computes about the run depends on it.
#[test]
fn character_lengths_sum_to_the_run_value() {
    for source in [
        "Reward payout flow",
        "보상 약관 확인",
        "e\u{301}tude",
        "👍🏻 ok",
        "",
    ] {
        let row = para(source);
        let mut doc = DocumentAccessibility::default();
        let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
        for (id, node) in &text.runs {
            let value = node.value().unwrap_or_default();
            let sum: usize = node
                .character_lengths()
                .iter()
                .copied()
                .map(usize::from)
                .sum();
            assert_eq!(sum, value.len(), "run {id:?} of {source:?}");
            let count = node.character_lengths().len();
            assert_eq!(
                node.character_positions().map(<[f32]>::len),
                Some(count),
                "positions must be one per character"
            );
            assert_eq!(
                node.character_widths().map(<[f32]>::len),
                Some(count),
                "widths must be one per character"
            );
        }
    }
}

/// A character is a grapheme cluster, which is what the editor's own Left/Right
/// steps over. If the two disagreed, an AT would report a caret inside an
/// emoji the arrow keys can never reach.
#[test]
fn a_character_is_the_same_unit_the_arrow_keys_step_over() {
    let source = "a👍🏻e\u{301}b";
    let row = para(source);
    let mut doc = DocumentAccessibility::default();
    let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
    let published: Vec<usize> = text
        .runs
        .iter()
        .flat_map(|(_, node)| node.character_lengths().to_vec())
        .map(usize::from)
        .collect();
    let by_grapheme: Vec<usize> = source.graphemes(true).map(str::len).collect();
    assert_eq!(published, by_grapheme);

    // And the editor agrees, stepping with the same call `move_within` uses.
    let mut at = 0usize;
    let mut steps = Vec::new();
    while at < row.text.len() {
        let next = row.after(at, false);
        steps.push(next - at);
        at = next;
    }
    assert_eq!(steps, by_grapheme);
}

/// The round trip the whole plan turns on: a byte offset the editor owns
/// becomes an AccessKit position and comes back the same byte offset.
#[test]
fn every_caret_position_round_trips_through_accesskit() {
    for source in [
        "Reward payout flow",
        "보상 약관 확인",
        "Проверить условия",
        "a👍🏻e\u{301}b",
        "",
    ] {
        let mut row = para(source);
        let mut doc = DocumentAccessibility::default();
        let mut at = 0usize;
        loop {
            caret_at(&mut row, at);
            let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
            let selection = text
                .selection
                .unwrap_or_else(|| panic!("no selection published for {source:?} at {at}"));
            assert_eq!(
                selection.anchor, selection.focus,
                "a caret is a degenerate selection"
            );
            let back = doc
                .byte_range(&row, &selection)
                .unwrap_or_else(|| panic!("{source:?} at {at} did not map back"));
            assert_eq!(back, (at, at), "{source:?} at byte {at}");
            if at >= row.text.len() {
                break;
            }
            at = row.after(at, false);
        }
    }
}

/// The same for a range, including the end of the text — where the position
/// lands on the emptied strut run and is the case a naive implementation gets
/// wrong.
#[test]
fn a_selection_to_the_end_of_the_text_round_trips() {
    let mut row = para("Reward payout flow");
    let end = row.text.len();
    row.select_range_now(5, end);
    let mut doc = DocumentAccessibility::default();
    let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
    let selection = text.selection.expect("a range was published");
    assert_ne!(
        selection.anchor, selection.focus,
        "this range is not a caret"
    );
    assert_eq!(doc.byte_range(&row, &selection), Some((5, end)));
}

#[test]
fn a_paragraph_that_does_not_hold_the_caret_publishes_no_selection() {
    let row = para("Reward payout flow");
    let mut doc = DocumentAccessibility::default();
    let text = doc.paragraph(&row, base(), (0.0, 0.0), false);
    assert!(
        text.selection.is_none(),
        "an unfocused paragraph must report SupportedTextSelection_None, not a \
         second caret"
    );
}

/// A screen reader holds ranges across updates. New ids on every publish would
/// invalidate all of them, and it would start the paragraph again after every
/// keystroke.
#[test]
fn run_ids_are_stable_across_publishes() {
    let row = para("Reward payout flow");
    let mut doc = DocumentAccessibility::default();
    let first = doc.paragraph(&row, base(), (0.0, 0.0), true);
    let again = doc.paragraph(&row, base(), (0.0, 0.0), true);
    let ids =
        |text: &ParagraphText| -> Vec<NodeId> { text.runs.iter().map(|(id, _)| *id).collect() };
    assert_eq!(ids(&first), ids(&again));
}

#[test]
fn two_paragraphs_never_share_a_run_id() {
    let one = para("first");
    let two = para("second");
    let mut doc = DocumentAccessibility::default();
    let a = doc.paragraph(&one, NodeId(11), (0.0, 0.0), true);
    let b = doc.paragraph(&two, NodeId(22), (0.0, 0.0), false);
    for (id, _) in &a.runs {
        assert!(
            !b.runs.iter().any(|(other, _)| other == id),
            "{id:?} was published by two paragraphs"
        );
    }
}

/// `NodeId(0)` is `accesskit_windows`' placeholder root. Publishing one would
/// swap a text run for the empty tree served before activation.
#[test]
fn a_run_id_is_never_zero() {
    assert_ne!(run_id(0, 0), 0);
    // The pathological input: a base that cancels the mix exactly.
    let nth = 1u64;
    let base = nth.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    assert_ne!(run_id(base, nth), 0);
}

/// `word_starts` is a `Box<[u8]>` and the consumer binary-searches it, so it
/// has to be sorted and inside the run.
#[test]
fn word_starts_are_sorted_and_addressable() {
    let row = para("Reward payout flow for the quarter");
    let mut doc = DocumentAccessibility::default();
    let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
    for (id, node) in &text.runs {
        let starts = node.word_starts();
        let mut sorted = starts.to_vec();
        sorted.sort_unstable();
        assert_eq!(starts, sorted.as_slice(), "run {id:?}");
        let count = node.character_lengths().len();
        assert!(
            count <= MAX_WORD_STARTS_INDEX + 1,
            "run {id:?} is too long to address"
        );
        for start in starts {
            assert!(usize::from(*start) < count.max(1), "run {id:?}");
        }
    }
}

/// Bounds and character positions come out in the caller's coordinate space,
/// unscaled, so the one place that multiplies by the scale factor stays the
/// one place.
#[test]
fn geometry_is_offset_by_the_origin_and_nothing_else() {
    let row = para("Reward payout flow");
    let mut doc = DocumentAccessibility::default();
    let at_origin = doc.paragraph(&row, base(), (0.0, 0.0), true);
    let moved = doc.paragraph(&row, base(), (40.0, 7.5), true);
    for ((_, a), (_, b)) in at_origin.runs.iter().zip(moved.runs.iter()) {
        let (a, b) = (a.bounds().unwrap(), b.bounds().unwrap());
        assert!((b.x0 - a.x0 - 40.0).abs() < 1e-6);
        assert!((b.y0 - a.y0 - 7.5).abs() < 1e-6);
        // Character positions are relative to the run's own box, so they do not
        // move with it.
        assert_eq!(a.width(), b.width());
    }
    let positions = |text: &ParagraphText| -> Vec<f32> {
        text.runs
            .iter()
            .flat_map(|(_, node)| node.character_positions().unwrap_or_default().to_vec())
            .collect()
    };
    assert_eq!(positions(&at_origin), positions(&moved));
}

#[test]
fn a_row_that_is_gone_is_forgotten() {
    let one = para("first");
    let two = para("second");
    let mut doc = DocumentAccessibility::default();
    doc.paragraph(&one, NodeId(11), (0.0, 0.0), true);
    doc.paragraph(&two, NodeId(22), (0.0, 0.0), false);
    assert_eq!(doc.tracked_rows(), 2);
    doc.retain([one.id]);
    assert_eq!(doc.tracked_rows(), 1);
}

/// A request naming runs this row never published is refused rather than
/// clamped onto whatever is nearest — the stale-range case.
#[test]
fn a_request_for_an_unknown_run_is_refused() {
    let row = para("Reward payout flow");
    let mut doc = DocumentAccessibility::default();
    let stale = accesskit::TextSelection {
        anchor: accesskit::TextPosition {
            node: NodeId(999),
            character_index: 0,
        },
        focus: accesskit::TextPosition {
            node: NodeId(999),
            character_index: 3,
        },
    };
    assert_eq!(doc.byte_range(&row, &stale), None);
}

/// **What a `word_starts` entry is, against what `Ctrl+Arrow` does.**
///
/// Both come from parley, but through different calls: the published word
/// starts are `Cluster::is_word_boundary`, and the editor's word motion is
/// `Cursor::next_visual_word`. They are not required to be the same set, and
/// this pins which is which rather than leaving a reader to assume.
#[test]
fn published_word_starts_are_word_beginnings() {
    let row = para("Reward payout flow");
    let mut doc = DocumentAccessibility::default();
    let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
    let (_, node) = &text.runs[0];
    let value = node.value().unwrap();
    let lengths = node.character_lengths();
    let byte_of = |character: usize| -> usize {
        lengths
            .iter()
            .take(character)
            .copied()
            .map(usize::from)
            .sum()
    };
    let starts: Vec<usize> = node
        .word_starts()
        .iter()
        .map(|s| byte_of(usize::from(*s)))
        .collect();
    assert_eq!(
        starts,
        vec![0, 7, 14],
        "the word starts are the beginning of each word of {value:?}; the first          character of a run is allowed to be listed and parley lists it"
    );

    // And the editor's own forward word motion, from the same layout.
    //
    // **They are one space apart, and that is worth knowing rather than
    // hiding.** AccessKit's word unit runs start-of-word to start-of-next-word,
    // so trailing whitespace belongs to the word before it — `"Reward "` is
    // `0..7`. Koru's `Ctrl+Right` stops at the *end* of the word instead, at 6.
    // Reading is unaffected (a caret at 6 is inside `0..7`, so "current word"
    // is still "Reward"); what differs is an AT that moves by its own
    // `TextUnit_Word` rather than by sending the key — it lands one character
    // further on. See `guide/contributing/deps/ACCESSKIT.md`.
    let mut stops = Vec::new();
    let mut at = 0usize;
    while at < row.text.len() {
        let next = row.after(at, true);
        if next <= at {
            break;
        }
        stops.push(next);
        at = next;
    }
    assert_eq!(stops, vec![6, 13, 18]);
    for start in &starts[1..] {
        assert!(
            stops.contains(&(start - 1)),
            "{start} is a word start with no editor stop one space before it"
        );
    }
}

/// **Counters, not a stopwatch.** `shapings` is the number this plan is about:
/// the O(clusters) `build_nodes` call. Projecting the same row twice must cost
/// exactly one.
#[test]
fn projecting_is_counted_and_a_stale_request_is_counted_separately() {
    let row = para("Reward payout flow");
    let mut doc = DocumentAccessibility::default();
    assert_eq!(doc.counters(), Counters::default());
    let first = doc.paragraph(&row, base(), (0.0, 0.0), true);
    assert_eq!(doc.counters().paragraphs, 1);
    assert_eq!(doc.counters().runs, first.run_count() as u64);
    assert_eq!(doc.counters().shapings, 1);
    assert_eq!(doc.counters().stale_requests, 0);

    doc.paragraph(&row, base(), (0.0, 0.0), true);
    assert_eq!(
        doc.counters().paragraphs,
        2,
        "one per call, which is what a caller asked for"
    );
    assert_eq!(
        doc.counters().shapings,
        1,
        "the same layout at the same origin must not be shaped twice"
    );
    assert_eq!(doc.counters().reused, 1);

    let stale = accesskit::TextSelection {
        anchor: accesskit::TextPosition {
            node: NodeId(999),
            character_index: 0,
        },
        focus: accesskit::TextPosition {
            node: NodeId(999),
            character_index: 1,
        },
    };
    assert_eq!(doc.byte_range(&row, &stale), None);
    assert_eq!(doc.counters().stale_requests, 1);
}

/// **A soft wrap is one byte and two places on screen**, and the editor already
/// knows which it means. Publishing without the affinity makes a screen reader
/// say "end of line" where the caret is visibly at the start of the next.
#[test]
fn a_caret_at_a_soft_wrap_reports_the_side_the_editor_means() {
    let mut row = laid_out(
        "This paragraph is long enough to wrap onto several visual lines at \
         this width, which is the case the caret has to keep straight.",
        180.0,
    );
    assert!(row.line_count() > 1, "the fixture did not actually wrap");
    let mut doc = DocumentAccessibility::default();

    // The byte where the second visual line begins. `place_caret` settles
    // downstream, which is what arriving from above does.
    let wrap = {
        let text = doc.paragraph(&row, base(), (0.0, 0.0), true);
        text.runs[0].1.value().expect("a run has a value").len()
    };
    caret_at(&mut row, wrap);
    let downstream = doc
        .paragraph(&row, base(), (0.0, 0.0), true)
        .selection
        .expect("a caret");
    let starts_second_run = downstream.focus.node == doc_run_id(&doc, &row, 1);
    assert!(
        starts_second_run && downstream.focus.character_index == 0,
        "a downstream caret at a wrap is the start of the next line"
    );

    // Leaning the other way — what pressing End on the first line does — is the
    // same byte and the other side.
    row.select_range_now(wrap, wrap);
    row.selection =
        parley::Selection::from_byte_index(&row.layout, wrap, parley::layout::Affinity::Upstream);
    let upstream = doc
        .paragraph(&row, base(), (0.0, 0.0), true)
        .selection
        .expect("a caret");
    assert_eq!(
        upstream.focus.node,
        doc_run_id(&doc, &row, 0),
        "an upstream caret at a wrap is the end of the previous line"
    );

    // And both name the same byte, so nothing the editor does depends on which.
    assert_eq!(doc.byte_range(&row, &downstream), Some((wrap, wrap)));
    assert_eq!(doc.byte_range(&row, &upstream), Some((wrap, wrap)));
}

/// The id of the nth run this row last published.
fn doc_run_id(doc: &DocumentAccessibility, row: &Paragraph, nth: usize) -> NodeId {
    doc.published_run_ids(row.id)
        .get(nth)
        .copied()
        .expect("that many runs were published")
}
