// SPDX-License-Identifier: MPL-2.0

//! One reading of a row, shared by the two gestures that need it.

use super::{OBJECTIVE_CHARS, row_words};
use koru_core::WorkspaceId;
use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};

fn a_row(text: &str) -> (WorkspaceDoc, koru_core::NodeId) {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").expect("a document");
    let node = ws
        .insert(Parent::Root, Node::paragraph(text.to_owned()).unwrap())
        .expect("a row");
    (ws, node)
}

#[test]
fn a_rows_words_are_its_text_trimmed() {
    let (ws, node) = a_row("  Write the release notes  ");
    assert_eq!(row_words(&ws, node), "Write the release notes");
}

/// An empty row is an empty objective, not a refusal: `/delegate` typed alone
/// on a blank line opens the same empty box it always did.
#[test]
fn an_empty_row_is_an_empty_objective() {
    let (ws, node) = a_row("");
    assert!(row_words(&ws, node).is_empty());
}

/// A row longer than an objective may be is **cut, never refused** — the
/// person can see the box and edit it before pressing anything.
#[test]
fn a_row_longer_than_an_objective_is_cut_on_a_character() {
    let long: String = std::iter::repeat_n('é', OBJECTIVE_CHARS + 50).collect();
    let (ws, node) = a_row(&long);
    let words = row_words(&ws, node);
    assert_eq!(words.chars().count(), OBJECTIVE_CHARS);
    assert!(
        words.chars().all(|c| c == 'é'),
        "cut on characters, not bytes"
    );
}

/// A node that is not in the document reads as nothing rather than panicking.
#[test]
fn a_row_that_is_not_there_reads_as_nothing() {
    let (ws, _node) = a_row("something");
    assert!(row_words(&ws, koru_core::NodeId::new()).is_empty());
}
