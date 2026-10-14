// SPDX-License-Identifier: MPL-2.0

use koru_core::{Kind, Node, Parent, TableLayout, WorkspaceDoc, WorkspaceId};

#[test]
fn a_table_layout_round_trips_and_compact_remains_the_default() {
    let mut workspace = WorkspaceDoc::create(WorkspaceId::new(), "tables").unwrap();
    let compact = workspace
        .insert(
            Parent::Root,
            Node::table("| A |\n| --- |\n| one |").unwrap(),
        )
        .unwrap();
    let extended = workspace
        .insert(
            Parent::Root,
            Node::table("| B |\n| --- |\n| two |").unwrap(),
        )
        .unwrap();
    workspace
        .set_table_layout(extended, TableLayout::Extended)
        .unwrap();

    let reopened = WorkspaceDoc::from_snapshot(&workspace.export_snapshot().unwrap()).unwrap();
    assert_eq!(reopened.read(compact).unwrap().meta.table_layout, None);
    assert_eq!(
        reopened.read(extended).unwrap().meta.table_layout,
        Some(TableLayout::Extended)
    );
}

#[test]
fn table_layout_is_ignored_for_non_table_nodes() {
    let mut workspace = WorkspaceDoc::create(WorkspaceId::new(), "tables").unwrap();
    let paragraph = workspace
        .insert(Parent::Root, Node::paragraph("prose").unwrap())
        .unwrap();
    workspace
        .set_table_layout(paragraph, TableLayout::Extended)
        .unwrap();
    let node = workspace.read(paragraph).unwrap();
    assert_eq!(node.kind, Kind::Paragraph);
    assert_eq!(node.meta.table_layout, None);
}
