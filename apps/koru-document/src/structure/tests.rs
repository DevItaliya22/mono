// SPDX-License-Identifier: MPL-2.0

//! What Tab and Shift+Tab owe the task model.
//!
//! D-2 wired the classification at the two *pointer* gestures — a drop and a
//! paste — and left the keyboard alone. These are the keyboard half: SPEC §5's
//! "moving an unfinished structural child" under a completed task, asked at
//! every structural move this module makes, through the same
//! `koru_editor::tasks::run` seam and with the same answer.

use super::*;
use koru_core::attributes::{Lifecycle, TaskReason};
use koru_core::node::Node;
use koru_core::tasks::{ActivityIntent, ChildClass, LifecycleReason};
use koru_core::{ListStyle, NodeId, WorkspaceId};

fn document() -> (WorkspaceDoc, BuiltInDocument) {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").unwrap();
    let doc = koru_workspace::document(&mut ws, "spec.md").expect("a document");
    let view = BuiltInDocument::open(&ws, doc);
    (ws, view)
}

/// A task row followed by a one-item list — the shape `indent_list_below_task`
/// exists for, and the only one in which a checklist item can *become* the
/// structural child of a task in a single Tab.
fn task_then_list(style: ListStyle) -> (WorkspaceDoc, BuiltInDocument, NodeId, NodeId) {
    let (mut ws, mut view) = document();
    let root = view.editor.body.root.expect("a document root");
    let task = ws
        .insert(Parent::Node(root), Node::task("ship it").unwrap())
        .unwrap();
    let item = ws
        .insert(Parent::Node(root), Node::paragraph("draft").unwrap())
        .unwrap();
    ws.wrap_in_list_block_many(&[item], style).unwrap();
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    (ws, view, task, item)
}

fn complete(ws: &mut WorkspaceDoc, id: NodeId) {
    koru_core::tasks::ensure_task(ws, id, TaskReason::Explicit).unwrap();
    koru_core::tasks::complete_task(
        ws,
        id,
        LifecycleReason::ExplicitComplete,
        koru_editor::tasks::today(),
    )
    .unwrap();
}

fn lifecycle(ws: &WorkspaceDoc, id: NodeId) -> Lifecycle {
    koru_core::tasks::TaskFields::of(&ws.read(id).unwrap()).lifecycle
}

/// The fragment move's landing container: a list item's ListBlock's owner.
fn owner_of(ws: &WorkspaceDoc, item: NodeId) -> NodeId {
    let Ok(Parent::Node(block)) = ws.parent(item) else {
        panic!("a list item belongs to a ListBlock")
    };
    let Ok(Parent::Node(owner)) = ws.parent(block) else {
        panic!("and that block belongs to something")
    };
    owner
}

/// **Tab is a structural move, so it classifies.** A checklist item nested under
/// a completed task is unfinished work landing under finished work, and SPEC §5
/// reopens the task for it — told as a *move*, because that is the gesture.
#[test]
fn tab_indenting_an_unchecked_item_under_a_completed_task_reopens_it() {
    let (mut ws, mut view, task, item) = task_then_list(ListStyle::Checkbox);
    complete(&mut ws, task);
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    view.editor.body.focus_on(item);

    view.indent_structure(&mut ws, 1);
    assert_eq!(owner_of(&ws, item), task, "the item is the task's now");
    assert_eq!(
        lifecycle(&ws, task),
        Lifecycle::Open,
        "so the task it landed in reopened"
    );

    let owed = view.editor.body.take_task_activity();
    assert_eq!(owed.len(), 1, "one classification, one thing owed");
    assert!(
        matches!(
            owed[0].intents.first(),
            Some(ActivityIntent::Reopened {
                reason: LifecycleReason::UnfinishedChildAdded,
                ..
            })
        ),
        "with SPEC §5's reason"
    );
    assert!(
        owed[0].intents.iter().any(|intent| matches!(
            intent,
            ActivityIntent::ChildMoved {
                class: ChildClass::UnfinishedStructural,
                reopened: true,
                ..
            }
        )),
        "and told as a move rather than an add"
    );
}

/// An **already-completed** child is not unfinished work.
#[test]
fn indenting_a_ticked_item_under_a_completed_task_reopens_nothing() {
    let (mut ws, mut view, task, item) = task_then_list(ListStyle::Checkbox);
    complete(&mut ws, task);
    ws.set_checkbox(item, true).unwrap();
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    view.editor.body.focus_on(item);

    view.indent_structure(&mut ws, 1);
    assert_eq!(owner_of(&ws, item), task, "it still moved");
    assert_eq!(
        lifecycle(&ws, task),
        Lifecycle::Completed,
        "finished work leaves it finished"
    );
    assert!(view.editor.body.take_task_activity().is_empty());
}

/// **Passive content reopens nothing.** A bullet is a note — no box, no
/// lifecycle — so filing one under a completed task is filing.
#[test]
fn indenting_passive_content_under_a_completed_task_reopens_nothing() {
    let (mut ws, mut view, task, item) = task_then_list(ListStyle::Bullet);
    complete(&mut ws, task);
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    view.editor.body.focus_on(item);

    view.indent_structure(&mut ws, 1);
    assert_eq!(owner_of(&ws, item), task);
    assert_eq!(lifecycle(&ws, task), Lifecycle::Completed);
    assert!(view.editor.body.take_task_activity().is_empty());
}

/// **Moving out is not adding under.** Shift+Tab lifts the list off the task,
/// so the classification finds no completed ancestor at all.
///
/// The starting state — an unchecked item inside a completed task — is SPEC
/// §5's concurrent residue, which is exactly the state that must not be
/// "corrected" by a gesture that takes work *away* from the task.
///
/// It is, though, the gesture that owes a **departure** (T-0c): the item left
/// the task, so `task.child.removed` is owed and nothing else is.
#[test]
fn shift_tab_out_of_a_completed_task_reopens_nothing() {
    let (mut ws, mut view, task, item) = task_then_list(ListStyle::Checkbox);
    let root = view.editor.body.root.expect("a document root");
    let Ok(Parent::Node(block)) = ws.parent(item) else {
        panic!("the item belongs to a ListBlock")
    };
    ws.move_node(block, Parent::Node(task), 0).unwrap();
    complete(&mut ws, task);
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    view.editor.body.focus_on(item);

    view.indent_structure(&mut ws, -1);
    assert_eq!(owner_of(&ws, item), root, "it left the task");
    assert_eq!(
        lifecycle(&ws, task),
        Lifecycle::Completed,
        "which is not a reason to reopen it"
    );

    let owed = view.editor.body.take_task_activity();
    assert_eq!(owed.len(), 1, "one gesture, one chain");
    let [ActivityIntent::ChildRemoved { task: left, child }] = owed[0].intents[..] else {
        panic!("the departure, and only the departure: {:?}", owed[0]);
    };
    assert_eq!(child, item);
    assert_eq!(left.node_id(), task, "filed under the task it left");
}

// -------------------------------------------------------- departures (T-0c)

/// **An indent that stays inside one task owes no departure.** The item is
/// still under the task it was recorded against, so nothing left it — which is
/// what makes the recording a *before/after* comparison rather than a claim
/// that every move is a removal.
#[test]
fn indenting_within_one_task_owes_no_departure() {
    let (mut ws, mut view, task, item) = task_then_list(ListStyle::Checkbox);
    // Put the list inside the task first, then Tab it one level deeper.
    let Ok(Parent::Node(block)) = ws.parent(item) else {
        panic!("the item belongs to a ListBlock")
    };
    let step = ws
        .insert(Parent::Node(task), Node::task("a step").unwrap())
        .unwrap();
    ws.move_node(block, Parent::Node(task), 1).unwrap();
    koru_core::tasks::ensure_task(&mut ws, task, TaskReason::Explicit).unwrap();
    koru_core::tasks::ensure_task(&mut ws, step, TaskReason::Explicit).unwrap();
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    view.editor.body.focus_on(item);

    view.indent_structure(&mut ws, 1);
    assert_eq!(owner_of(&ws, item), step, "it nested under the step");
    assert!(
        view.editor.body.take_task_activity().is_empty(),
        "still inside the same task, so nothing departed"
    );
}

/// **Passive content leaving a task owes nothing** — the mirror of
/// `indenting_passive_content_under_a_completed_task_reopens_nothing`, and the
/// reading T-0c took from ADR-273's row.
#[test]
fn passive_content_leaving_a_task_owes_no_departure() {
    let (mut ws, mut view, task, item) = task_then_list(ListStyle::Bullet);
    let root = view.editor.body.root.expect("a document root");
    let Ok(Parent::Node(block)) = ws.parent(item) else {
        panic!("the item belongs to a ListBlock")
    };
    ws.move_node(block, Parent::Node(task), 0).unwrap();
    koru_core::tasks::ensure_task(&mut ws, task, TaskReason::Explicit).unwrap();
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    view.editor.body.focus_on(item);

    view.indent_structure(&mut ws, -1);
    assert_eq!(owner_of(&ws, item), root, "the bullet left the task");
    assert!(
        view.editor.body.take_task_activity().is_empty(),
        "a bullet is filing, not work"
    );
}

/// **A bare task outdented out of another reports the departure**, through the
/// `reparent` fallback — the branch `indent_structure` reads its landings off
/// the tree for.
#[test]
fn shift_tab_of_a_bare_task_out_of_a_task_reports_the_departure() {
    let (mut ws, mut view) = document();
    let root = view.editor.body.root.expect("a document root");
    let outer = ws
        .insert(Parent::Node(root), Node::task("ship it").unwrap())
        .unwrap();
    let inner = ws
        .insert(Parent::Node(outer), Node::task("draft").unwrap())
        .unwrap();
    koru_core::tasks::ensure_task(&mut ws, outer, TaskReason::Explicit).unwrap();
    koru_core::tasks::ensure_task(&mut ws, inner, TaskReason::Explicit).unwrap();
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    view.editor.body.focus_on(inner);

    view.indent_structure(&mut ws, -1);
    assert_eq!(
        ws.parent(inner).unwrap(),
        Parent::Node(root),
        "it left the task"
    );

    let owed = view.editor.body.take_task_activity();
    assert_eq!(owed.len(), 1);
    let [ActivityIntent::ChildRemoved { task, child }] = owed[0].intents[..] else {
        panic!("the departure: {:?}", owed[0]);
    };
    assert_eq!(child, inner);
    assert_eq!(task.node_id(), outer);
}

/// **Tab on a bare task classifies too.** A task row carries no ListBlock, so
/// it takes `reparent` rather than either list path — the one branch of
/// `indent_structure` that rebuilds before the classification is asked, and
/// therefore the one that has to rebuild again.
#[test]
fn tab_indenting_a_bare_task_under_a_completed_one_reopens_it() {
    let (mut ws, mut view) = document();
    let root = view.editor.body.root.expect("a document root");
    let finished = ws
        .insert(Parent::Node(root), Node::task("ship it").unwrap())
        .unwrap();
    let fresh = ws
        .insert(Parent::Node(root), Node::task("draft").unwrap())
        .unwrap();
    complete(&mut ws, finished);
    koru_core::tasks::ensure_task(&mut ws, fresh, TaskReason::Explicit).unwrap();
    view.editor.body.rebuild(&ws);
    view.editor.body.task_activity.clear();
    view.editor.body.focus_on(fresh);

    view.indent_structure(&mut ws, 1);
    assert_eq!(
        ws.parent(fresh).unwrap(),
        Parent::Node(finished),
        "it moved"
    );
    assert_eq!(
        lifecycle(&ws, finished),
        Lifecycle::Open,
        "and the task it landed in reopened"
    );
    assert!(
        view.editor
            .body
            .row(finished)
            .is_some_and(|row| !row.done && row.task.as_ref().is_some_and(|t| !t.completed)),
        "the rows were rebuilt from the reopened state, not the completed one"
    );
    assert_eq!(view.editor.body.take_task_activity().len(), 1);
}

/// **An ordinary indent owes nothing and costs no undo step.** With no completed
/// task above it the classification answers `None` and opens no group, so one
/// Ctrl+Z is still the whole gesture.
#[test]
fn an_ordinary_indent_owes_nothing_and_adds_no_undo_step() {
    let (mut ws, mut view, task, item) = task_then_list(ListStyle::Checkbox);
    let root = view.editor.body.root.expect("a document root");
    ws.enable_undo();
    view.editor.body.focus_on(item);

    view.indent_structure(&mut ws, 1);
    assert_eq!(owner_of(&ws, item), task, "it moved");
    assert!(
        view.editor.body.take_task_activity().is_empty(),
        "no task above it, no activity"
    );

    assert!(ws.undo().unwrap());
    assert_eq!(
        owner_of(&ws, item),
        root,
        "one Ctrl+Z puts the fragment back"
    );
}
