// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_core::{Attr, AttrValue, DocRef, Mark, Ref};

fn fixture(text: &str) -> (WorkspaceDoc, NodeId, NodeId) {
    let mut model = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "Test").unwrap();
    let root = model
        .insert(Parent::Root, Node::document("Test").unwrap())
        .unwrap();
    let block = model
        .insert(Parent::Node(root), Node::paragraph(text).unwrap())
        .unwrap();
    (model, root, block)
}
fn edit(block: NodeId, before: &str, after: &str) -> DocumentTextEdit {
    DocumentTextEdit {
        block_id: block.to_storage(),
        before: before.into(),
        after: after.into(),
        create_paragraph: false,
        split: None,
    }
}
fn text(model: &WorkspaceDoc, block: NodeId) -> String {
    model.read(block).unwrap().content.as_text().unwrap().into()
}

#[test]
fn unicode_splice_stays_on_scalar_boundaries() {
    for (before, after) in [
        ("a😀z", "a😄z"),
        ("éمرحبا", "é!مرحبا"),
        ("", "你好"),
        ("👋", ""),
    ] {
        let (range, inserted) = splice(before, after);
        let mut actual = before.to_owned();
        actual.replace_range(range, inserted);
        assert_eq!(actual, after);
    }
}

#[test]
fn editing_preserves_marks_and_reference_attributes_and_replicates() {
    let (mut model, _, block) = fixture("Bold tail");
    model.set_mark(block, 0..4, Mark::Bold, true).unwrap();
    model
        .insert_pill(
            block,
            9,
            &Attr::labelled(
                "document",
                AttrValue::Ref(Ref::in_doc(
                    NodeId::new(),
                    DocRef::Document(koru_core::DocumentId::new()),
                )),
                "Linked",
            ),
        )
        .unwrap();
    let snapshot = model.export_snapshot().unwrap();
    let mut peer = WorkspaceDoc::from_snapshot(&snapshot).unwrap();
    let before_version = model.version();
    let before = text(&model, block);
    let after = before.replace("tail", "longer tail 😀");
    apply_text_edit(&mut model, block, &edit(block, &before, &after)).unwrap();
    let update = model.export_updates_since(&before_version).unwrap();
    peer.import(&update).unwrap();
    assert_eq!(text(&peer, block), after);
    let runs = peer.runs(block).unwrap();
    assert!(runs.iter().any(|run| run.is_pill()));
    assert_eq!(
        model.read(block).unwrap().content.attrs(),
        peer.read(block).unwrap().content.attrs()
    );
    assert!(peer.runs(block).unwrap().iter().any(|run| run.bold));
}

#[test]
fn stale_edit_and_forged_anchor_do_not_modify_the_document() {
    let (mut model, _, block) = fixture("latest");
    let version = model.version();
    assert_eq!(
        apply_text_edit(&mut model, block, &edit(block, "old", "draft")),
        Err("text_edit_conflict".into())
    );
    assert_eq!(
        apply_text_edit(&mut model, block, &edit(block, "latest", "latest\u{fffc}")),
        Err("invalid_inline_text".into())
    );
    assert_eq!(version, model.version());
    assert_eq!(text(&model, block), "latest");
}

#[test]
fn paragraph_creation_and_acknowledgement_retry_are_idempotent() {
    let (mut model, root, _) = fixture("");
    let block = NodeId::new();
    let mut command = edit(block, "", "Hello phone");
    command.create_paragraph = true;
    apply_text_edit(&mut model, block, &command).unwrap();
    let version = model.version();
    apply_text_edit(&mut model, block, &command).unwrap();
    assert_eq!(model.version(), version);
    assert_eq!(model.parent(block).unwrap(), Parent::Node(root));
}

#[test]
fn root_title_and_other_roots_are_not_body_edit_targets() {
    let (mut model, root, _) = fixture("body");
    assert!(apply_text_edit(&mut model, root, &edit(root, "Test", "Wrong")).is_err());
    let detached = model
        .insert(Parent::Root, Node::paragraph("Other").unwrap())
        .unwrap();
    assert!(apply_text_edit(&mut model, detached, &edit(detached, "Other", "Wrong")).is_err());
    assert_eq!(text(&model, detached), "Other");
}

#[test]
fn offline_phone_and_tablet_edits_converge_without_a_desktop() {
    let (model, _, block) = fixture("middle");
    let snapshot = model.export_snapshot().unwrap();
    let mut phone = WorkspaceDoc::from_snapshot(&snapshot).unwrap();
    let mut tablet = WorkspaceDoc::from_snapshot(&snapshot).unwrap();
    let base = phone.version();
    apply_text_edit(&mut phone, block, &edit(block, "middle", "phone middle")).unwrap();
    apply_text_edit(&mut tablet, block, &edit(block, "middle", "middle tablet")).unwrap();
    let phone_update = phone.export_updates_since(&base).unwrap();
    let tablet_update = tablet.export_updates_since(&base).unwrap();
    phone.import(&tablet_update).unwrap();
    tablet.import(&phone_update).unwrap();
    assert_eq!(text(&phone, block), "phone middle tablet");
    assert_eq!(text(&tablet, block), text(&phone, block));
}

#[test]
fn deleting_a_reference_anchor_removes_its_attribute_together() {
    let (mut model, _, block) = fixture("Text");
    model
        .insert_pill(
            block,
            4,
            &Attr::labelled(
                "document",
                AttrValue::Ref(Ref::in_doc(
                    NodeId::new(),
                    DocRef::Document(koru_core::DocumentId::new()),
                )),
                "Linked",
            ),
        )
        .unwrap();
    let before = text(&model, block);
    assert_eq!(model.read(block).unwrap().content.attrs().len(), 1);
    apply_text_edit(&mut model, block, &edit(block, &before, "Text")).unwrap();
    assert!(model.read(block).unwrap().content.attrs().is_empty());
    assert_eq!(text(&model, block), "Text");
}

#[test]
fn newer_document_schema_is_never_written() {
    let (model, _, block) = fixture("Keep me");
    let doc = loro::LoroDoc::new();
    doc.import(&model.export_snapshot().unwrap()).unwrap();
    doc.get_map("meta")
        .insert("schema_version", i64::MAX)
        .unwrap();
    doc.commit();
    let mut future = WorkspaceDoc::adopt(doc).unwrap();
    let version = future.version();
    assert_eq!(
        apply_text_edit(&mut future, block, &edit(block, "Keep me", "Changed")),
        Err("document_read_only".into())
    );
    assert_eq!(future.version(), version);
}

fn split_edit(
    block: NodeId,
    before: &str,
    after: &str,
    tail: NodeId,
    at: usize,
) -> DocumentTextEdit {
    let mut command = edit(block, before, after);
    command.split = Some(koru_contract::mobile_proto::DocumentTextSplit {
        block_id: tail.to_storage(),
        at_utf8: at as u32,
    });
    command
}

#[test]
fn enter_moves_formatted_text_and_references_and_replicates_both_halves() {
    let (mut model, root, block) = fixture("Head Bold");
    model.set_mark(block, 5..9, Mark::Bold, true).unwrap();
    model
        .insert_pill(
            block,
            9,
            &Attr::labelled(
                "document",
                AttrValue::Ref(Ref::in_doc(
                    NodeId::new(),
                    DocRef::Document(koru_core::DocumentId::new()),
                )),
                "Linked",
            ),
        )
        .unwrap();
    let snapshot = model.export_snapshot().unwrap();
    let mut peer = WorkspaceDoc::from_snapshot(&snapshot).unwrap();
    let version = model.version();
    let before = text(&model, block);
    let tail = NodeId::new();
    let command = split_edit(block, &before, &before, tail, 5);
    apply_text_edit(&mut model, block, &command).unwrap();
    peer.import(&model.export_updates_since(&version).unwrap())
        .unwrap();
    assert_eq!(text(&peer, block), "Head ");
    assert_eq!(text(&peer, tail), before[5..]);
    assert!(peer.runs(tail).unwrap().iter().any(|run| run.bold));
    assert_eq!(peer.read(tail).unwrap().content.attrs().len(), 1);
    assert!(peer.read(block).unwrap().content.attrs().is_empty());
    assert_eq!(
        peer.children(Parent::Node(root)).unwrap(),
        vec![block, tail]
    );
    let committed = model.version();
    apply_text_edit(&mut model, block, &command).unwrap();
    assert_eq!(model.version(), committed);
}

#[test]
fn enter_accepts_pending_text_and_unicode_but_rejects_mid_scalar_offsets() {
    let (mut model, _, block) = fixture("Hello");
    let tail = NodeId::new();
    let command = split_edit(block, "Hello", "Hello 😀tail", tail, "Hello 😀".len());
    let mut invalid = command.clone();
    invalid.split.as_mut().unwrap().at_utf8 -= 1;
    let version = model.version();
    assert!(apply_text_edit(&mut model, block, &invalid).is_err());
    assert_eq!(model.version(), version);
    apply_text_edit(&mut model, block, &command).unwrap();
    assert_eq!(text(&model, block), "Hello 😀");
    assert_eq!(text(&model, tail), "tail");
}

#[test]
fn enter_can_create_an_empty_paragraph_and_preserves_existing_children() {
    let (mut model, _, block) = fixture("Heading");
    let child = model
        .insert(Parent::Node(block), Node::paragraph("Keep child").unwrap())
        .unwrap();
    let tail = NodeId::new();
    apply_text_edit(
        &mut model,
        block,
        &split_edit(block, "Heading", "Heading", tail, 7),
    )
    .unwrap();
    assert_eq!(text(&model, tail), "");
    assert_eq!(
        model.children(Parent::Node(block)).unwrap(),
        vec![tail, child]
    );
    let blank = NodeId::new();
    let next = NodeId::new();
    let mut command = split_edit(blank, "", "", next, 0);
    command.create_paragraph = true;
    apply_text_edit(&mut model, blank, &command).unwrap();
    assert_eq!(text(&model, blank), "");
    assert_eq!(text(&model, next), "");
}
