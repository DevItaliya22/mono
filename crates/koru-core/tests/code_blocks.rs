// SPDX-License-Identifier: MPL-2.0

//! Code blocks: a multi-line `Text` kind, the `language` meta field, the
//! `set_kind` / `set_language` operations, and the inline-code mark.
//!
//! These pin the model contracts the editor and `koru-blocks` build on: a code
//! block's text may contain newlines, the grammar round-trips through storage,
//! and inline code is a mark like any other (so split/merge carry it).

use koru_core::node::{Content, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::text::Mark;
use koru_core::{Kind, NodeId, WorkspaceId};

fn ws() -> WorkspaceDoc {
    WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap()
}

/// A fresh paragraph under the root, the shape `/code` and the fence start from.
fn paragraph(ws: &mut WorkspaceDoc, text: &str) -> NodeId {
    ws.insert(Parent::Root, Node::paragraph(text).unwrap())
        .unwrap()
}

#[test]
fn set_text_allows_newlines_for_a_code_block() {
    let mut ws = ws();
    let id = ws
        .insert(Parent::Root, Node::code("let x = 1;").unwrap())
        .unwrap();

    // A multi-line body replaces the single-line one — illegal for a task,
    // legal for a code block.
    ws.set_text(id, "let x = 1;\nlet y = 2;").unwrap();
    assert_eq!(
        ws.read(id).unwrap().content.as_text(),
        Some("let x = 1;\nlet y = 2;")
    );
}

#[test]
fn insert_text_allows_a_newline_in_a_code_block() {
    let mut ws = ws();
    let id = ws.insert(Parent::Root, Node::code("ab").unwrap()).unwrap();

    ws.insert_text(id, 1, "\n").unwrap();
    assert_eq!(ws.read(id).unwrap().content.as_text(), Some("a\nb"));
}

#[test]
fn a_task_still_rejects_a_newline() {
    let mut ws = ws();
    let id = ws.insert(Parent::Root, Node::task("ab").unwrap()).unwrap();

    assert!(ws.set_text(id, "a\nb").is_err());
    assert!(ws.insert_text(id, 1, "\n").is_err());
    assert_eq!(ws.read(id).unwrap().content.as_text(), Some("ab"));
}

#[test]
fn set_kind_relabels_a_paragraph_to_code_and_back() {
    let mut ws = ws();
    let id = paragraph(&mut ws, "fn main() {}");

    // Paragraph -> Code keeps the text and identity.
    ws.set_kind(id, Kind::Code).unwrap();
    let node = ws.read(id).unwrap();
    assert_eq!(node.kind, Kind::Code);
    assert_eq!(node.content.as_text(), Some("fn main() {}"));

    // Once it is a code block, newlines are legal — so Enter-as-newline works
    // the moment the conversion lands.
    ws.set_text(id, "fn main() {\n}").unwrap();
    assert_eq!(
        ws.read(id).unwrap().content.as_text(),
        Some("fn main() {\n}")
    );

    // Code -> Paragraph relabels back, but the multi-line text is now illegal
    // for the new kind, so the relabel refuses rather than producing a node the
    // one-line rule forbids.
    assert!(ws.set_kind(id, Kind::Paragraph).is_err());
    ws.set_text(id, "plain again").unwrap();
    ws.set_kind(id, Kind::Paragraph).unwrap();
    assert_eq!(ws.read(id).unwrap().kind, Kind::Paragraph);
}

#[test]
fn set_kind_to_a_blob_kind_is_refused() {
    let mut ws = ws();
    let id = paragraph(&mut ws, "not a blob");
    // Relabelling to an image would leave text content under a blob kind — use
    // `set_image` for that, which swaps the content too.
    assert!(ws.set_kind(id, Kind::Image).is_err());
    assert_eq!(ws.read(id).unwrap().kind, Kind::Paragraph);
}

#[test]
fn relabelling_a_parent_into_code_outdents_its_children() {
    let mut ws = ws();
    let parent = ws
        .insert(Parent::Root, Node::task("fn main() {}").unwrap())
        .unwrap();
    let child = ws
        .insert(Parent::Node(parent), Node::task("let x = 1;").unwrap())
        .unwrap();
    let grandchild = ws
        .insert(Parent::Node(child), Node::task("let y = 2;").unwrap())
        .unwrap();

    ws.set_kind(parent, Kind::Code).unwrap();

    assert_eq!(ws.read(parent).unwrap().kind, Kind::Code);
    assert_eq!(
        ws.read(parent).unwrap().content.as_text(),
        Some("fn main() {}")
    );
    // The children outdented into the parent's former place, keeping their
    // shape — not stranded under a node that cannot adopt them.
    assert_eq!(ws.parent(child).unwrap(), Parent::Root);
    assert_eq!(ws.parent(grandchild).unwrap(), Parent::Node(child));
    assert_eq!(ws.children(Parent::Node(parent)).unwrap().len(), 0);
}

#[test]
fn language_round_trips_through_storage() {
    let mut ws = ws();
    let id = paragraph(&mut ws, "let x = 1;");
    ws.set_kind(id, Kind::Code).unwrap();

    ws.set_language(id, Some("rust")).unwrap();
    assert_eq!(ws.read(id).unwrap().meta.language.as_deref(), Some("rust"));

    // `None` removes it — auto-detect — and re-reading reports absent.
    ws.set_language(id, None).unwrap();
    assert_eq!(ws.read(id).unwrap().meta.language, None);
}

#[test]
fn a_code_block_survives_export_and_import() {
    let mut ws = ws();
    let id = ws
        .insert(Parent::Root, Node::code("let x = 1;\nlet y = 2;").unwrap())
        .unwrap();
    ws.set_language(id, Some("rust")).unwrap();

    let bytes = ws.export_snapshot().unwrap();
    let loaded = WorkspaceDoc::from_snapshot(&bytes).unwrap();
    let loaded_id = loaded.walk().unwrap().first().unwrap().0;
    let node = loaded.read(loaded_id).unwrap();
    assert_eq!(node.kind, Kind::Code);
    assert_eq!(node.content.as_text(), Some("let x = 1;\nlet y = 2;"));
    assert_eq!(node.meta.language.as_deref(), Some("rust"));
}

#[test]
fn inline_code_mark_round_trips() {
    let mut ws = ws();
    let id = paragraph(&mut ws, "run npm install");

    ws.set_mark(id, 4..15, Mark::Code, true).unwrap();
    let code_runs: Vec<_> = ws
        .runs(id)
        .unwrap()
        .into_iter()
        .filter(|r| r.code)
        .collect();
    assert_eq!(code_runs.len(), 1);
    assert_eq!(code_runs[0].text, "npm install");

    // Toggling off leaves plain text behind, with no orphan mark.
    ws.set_mark(id, 4..15, Mark::Code, false).unwrap();
    assert!(!ws.runs(id).unwrap().iter().any(|r| r.code));
}

/// Inline code is a mark, so the structural ops carry it the way they carry
/// bold — splitting a line inside an inline-code span keeps both halves coded
/// on their respective pieces. No delimiters are stored, so the text is plain.
#[test]
fn inline_code_survives_a_split() {
    let mut ws = ws();
    let id = paragraph(&mut ws, "use cargo now");
    // Mark `cargo` — bytes 4..9. The backticks are not in the text; this is a
    // mark, not a delimiter.
    ws.set_mark(id, 4..9, Mark::Code, true).unwrap();

    let tail = ws.split(id, 7).unwrap();
    let head = ws.read(id).unwrap();
    assert_eq!(head.content.as_text(), Some("use car"));
    // The head kept the front of the marked range, still coded.
    assert!(ws.runs(id).unwrap().iter().any(|r| r.code));
    // The tail kept the back half, also coded.
    assert!(ws.runs(tail).unwrap().iter().any(|r| r.code));
}

/// A code block has no `Content::Text` bytes that `insert_text` would reject
/// for being a blob — confirming `Kind::Code` is `Shape::Text` end to end.
#[test]
fn code_is_a_text_shaped_kind() {
    assert_eq!(Kind::Code.shape(), koru_core::node::Shape::Text);
    let node = Node::new(Kind::Code, Content::text("ok")).unwrap();
    assert_eq!(node.kind, Kind::Code);
}
