// SPDX-License-Identifier: MPL-2.0

//! The routing table's five arms, exercised from the mechanism's new home —
//! and the sink's bound, which the priority arm used to walk past.

use super::*;
use crate::WorkspaceId;
use crate::attributes::{KEY_DUE, KEY_PRIORITY, KEY_TASK, TaskReason};
use crate::node::Node;
use crate::schema::Parent;
use crate::tasks::{TaskFields, ensure_task, is_task_node};
use crate::time::{DateOnly, Temporal};

fn a_row() -> (WorkspaceDoc, NodeId) {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "Tasks").unwrap();
    let page = ws
        .insert(Parent::Root, Node::document("Plans").unwrap())
        .unwrap();
    let row = ws
        .insert(
            Parent::Node(page),
            Node::paragraph("post the form").unwrap(),
        )
        .unwrap();
    (ws, row)
}

fn on(s: &str) -> AttrValue {
    AttrValue::Date(DateOnly::parse(s).unwrap())
}

fn priority_of(ws: &WorkspaceDoc, node: NodeId) -> Option<Priority> {
    ws.read(node).unwrap().priority()
}

fn priority_changes(sink: &ActivitySink) -> Vec<(AttrChange, Option<Priority>)> {
    sink.iter()
        .flat_map(|a| a.intents.iter())
        .filter_map(|i| match i {
            ActivityIntent::PriorityChanged {
                change, priority, ..
            } => Some((*change, *priority)),
            _ => None,
        })
        .collect()
}

/// `Refused`: a command-only key does not land and owes nothing — and a commit
/// where nothing landed still answers `Ok` when nothing was *refused* by a
/// command either, because a skipped node is not a failure.
#[test]
fn a_command_only_key_is_refused_at_the_door() {
    let (mut ws, row) = a_row();
    let mut sink = ActivitySink::new();
    set_attribute(
        &mut ws,
        &[row],
        KEY_TASK,
        AttrValue::Text("explicit".into()),
        &mut sink,
    )
    .unwrap();
    assert!(!is_task_node(&ws, row).unwrap(), "nothing was written");
    assert!(sink.is_empty(), "and nothing is owed");
}

/// `Due`: the command door. A deadline alone makes a task, and the command's
/// own outcome is what the sink carries.
#[test]
fn a_due_takes_the_command_door_and_owes_the_commands_activity() {
    let (mut ws, row) = a_row();
    let mut sink = ActivitySink::new();
    set_attribute(&mut ws, &[row], KEY_DUE, on("2026-09-04"), &mut sink).unwrap();
    assert!(
        is_task_node(&ws, row).unwrap(),
        "a deadline alone makes a task"
    );
    assert_eq!(
        TaskFields::of(&ws.read(row).unwrap()).due,
        Some(Temporal::Date(DateOnly::parse("2026-09-04").unwrap()))
    );
    assert_eq!(sink.len(), 1, "one command, one thing owed");
    assert!(!sink[0].intents.is_empty());

    clear_attribute(&mut ws, &[row], KEY_DUE, &mut sink).unwrap();
    assert!(
        is_task_node(&ws, row).unwrap(),
        "clearing a due marks first"
    );
    assert_eq!(sink.len(), 2);
}

/// `Due` on a kind that cannot be a task is a refusal — and when nothing else
/// landed, it is reported rather than swallowed.
#[test]
fn a_due_on_an_incapable_kind_is_reported_when_nothing_landed() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "Tasks").unwrap();
    let page = ws
        .insert(Parent::Root, Node::document("Plans").unwrap())
        .unwrap();
    let code = ws
        .insert(Parent::Node(page), Node::code("fn main() {}").unwrap())
        .unwrap();
    let mut sink = ActivitySink::new();
    let before = ws.version();
    assert!(set_attribute(&mut ws, &[code], KEY_DUE, on("2026-09-04"), &mut sink).is_err());
    assert_eq!(ws.version(), before, "nothing written");
    assert!(sink.is_empty(), "nothing owed");
}

/// `PriorityOnTask`: the ordinary door plus one intent — set, changed,
/// cleared — and a write that changes nothing owes nothing.
#[test]
fn a_priority_on_a_task_owes_set_changed_and_cleared_and_nothing_for_a_no_op() {
    let (mut ws, row) = a_row();
    ensure_task(&mut ws, row, TaskReason::Explicit).unwrap();
    let mut sink = ActivitySink::new();

    set_attribute(
        &mut ws,
        &[row],
        KEY_PRIORITY,
        AttrValue::Text("high".into()),
        &mut sink,
    )
    .unwrap();
    assert_eq!(priority_of(&ws, row), Some(Priority::High));
    set_attribute(
        &mut ws,
        &[row],
        KEY_PRIORITY,
        AttrValue::Text("medium".into()),
        &mut sink,
    )
    .unwrap();
    assert_eq!(priority_of(&ws, row), Some(Priority::Medium));
    set_attribute(
        &mut ws,
        &[row],
        KEY_PRIORITY,
        AttrValue::Text("medium".into()),
        &mut sink,
    )
    .unwrap();
    clear_attribute(&mut ws, &[row], KEY_PRIORITY, &mut sink).unwrap();
    assert_eq!(priority_of(&ws, row), None);
    clear_attribute(&mut ws, &[row], KEY_PRIORITY, &mut sink).unwrap();

    assert_eq!(
        priority_changes(&sink),
        vec![
            (AttrChange::Set, Some(Priority::High)),
            (AttrChange::Changed, Some(Priority::Medium)),
            (AttrChange::Cleared, None),
        ],
        "a no-op set and a no-op clear owe nothing"
    );
    for owed in &sink {
        assert_eq!(owed.intents.len(), 1, "one intent, its own chain root");
    }
}

/// `Ordinary`: a priority on a plain node is an attribute like any other, and
/// nothing is owed — priority never creates a task (SPEC §1).
#[test]
fn a_priority_on_a_plain_node_is_ordinary_and_owes_nothing() {
    let (mut ws, row) = a_row();
    let mut sink = ActivitySink::new();
    set_attribute(
        &mut ws,
        &[row],
        KEY_PRIORITY,
        AttrValue::Text("high".into()),
        &mut sink,
    )
    .unwrap();
    assert_eq!(priority_of(&ws, row), Some(Priority::High));
    assert!(!is_task_node(&ws, row).unwrap());
    assert!(sink.is_empty());
}

/// **The sink is bounded whichever arm fills it.** The priority arm used to
/// push directly, walking past the limit; it goes through `remember` now.
#[test]
fn the_priority_arm_keeps_the_sink_bounded() {
    let (mut ws, row) = a_row();
    ensure_task(&mut ws, row, TaskReason::Explicit).unwrap();
    let mut sink = ActivitySink::new();
    let tokens = ["high", "low"];
    for i in 0..(SINK_LIMIT + 7) {
        set_attribute(
            &mut ws,
            &[row],
            KEY_PRIORITY,
            AttrValue::Text(tokens[i % 2].into()),
            &mut sink,
        )
        .unwrap();
    }
    assert_eq!(sink.len(), SINK_LIMIT);
    assert_eq!(
        priority_changes(&sink).last().map(|(_, p)| *p),
        Some(Some(Priority::High)),
        "the newest entry survived and the oldest went"
    );
}
