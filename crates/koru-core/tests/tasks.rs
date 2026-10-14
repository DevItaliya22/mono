// SPDX-License-Identifier: MPL-2.0

//! Tasks across two documents: what two peers agree on after they meet, and
//! what a copy carries.
//!
//! The single-document behaviour is beside the module, in
//! `src/tasks/tests.rs`. These two properties are not about one function.

use koru_core::attributes::{KEY_DUE, KEY_PRIORITY, Lifecycle, TaskReason};
use koru_core::node::{AttrValue, BlobHash, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::tasks::{
    LifecycleReason, TaskEffect, TaskFields, complete_task, ensure_task, is_task, set_due,
};
use koru_core::time::{DateOnly, Temporal};
use koru_core::{ListStyle, NodeId, Priority, WorkspaceId};

fn forked_pair() -> (WorkspaceDoc, WorkspaceDoc, NodeId) {
    let mut a = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let node = a
        .insert(Parent::Root, Node::paragraph("Launch referral").unwrap())
        .unwrap();
    let snapshot = a.export_snapshot().unwrap();
    let b = WorkspaceDoc::from_snapshot(&snapshot).unwrap();
    (a, b, node)
}

fn sync(a: &mut WorkspaceDoc, b: &mut WorkspaceDoc) {
    let from_a = a.export_all_updates().unwrap();
    let from_b = b.export_all_updates().unwrap();
    a.import(&from_b).unwrap();
    b.import(&from_a).unwrap();
}

fn fields(ws: &WorkspaceDoc, node: NodeId) -> TaskFields {
    TaskFields::of(&ws.read(node).unwrap())
}

fn on(s: &str) -> Temporal {
    Temporal::Date(DateOnly::parse(s).unwrap())
}

/// Two peers make the same node a task at the same time. They converge on one
/// marker without talking — the cardinality-one write folds the duplicates into
/// the lowest `AttrId`, and reads take the same record.
#[test]
fn two_peers_creating_one_task_converge() {
    let (mut a, mut b, node) = forked_pair();

    ensure_task(&mut a, node, TaskReason::Explicit).unwrap();
    ensure_task(&mut b, node, TaskReason::TasksView).unwrap();

    sync(&mut a, &mut b);

    let from_a = fields(&a, node);
    let from_b = fields(&b, node);
    assert_eq!(from_a, from_b, "the replicas agree");
    assert!(
        matches!(
            from_a.marker,
            Some(TaskReason::Explicit) | Some(TaskReason::TasksView)
        ),
        "and it is one of the two written, never a third"
    );
    assert!(is_task(&from_a));

    // The next canonical command on either side leaves one record standing.
    complete_task(
        &mut a,
        node,
        LifecycleReason::ExplicitComplete,
        koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
    )
    .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(fields(&b, node).lifecycle, Lifecycle::Completed);
}

/// One peer types `/task`; the other, offline, gives the same node a deadline.
/// **The winner is deterministic and nothing depends on which it is**: both
/// replicas read the same reason, and every lifecycle question answers the same
/// either way, because they all ask `is_task`.
#[test]
fn a_concurrent_explicit_and_due_creation_converge_on_one_reason() {
    let (mut a, mut b, node) = forked_pair();

    ensure_task(&mut a, node, TaskReason::Explicit).unwrap();
    let due = set_due(&mut b, node, on("2026-09-10")).unwrap();
    assert_eq!(due.created_with_reason, Some(TaskReason::Due));

    sync(&mut a, &mut b);

    let from_a = fields(&a, node);
    assert_eq!(from_a, fields(&b, node), "the replicas agree");
    assert!(
        matches!(
            from_a.marker,
            Some(TaskReason::Explicit) | Some(TaskReason::Due)
        ),
        "one of the two, deterministically"
    );
    assert_eq!(from_a.due, Some(on("2026-09-10")), "the deadline survives");

    // Whichever reason won, the task behaves identically.
    assert!(is_task(&from_a));
    assert_eq!(
        complete_task(
            &mut a,
            node,
            LifecycleReason::ExplicitComplete,
            koru_core::time::DateOnly::parse("2026-09-06").unwrap()
        )
        .unwrap()
        .effect,
        TaskEffect::Completed
    );
    sync(&mut a, &mut b);
    assert_eq!(fields(&a, node), fields(&b, node));
    assert_eq!(fields(&b, node).lifecycle, Lifecycle::Completed);
}

/// **A copy is an independent task, never an alias** (SPEC §6): it keeps the
/// marker, the lifecycle, the due, the priority and the subtree's check state,
/// and every node in it is a different node.
#[test]
fn a_copy_keeps_its_task_state_and_gains_fresh_identities() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let task = ws
        .insert(Parent::Root, Node::paragraph("Launch").unwrap())
        .unwrap();
    ensure_task(&mut ws, task, TaskReason::Explicit).unwrap();
    set_due(&mut ws, task, on("2026-09-10")).unwrap();
    ws.set_attribute(task, KEY_PRIORITY, AttrValue::Text("high".into()))
        .unwrap();
    let block = ws
        .insert(Parent::Node(task), Node::list_block().unwrap())
        .unwrap();
    ws.set_list_style(block, Some(ListStyle::Checkbox)).unwrap();
    let item = ws
        .insert(
            Parent::Node(block),
            Node::paragraph("Write the copy").unwrap(),
        )
        .unwrap();
    complete_task(
        &mut ws,
        task,
        LifecycleReason::ExplicitComplete,
        koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
    )
    .unwrap();

    let copy = ws.duplicate(task, Parent::Root, "").unwrap();
    assert_ne!(copy, task, "a copy is a different node");

    let original = fields(&ws, task);
    let copied = fields(&ws, copy);
    assert_eq!(copied.marker, original.marker);
    assert_eq!(copied.due, original.due);
    assert_eq!(copied.lifecycle, Lifecycle::Completed);
    assert!(copied.done);
    assert_eq!(ws.read(copy).unwrap().priority(), Some(Priority::High));

    // The subtree came with it, with new ids and its check state intact.
    let copied_item = ws
        .children(Parent::Node(ws.children(Parent::Node(copy)).unwrap()[0]))
        .unwrap()[0];
    assert_ne!(copied_item, item);
    assert!(ws.read(copied_item).unwrap().meta.done);

    // Independent: reopening one says nothing about the other.
    koru_core::tasks::reopen_task(&mut ws, copy, LifecycleReason::ExplicitReopen).unwrap();
    assert_eq!(fields(&ws, copy).lifecycle, Lifecycle::Open);
    assert_eq!(fields(&ws, task).lifecycle, Lifecycle::Completed);
}

/// The same, for a node whose content is a blob: an image task's deadline has
/// to survive a copy, which is only true because a blob node carries its
/// attributes.
#[test]
fn a_copied_image_is_still_the_task_it_was() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let image = ws
        .insert(
            Parent::Root,
            Node::image(BlobHash::new("fnv1a", "abc"), "image/png", Some(4), Some(4)).unwrap(),
        )
        .unwrap();
    set_due(&mut ws, image, on("2026-09-10")).unwrap();

    let copy = ws.duplicate(image, Parent::Root, "").unwrap();
    let copied = fields(&ws, copy);
    assert!(is_task(&copied), "a copied image task is still a task");
    assert_eq!(copied.due, Some(on("2026-09-10")));
    assert_eq!(
        ws.read(copy).unwrap().attrs_with_role(KEY_DUE).count(),
        1,
        "one due record, not two"
    );
}
