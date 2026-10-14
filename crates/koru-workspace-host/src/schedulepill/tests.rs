// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};

fn holding() -> (koru_workspace::Workspace, DocumentId, NodeId) {
    let mut doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "Work").unwrap();
    let root = doc
        .insert(Parent::Root, Node::document("Plan").unwrap())
        .unwrap();
    let document = DocumentId::new();
    (
        koru_workspace::Workspace::holding_for_tests(doc, document),
        document,
        root,
    )
}

fn schedule_attrs(doc: &WorkspaceDoc, node: NodeId) -> Vec<koru_core::node::Attr> {
    doc.read(node)
        .unwrap()
        .content
        .attrs()
        .iter()
        .filter(|attr| attr.role == koru_core::attributes::ROLE_SCHEDULE)
        .cloned()
        .collect()
}

#[test]
fn schedule_pill_attr_keeps_the_typed_identity_and_fallback_name() {
    let schedule = ScheduleId::mint();
    let pill = SchedulePill {
        document: DocumentId::new(),
        node: NodeId::new(),
        schedule,
        label: "Research".into(),
    };

    let attr = pill.attr();
    assert_eq!(attr.role, koru_core::attributes::ROLE_SCHEDULE);
    assert_eq!(attr.label.as_deref(), Some("Research"));
    assert!(matches!(attr.value, AttrValue::Text(ref id) if id == &schedule.to_storage()));
}

#[test]
fn duplicate_schedule_identity_is_detected_without_label_matching() {
    let mut doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "t").unwrap();
    let node = doc
        .insert(Parent::Root, Node::paragraph("Plan").unwrap())
        .unwrap();
    let schedule = ScheduleId::mint();
    doc.insert_pill(
        node,
        4,
        &Attr::labelled(
            koru_core::attributes::ROLE_SCHEDULE,
            AttrValue::Text(schedule.to_storage()),
            "Old name",
        ),
    )
    .unwrap();

    assert!(already_there(&doc, node, schedule).unwrap());
}

#[test]
fn serving_a_schedule_places_one_inline_pill_after_the_origin_text() {
    let (mut workspace, document, root) = holding();
    let node = workspace
        .ws
        .insert(
            Parent::Node(root),
            Node::paragraph("Research this").unwrap(),
        )
        .unwrap();
    let schedule = ScheduleId::mint();
    let pill = SchedulePill {
        document,
        node,
        schedule,
        label: "Morning research".into(),
    };

    serve(&mut workspace, &pill).unwrap();

    let read = workspace.ws.read(node).unwrap();
    assert!(
        read.content
            .as_text()
            .unwrap()
            .starts_with("Research this ")
    );
    let attrs = schedule_attrs(&workspace.ws, node);
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0].label.as_deref(), Some("Morning research"));
    assert!(matches!(&attrs[0].value, AttrValue::Text(id) if id == &schedule.to_storage()));
    assert!(
        workspace
            .ws
            .runs(node)
            .unwrap()
            .iter()
            .any(|run| run.pill == Some(attrs[0].id))
    );
}

#[test]
fn replaying_the_same_schedule_writes_no_second_pill() {
    let (mut workspace, document, root) = holding();
    let node = workspace
        .ws
        .insert(
            Parent::Node(root),
            Node::paragraph("Research this").unwrap(),
        )
        .unwrap();
    let pill = SchedulePill {
        document,
        node,
        schedule: ScheduleId::mint(),
        label: "Morning research".into(),
    };

    serve(&mut workspace, &pill).unwrap();
    let after_once = workspace
        .ws
        .read(node)
        .unwrap()
        .content
        .as_text()
        .unwrap()
        .to_owned();
    serve(&mut workspace, &pill).unwrap();

    assert_eq!(schedule_attrs(&workspace.ws, node).len(), 1);
    assert_eq!(
        workspace.ws.read(node).unwrap().content.as_text().unwrap(),
        after_once
    );
}
