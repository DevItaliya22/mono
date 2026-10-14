// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::unified::{DiffOptions, DiffRow, DiffStats, unified_with_correspondence};
use crate::{SemanticDocument, SemanticNode};
use koru_core::{Content, Kind, Node, NodeId, NodeMeta, Parent};

fn block(id: NodeId, text: &str) -> SemanticNode {
    SemanticNode {
        node: Node {
            id,
            kind: Kind::Paragraph,
            meta: NodeMeta::default(),
            content: Content::text(text),
        },
        parent: Parent::Root,
        ordinal: 0,
        depth: 1,
        runs: Vec::new(),
    }
}

fn heading(id: NodeId, text: &str, level: u8) -> SemanticNode {
    let mut meta = NodeMeta::default();
    meta.heading = koru_core::Heading::from_level(level as i64);
    SemanticNode {
        node: Node {
            id,
            kind: Kind::Paragraph,
            meta,
            content: Content::text(text),
        },
        parent: Parent::Root,
        ordinal: 0,
        depth: 1,
        runs: Vec::new(),
    }
}

fn quote(id: NodeId, text: &str) -> SemanticNode {
    SemanticNode {
        node: Node {
            id,
            kind: Kind::Quote,
            meta: NodeMeta::default(),
            content: Content::text(text),
        },
        parent: Parent::Root,
        ordinal: 0,
        depth: 1,
        runs: Vec::new(),
    }
}

fn task(id: NodeId, text: &str, done: bool) -> SemanticNode {
    let mut meta = NodeMeta::default();
    meta.done = done;
    SemanticNode {
        node: Node {
            id,
            kind: Kind::Task,
            meta,
            content: Content::text(text),
        },
        parent: Parent::Root,
        ordinal: 0,
        depth: 1,
        runs: Vec::new(),
    }
}

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

fn diff_of_with_context(
    before: &SemanticDocument,
    after: &SemanticDocument,
    context: usize,
) -> crate::unified::UnifiedDiff {
    let options = DiffOptions {
        context,
        ..DiffOptions::default()
    };
    let (diff, correspondence) = snapshot_compare(before, after, options);
    unified_with_correspondence(before, after, &diff, &correspondence, options)
}

fn diff_of(before: &SemanticDocument, after: &SemanticDocument) -> crate::unified::UnifiedDiff {
    diff_of_with_context(before, after, DiffOptions::default().context)
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
    let out = diff_of(&make(), &make());
    assert_eq!(out.rows, vec![DiffRow::Skipped { blocks: 3 }]);
    assert_eq!(
        out.stats,
        DiffStats {
            unchanged: 3,
            ..Default::default()
        }
    );
}

#[test]
fn identity_invariance_same_visible_content_fresh_ids_produces_empty_diff() {
    let before_id = ids(3);
    let after_id = ids(3);
    let before = doc(vec![
        block(before_id[0], "one"),
        block(before_id[1], "two"),
        block(before_id[2], "three"),
    ]);
    let after = doc(vec![
        block(after_id[0], "one"),
        block(after_id[1], "two"),
        block(after_id[2], "three"),
    ]);
    let out = diff_of(&before, &after);
    assert_eq!(out.rows, vec![DiffRow::Skipped { blocks: 3 }]);
    assert_eq!(
        out.stats,
        DiffStats {
            unchanged: 3,
            ..Default::default()
        }
    );
}

#[test]
fn pure_addition() {
    let id = ids(2);
    let before = doc(Vec::new());
    let after = doc(vec![block(id[0], "one"), block(id[1], "two")]);
    let out = diff_of(&before, &after);
    assert!(out.rows.iter().all(|row| matches!(row, DiffRow::Added(_))));
    assert_eq!(out.stats.added, 2);
}

#[test]
fn pure_removal() {
    let id = ids(2);
    let before = doc(vec![block(id[0], "one"), block(id[1], "two")]);
    let after = doc(Vec::new());
    let out = diff_of(&before, &after);
    assert_eq!(out.rows.len(), 2);
    assert!(
        out.rows
            .iter()
            .all(|row| matches!(row, DiffRow::Removed(_)))
    );
    assert_eq!(out.stats.removed, 2);
}

#[test]
fn modification_highlights_only_changed_words() {
    let before_id = ids(1);
    let after_id = ids(1);
    let before = doc(vec![block(before_id[0], "The brown fox jumps.")]);
    let after = doc(vec![block(after_id[0], "The red fox jumps.")]);
    let out = diff_of(&before, &after);
    let DiffRow::Modified { spans, .. } = &out.rows[0] else {
        panic!("expected a modified row: {:?}", out.rows);
    };
    let old_text: String = spans
        .iter()
        .filter(|s| s.tag != crate::DiffTag::Insert)
        .map(|s| s.text.as_str())
        .collect();
    let new_text: String = spans
        .iter()
        .filter(|s| s.tag != crate::DiffTag::Delete)
        .map(|s| s.text.as_str())
        .collect();
    assert_eq!(old_text, "The brown fox jumps.");
    assert_eq!(new_text, "The red fox jumps.");
    assert!(
        spans
            .iter()
            .any(|s| s.tag == crate::DiffTag::Delete && s.text.contains("brown"))
    );
    assert!(
        spans
            .iter()
            .any(|s| s.tag == crate::DiffTag::Insert && s.text.contains("red"))
    );
}

#[test]
fn same_text_kind_change_is_modified_with_property_badge() {
    let before_id = ids(1);
    let after_id = ids(1);
    let before = doc(vec![block(before_id[0], "Hello")]);
    let after = doc(vec![quote(after_id[0], "Hello")]);
    let (diff, correspondence) = snapshot_compare(&before, &after, DiffOptions::default());
    assert_eq!(diff.changes.len(), 1);
    let crate::NodeChange::Modified(m) = &diff.changes[0] else {
        panic!("expected modified: {:?}", diff.changes);
    };
    assert!(m.kind_changed);
    assert!(!m.content_changed);

    let out = unified_with_correspondence(
        &before,
        &after,
        &diff,
        &correspondence,
        DiffOptions::default(),
    );
    assert!(matches!(out.rows[0], DiffRow::Modified { .. }));
}

#[test]
fn same_text_heading_change_is_modified_with_property_badge() {
    let before_id = ids(1);
    let after_id = ids(1);
    let before = doc(vec![block(before_id[0], "Hello")]);
    let after = doc(vec![heading(after_id[0], "Hello", 1)]);
    let (diff, correspondence) = snapshot_compare(&before, &after, DiffOptions::default());
    assert_eq!(diff.changes.len(), 1);
    let crate::NodeChange::Modified(m) = &diff.changes[0] else {
        panic!("expected modified: {:?}", diff.changes);
    };
    assert!(m.metadata_changed);
    assert!(!m.kind_changed);
    assert!(!m.content_changed);

    let out = unified_with_correspondence(
        &before,
        &after,
        &diff,
        &correspondence,
        DiffOptions::default(),
    );
    assert!(matches!(out.rows[0], DiffRow::Modified { .. }));
}

#[test]
fn same_text_done_change_is_modified_with_property_badge() {
    let before_id = ids(1);
    let after_id = ids(1);
    let before = doc(vec![task(before_id[0], "Buy milk", false)]);
    let after = doc(vec![task(after_id[0], "Buy milk", true)]);
    let (diff, correspondence) = snapshot_compare(&before, &after, DiffOptions::default());
    let crate::NodeChange::Modified(m) = &diff.changes[0] else {
        panic!("expected modified: {:?}", diff.changes);
    };
    assert!(m.metadata_changed);
    assert!(!m.content_changed);
    let out = unified_with_correspondence(
        &before,
        &after,
        &diff,
        &correspondence,
        DiffOptions::default(),
    );
    assert!(matches!(out.rows[0], DiffRow::Modified { .. }));
}

#[test]
fn same_text_depth_change_is_modified_with_property_badge() {
    let before_id = ids(1);
    let after_id = ids(1);
    let mut before_node = block(before_id[0], "Indented");
    before_node.depth = 1;
    let mut after_node = block(after_id[0], "Indented");
    after_node.depth = 2;
    let before = doc(vec![before_node]);
    let after = doc(vec![after_node]);
    let (diff, correspondence) = snapshot_compare(&before, &after, DiffOptions::default());
    let crate::NodeChange::Modified(m) = &diff.changes[0] else {
        panic!("expected modified: {:?}", diff.changes);
    };
    assert!(m.metadata_changed);
    assert!(!m.content_changed);
    let out = unified_with_correspondence(
        &before,
        &after,
        &diff,
        &correspondence,
        DiffOptions::default(),
    );
    assert!(matches!(out.rows[0], DiffRow::Modified { .. }));
}

#[test]
fn removal_anchors_between_content_paired_neighbors() {
    // F10 scenario: before [A, B, C], after [A', C'] where A≈A' and C≈C'.
    // B was genuinely removed and must render between A' and C', not at the top.
    let before_id = ids(3);
    let after_id = ids(2);
    let before = doc(vec![
        block(before_id[0], "the quick brown fox jumps"),
        block(before_id[1], "beta"),
        block(before_id[2], "the lazy brown dog sleeps"),
    ]);
    let after = doc(vec![
        block(after_id[0], "the quick red fox jumps"),
        block(after_id[1], "the lazy red dog sleeps"),
    ]);
    let out = diff_of(&before, &after);
    assert!(matches!(
        out.rows.as_slice(),
        [
            DiffRow::Modified { .. },
            DiffRow::Removed(removed),
            DiffRow::Modified { .. },
        ] if removed.text == "beta"
    ));
}

#[test]
fn reordering_is_reported_as_add_remove() {
    let before_id = ids(3);
    let after_id = ids(3);
    let before = doc(vec![
        block(before_id[0], "first"),
        block(before_id[1], "second"),
        block(before_id[2], "third"),
    ]);
    let after = doc(vec![
        block(after_id[0], "third"),
        block(after_id[1], "first"),
        block(after_id[2], "second"),
    ]);
    let out = diff_of(&before, &after);
    // Patience aligns "first" and "second"; "third" reads as removed from its
    // old position and added at its new position — no Moved/Reordered rows.
    assert_eq!(out.stats.added, 1);
    assert_eq!(out.stats.removed, 1);
    assert_eq!(out.stats.modified, 0);
    assert_eq!(out.stats.unchanged, 2);
    assert!(
        out.rows
            .iter()
            .all(|row| !matches!(row, DiffRow::Moved { .. } | DiffRow::Reordered { .. }))
    );
}

#[test]
fn context_folding_keeps_adjacent_blocks() {
    let before_id = ids(7);
    let after_id = ids(7);
    let texts = [
        "the quick brown fox jumps",
        "a",
        "b",
        "c",
        "d",
        "e",
        "the lazy brown dog sleeps",
    ];
    let before = doc(texts
        .iter()
        .zip(&before_id)
        .map(|(t, &i)| block(i, t))
        .collect());
    let mut edited: Vec<SemanticNode> = texts
        .iter()
        .zip(&after_id)
        .map(|(t, &i)| block(i, t))
        .collect();
    edited[0] = block(after_id[0], "the quick red fox jumps");
    edited[6] = block(after_id[6], "the lazy red dog sleeps");
    let after = doc(edited);
    let out = diff_of_with_context(&before, &after, 1);
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
fn every_fixture_language_reconstructs_byte_exactly() {
    // Repeating the body gives a token-set similarity of 1.0 in every script,
    // so the block pairs as Modified and we can verify grapheme fidelity.
    for sample in koru_store_core::fixtures::SAMPLES {
        let before_id = NodeId::new();
        let after_id = NodeId::new();
        let before = doc(vec![block(before_id, sample.body)]);
        let edited = format!("{} {}", sample.body, sample.body);
        let after = doc(vec![block(after_id, &edited)]);
        let out = diff_of(&before, &after);
        let DiffRow::Modified { spans, .. } = &out.rows[0] else {
            panic!("{}: expected a modified row: {:?}", sample.tag, out.rows);
        };
        let old_text: String = spans
            .iter()
            .filter(|s| s.tag != crate::DiffTag::Insert)
            .map(|s| s.text.as_str())
            .collect();
        let new_text: String = spans
            .iter()
            .filter(|s| s.tag != crate::DiffTag::Delete)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(old_text, sample.body, "{}", sample.tag);
        assert_eq!(new_text, edited, "{}", sample.tag);
    }
}

#[test]
fn full_page_replacement_highlights_only_changed_words() {
    let before_id = ids(3);
    let after_id = ids(3);
    let before = doc(vec![
        block(before_id[0], "The quick brown fox."),
        block(before_id[1], "Jumps over the lazy dog."),
        block(before_id[2], "The end."),
    ]);
    let after = doc(vec![
        block(after_id[0], "The quick brown fox."),
        block(after_id[1], "Jumps over the lazy cat."),
        block(after_id[2], "The end."),
    ]);
    let out = diff_of(&before, &after);
    assert_eq!(out.stats.modified, 1);
    assert_eq!(out.stats.added, 0);
    assert_eq!(out.stats.removed, 0);
    assert_eq!(out.stats.unchanged, 2);
    let DiffRow::Modified { spans, .. } = &out.rows[1] else {
        panic!("expected second row to be modified: {:?}", out.rows);
    };
    let changes: Vec<_> = spans
        .iter()
        .filter(|s| s.tag != crate::DiffTag::Equal)
        .collect();
    assert_eq!(
        changes.len(),
        2,
        "expected one deletion and one insertion: {:?}",
        spans
    );
    assert!(
        changes
            .iter()
            .any(|s| s.tag == crate::DiffTag::Delete && s.text.contains("dog"))
    );
    assert!(
        changes
            .iter()
            .any(|s| s.tag == crate::DiffTag::Insert && s.text.contains("cat"))
    );
}
