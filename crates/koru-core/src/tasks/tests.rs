// SPDX-License-Identifier: MPL-2.0

//! The domain's own suite: what a command does, what it refuses, and what it
//! leaves alone when it refuses.
//!
//! Convergence and copy/paste live in `tests/tasks.rs`, because both are
//! properties of two documents rather than of one function.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone};

use super::*;

/// The day a completion is recorded on in this suite. **Fixed, not the
/// clock**: `complete_task` takes the day precisely so a test can name it,
/// and a suite that read `Local::now` would assert on a different value
/// every midnight.
fn a_day() -> crate::time::DateOnly {
    crate::time::DateOnly::parse("2026-09-06").expect("a canonical day")
}
use crate::attributes::{COMMAND_ONLY_KEYS, KEY_COLOR, KEY_PRIORITY, ValueType};
use crate::ids::{DocumentId, WorkspaceId};
use crate::node::{BlobHash, Content, NodeMeta};
use crate::time::{FloatingDateTime, TimeZoneName, ZonedDateTime};

// ------------------------------------------------------------------ fixtures

fn doc() -> WorkspaceDoc {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "tasks").unwrap();
    ws.enable_undo();
    ws
}

fn row(ws: &mut WorkspaceDoc, parent: Parent, text: &str) -> NodeId {
    ws.insert(parent, Node::paragraph(text).unwrap()).unwrap()
}

/// A checkbox-styled list block — what makes its items *checkable* without
/// making any of them a task.
fn checklist(ws: &mut WorkspaceDoc, parent: Parent) -> NodeId {
    let block = ws.insert(parent, Node::list_block().unwrap()).unwrap();
    ws.set_list_style(block, Some(ListStyle::Checkbox)).unwrap();
    block
}

fn item(ws: &mut WorkspaceDoc, block: NodeId, text: &str, done: bool) -> NodeId {
    let id = ws
        .insert(Parent::Node(block), Node::paragraph(text).unwrap())
        .unwrap();
    if done {
        ws.set_done(id, done).unwrap();
    }
    id
}

fn an_image(ws: &mut WorkspaceDoc, parent: Parent) -> NodeId {
    ws.insert(
        parent,
        Node::image(BlobHash::new("fnv1a", "abc"), "image/png", Some(4), Some(4)).unwrap(),
    )
    .unwrap()
}

fn a_file(ws: &mut WorkspaceDoc, parent: Parent) -> NodeId {
    ws.insert(
        parent,
        Node::file(
            "notes.pdf",
            BlobHash::new("fnv1a", "def"),
            "application/pdf",
        )
        .unwrap(),
    )
    .unwrap()
}

fn day(s: &str) -> DateOnly {
    DateOnly::parse(s).unwrap()
}

fn on(s: &str) -> Temporal {
    Temporal::Date(day(s))
}

fn fields(ws: &WorkspaceDoc, node: NodeId) -> TaskFields {
    TaskFields::of(&ws.read(node).unwrap())
}

fn lifecycle(ws: &WorkspaceDoc, node: NodeId) -> Lifecycle {
    fields(ws, node).lifecycle
}

fn done(ws: &WorkspaceDoc, node: NodeId) -> bool {
    ws.read(node).unwrap().meta.done
}

/// Everything a command could have written, as one string. Compared before and
/// after a refusal to prove the document is exactly where it was.
fn fingerprint(ws: &WorkspaceDoc) -> String {
    let mut out = String::new();
    for (id, depth) in ws.walk_with_trash().unwrap() {
        let node = ws.read(id).unwrap();
        let mut attrs: Vec<String> = node
            .content
            .attrs()
            .iter()
            .map(|a| format!("{}={:?}", a.role, a.value))
            .collect();
        attrs.sort();
        out.push_str(&format!(
            "{depth} {:?} {} done={} [{}]\n",
            node.kind,
            node.content.as_text().unwrap_or_default(),
            node.meta.done,
            attrs.join(","),
        ));
    }
    out
}

/// A fixed-offset stand-in for a real zone — the precedent is `time/tests.rs`,
/// which needs one for the same reason: this crate carries no timezone
/// database, and a bucket has to be asked in *some* zone.
struct FixedZone {
    name: TimeZoneName,
    offset_seconds: i32,
}

impl FixedZone {
    fn new(name: &str, offset_seconds: i32) -> Self {
        Self {
            name: TimeZoneName::parse(name).expect("a test zone name is valid"),
            offset_seconds,
        }
    }
}

impl ZoneRules for FixedZone {
    fn zone(&self) -> &TimeZoneName {
        &self.name
    }

    fn instant_of(&self, wall: NaiveDateTime) -> i64 {
        chrono::FixedOffset::east_opt(self.offset_seconds)
            .unwrap()
            .from_local_datetime(&wall)
            .single()
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_default()
    }

    fn wall_of(&self, instant_ms: i64) -> NaiveDateTime {
        let offset = chrono::FixedOffset::east_opt(self.offset_seconds).unwrap();
        chrono::DateTime::from_timestamp_millis(instant_ms)
            .map(|utc| utc.with_timezone(&offset).naive_local())
            .unwrap_or_default()
    }
}

// ------------------------------------------------------------ the derivation

/// Marking twice is one task and one undo step's worth of work: the second call
/// is a guarded no-op, not a second write and not an error.
#[test]
fn marking_a_task_is_idempotent() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Ship it");

    let first = ensure_task(&mut ws, node, TaskReason::Explicit).unwrap();
    assert_eq!(first.effect, TaskEffect::Created);
    assert_eq!(first.created_with_reason, Some(TaskReason::Explicit));

    let again = ensure_task(&mut ws, node, TaskReason::TasksView).unwrap();
    assert_eq!(again.effect, TaskEffect::Unchanged);
    assert!(again.activity.is_empty(), "a no-op says nothing happened");
    assert_eq!(
        fields(&ws, node).marker,
        Some(TaskReason::Explicit),
        "the second call does not rewrite how the task came to exist"
    );
    assert_eq!(
        ws.read(node).unwrap().attrs_with_role(KEY_TASK).count(),
        1,
        "one marker record, not two"
    );
}

/// SPEC §1, and the thing the whole feature is organised against: **setting
/// priority never creates a task**.
#[test]
fn priority_never_marks_a_task() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Reply to Dana");

    ws.set_attribute(node, KEY_PRIORITY, AttrValue::Text("urgent".into()))
        .unwrap();
    ws.set_attribute(node, KEY_COLOR, AttrValue::Text("red".into()))
        .unwrap();

    assert!(!is_task_node(&ws, node).unwrap());
    assert_eq!(
        route_attribute(&ws, node, KEY_PRIORITY).unwrap(),
        AttributeRoute::Ordinary,
        "priority on a non-task is the ordinary door and creates nothing"
    );
    ensure_task(&mut ws, node, TaskReason::Explicit).unwrap();
    assert_eq!(
        route_attribute(&ws, node, KEY_PRIORITY).unwrap(),
        AttributeRoute::PriorityOnTask,
        "on a task it is still the ordinary door, plus an activity intent"
    );
}

/// A due with no marker **is** a task — the legacy bare role included, which is
/// what makes an old due-dated row a task with no migration.
#[test]
fn a_due_alone_makes_a_task() {
    let mut ws = doc();
    let registered = row(&mut ws, Parent::Root, "Renew the domain");
    ws.set_attribute(registered, KEY_DUE, AttrValue::Date(day("2026-09-10")))
        .unwrap();
    assert!(is_task_node(&ws, registered).unwrap());
    assert_eq!(
        fields(&ws, registered).marker,
        None,
        "no marker was written"
    );

    let legacy = row(&mut ws, Parent::Root, "Written by an older build");
    ws.set_attr(
        legacy,
        &Attr::new(LEGACY_DUE_ROLE, AttrValue::Date(day("2026-09-11"))),
    )
    .unwrap();
    assert!(
        is_task_node(&ws, legacy).unwrap(),
        "the pre-registry bare role counts"
    );
}

/// Clearing the deadline never removes the task: `clear_due` marks first, in
/// the same group, and the node is still a task afterwards (SPEC §3).
#[test]
fn clearing_the_due_keeps_the_task() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Renew the domain");
    ws.set_attribute(node, KEY_DUE, AttrValue::Date(day("2026-09-10")))
        .unwrap();

    let outcome = clear_due(&mut ws, node).unwrap();
    assert_eq!(outcome.effect, TaskEffect::DueCleared);
    assert_eq!(fields(&ws, node).due, None);
    assert_eq!(
        fields(&ws, node).marker,
        Some(TaskReason::Due),
        "the marker records the deadline as the reason, because that is where it came from"
    );
    assert!(is_task_node(&ws, node).unwrap());

    assert_eq!(
        clear_due(&mut ws, node).unwrap().effect,
        TaskEffect::Unchanged,
        "and again is a guarded no-op"
    );
}

/// `set_due` on a plain row creates and dates it in one command, and the
/// outcome says both — a host shows one success.
#[test]
fn setting_a_due_creates_the_task_once() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Renew the domain");

    let first = set_due(&mut ws, node, on("2026-09-10")).unwrap();
    assert_eq!(first.effect, TaskEffect::DueChanged);
    assert_eq!(first.created_with_reason, Some(TaskReason::Due));
    assert_eq!(first.activity.len(), 2);
    assert_eq!(first.activity[0].event(), "task.created");
    assert_eq!(first.activity[1].event(), "task.due.changed");

    let moved = set_due(&mut ws, node, on("2026-09-12")).unwrap();
    assert_eq!(moved.created_with_reason, None, "it existed already");
    assert_eq!(moved.activity.len(), 1);
    assert!(matches!(
        moved.activity[0],
        ActivityIntent::DueChanged {
            change: AttrChange::Changed,
            temporal_kind: Some(TemporalKind::Date),
            ..
        }
    ));
    assert_eq!(
        set_due(&mut ws, node, on("2026-09-12")).unwrap().effect,
        TaskEffect::Unchanged
    );
}

/// A shared deadline carries the zone it was meant in, so a floating wall clock
/// is refused — by the registry, through the command.
#[test]
fn a_floating_due_is_refused() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Take the pills");
    let floating = Temporal::Floating(FloatingDateTime::parse("2026-09-10T08:00:00").unwrap());

    assert!(matches!(
        set_due(&mut ws, node, floating),
        Err(StoreError::AttributeRejected { .. })
    ));
    assert!(!is_task_node(&ws, node).unwrap(), "and nothing was created");
}

// ---------------------------------------------------------- classification

/// SPEC §5's cases, as a table. The two transitions it also lists are not
/// classes and are proved by the reopen tests below.
#[test]
fn the_classification_table() {
    let mut ws = doc();
    let block = checklist(&mut ws, Parent::Root);
    let unchecked = item(&mut ws, block, "Draft the email", false);
    let checked = item(&mut ws, block, "Book the room", true);

    let open_task = row(&mut ws, Parent::Root, "Nested task");
    ensure_task(&mut ws, open_task, TaskReason::Explicit).unwrap();
    let done_task = row(&mut ws, Parent::Root, "Finished task");
    ensure_task(&mut ws, done_task, TaskReason::Explicit).unwrap();
    complete_task(
        &mut ws,
        done_task,
        LifecycleReason::ExplicitComplete,
        a_day(),
    )
    .unwrap();

    let note = row(&mut ws, Parent::Root, "Just a note");
    let picture = an_image(&mut ws, Parent::Root);
    let file = a_file(&mut ws, Parent::Root);
    let picture_task = an_image(&mut ws, Parent::Root);
    ensure_task(&mut ws, picture_task, TaskReason::Explicit).unwrap();
    let legacy_row = ws
        .insert(Parent::Root, Node::task("An old outline row").unwrap())
        .unwrap();

    for (node, expected, why) in [
        (
            unchecked,
            ChildClass::UnfinishedStructural,
            "a new unchecked checklist item",
        ),
        (
            checked,
            ChildClass::FinishedStructural,
            "an already-checked item",
        ),
        (
            open_task,
            ChildClass::UnfinishedStructural,
            "an open nested task",
        ),
        (
            done_task,
            ChildClass::FinishedStructural,
            "a completed nested task",
        ),
        (
            picture_task,
            ChildClass::UnfinishedStructural,
            "an image that is itself an open task",
        ),
        (note, ChildClass::Passive, "a plain note"),
        (picture, ChildClass::Passive, "a plain image"),
        (file, ChildClass::Passive, "a plain file"),
        (
            legacy_row,
            ChildClass::Passive,
            "a bare Kind::Task row is not checkable by kind",
        ),
    ] {
        assert_eq!(classify_child(&ws, node).unwrap(), expected, "{why}");
    }
}

/// A childless task's box and its lifecycle are one synced state; a task with
/// structural children keeps a box that is only a box.
#[test]
fn only_a_childless_task_is_completed_by_its_box() {
    let mut ws = doc();
    let leaf = row(&mut ws, Parent::Root, "Buy milk");
    ensure_task(&mut ws, leaf, TaskReason::Explicit).unwrap();
    assert!(checkbox_completes(&ws, leaf).unwrap());

    let parent = row(&mut ws, Parent::Root, "Launch");
    ensure_task(&mut ws, parent, TaskReason::Explicit).unwrap();
    let block = checklist(&mut ws, Parent::Node(parent));
    item(&mut ws, block, "Write the copy", false);
    assert!(
        !checkbox_completes(&ws, parent).unwrap(),
        "a task with structural children is completed by the explicit action"
    );

    let plain = row(&mut ws, Parent::Root, "Not a task");
    assert!(
        !checkbox_completes(&ws, plain).unwrap(),
        "a plain checklist item keeps its ordinary check path"
    );

    // Passive content does not make a leaf into a group.
    let with_a_note = row(&mut ws, Parent::Root, "Call the vet");
    ensure_task(&mut ws, with_a_note, TaskReason::Explicit).unwrap();
    row(&mut ws, Parent::Node(with_a_note), "the number is …");
    assert!(checkbox_completes(&ws, with_a_note).unwrap());
}

/// The ordinary checkbox door moves the box and nothing else: no task, no
/// lifecycle, a guarded no-op when the value is unchanged, and one undo step
/// per click.
#[test]
fn the_checkbox_door_moves_only_the_box() {
    let mut ws = doc();
    let block = checklist(&mut ws, Parent::Root);
    let node = item(&mut ws, block, "Book the room", false);
    ws.checkpoint();

    assert!(ws.set_checkbox(node, true).unwrap());
    assert!(done(&ws, node));
    assert!(!is_task_node(&ws, node).unwrap(), "it creates no task");
    assert_eq!(lifecycle(&ws, node), Lifecycle::Open);
    assert!(
        !ws.set_checkbox(node, true).unwrap(),
        "setting it where it is leaves no phantom Ctrl+Z"
    );

    assert!(ws.undo().unwrap());
    assert!(!done(&ws, node), "one click, one undo step");
}

/// Promoting a node whose box is already ticked starts the task completed —
/// the childless task and its box are one synced state from the first moment
/// (owner decision, 2026-08-31).
#[test]
fn ensure_task_adopts_a_checked_box() {
    let mut ws = doc();
    let block = checklist(&mut ws, Parent::Root);
    let ticked = item(&mut ws, block, "Book the room", true);

    let outcome = ensure_task(&mut ws, ticked, TaskReason::Explicit).unwrap();
    assert_eq!(outcome.effect, TaskEffect::Created);
    assert_eq!(outcome.reason, Some(LifecycleReason::CheckboxCompleted));
    assert_eq!(lifecycle(&ws, ticked), Lifecycle::Completed);
    assert!(done(&ws, ticked), "and the box is left where it was");

    // With structural children below it, the box is only the box.
    let group = row(&mut ws, Parent::Root, "Launch");
    ws.set_done(group, true).unwrap();
    let inner = checklist(&mut ws, Parent::Node(group));
    item(&mut ws, inner, "Write the copy", false);
    ensure_task(&mut ws, group, TaskReason::Explicit).unwrap();
    assert_eq!(
        lifecycle(&ws, group),
        Lifecycle::Open,
        "a rich task's ticked box is not a claim about its lifecycle"
    );
}

// --------------------------------------------------------- cascade & reopen

/// Completing cascades down; reopening does not. The asymmetry is SPEC §5's,
/// and it is the whole reason the two are separate commands.
#[test]
fn completion_cascades_and_reopen_does_not() {
    let mut ws = doc();
    let parent = row(&mut ws, Parent::Root, "Launch");
    ensure_task(&mut ws, parent, TaskReason::Explicit).unwrap();
    let block = checklist(&mut ws, Parent::Node(parent));
    let one = item(&mut ws, block, "Write the copy", false);
    let two = item(&mut ws, block, "Book the room", false);
    let nested = row(&mut ws, Parent::Node(parent), "Sub-task");
    ensure_task(&mut ws, nested, TaskReason::Explicit).unwrap();
    let note = row(&mut ws, Parent::Node(parent), "a passive note");

    let outcome =
        complete_task(&mut ws, parent, LifecycleReason::ExplicitComplete, a_day()).unwrap();
    assert_eq!(outcome.effect, TaskEffect::Completed);
    assert_eq!(outcome.cascaded.len(), 1, "one nested task");
    assert_eq!(outcome.cascaded[0].node_id(), nested);
    assert_eq!(lifecycle(&ws, parent), Lifecycle::Completed);
    assert_eq!(lifecycle(&ws, nested), Lifecycle::Completed);
    assert!(done(&ws, parent) && done(&ws, one) && done(&ws, two) && done(&ws, nested));
    assert!(!done(&ws, note), "passive content is not a checkable");
    assert!(
        !done(&ws, block),
        "the list container itself is not a checkable either"
    );

    // Every event of the cascade shares one causation id.
    assert_eq!(
        outcome
            .activity
            .iter()
            .filter(|i| i.event() == "task.completed")
            .count(),
        2
    );

    let reopened = reopen_task(&mut ws, parent, LifecycleReason::ExplicitReopen).unwrap();
    assert_eq!(reopened.effect, TaskEffect::Reopened);
    assert_eq!(lifecycle(&ws, parent), Lifecycle::Open);
    assert!(!done(&ws, parent), "the box follows its own task");
    assert_eq!(
        lifecycle(&ws, nested),
        Lifecycle::Completed,
        "reopen never cascades downward"
    );
    assert!(done(&ws, one), "and it un-ticks nothing below it");
}

/// SPEC §5's auto-reopen: unfinished work put under a completed task reopens
/// it, and the chain above it, with reason `UNFINISHED_CHILD_ADDED`. Passive
/// content and already-finished work do not.
#[test]
fn unfinished_work_reopens_every_completed_task_above_it() {
    let mut ws = doc();
    let top = row(&mut ws, Parent::Root, "Quarter");
    ensure_task(&mut ws, top, TaskReason::Explicit).unwrap();
    let middle = row(&mut ws, Parent::Node(top), "Launch");
    ensure_task(&mut ws, middle, TaskReason::Explicit).unwrap();
    complete_task(&mut ws, top, LifecycleReason::ExplicitComplete, a_day()).unwrap();
    assert_eq!(lifecycle(&ws, middle), Lifecycle::Completed);

    let note = row(&mut ws, Parent::Node(middle), "a thought");
    assert!(
        reclassify(&mut ws, note).unwrap().is_none(),
        "passive content never reopens anything"
    );
    let block = checklist(&mut ws, Parent::Node(middle));
    let finished = item(&mut ws, block, "Already done", true);
    assert!(reclassify(&mut ws, finished).unwrap().is_none());
    assert_eq!(lifecycle(&ws, top), Lifecycle::Completed);

    let fresh = item(&mut ws, block, "One more thing", false);
    let outcome = reclassify(&mut ws, fresh).unwrap().expect("it reopened");
    assert_eq!(outcome.reason, Some(LifecycleReason::UnfinishedChildAdded));
    assert_eq!(outcome.task.node_id(), middle, "nearest first");
    assert_eq!(outcome.reopened_ancestors.len(), 1);
    assert_eq!(outcome.reopened_ancestors[0].node_id(), top);
    assert_eq!(lifecycle(&ws, middle), Lifecycle::Open);
    assert_eq!(lifecycle(&ws, top), Lifecycle::Open);
    assert!(!done(&ws, top) && !done(&ws, middle));
    assert_eq!(outcome.activity.last().unwrap().event(), "task.child.added");

    // The same classification, told as a move. Completing the chain again
    // ticks `fresh` on the way down, so it has to be unfinished again before
    // it can reopen anything.
    complete_task(&mut ws, top, LifecycleReason::ExplicitComplete, a_day()).unwrap();
    assert!(done(&ws, fresh), "the cascade reached it");
    ws.set_checkbox(fresh, false).unwrap();
    let moved = reclassify_moved(&mut ws, fresh).unwrap();
    assert!(matches!(
        moved.unwrap().activity.last(),
        Some(ActivityIntent::ChildMoved { .. })
    ));
}

/// **The creating commands own the chain above them too** (D-2c, SPEC §5(d)'s
/// second case). Marking a plain row under a completed task makes an open task
/// there — unfinished structural work that did not exist a moment ago — so the
/// chain reopens inside `ensure_task`, in its own atomic group and under its own
/// causation id, exactly as `reopen_task`'s does.
///
/// **In the domain rather than at `/task`'s surfaces.** The editor's slash
/// command, MCP's route-through, the image and file context rows and D-3's
/// native task all reach this one function; a hook at each of them would be four
/// call sites that have to agree, and a fifth surface that forgets.
#[test]
fn marking_a_descendant_of_a_completed_task_reopens_it() {
    let mut ws = doc();
    let top = row(&mut ws, Parent::Root, "Quarter");
    ensure_task(&mut ws, top, TaskReason::Explicit).unwrap();
    let middle = row(&mut ws, Parent::Node(top), "Launch");
    ensure_task(&mut ws, middle, TaskReason::Explicit).unwrap();
    complete_task(&mut ws, top, LifecycleReason::ExplicitComplete, a_day()).unwrap();
    let note = row(&mut ws, Parent::Node(middle), "chase the printer");
    assert_eq!(lifecycle(&ws, top), Lifecycle::Completed);

    let outcome = ensure_task(&mut ws, note, TaskReason::Explicit).unwrap();
    assert_eq!(outcome.effect, TaskEffect::Created);
    assert_eq!(lifecycle(&ws, middle), Lifecycle::Open, "nearest first");
    assert_eq!(lifecycle(&ws, top), Lifecycle::Open, "and the whole chain");
    assert!(!done(&ws, top) && !done(&ws, middle));
    assert_eq!(outcome.reopened_ancestors.len(), 2);
    assert_eq!(outcome.reopened_ancestors[0].node_id(), middle);
    assert_eq!(outcome.activity[0].event(), "task.created");
    assert_eq!(
        outcome
            .activity
            .iter()
            .filter(|i| i.event() == "task.reopened")
            .count(),
        2
    );
    assert_eq!(outcome.activity.last().unwrap().event(), "task.child.added");
}

/// **A deadline alone makes a task (SPEC §1), so it owes the same chain.** The
/// two creating commands cannot answer this differently, or the reopen would
/// depend on which door the person used — `/task` then a date, or a date.
#[test]
fn a_due_on_a_descendant_of_a_completed_task_reopens_it() {
    let mut ws = doc();
    let parent = row(&mut ws, Parent::Root, "Launch");
    ensure_task(&mut ws, parent, TaskReason::Explicit).unwrap();
    complete_task(&mut ws, parent, LifecycleReason::ExplicitComplete, a_day()).unwrap();
    let note = row(&mut ws, Parent::Node(parent), "book the room");

    let outcome = set_due(&mut ws, note, on("2026-09-10")).unwrap();
    assert_eq!(outcome.created_with_reason, Some(TaskReason::Due));
    assert_eq!(lifecycle(&ws, parent), Lifecycle::Open);
    assert_eq!(outcome.reopened_ancestors.len(), 1);
    assert_eq!(outcome.activity.last().unwrap().event(), "task.child.added");

    // And a *second* date on a row that is already a task changes nothing above
    // it: the classification did not move, so there is nothing to tell.
    complete_task(&mut ws, parent, LifecycleReason::ExplicitComplete, a_day()).unwrap();
    let again = set_due(&mut ws, note, on("2026-09-20")).unwrap();
    assert!(again.reopened_ancestors.is_empty());
    assert_eq!(
        lifecycle(&ws, parent),
        Lifecycle::Completed,
        "an ordinary date edit owes the chain nothing"
    );
}

/// **Neither creating command rewrites concurrent residue.** A row that was
/// already open work under a completed task is SPEC §5's residue — a remote
/// peer's edit, which this device deliberately does not reconcile — and
/// normalizing its marker is not a new arrival. `ensure_task` on a due-derived
/// task, and a promotion that adopts a checked box, both leave the chain alone.
#[test]
fn normalizing_or_adopting_reopens_nothing() {
    let mut ws = doc();
    let parent = row(&mut ws, Parent::Root, "Launch");
    ensure_task(&mut ws, parent, TaskReason::Explicit).unwrap();
    let block = checklist(&mut ws, Parent::Node(parent));
    let dated = item(&mut ws, block, "residue", false);
    // A genuinely *due-derived* task — a deadline and no marker — which is what
    // `ensure_task` normalizes. `set_due` would write the marker itself and make
    // this the idempotent path instead.
    faithful_import::restore_task(
        &mut ws,
        dated,
        &TaskFields {
            marker: None,
            due: Some(on("2026-09-10")),
            lifecycle: Lifecycle::Open,
            done: false,
            pinned_today: false,
            completed_at: None,
        },
    )
    .unwrap();
    let checked = item(&mut ws, block, "already finished", true);
    // Stood up rather than completed through the cascade, which would tick the
    // residue row this case is about.
    faithful_import::restore_task(
        &mut ws,
        parent,
        &TaskFields {
            marker: Some(TaskReason::Explicit),
            due: None,
            lifecycle: Lifecycle::Completed,
            done: false,
            pinned_today: false,
            completed_at: None,
        },
    )
    .unwrap();
    assert_eq!(lifecycle(&ws, parent), Lifecycle::Completed);

    let normalized = ensure_task(&mut ws, dated, TaskReason::Explicit).unwrap();
    assert_eq!(normalized.created_with_reason, Some(TaskReason::Due));
    assert!(normalized.reopened_ancestors.is_empty());
    assert_eq!(
        lifecycle(&ws, parent),
        Lifecycle::Completed,
        "the marker moved, the classification did not"
    );

    let adopted = ensure_task(&mut ws, checked, TaskReason::Explicit).unwrap();
    assert!(adopted.reopened_ancestors.is_empty());
    assert_eq!(lifecycle(&ws, checked), Lifecycle::Completed);
    assert_eq!(
        lifecycle(&ws, parent),
        Lifecycle::Completed,
        "a promotion that starts finished is not unfinished work"
    );
}

/// A descendant *becoming* an open task is one of SPEC's two transitions:
/// reopening it walks up on its own, inside the command.
#[test]
fn reopening_a_child_task_reopens_its_completed_parent() {
    let mut ws = doc();
    let parent = row(&mut ws, Parent::Root, "Launch");
    ensure_task(&mut ws, parent, TaskReason::Explicit).unwrap();
    let child = row(&mut ws, Parent::Node(parent), "Write the copy");
    ensure_task(&mut ws, child, TaskReason::Explicit).unwrap();
    complete_task(&mut ws, parent, LifecycleReason::ExplicitComplete, a_day()).unwrap();

    let outcome = reopen_task(&mut ws, child, LifecycleReason::CheckboxReopened).unwrap();
    assert_eq!(lifecycle(&ws, child), Lifecycle::Open);
    assert_eq!(
        lifecycle(&ws, parent),
        Lifecycle::Open,
        "the parent hears about it"
    );
    assert_eq!(outcome.reopened_ancestors.len(), 1);
    assert!(
        outcome
            .activity
            .iter()
            .any(|i| i.event() == "task.child.reopened")
    );
}

// -------------------------------------------------------------- removal

/// Remove clears the task-owned fields and **nothing else**: priority is a
/// general content attribute and the checkbox is content that predates
/// task-ness (SPEC §6, owner decision 2026-08-31).
#[test]
fn removing_from_tasks_keeps_the_box_and_the_priority() {
    let mut ws = doc();
    let block = checklist(&mut ws, Parent::Root);
    let node = item(&mut ws, block, "Book the room", false);
    ensure_task(&mut ws, node, TaskReason::Explicit).unwrap();
    set_due(&mut ws, node, on("2026-09-10")).unwrap();
    ws.set_attribute(node, KEY_PRIORITY, AttrValue::Text("high".into()))
        .unwrap();
    complete_task(&mut ws, node, LifecycleReason::ExplicitComplete, a_day()).unwrap();
    assert!(done(&ws, node));

    let outcome = remove_from_tasks(&mut ws, node).unwrap();
    assert_eq!(outcome.effect, TaskEffect::Removed);
    assert_eq!(outcome.activity.len(), 1);
    assert_eq!(outcome.activity[0].event(), "task.removed");

    let after = ws.read(node).unwrap();
    assert!(!is_task(&TaskFields::of(&after)));
    assert_eq!(
        after.due(),
        None,
        "it never leaves a due-derived task behind"
    );
    assert_eq!(TaskFields::of(&after).lifecycle, Lifecycle::Open);
    assert_eq!(
        after.priority(),
        Some(Priority::High),
        "priority survives — it is not a task field"
    );
    assert!(
        after.meta.done,
        "and stopping tracking never unticks a checked item"
    );

    assert_eq!(
        remove_from_tasks(&mut ws, node).unwrap().effect,
        TaskEffect::Unchanged
    );
}

/// A legacy bare-role due goes with the rest — otherwise Remove would leave a
/// due-derived task standing.
#[test]
fn removing_takes_the_legacy_due_too() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Written by an older build");
    ws.set_attr(
        node,
        &Attr::new(LEGACY_DUE_ROLE, AttrValue::Date(day("2026-09-11"))),
    )
    .unwrap();

    remove_from_tasks(&mut ws, node).unwrap();
    assert!(!is_task_node(&ws, node).unwrap());
    assert_eq!(ws.read(node).unwrap().due(), None);
}

// ------------------------------------------------------------- preflight

/// **Every expected failure happens before the first write.** One injection per
/// fallible preflight step of Remove and of the completion cascade, each
/// asserting the document is byte-for-byte where it was and that no undo step
/// was opened.
#[test]
fn a_refused_command_leaves_the_document_unchanged() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Launch");
    ensure_task(&mut ws, node, TaskReason::Explicit).unwrap();
    let quote = ws
        .insert(Parent::Root, Node::quote("not task-capable").unwrap())
        .unwrap();
    let plain = row(&mut ws, Parent::Root, "not a task");
    ws.checkpoint();
    let before = fingerprint(&ws);
    let undo_before = ws.can_undo();

    // Step 2 of both tables: the target read, on an id that resolves to nothing.
    let missing = NodeId::new();
    assert!(matches!(
        remove_from_tasks(&mut ws, missing),
        Err(StoreError::NoSuchNode(_))
    ));
    assert!(matches!(
        complete_task(&mut ws, missing, LifecycleReason::ExplicitComplete, a_day()),
        Err(StoreError::NoSuchNode(_))
    ));

    // Step 3 of the cascade: kind validation.
    assert!(matches!(
        complete_task(&mut ws, quote, LifecycleReason::ExplicitComplete, a_day()),
        Err(StoreError::TaskRejected {
            why: TaskRefusal::KindNotCapable,
            ..
        })
    ));
    assert!(matches!(
        reopen_task(&mut ws, quote, LifecycleReason::ExplicitReopen),
        Err(StoreError::TaskRejected {
            why: TaskRefusal::KindNotCapable,
            ..
        })
    ));
    assert!(matches!(
        ensure_task(&mut ws, quote, TaskReason::Explicit),
        Err(StoreError::TaskRejected {
            why: TaskRefusal::KindNotCapable,
            ..
        })
    ));
    assert!(matches!(
        set_due(&mut ws, quote, on("2026-09-10")),
        Err(StoreError::TaskRejected {
            why: TaskRefusal::KindNotCapable,
            ..
        })
    ));

    // Step 4 of the cascade: state validation.
    assert!(matches!(
        complete_task(&mut ws, plain, LifecycleReason::ExplicitComplete, a_day()),
        Err(StoreError::TaskRejected {
            why: TaskRefusal::NotATask,
            ..
        })
    ));
    assert!(matches!(
        reopen_task(&mut ws, plain, LifecycleReason::ExplicitReopen),
        Err(StoreError::TaskRejected {
            why: TaskRefusal::NotATask,
            ..
        })
    ));

    // Step 5 of Remove: not a task at all is a guarded no-op, not an error.
    assert_eq!(
        remove_from_tasks(&mut ws, plain).unwrap().effect,
        TaskEffect::Unchanged
    );

    assert_eq!(fingerprint(&ws), before, "nothing was written");
    assert_eq!(ws.can_undo(), undo_before, "and no undo step was opened");
}

/// Step 0 of both tables: a document with no workspace identity cannot name the
/// task the outcome is about, so the command refuses before it reads the node.
#[test]
fn a_document_with_no_workspace_identity_refuses_first() {
    let mut ws = WorkspaceDoc::adopt(loro::LoroDoc::new()).unwrap();
    let node = ws
        .insert(Parent::Root, Node::paragraph("Launch").unwrap())
        .unwrap();
    let before = fingerprint(&ws);

    for result in [
        ensure_task(&mut ws, node, TaskReason::Explicit),
        set_due(&mut ws, node, on("2026-09-10")),
        clear_due(&mut ws, node),
        complete_task(&mut ws, node, LifecycleReason::ExplicitComplete, a_day()),
        reopen_task(&mut ws, node, LifecycleReason::ExplicitReopen),
        remove_from_tasks(&mut ws, node),
    ] {
        assert!(matches!(result, Err(StoreError::Malformed(_))));
    }
    assert!(matches!(
        reclassify(&mut ws, node),
        Err(StoreError::Malformed(_))
    ));
    assert_eq!(fingerprint(&ws), before);
}

/// **A completion records the day it happened on, and losing the completion
/// loses the day with it.**
///
/// The day is what the Completed queue's window is asked against, so the two
/// have to move together: a task that is open again was finished on no day,
/// and one taken off Tasks altogether carries no completion at all. One day
/// for a whole cascade, because a cascade is one gesture at one moment.
#[test]
fn a_completion_records_its_day_and_reopening_takes_it_back() {
    let mut ws = doc();
    let parent = row(&mut ws, Parent::Root, "Ship the release");
    let child = row(&mut ws, Parent::Node(parent), "Write the notes");
    ensure_task(&mut ws, parent, TaskReason::Explicit).unwrap();
    ensure_task(&mut ws, child, TaskReason::Explicit).unwrap();
    assert_eq!(fields(&ws, parent).completed_at, None, "open, so no day");

    let day = crate::time::DateOnly::parse("2026-09-06").unwrap();
    complete_task(&mut ws, parent, LifecycleReason::ExplicitComplete, day).unwrap();
    assert_eq!(fields(&ws, parent).completed_at, Some(day));
    assert_eq!(
        fields(&ws, child).completed_at,
        Some(day),
        "the cascade is one gesture at one moment"
    );

    // Completing again on another day is a no-op: it was already finished.
    let later = crate::time::DateOnly::parse("2026-09-20").unwrap();
    complete_task(&mut ws, parent, LifecycleReason::ExplicitComplete, later).unwrap();
    assert_eq!(
        fields(&ws, parent).completed_at,
        Some(day),
        "the day it was finished on, not the day it was asked about again"
    );

    // Reopening takes the day with the lifecycle, on the task and its chain.
    reopen_task(&mut ws, child, LifecycleReason::ExplicitReopen).unwrap();
    assert_eq!(fields(&ws, child).completed_at, None);
    assert_eq!(
        fields(&ws, parent).completed_at,
        None,
        "and off the ancestor the reopen carried with it"
    );

    // ...and removing it from Tasks carries the whole record away.
    complete_task(&mut ws, child, LifecycleReason::ExplicitComplete, day).unwrap();
    assert_eq!(fields(&ws, child).completed_at, Some(day));
    remove_from_tasks(&mut ws, child).unwrap();
    assert_eq!(fields(&ws, child).completed_at, None);
    assert!(!is_task(&fields(&ws, child)));
}

// ------------------------------------------------------------- the doors

/// The command-only keys are registered — so the read path can ask about them
/// — and refused at every generic door, so nothing but a command writes one.
///
/// **Filterable is the shared truth; the other two are per key.** A marker and
/// a lifecycle are closed vocabularies with no order, so they group and do not
/// sort. A completion *day* is the other way round: *finished since Tuesday*
/// is a window and *most recently finished* is an order, while a group per
/// calendar day would be a heading over one row each.
#[test]
fn the_command_only_keys_are_registered_and_refused() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Launch");

    for key in COMMAND_ONLY_KEYS {
        let def = crate::attributes::system_definition(key).expect("registered");
        assert!(def.filterable, "{key} is what the read path asks about");
        let (sortable, groupable) = if *key == crate::attributes::KEY_COMPLETED_AT {
            (true, false)
        } else {
            (false, true)
        };
        assert_eq!(def.sortable, sortable, "{key}");
        assert_eq!(def.groupable, groupable, "{key}");
        assert!(
            matches!(
                ws.set_attribute(node, key, AttrValue::Text("explicit".into())),
                Err(StoreError::AttributeRejected { .. })
            ),
            "{key} is not writable through the generic door"
        );
        assert!(matches!(
            ws.set_attribute_on_all(&[node], key, AttrValue::Text("open".into())),
            Err(StoreError::AttributeRejected { .. })
        ));
        assert!(matches!(
            ws.clear_attribute(node, key),
            Err(StoreError::AttributeRejected { .. })
        ));
        assert!(matches!(
            ws.clear_attribute_on_all(&[node], key),
            Err(StoreError::AttributeRejected { .. })
        ));
        assert_eq!(
            route_attribute(&ws, node, key).unwrap(),
            AttributeRoute::Refused
        );
    }
    assert!(!is_task_node(&ws, node).unwrap(), "and none of it landed");
}

/// SPEC §4's exclusion is a **refusal**, not a hidden control: a kind that
/// cannot carry task state cannot be given a due through any door.
#[test]
fn an_excluded_kind_cannot_be_given_a_due() {
    let mut ws = doc();
    let quote = ws
        .insert(Parent::Root, Node::quote("a quotation").unwrap())
        .unwrap();

    assert!(matches!(
        ws.set_attribute(quote, KEY_DUE, AttrValue::Date(day("2026-09-10"))),
        Err(StoreError::TaskRejected {
            why: TaskRefusal::KindNotCapable,
            ..
        })
    ));
    assert!(matches!(
        ws.set_attribute_on_all(&[quote], KEY_DUE, AttrValue::Date(day("2026-09-10"))),
        Err(StoreError::TaskRejected { .. })
    ));
    assert!(matches!(
        ws.clear_attribute(quote, KEY_DUE),
        Err(StoreError::TaskRejected { .. })
    ));
    assert!(matches!(
        route_attribute(&ws, quote, KEY_DUE),
        Err(StoreError::TaskRejected { .. })
    ));
    // The ordinary attributes are untouched by any of this.
    ws.set_attribute(quote, KEY_COLOR, AttrValue::Text("teal".into()))
        .unwrap();
}

/// The multi-select apply for a custom definition validates instead of writing
/// raw — `set_attribute_on_all`'s missing half.
#[test]
fn a_custom_definition_applies_to_many_nodes_at_once() {
    let mut ws = doc();
    let a = row(&mut ws, Parent::Root, "One");
    let b = row(&mut ws, Parent::Root, "Two");
    let def =
        crate::attributes::AttributeDefinition::custom("user.effort", "Effort", ValueType::Number);

    assert!(
        ws.set_attribute_defined_on_all(&[a, b], &def, AttrValue::Number(3.0))
            .unwrap()
    );
    assert!(
        !ws.set_attribute_defined_on_all(&[a, b], &def, AttrValue::Number(3.0))
            .unwrap(),
        "and again is a guarded no-op"
    );
    assert!(matches!(
        ws.set_attribute_defined_on_all(&[a, b], &def, AttrValue::Text("three".into())),
        Err(StoreError::AttributeRejected { .. })
    ));
}

// ------------------------------------------------------------ the kind matrix

/// The complete per-kind domain matrix over SPEC §4's closed set — and the
/// exclusion list, which is a refusal rather than a hidden control.
#[test]
fn the_per_kind_matrix() {
    for kind in TASK_CAPABLE_KINDS {
        let mut ws = doc();
        let node = match kind {
            Kind::Paragraph => row(&mut ws, Parent::Root, "A row"),
            Kind::Task => ws
                .insert(Parent::Root, Node::task("An outline row").unwrap())
                .unwrap(),
            Kind::Document => ws
                .insert(Parent::Root, Node::document("A page").unwrap())
                .unwrap(),
            Kind::Image => an_image(&mut ws, Parent::Root),
            Kind::File => a_file(&mut ws, Parent::Root),
            other => unreachable!("{other:?} is not in the capable set"),
        };
        assert!(task_capable(kind));

        // A due creates the task, whatever the kind.
        assert_eq!(
            set_due(&mut ws, node, on("2026-09-10"))
                .unwrap()
                .created_with_reason,
            Some(TaskReason::Due),
            "{kind:?}: a due creates a task"
        );
        // The explicit action completes it, whatever the kind — an image and a
        // file have no owned content and no box to tick, so this is the only
        // way they are ever completed.
        let outcome =
            complete_task(&mut ws, node, LifecycleReason::ExplicitComplete, a_day()).unwrap();
        assert_eq!(outcome.effect, TaskEffect::Completed, "{kind:?}");
        assert_eq!(lifecycle(&ws, node), Lifecycle::Completed, "{kind:?}");
        assert!(outcome.cascaded.is_empty(), "{kind:?}: nothing below it");
        if matches!(kind, Kind::Image | Kind::File) {
            assert!(
                !kind.accepts_children(),
                "an image or file task owns no content"
            );
        }
        assert_eq!(
            remove_from_tasks(&mut ws, node).unwrap().effect,
            TaskEffect::Removed,
            "{kind:?}"
        );
    }

    // Everything else is refused, at the command and at the door.
    let mut ws = doc();
    for node in [
        ws.insert(Parent::Root, Node::quote("q").unwrap()).unwrap(),
        ws.insert(Parent::Root, Node::code("fn main() {}").unwrap())
            .unwrap(),
        ws.insert(Parent::Root, Node::table("| a |").unwrap())
            .unwrap(),
        ws.insert(Parent::Root, Node::divider().unwrap()).unwrap(),
        ws.insert(Parent::Root, Node::list_block().unwrap())
            .unwrap(),
        ws.insert(
            Parent::Root,
            Node::new(Kind::Contact, Content::text("Dana")).unwrap(),
        )
        .unwrap(),
        ws.insert(
            Parent::Root,
            Node::new(
                Kind::Canvas,
                Content::blob(BlobHash::new("fnv1a", "canvas"), "image/png", None, None),
            )
            .unwrap(),
        )
        .unwrap(),
    ] {
        let kind = ws.read(node).unwrap().kind;
        assert!(!task_capable(kind), "{kind:?}");
        assert!(matches!(
            ensure_task(&mut ws, node, TaskReason::Explicit),
            Err(StoreError::TaskRejected { .. })
        ));
        assert!(matches!(
            set_due(&mut ws, node, on("2026-09-10")),
            Err(StoreError::TaskRejected { .. })
        ));
    }
    assert!(
        !task_capable(Kind::Trash),
        "the hidden root is not nameable"
    );
}

// ---------------------------------------------------------------- buckets

/// SPEC §7's rules, including the one that decides a *zoned* deadline's day:
/// the same instant is two different days in two zones, and the bucket follows
/// the reader.
#[test]
fn the_bucket_table_including_a_zoned_midnight() {
    let tokyo = FixedZone::new("Asia/Tokyo", 9 * 3600);
    let new_york = FixedZone::new("America/New_York", -5 * 3600);
    let today = day("2026-08-31");
    let high = Some(Priority::High);
    let low = Some(Priority::Low);

    // Rule 1: completed wins over every due.
    assert_eq!(
        bucket(
            Some(&on("2026-01-01")),
            Lifecycle::Completed,
            high,
            today,
            &tokyo
        ),
        Bucket::Completed
    );
    // Rules 3, 4, 6, 7 over plain dates.
    for (due, expected) in [
        ("2026-08-01", Bucket::Today),
        ("2026-08-31", Bucket::Today),
        ("2026-09-01", Bucket::Next),
        ("2026-09-07", Bucket::Next),
        ("2026-09-08", Bucket::Upcoming),
        ("2026-09-30", Bucket::Upcoming),
        ("2026-10-01", Bucket::Later),
    ] {
        assert_eq!(
            bucket(Some(&on(due)), Lifecycle::Open, None, today, &tokyo),
            expected,
            "{due}"
        );
    }
    // Rule 5: no day, priority ≥ High.
    assert_eq!(
        bucket(None, Lifecycle::Open, high, today, &tokyo),
        Bucket::Next
    );
    assert_eq!(
        bucket(None, Lifecycle::Open, low, today, &tokyo),
        Bucket::Later
    );
    assert_eq!(
        bucket(None, Lifecycle::Open, None, today, &tokyo),
        Bucket::Later
    );
    // A floating value's day needs no zone.
    let floating = Temporal::Floating(FloatingDateTime::parse("2026-09-02T08:00:00").unwrap());
    assert_eq!(
        bucket(Some(&floating), Lifecycle::Open, None, today, &tokyo),
        Bucket::Next
    );

    // Rule 2, the zoned half: 00:30 on September 1 in Tokyo is 11:30 on
    // August 31 in New York — tomorrow to one reader, today to the other.
    let midnight = NaiveDate::from_ymd_opt(2026, 9, 1)
        .unwrap()
        .and_time(NaiveTime::from_hms_opt(0, 30, 0).unwrap());
    let zoned = Temporal::Zoned(ZonedDateTime::new(
        tokyo.instant_of(midnight),
        TimeZoneName::parse("Asia/Tokyo").unwrap(),
    ));
    assert_eq!(
        bucket(Some(&zoned), Lifecycle::Open, None, today, &tokyo),
        Bucket::Next
    );
    assert_eq!(
        bucket(Some(&zoned), Lifecycle::Open, None, today, &new_york),
        Bucket::Today,
        "the same instant, read where it is still August 31"
    );
}

// ------------------------------------------------------------- breadcrumb

/// One rule, so the desktop's breadcrumb and the projection's are the same
/// string without the two comparing notes.
#[test]
fn the_breadcrumb_rule() {
    assert_eq!(
        breadcrumb(&["Acme", "Plans", "Launch"]),
        "Acme / Plans / Launch"
    );
    assert_eq!(
        breadcrumb(&["Acme", "", "  ", "Launch"]),
        "Acme / Launch",
        "empty segments do not become empty steps"
    );
    assert_eq!(
        breadcrumb(&["  Acme  ", "A\ntwo-line\ttitle"]),
        "Acme / A two-line title",
        "one line, collapsed"
    );

    let long = "x".repeat(80);
    let out = breadcrumb(&[&long]);
    assert_eq!(out.chars().count(), 60, "truncated by characters");

    let many: Vec<String> = (1..=12).map(|i| format!("L{i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        breadcrumb(&refs),
        "L1 / L2 / L3 / L4 / … / L10 / L11 / L12",
        "eight items, with the middle elided"
    );

    // Characters, never bytes: eight Japanese characters are 24 bytes.
    let japanese = "会議の準備をする".repeat(10);
    assert_eq!(breadcrumb(&[&japanese]).chars().count(), 60);
}

// ------------------------------------------------------------- references

/// A `task:` reference resolves only while the target is still a task, and
/// answers **ordinary Not Found** otherwise — no removed state, no fallback to
/// the block underneath.
#[test]
fn a_task_reference_resolves_only_while_it_is_a_task() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Launch");
    let outcome = ensure_task(&mut ws, node, TaskReason::Explicit).unwrap();
    let reference = outcome.task;

    assert_eq!(
        refs::resolve_task_ref(&ws, &reference),
        refs::TaskRefResolution::Found(node)
    );
    assert_eq!(
        refs::resolve_task_ref(
            &ws,
            &EntityRef::block(reference.workspace_id(), reference.document_id(), node)
        ),
        refs::TaskRefResolution::NotFound,
        "a block reference is not a spelling of the task"
    );
    assert_eq!(
        refs::resolve_task_ref(
            &ws,
            &EntityRef::task(
                reference.workspace_id(),
                reference.document_id(),
                NodeId::new()
            )
        ),
        refs::TaskRefResolution::NotFound
    );
    assert_eq!(
        refs::resolve_task_ref(
            &ws,
            &EntityRef::task(WorkspaceId::new(), DocumentId::new(), node)
        ),
        refs::TaskRefResolution::NotFound,
        "another workspace is not this document's to answer for"
    );

    remove_from_tasks(&mut ws, node).unwrap();
    assert_eq!(
        refs::resolve_task_ref(&ws, &reference),
        refs::TaskRefResolution::NotFound,
        "a removed task is as absent as one that never existed"
    );
}

/// **Two tasks on one page carry the same document segment**, because the
/// segment names the document and not the row.
///
/// This is the property [TECHDEBT 72] recorded the absence of: the segment used
/// to be `WorkspaceDoc::document_id(node)`, which stores an id on a *document
/// node* and derives one from the node's own uuid for everything else — so every
/// row of a page carried a different middle segment and none of them was the
/// page. Anything that ever grouped or filtered by it would have answered per
/// node, silently.
///
/// Three shapes in one test, because the fix is a chain of fallbacks and each
/// link is load-bearing: the store's stamp when there is one, the page's root
/// node when there is not, and one shared answer either way.
///
/// [TECHDEBT 57]: ../../../../docs/contributing/techdebt/README.md
#[test]
fn two_tasks_on_one_page_carry_the_same_document_segment() {
    let mut ws = doc();
    let page = ws
        .insert(Parent::Root, Node::document("Plans").unwrap())
        .unwrap();
    let first = row(&mut ws, Parent::Node(page), "Launch referral");
    let second = row(&mut ws, Parent::Node(page), "Ship the copy");
    let deep = row(&mut ws, Parent::Node(second), "Draft it");
    let one = ensure_task(&mut ws, first, TaskReason::Explicit)
        .unwrap()
        .task;
    let two = ensure_task(&mut ws, second, TaskReason::Explicit)
        .unwrap()
        .task;
    let three = ensure_task(&mut ws, deep, TaskReason::Explicit)
        .unwrap()
        .task;

    assert_eq!(one.document_id(), two.document_id());
    assert_eq!(two.document_id(), three.document_id(), "at any depth");
    assert_eq!(
        one.document_id(),
        ws.document_id(page),
        "and it is the page's own id, not a row's"
    );
    assert_ne!(
        one.document_id(),
        ws.document_id(first),
        "which is exactly what a row derives for itself"
    );

    // The title row is a task-capable node like any other, and promoting *it*
    // does not give the page a second identity.
    let page_task = ensure_task(&mut ws, page, TaskReason::Explicit)
        .unwrap()
        .task;
    assert_eq!(page_task.document_id(), one.document_id());

    // **The store's answer wins over the page's.** A document the store made
    // arrives empty and is seeded here, so its root node's minted id is not the
    // id the manifest lists it under — the stamp is; and it is what every
    // reference minted afterwards carries.
    let manifest = DocumentId::new();
    assert!(ws.stamp_document_id(manifest).unwrap(), "it wrote");
    assert!(!ws.stamp_document_id(DocumentId::new()).unwrap(), "once");
    let later = row(&mut ws, Parent::Node(page), "Book it");
    let after = ensure_task(&mut ws, later, TaskReason::Explicit)
        .unwrap()
        .task;
    assert_eq!(after.document_id(), manifest);
    assert_eq!(ws.own_document_id(), Some(manifest));
}

/// A document nothing registered has no id to tell the truth with, and says so
/// rather than inventing one per row.
///
/// [`DocumentId::UNIDENTIFIED`] is a constant, so the property the segment
/// exists for still holds: every reference minted in one such document agrees
/// with every other. Only a test or a headless caller builds one — everything
/// the store hands out is stamped at the seam that opened it.
#[test]
fn a_document_with_no_id_of_its_own_says_so_once() {
    let mut ws = doc();
    let first = row(&mut ws, Parent::Root, "Launch referral");
    let second = row(&mut ws, Parent::Root, "Ship the copy");
    let one = ensure_task(&mut ws, first, TaskReason::Explicit)
        .unwrap()
        .task;
    let two = ensure_task(&mut ws, second, TaskReason::Explicit)
        .unwrap()
        .task;

    assert_eq!(ws.own_document_id(), None);
    assert_eq!(one.document_id(), DocumentId::UNIDENTIFIED);
    assert_eq!(one.document_id(), two.document_id());
    assert_eq!(
        DocumentId::UNIDENTIFIED.to_storage(),
        "00000000-0000-0000-0000-000000000000",
        "nil, so a reader who does look at one sees unknown"
    );
}

// ---------------------------------------------------------- faithful import

/// An import restores what the archive said and runs nothing: no command, no
/// classification, no reopen, no marker fabricated from a value.
#[test]
fn a_faithful_import_restores_without_classifying() {
    let mut ws = doc();
    let parent = row(&mut ws, Parent::Root, "Launch");
    let block = checklist(&mut ws, Parent::Node(parent));
    let child = item(&mut ws, block, "Write the copy", false);

    faithful_import::restore_task(
        &mut ws,
        parent,
        &TaskFields {
            marker: Some(TaskReason::Explicit),
            due: Some(on("2026-09-10")),
            lifecycle: Lifecycle::Completed,
            done: true,
            pinned_today: false,
            completed_at: None,
        },
    )
    .unwrap();
    faithful_import::restore_done(&mut ws, parent, true).unwrap();

    assert_eq!(
        lifecycle(&ws, parent),
        Lifecycle::Completed,
        "a completed task with an unchecked child stays completed"
    );
    assert_eq!(
        classify_child(&ws, child).unwrap(),
        ChildClass::UnfinishedStructural,
        "the child is what it is; nothing acted on it"
    );

    // A due with no marker restores a due-derived task, which is what it was.
    let derived = row(&mut ws, Parent::Root, "Renew the domain");
    faithful_import::restore_task(
        &mut ws,
        derived,
        &TaskFields {
            due: Some(on("2026-09-11")),
            ..TaskFields::default()
        },
    )
    .unwrap();
    let restored = ws.read(derived).unwrap();
    assert!(is_task(&TaskFields::of(&restored)));
    assert_eq!(TaskFields::of(&restored).marker, None, "no marker invented");
    assert_eq!(
        TaskFields::of(&restored).lifecycle,
        Lifecycle::Open,
        "absence is open, and an import writes no transition"
    );
}

// ----------------------------------------------------------------- outcomes

/// The debug line a host prints is tokens and counts — never a title, a date or
/// a line of text, for the same reason the intents carry none.
#[test]
fn an_outcome_describes_itself_without_content() {
    let mut ws = doc();
    let node = ws
        .insert(Parent::Root, Node::paragraph("Secret plan").unwrap())
        .unwrap();
    let outcome = set_due(&mut ws, node, on("2026-09-10")).unwrap();
    let line = outcome.describe();

    assert!(line.contains("effect=due_changed"));
    assert!(line.contains("created=due"));
    assert!(!line.contains("Secret plan"));
    assert!(!line.contains("2026-09-10"));
}

/// Every lifecycle reason is a completing one or a reopening one, and the two
/// vocabularies are derived from the enum rather than typed twice.
#[test]
fn the_lifecycle_reasons_round_trip() {
    for reason in LifecycleReason::ALL {
        assert_eq!(LifecycleReason::parse(reason.as_str()), Some(reason));
    }
    assert_eq!(
        LifecycleReason::ALL
            .iter()
            .filter(|r| r.completes())
            .count(),
        3
    );
    for reason in TaskReason::ALL {
        assert_eq!(TaskReason::parse(reason.as_str()), Some(reason));
    }
    for value in Lifecycle::ALL {
        assert_eq!(Lifecycle::parse(value.as_str()), Some(value));
    }
    assert_eq!(
        Lifecycle::parse("archived"),
        None,
        "a token from a newer build is not a lifecycle this build knows…"
    );
    assert_eq!(
        Lifecycle::default(),
        Lifecycle::Open,
        "…and what it reads as is open"
    );
    let _ = NodeMeta::default();
}

// -------------------------------------------- a document the store handed over

/// A document shaped the way the **store** hands one over: an empty `LoroDoc`
/// the app seeds nodes into, so `WorkspaceDoc::create` — the only writer of the
/// `C_META` id — never ran for it. Every document in the running app has this
/// shape; every other fixture in this file has the other one, which is why the
/// whole suite passed while `/due` refused on real content.
fn store_shaped_doc() -> WorkspaceDoc {
    let mut ws = WorkspaceDoc::adopt(loro::LoroDoc::new()).unwrap();
    ws.enable_undo();
    ws
}

/// **The bug, stated as the preflight sees it.** Step 0 of every command's
/// preflight is `ws.id()`, and a store-made document has none — so a `/due` on
/// a perfectly ordinary paragraph refused with `Malformed`, which the editor
/// then discarded.
#[test]
fn a_document_with_no_workspace_id_refuses_every_command() {
    let mut ws = store_shaped_doc();
    assert_eq!(ws.id(), None, "the store never wrote one");
    let node = row(&mut ws, Parent::Root, "plain paragraph");

    let refusal = set_due(&mut ws, node, on_a_date());
    assert!(
        matches!(&refusal, Err(StoreError::Malformed(why))
            if why.contains("no workspace id")),
        "step 0 of the preflight, not the node's own state: {refusal:?}"
    );
    assert!(
        ws.read(node).unwrap().due().is_none(),
        "and nothing was written on the way to refusing"
    );
}

/// [`WorkspaceDoc::stamp_workspace_id`] is what the open path calls, and it is
/// the whole fix: the same document, stamped the way a store join now stamps
/// it, takes a due.
#[test]
fn stamping_the_workspace_id_is_what_lets_a_command_run() {
    let mut ws = store_shaped_doc();
    let node = row(&mut ws, Parent::Root, "plain paragraph");
    let workspace = WorkspaceId::new();

    ws.stamp_workspace_id(workspace).unwrap();
    assert_eq!(ws.id(), Some(workspace));

    let out = set_due(&mut ws, node, on_a_date()).expect("a due on a plain row lands");
    assert_eq!(out.effect, TaskEffect::DueChanged);
    assert_eq!(
        out.created_with_reason,
        Some(TaskReason::Due),
        "and the row became a task, with the deadline as the reason"
    );
    assert_eq!(ws.read(node).unwrap().due(), Some(on_a_date()));
    assert_eq!(
        out.task.workspace_id(),
        workspace,
        "the task is filed under the workspace the store named"
    );
}

/// **A stamp never overwrites one.** A document that already carries an id
/// keeps it — including one it gained by being moved into another workspace,
/// which is the case that would otherwise be silently undone by whoever opened
/// it next.
#[test]
fn stamping_leaves_an_existing_workspace_id_alone() {
    let already = WorkspaceId::new();
    let mut ws = WorkspaceDoc::create(already, "made the old way").unwrap();
    ws.stamp_workspace_id(WorkspaceId::new()).unwrap();
    assert_eq!(ws.id(), Some(already));
}

/// **Two peers stamping the same document converge**, which is what makes
/// stamping at open safe at all. `C_META` is last-writer-wins, and both
/// replicas were handed the *same* id by the store, so whichever write wins the
/// two agree — and they agree with what the store said.
#[test]
fn two_peers_stamping_one_document_converge() {
    let mut a = store_shaped_doc();
    let node = row(&mut a, Parent::Root, "plain paragraph");
    let mut b = WorkspaceDoc::from_snapshot(&a.export_snapshot().unwrap()).unwrap();
    let workspace = WorkspaceId::new();

    a.stamp_workspace_id(workspace).unwrap();
    b.stamp_workspace_id(workspace).unwrap();

    let from_a = a.export_all_updates().unwrap();
    let from_b = b.export_all_updates().unwrap();
    a.import(&from_b).unwrap();
    b.import(&from_a).unwrap();

    assert_eq!(a.id(), Some(workspace));
    assert_eq!(b.id(), Some(workspace), "and they agree");
    assert!(set_due(&mut a, node, on_a_date()).is_ok());
    assert!(set_due(&mut b, node, on_a_date()).is_ok());
}

fn on_a_date() -> Temporal {
    Temporal::Date(crate::time::DateOnly::parse("2026-09-04").unwrap())
}

// ------------------------------------------------------- departing (T-0c)

/// A completed task holding a checklist, and an open task beside it.
/// `(under, item, elsewhere)`.
fn a_task_with_work(ws: &mut WorkspaceDoc) -> (NodeId, NodeId, NodeId) {
    let under = row(ws, Parent::Root, "the task");
    ensure_task(ws, under, TaskReason::Explicit).unwrap();
    let block = checklist(ws, Parent::Node(under));
    let one = item(ws, block, "a thing to do", false);
    let elsewhere = row(ws, Parent::Root, "somewhere else");
    (under, one, elsewhere)
}

#[test]
fn a_structural_child_names_the_task_it_would_leave() {
    let mut ws = doc();
    let (under, one, _) = a_task_with_work(&mut ws);

    let departing = departing(&ws, one).unwrap().expect("it is under a task");
    assert_eq!(departing.child(), one);
    assert_eq!(departing.task().node_id(), under);
    assert_eq!(
        departing.deleted(),
        ActivityIntent::ChildRemoved {
            task: departing.task(),
            child: one,
        },
        "and a delete spends it unconditionally"
    );
    assert_eq!(departing.deleted().event(), "task.child.removed");
}

/// **Structural only** — the reading this plan took from ADR-273's row. The
/// departure event is the other half of `task.child.added`, and that pair is
/// about work: passive content has no standing under a task on the way in, so
/// it gets none on the way out either.
#[test]
fn passive_content_leaving_a_task_owes_nothing() {
    let mut ws = doc();
    let under = row(&mut ws, Parent::Root, "the task");
    ensure_task(&mut ws, under, TaskReason::Explicit).unwrap();

    let note = row(&mut ws, Parent::Node(under), "a note about it");
    let picture = an_image(&mut ws, Parent::Node(under));
    let paper = a_file(&mut ws, Parent::Node(under));

    for passive in [note, picture, paper] {
        assert_eq!(classify_child(&ws, passive).unwrap(), ChildClass::Passive);
        assert_eq!(
            departing(&ws, passive).unwrap(),
            None,
            "passive content owes no departure"
        );
    }
}

/// A finished structural child still departs. The row carries no class, and
/// ADR-273 says why: what the child *was* is not what a removal is about.
#[test]
fn a_finished_structural_child_departs_too() {
    let mut ws = doc();
    let under = row(&mut ws, Parent::Root, "the task");
    ensure_task(&mut ws, under, TaskReason::Explicit).unwrap();
    let block = checklist(&mut ws, Parent::Node(under));
    let ticked = item(&mut ws, block, "already done", true);

    assert_eq!(
        classify_child(&ws, ticked).unwrap(),
        ChildClass::FinishedStructural
    );
    assert!(departing(&ws, ticked).unwrap().is_some());
}

#[test]
fn a_row_under_no_task_owes_nothing() {
    let mut ws = doc();
    let block = checklist(&mut ws, Parent::Root);
    let loose = item(&mut ws, block, "a plain checklist item", false);
    assert_eq!(
        classify_child(&ws, loose).unwrap(),
        ChildClass::UnfinishedStructural,
        "structural, but under nothing"
    );
    assert_eq!(departing(&ws, loose).unwrap(), None);
}

/// The nearest task, not the outermost: a departure is filed under the task
/// that actually lost the child.
#[test]
fn a_departure_names_the_nearest_task_ancestor() {
    let mut ws = doc();
    let outer = row(&mut ws, Parent::Root, "outer");
    ensure_task(&mut ws, outer, TaskReason::Explicit).unwrap();
    let inner = row(&mut ws, Parent::Node(outer), "inner");
    ensure_task(&mut ws, inner, TaskReason::Explicit).unwrap();
    let block = checklist(&mut ws, Parent::Node(inner));
    let one = item(&mut ws, block, "a thing", false);

    let departing = departing(&ws, one).unwrap().unwrap();
    assert_eq!(departing.task().node_id(), inner);
}

/// The move half: only a move that actually took the child out of the recorded
/// task is a departure.
#[test]
fn a_move_inside_one_task_is_not_a_departure() {
    let mut ws = doc();
    let (under, one, _) = a_task_with_work(&mut ws);
    let recorded = departing(&ws, one).unwrap().unwrap();

    // Out of the checklist, but still under the same task.
    ws.move_node(one, Parent::Node(under), 0).unwrap();
    assert_eq!(
        recorded.moved(&ws).unwrap(),
        None,
        "the task it was recorded under is still an ancestor"
    );
}

#[test]
fn a_move_out_of_a_task_is_a_departure() {
    let mut ws = doc();
    let (under, one, elsewhere) = a_task_with_work(&mut ws);
    let recorded = departing(&ws, one).unwrap().unwrap();

    ws.move_node(one, Parent::Node(elsewhere), 0).unwrap();
    let intent = recorded.moved(&ws).unwrap().expect("it left");
    assert_eq!(
        intent,
        ActivityIntent::ChildRemoved {
            task: recorded.task(),
            child: one,
        }
    );
    assert_eq!(intent.subject().node_id(), under);
}

/// Moving from one task to another owes **both** halves, and they are separate
/// questions asked of separate functions: this one, and `reclassify_moved`.
#[test]
fn a_move_between_two_tasks_owes_a_departure_and_an_arrival() {
    let mut ws = doc();
    let left = row(&mut ws, Parent::Root, "the first task");
    ensure_task(&mut ws, left, TaskReason::Explicit).unwrap();
    // A nested *task*, so it stays structural wherever it lands — a checklist
    // item is only checkable while its checkbox list is its parent.
    let one = row(&mut ws, Parent::Node(left), "a subtask");
    ensure_task(&mut ws, one, TaskReason::Explicit).unwrap();

    let right = row(&mut ws, Parent::Root, "the other task");
    ensure_task(&mut ws, right, TaskReason::Explicit).unwrap();
    complete_task(&mut ws, right, LifecycleReason::ExplicitComplete, a_day()).unwrap();

    let recorded = departing(&ws, one).unwrap().unwrap();
    ws.move_node(one, Parent::Node(right), 0).unwrap();

    let left_it = recorded
        .moved(&ws)
        .unwrap()
        .expect("it left the first task");
    assert_eq!(left_it.subject().node_id(), left);

    let arrived = reclassify_moved(&mut ws, one)
        .unwrap()
        .expect("and it reopened the second");
    assert!(
        arrived
            .activity
            .iter()
            .any(|i| matches!(i, ActivityIntent::ChildMoved { child, .. } if *child == one)),
        "the arrival is told as a move, under the task it joined"
    );
}

/// **The faithful door is not on this path.** `departing` reads and decides;
/// nothing writes, and no store primitive calls it — which is the whole of an
/// importer's exemption, and the reason it is a call rather than a side effect
/// of [`WorkspaceDoc::delete`].
#[test]
fn a_delete_that_never_asks_mints_nothing() {
    let mut ws = doc();
    let (_, one, _) = a_task_with_work(&mut ws);
    let before = fingerprint(&ws);
    let _ = departing(&ws, one).unwrap();
    assert_eq!(fingerprint(&ws), before, "asking writes nothing");

    // And the primitive an importer uses is inert on its own: there is no
    // intent anywhere to collect, because nothing produced one.
    ws.delete(one).unwrap();
    assert!(ws.read(one).is_err());
}

/// A document with no workspace identity records nothing — the same read every
/// command's preflight makes first.
#[test]
fn a_document_with_no_workspace_id_owes_no_departure() {
    let mut ws = store_shaped_doc();
    let under = row(&mut ws, Parent::Root, "the task");
    faithful_import::restore_task(
        &mut ws,
        under,
        &TaskFields {
            marker: Some(TaskReason::Explicit),
            due: None,
            lifecycle: Lifecycle::Open,
            done: false,
            pinned_today: false,
            completed_at: None,
        },
    )
    .unwrap();
    let block = checklist(&mut ws, Parent::Node(under));
    let one = item(&mut ws, block, "a thing", false);

    assert_eq!(ws.id(), None);
    assert_eq!(departing(&ws, one).unwrap(), None);
}

#[test]
fn departing_all_keeps_only_what_owes_something() {
    let mut ws = doc();
    let (_, one, elsewhere) = a_task_with_work(&mut ws);
    let got = departing_all(&ws, &[one, elsewhere]);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].child(), one);
}

/// **`first_day_in` is `bucket`'s inverse, and this is what holds it there.**
///
/// Not four asserted offsets — the date each answer produces is fed straight
/// back to `bucket` and has to come out as the queue it was asked for. A
/// boundary constant can move and this test still means what it says; a table
/// of offsets asserted by hand would quietly become the second answer the
/// function exists to prevent.
#[test]
fn the_first_day_in_a_queue_is_a_day_that_queue_claims() {
    let today = day("2026-09-06");
    let zone = FixedZone::new("UTC", 0);
    for bucket in [Bucket::Today, Bucket::Next, Bucket::Upcoming] {
        let day = first_day_in(bucket, today).expect("these three are datable");
        assert_eq!(
            bucket_of(Some(&Temporal::Date(day)), today, &zone),
            bucket,
            "{bucket:?}"
        );
    }
}

/// *Later* and *Completed* seed nothing, for two different reasons that both
/// end in `None`: a dateless task with no priority is already Later, and no
/// date at all reaches Completed — lifecycle does, and `bucket` answers it
/// before it reads the due.
#[test]
fn later_and_completed_are_not_places_a_date_can_put_you() {
    let today = day("2026-09-06");
    assert_eq!(first_day_in(Bucket::Later, today), None);
    assert_eq!(first_day_in(Bucket::Completed, today), None);
    // And the first half of that sentence, verified rather than asserted.
    assert_eq!(
        bucket_of(None, today, &FixedZone::new("UTC", 0)),
        Bucket::Later,
        "which is why Later needs no date"
    );
}

/// One open task with no priority, placed on `today`.
fn bucket_of(due: Option<&Temporal>, today: DateOnly, zone: &dyn ZoneRules) -> Bucket {
    bucket(due, Lifecycle::Open, None, today, zone)
}

// ------------------------------------------------------- pinning to Today

/// **A pin is a membership, not a move.** The task keeps the bucket its due
/// names; the Today *queue* is the natural Today plus the pinned, and that
/// union is the projection's business rather than `bucket`'s.
#[test]
fn pinning_adds_a_membership_and_leaves_the_natural_bucket_alone() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Ship it");
    // Due in four days: Next, on the domain function's own reckoning.
    let due =
        crate::time::Temporal::Date(crate::time::DateOnly::parse("2026-09-10").expect("a day"));
    set_due(&mut ws, node, due.clone()).expect("a due makes it a task");
    let today = crate::time::DateOnly::parse("2026-09-06").expect("a day");
    let zone = FixedZone::new("UTC", 0);
    let before = bucket_of(Some(&due), today, &zone);
    assert_eq!(before, Bucket::Next, "four days out is Next");

    let outcome = pin_to_today(&mut ws, node).expect("pinned");

    assert_eq!(outcome.effect, TaskEffect::PinnedToday);
    assert!(TaskFields::of(&ws.read(node).unwrap()).pinned_today);
    let fields = TaskFields::of(&ws.read(node).unwrap());
    assert_eq!(
        bucket(fields.due.as_ref(), fields.lifecycle, None, today, &zone),
        before,
        "the natural bucket is untouched -- a pin does not edit the deadline"
    );
}

/// Both directions are guarded no-ops when there is nothing to do, so a
/// repeated gesture leaves no phantom Ctrl+Z. `unchanged` is a real answer.
#[test]
fn pinning_twice_and_unpinning_twice_change_nothing_the_second_time() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Ship it");
    ensure_task(&mut ws, node, TaskReason::Explicit).expect("a task");

    assert_eq!(
        pin_to_today(&mut ws, node).unwrap().effect,
        TaskEffect::PinnedToday
    );
    assert_eq!(
        pin_to_today(&mut ws, node).unwrap().effect,
        TaskEffect::Unchanged,
        "already on Today"
    );
    assert_eq!(
        unpin_from_today(&mut ws, node).unwrap().effect,
        TaskEffect::UnpinnedToday
    );
    assert_eq!(
        unpin_from_today(&mut ws, node).unwrap().effect,
        TaskEffect::Unchanged,
        "not on Today to begin with"
    );
}

/// **Unpinned is an absence, never a stored `false`.** One representation, so a
/// query asks whether the record exists rather than knowing about two ways of
/// saying no.
#[test]
fn unpinning_removes_the_record_rather_than_storing_false() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "Ship it");
    ensure_task(&mut ws, node, TaskReason::Explicit).expect("a task");
    pin_to_today(&mut ws, node).expect("pinned");
    assert_eq!(
        ws.read(node)
            .unwrap()
            .content
            .attrs()
            .iter()
            .filter(|a| a.role == crate::attributes::KEY_TODAY)
            .count(),
        1
    );

    unpin_from_today(&mut ws, node).expect("unpinned");

    assert!(
        ws.read(node)
            .unwrap()
            .content
            .attrs()
            .iter()
            .all(|a| a.role != crate::attributes::KEY_TODAY),
        "the record is gone, not set to false"
    );
}

/// Pinning is about where a *task* shows. It refuses a row that is not one
/// rather than quietly creating it -- that gesture is `ensure_task`, and a
/// second door to it would be a second authority on what a task is.
#[test]
fn pinning_refuses_a_row_that_is_not_a_task() {
    let mut ws = doc();
    let node = row(&mut ws, Parent::Root, "just prose");

    let refused = pin_to_today(&mut ws, node);

    assert!(matches!(refused, Err(StoreError::TaskRejected { .. })));
    assert!(
        !is_task_node(&ws, node).unwrap(),
        "and it did not become one on the way"
    );
}

/// `koru.today` is a classification, so the generic doors refuse it like the
/// other command-only keys -- pinned via `COMMAND_ONLY_KEYS`, which the
/// registration test above walks.
#[test]
fn the_today_key_is_command_only() {
    assert!(COMMAND_ONLY_KEYS.contains(&crate::attributes::KEY_TODAY));
}

/// **Opening a document to read it adds nothing to its version vector.**
///
/// `LoroDoc::new()` mints a random writing session every time, so the fear was
/// that merely opening a document grew its history — which under the old
/// covering-revision shape would have been fatal, since the requirement
/// carried one entry per writer. It does not: a version vector gains an entry
/// only for a session that actually *writes*. Importing a snapshot is not a
/// write.
///
/// This is the property the receipt shape no longer depends on, and it is
/// pinned here anyway: it is what decides how fast the vector grows, and
/// therefore what a persistent-peer-id optimisation would be worth.
#[test]
fn repeated_read_only_opens_do_not_grow_the_version_vector() {
    let mut ws = doc();
    let _ = row(&mut ws, Parent::Root, "Ship it");
    let snapshot = ws.export_snapshot().unwrap();
    let baseline = ws.version().encode().len();

    for _ in 0..50 {
        let reopened = crate::schema::WorkspaceDoc::from_snapshot(&snapshot).unwrap();
        assert_eq!(
            reopened.version().encode().len(),
            baseline,
            "a read-only open contributes no entry"
        );
    }
}

/// ...and one that **writes** does add an entry, which is the other half of
/// the same fact and the reason the vector grows at all.
#[test]
fn a_session_that_writes_adds_one_entry_to_the_version_vector() {
    let mut ws = doc();
    let _ = row(&mut ws, Parent::Root, "Ship it");
    let snapshot = ws.export_snapshot().unwrap();
    let baseline = ws.version().encode().len();

    let mut writer = crate::schema::WorkspaceDoc::from_snapshot(&snapshot).unwrap();
    let _ = row(&mut writer, Parent::Root, "and another");

    assert!(
        writer.version().encode().len() > baseline,
        "a writing session is in the history from its first operation"
    );
}
