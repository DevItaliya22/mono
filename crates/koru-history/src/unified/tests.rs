// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::compare;
use koru_core::{Content, Kind, Node, NodeMeta};

fn block(id: NodeId, text: &str) -> SemanticNode {
    SemanticNode {
        node: Node {
            id,
            kind: Kind::Paragraph,
            meta: NodeMeta::default(),
            content: Content::text(text),
        },
        parent: koru_core::Parent::Root,
        ordinal: 0,
        depth: 1,
        runs: Vec::new(),
    }
}

/// A document whose ordinals are sibling positions, the way
/// [`SemanticDocument::read`] produces them.
fn doc(nodes: Vec<SemanticNode>) -> SemanticDocument {
    let mut nodes = nodes;
    for (index, node) in nodes.iter_mut().enumerate() {
        node.ordinal = index;
    }
    SemanticDocument {
        schema_version: None,
        nodes,
    }
}

fn ids(count: usize) -> Vec<NodeId> {
    (0..count).map(|_| NodeId::new()).collect()
}

fn diff_of(before: &SemanticDocument, after: &SemanticDocument, context: usize) -> UnifiedDiff {
    unified(
        before,
        after,
        &compare(before, after),
        DiffOptions {
            context,
            ..DiffOptions::default()
        },
    )
}

fn text_of<'a>(spans: impl IntoIterator<Item = &'a TextSpan>) -> String {
    spans.into_iter().map(|s| s.text.as_str()).collect()
}

#[test]
fn identical_documents_fold_into_one_skipped_row() {
    let id = ids(3);
    let make = || {
        doc(vec![
            block(id[0], "one"),
            block(id[1], "two"),
            block(id[2], "three"),
        ])
    };
    let out = diff_of(&make(), &make(), 3);
    assert_eq!(out.rows, vec![DiffRow::Skipped { blocks: 3 }]);
    assert_eq!(out.stats.unchanged, 3);
    assert_eq!(
        out.stats,
        DiffStats {
            unchanged: 3,
            ..Default::default()
        }
    );
}

#[test]
fn a_single_unchanged_block_outranks_its_own_fold_marker() {
    let id = ids(1);
    let make = || doc(vec![block(id[0], "alone")]);
    let out = diff_of(&make(), &make(), 3);
    assert!(matches!(out.rows.as_slice(), [DiffRow::Context(_)]));
}

#[test]
fn an_addition_sits_between_its_context() {
    let id = ids(4);
    let before = doc(vec![
        block(id[0], "a"),
        block(id[1], "b"),
        block(id[2], "c"),
    ]);
    let after = doc(vec![
        block(id[0], "a"),
        block(id[1], "b"),
        block(id[3], "new"),
        block(id[2], "c"),
    ]);
    let out = diff_of(&before, &after, 3);
    assert!(matches!(
        out.rows.as_slice(),
        [
            DiffRow::Context(_),
            DiffRow::Context(_),
            DiffRow::Added(added),
            DiffRow::Context(_),
        ] if added.text == "new"
    ));
    assert_eq!(out.stats.added, 1);
    assert_eq!(out.stats.unchanged, 3);
}

#[test]
fn a_removal_appears_where_the_block_used_to_be() {
    let id = ids(3);
    let before = doc(vec![
        block(id[0], "a"),
        block(id[1], "gone"),
        block(id[2], "c"),
    ]);
    let after = doc(vec![block(id[0], "a"), block(id[2], "c")]);
    let out = diff_of(&before, &after, 3);
    assert!(matches!(
        out.rows.as_slice(),
        [
            DiffRow::Context(a),
            DiffRow::Removed(removed),
            DiffRow::Context(c),
        ] if a.text == "a" && removed.text == "gone" && c.text == "c"
    ));
    assert_eq!(out.stats.removed, 1);
}

#[test]
fn a_removal_at_the_top_comes_first() {
    let id = ids(2);
    let before = doc(vec![block(id[0], "gone"), block(id[1], "kept")]);
    let after = doc(vec![block(id[1], "kept")]);
    let out = diff_of(&before, &after, 3);
    assert!(matches!(
        out.rows.as_slice(),
        [DiffRow::Removed(removed), DiffRow::Context(_)] if removed.text == "gone"
    ));
}

#[test]
fn removing_a_middle_block_reorders_nothing_below_it() {
    // Ordinals are positions: deleting "gone" shifts every later sibling. The
    // shift is bookkeeping, not a change a person made, and must not surface.
    let id = ids(5);
    let before = doc(vec![
        block(id[0], "a"),
        block(id[1], "gone"),
        block(id[2], "b"),
        block(id[3], "c"),
        block(id[4], "d"),
    ]);
    let after = doc(vec![
        block(id[0], "a"),
        block(id[2], "b"),
        block(id[3], "c"),
        block(id[4], "d"),
    ]);
    let diff = compare(&before, &after);
    assert_eq!(
        diff.changes.len(),
        1,
        "only the removal is a change: {:?}",
        diff.changes
    );
    assert_eq!(diff.unchanged, 4);
    let out = unified(
        &before,
        &after,
        &diff,
        DiffOptions {
            context: 3,
            ..DiffOptions::default()
        },
    );
    assert!(
        out.rows
            .iter()
            .all(|row| !matches!(row, DiffRow::Reordered { .. }))
    );
}

#[test]
fn a_swap_flags_the_block_that_moved_up() {
    let id = ids(4);
    let before = doc(vec![
        block(id[0], "a"),
        block(id[1], "b"),
        block(id[2], "c"),
        block(id[3], "d"),
    ]);
    let after = doc(vec![
        block(id[0], "a"),
        block(id[2], "c"),
        block(id[1], "b"),
        block(id[3], "d"),
    ]);
    let out = diff_of(&before, &after, 0);
    let reordered: Vec<&BlockView> = out
        .rows
        .iter()
        .filter_map(|row| match row {
            DiffRow::Reordered { block } => Some(block),
            _ => None,
        })
        .collect();
    assert_eq!(out.stats.reordered, 1);
    assert_eq!(reordered[0].text, "c");
}

#[test]
fn moving_one_block_to_the_end_flags_only_it() {
    let id = ids(4);
    let before = doc(vec![
        block(id[0], "a"),
        block(id[1], "b"),
        block(id[2], "c"),
        block(id[3], "d"),
    ]);
    let after = doc(vec![
        block(id[1], "b"),
        block(id[2], "c"),
        block(id[3], "d"),
        block(id[0], "a"),
    ]);
    let out = diff_of(&before, &after, 0);
    let reordered: Vec<&BlockView> = out
        .rows
        .iter()
        .filter_map(|row| match row {
            DiffRow::Reordered { block } => Some(block),
            _ => None,
        })
        .collect();
    assert_eq!(out.stats.reordered, 1, "{:?}", out.rows);
    assert_eq!(reordered[0].text, "a");
    assert_eq!(out.stats.unchanged, 3);
}

#[test]
fn a_modification_splits_into_reconstructable_sides() {
    let id = ids(1);
    let before = doc(vec![block(id[0], "The brown fox jumps.")]);
    let after = doc(vec![block(id[0], "The red fox jumps.")]);
    let out = diff_of(&before, &after, 3);
    let DiffRow::Modified { spans, .. } = &out.rows[0] else {
        panic!("expected a modified row: {:?}", out.rows);
    };
    assert_eq!(text_of(old_side(spans)), "The brown fox jumps.");
    assert_eq!(text_of(new_side(spans)), "The red fox jumps.");
    assert!(
        spans
            .iter()
            .any(|s| s.tag == DiffTag::Delete && s.text.contains("brown"))
    );
    assert!(
        spans
            .iter()
            .any(|s| s.tag == DiffTag::Insert && s.text.contains("red"))
    );
}

#[test]
fn folding_keeps_context_on_both_sides_of_the_gap() {
    let id = ids(7);
    let texts = ["m", "a", "b", "c", "d", "e", "n"];
    let before = doc(texts.iter().zip(&id).map(|(t, &i)| block(i, t)).collect());
    let mut edited: Vec<SemanticNode> = texts.iter().zip(&id).map(|(t, &i)| block(i, t)).collect();
    edited[0] = block(id[0], "m changed");
    edited[6] = block(id[6], "n changed");
    let after = doc(edited);
    let out = diff_of(&before, &after, 1);
    assert!(
        matches!(
            out.rows.as_slice(),
            [
                DiffRow::Modified { .. },
                DiffRow::Context(head),
                DiffRow::Skipped { blocks: 3 },
                DiffRow::Context(tail),
                DiffRow::Modified { .. },
            ] if head.text == "a" && tail.text == "e"
        ),
        "{:?}",
        out.rows
    );
}

#[test]
fn runs_at_the_document_edges_keep_only_the_adjacent_side() {
    let id = ids(7);
    let texts = ["a", "b", "c", "m", "d", "e", "f"];
    let before = doc(texts.iter().zip(&id).map(|(t, &i)| block(i, t)).collect());
    let mut edited: Vec<SemanticNode> = texts.iter().zip(&id).map(|(t, &i)| block(i, t)).collect();
    edited[3] = block(id[3], "m changed");
    let after = doc(edited);
    let out = diff_of(&before, &after, 1);
    assert!(
        matches!(
            out.rows.as_slice(),
            [
                DiffRow::Skipped { blocks: 2 },
                DiffRow::Context(lead),
                DiffRow::Modified { .. },
                DiffRow::Context(trail),
                DiffRow::Skipped { blocks: 2 },
            ] if lead.text == "c" && trail.text == "d"
        ),
        "{:?}",
        out.rows
    );
}

#[test]
fn empty_to_content_is_all_added() {
    let id = ids(2);
    let before = doc(Vec::new());
    let after = doc(vec![block(id[0], "one"), block(id[1], "two")]);
    let out = diff_of(&before, &after, 3);
    assert!(out.rows.iter().all(|row| matches!(row, DiffRow::Added(_))));
    assert_eq!(out.stats.added, 2);
}

#[test]
fn content_to_empty_is_all_removed() {
    let id = ids(2);
    let before = doc(vec![block(id[0], "one"), block(id[1], "two")]);
    let after = doc(Vec::new());
    let out = diff_of(&before, &after, 3);
    assert!(matches!(
        out.rows.as_slice(),
        [DiffRow::Removed(one), DiffRow::Removed(two)]
            if one.text == "one" && two.text == "two"
    ));
    assert_eq!(out.stats.removed, 2);
}

#[test]
fn every_fixture_language_reconstructs_byte_exactly() {
    for sample in koru_store_core::fixtures::SAMPLES {
        let id = NodeId::new();
        let before = doc(vec![block(id, sample.body)]);
        let edited = format!("{} {}", sample.title, sample.body);
        let after = doc(vec![block(id, &edited)]);
        let out = diff_of(&before, &after, 3);
        let DiffRow::Modified { spans, .. } = &out.rows[0] else {
            panic!("{}: expected a modified row", sample.tag);
        };
        assert_eq!(text_of(old_side(spans)), sample.body, "{}", sample.tag);
        assert_eq!(text_of(new_side(spans)), edited, "{}", sample.tag);
    }
}

#[test]
fn bidi_controls_survive_byte_exactly() {
    let id = NodeId::new();
    let line = koru_store_core::fixtures::BIDI_LINE;
    let before = doc(vec![block(id, line)]);
    let edited = format!("{line} …");
    let after = doc(vec![block(id, &edited)]);
    let out = diff_of(&before, &after, 3);
    let DiffRow::Modified { spans, .. } = &out.rows[0] else {
        panic!("expected a modified row");
    };
    assert_eq!(text_of(old_side(spans)), line);
    assert_eq!(text_of(new_side(spans)), edited);
}

#[test]
fn nfc_and_nfd_are_different_content_not_equal() {
    // Content is never normalized; the diff must agree.
    let id = NodeId::new();
    let before = doc(vec![block(id, koru_store_core::fixtures::COMPOSED)]);
    let after = doc(vec![block(id, koru_store_core::fixtures::DECOMPOSED)]);
    let out = diff_of(&before, &after, 3);
    let DiffRow::Modified { spans, .. } = &out.rows[0] else {
        panic!("expected a modified row: {:?}", out.rows);
    };
    assert_eq!(
        text_of(old_side(spans)),
        koru_store_core::fixtures::COMPOSED
    );
    assert_eq!(
        text_of(new_side(spans)),
        koru_store_core::fixtures::DECOMPOSED
    );
}

#[test]
fn the_model_round_trips_through_serde() {
    let id = ids(2);
    let before = doc(vec![block(id[0], "one"), block(id[1], "The brown fox.")]);
    let after = doc(vec![block(id[1], "The red fox.")]);
    let out = diff_of(&before, &after, 3);
    let json = serde_json::to_string(&out).expect("serialize");
    let back: UnifiedDiff = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(out, back);
}
