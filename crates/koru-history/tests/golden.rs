// SPDX-License-Identifier: MPL-2.0

//! Pins the serialized shape of [`koru_history::unified::UnifiedDiff`].
//!
//! The rows are meant to cross process boundaries (worker → view today, IPC or
//! MCP tomorrow), so a change to the wire shape must be a deliberate edit to
//! the fixture, never a surprise. The document pair below exercises every row
//! variant: heading and body context, a folded run, an addition, a removal, a
//! word-level modification, and a reorder.

use koru_core::{Content, Heading, Kind, Node, NodeId, NodeMeta, Parent};
use koru_history::snapshot_diff::snapshot_compare;
use koru_history::unified::{DiffOptions, unified, unified_with_correspondence};
use koru_history::{SemanticDocument, SemanticNode, compare};
use serde_json::Value;

fn block(id: NodeId, kind: Kind, text: &str) -> SemanticNode {
    SemanticNode {
        node: Node {
            id,
            kind,
            meta: NodeMeta::default(),
            content: Content::text(text),
        },
        parent: Parent::Root,
        ordinal: 0,
        depth: 1,
        runs: Vec::new(),
    }
}

fn heading(id: NodeId, text: &str) -> SemanticNode {
    let mut node = block(id, Kind::Paragraph, text);
    node.node.meta.heading = Some(Heading::H2);
    node
}

fn task(id: NodeId, text: &str, done: bool) -> SemanticNode {
    let mut node = block(id, Kind::Task, text);
    node.node.meta.done = done;
    node
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

#[test]
fn the_wire_shape_matches_the_fixture() {
    let id: Vec<NodeId> = (0..10).map(|_| NodeId::new()).collect();
    let before = doc(vec![
        heading(id[0], "Plans"),
        block(id[1], Kind::Paragraph, "The brown fox jumps."),
        block(id[2], Kind::Paragraph, "An old note."),
        block(id[3], Kind::Paragraph, "unchanged one"),
        block(id[4], Kind::Paragraph, "unchanged two"),
        block(id[5], Kind::Paragraph, "unchanged three"),
        block(id[6], Kind::Paragraph, "unchanged four"),
        block(id[7], Kind::Code, "let x = 1;\nlet y = 2;"),
        task(id[8], "ship it", false),
    ]);
    // The code block moved up unedited (a pure reorder); the task was checked
    // off, which outranks any shuffle and reads as Modified.
    let after = doc(vec![
        heading(id[0], "Plans"),
        block(id[1], Kind::Paragraph, "The red fox jumps."),
        block(id[3], Kind::Paragraph, "unchanged one"),
        block(id[4], Kind::Paragraph, "unchanged two"),
        block(id[5], Kind::Paragraph, "unchanged three"),
        block(id[7], Kind::Code, "let x = 1;\nlet y = 2;"),
        block(id[6], Kind::Paragraph, "unchanged four"),
        task(id[8], "ship it", true),
        block(id[9], Kind::Paragraph, "A new closing thought."),
    ]);

    let out = unified(
        &before,
        &after,
        &compare(&before, &after),
        DiffOptions {
            context: 1,
            ..DiffOptions::default()
        },
    );
    let actual = serde_json::to_value(&out).expect("serialize");
    let expected: Value = serde_json::from_str(include_str!("fixtures/unified.expected.json"))
        .expect("fixture parses");
    assert_eq!(
        actual,
        expected,
        "\nthe wire shape changed; if deliberate, update the fixture to:\n{}",
        serde_json::to_string_pretty(&actual).expect("pretty")
    );
}

#[test]
fn full_page_replacement_highlights_only_changed_words() {
    // Delete an entire page and paste back almost the same text with fresh ids.
    // Only the words that actually changed should be highlighted.
    let before_id: Vec<NodeId> = (0..3).map(|_| NodeId::new()).collect();
    let after_id: Vec<NodeId> = (0..3).map(|_| NodeId::new()).collect();
    let before = doc(vec![
        block(before_id[0], Kind::Paragraph, "The quick brown fox."),
        block(before_id[1], Kind::Paragraph, "Jumps over the lazy dog."),
        block(before_id[2], Kind::Paragraph, "The end."),
    ]);
    let after = doc(vec![
        block(after_id[0], Kind::Paragraph, "The quick brown fox."),
        block(after_id[1], Kind::Paragraph, "Jumps over the lazy cat."),
        block(after_id[2], Kind::Paragraph, "The end."),
    ]);

    let (diff, correspondence) = snapshot_compare(&before, &after, DiffOptions::default());
    let out = unified_with_correspondence(
        &before,
        &after,
        &diff,
        &correspondence,
        DiffOptions::default(),
    );

    assert_eq!(out.stats.added, 0);
    assert_eq!(out.stats.removed, 0);
    assert_eq!(out.stats.modified, 1);
    assert_eq!(out.stats.unchanged, 2);

    let actual = serde_json::to_value(&out).expect("serialize");
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/full_page_replacement.expected.json"))
            .expect("fixture parses");
    assert_eq!(
        actual,
        expected,
        "\nthe wire shape changed; if deliberate, update the fixture to:\n{}",
        serde_json::to_string_pretty(&actual).expect("pretty")
    );
}
