// SPDX-License-Identifier: MPL-2.0

//! The diff against real Loro history: two genuine CRDT snapshots, decoded
//! through the same `SemanticDocument::from_snapshot` the history worker
//! uses, diffed both ways. Everything below the HTTP fetch is real.

use koru_core::schema::WorkspaceDoc;
use koru_core::{Content, Kind, Node, NodeId, NodeMeta, Parent, WorkspaceId};
use koru_history::unified::{DiffOptions, DiffRow, unified};
use koru_history::{DiffTag, SemanticDocument, compare};

fn paragraph(text: &str) -> Node {
    Node {
        id: NodeId::new(),
        kind: Kind::Paragraph,
        meta: NodeMeta::default(),
        content: Content::text(text),
    }
}

#[test]
fn two_real_snapshots_diff_forward_and_reverse() {
    let mut doc = WorkspaceDoc::create(WorkspaceId::new(), "history test").expect("create");
    let node = doc
        .insert(Parent::Root, paragraph("Hello world."))
        .expect("insert");
    let v1 = doc.export_snapshot().expect("v1");

    doc.set_text(node, "Hello beautiful world.").expect("edit");
    let v2 = doc.export_snapshot().expect("v2");

    let a = SemanticDocument::from_snapshot(&v1).expect("decode v1");
    let b = SemanticDocument::from_snapshot(&v2).expect("decode v2");

    let forward = unified(&a, &b, &compare(&a, &b), DiffOptions::default());
    assert_eq!(forward.stats.modified, 1, "{:?}", forward.rows);
    let inserted: Vec<&str> = forward
        .rows
        .iter()
        .filter_map(|row| match row {
            DiffRow::Modified { spans, .. } => Some(spans),
            _ => None,
        })
        .flatten()
        .filter(|s| s.tag == DiffTag::Insert)
        .map(|s| s.text.as_str())
        .collect();
    assert!(
        inserted.contains(&"beautiful"),
        "forward diff must report the added word: {inserted:?}"
    );

    // The same two snapshots the other way round: the addition reverses.
    let reverse = unified(&b, &a, &compare(&b, &a), DiffOptions::default());
    assert_eq!(reverse.stats.modified, 1);
    let removed: Vec<&str> = reverse
        .rows
        .iter()
        .filter_map(|row| match row {
            DiffRow::Modified { spans, .. } => Some(spans),
            _ => None,
        })
        .flatten()
        .filter(|s| s.tag == DiffTag::Delete)
        .map(|s| s.text.as_str())
        .collect();
    assert!(
        removed.contains(&"beautiful"),
        "reverse diff must report the same word as removed: {removed:?}"
    );
}

#[test]
fn a_block_added_in_history_is_added_in_the_diff() {
    let mut doc = WorkspaceDoc::create(WorkspaceId::new(), "history test").expect("create");
    doc.insert(Parent::Root, paragraph("First."))
        .expect("insert");
    let v1 = doc.export_snapshot().expect("v1");
    doc.insert(Parent::Root, paragraph("Second thought."))
        .expect("insert");
    let v2 = doc.export_snapshot().expect("v2");

    let a = SemanticDocument::from_snapshot(&v1).expect("decode v1");
    let b = SemanticDocument::from_snapshot(&v2).expect("decode v2");
    let out = unified(&a, &b, &compare(&a, &b), DiffOptions::default());
    assert_eq!(out.stats.added, 1, "{:?}", out.rows);
    assert!(
        out.rows
            .iter()
            .any(|row| matches!(row, DiffRow::Added(block) if block.text == "Second thought."))
    );
}
