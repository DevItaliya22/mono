// SPDX-License-Identifier: MPL-2.0

//! The editor's task seam: the golden table replayed through it, and what the
//! seam itself is allowed to do.

use super::*;
use koru_core::tasks::golden::{self, CASES};

/// **The golden cases, through the editor-visible path.**
///
/// `koru-core`'s own reference run executes this same table against the
/// commands directly, which is what makes a row an expectation rather than a
/// wish; this run puts [`run`] between the table and the commands. Everything
/// the seam adds — the sink, the causation id it keeps, the order it keeps them
/// in — is invisible to [`golden::verify`] **by design**, because none of it may
/// change a domain answer. A red case here means this seam drifted, never that
/// the row was fiction.
///
/// Every disagreement is reported rather than the first: a change to a command
/// usually moves several rows, and finding them one `cargo test` at a time is
/// the slow way to learn that.
#[test]
fn the_golden_cases_survive_the_editors_seam() {
    let mut bad: Vec<String> = Vec::new();
    for case in CASES {
        let mut fx = golden::Fixture::build(case);
        let target = fx.node(case.command.target());
        let mut sink = ActivitySink::new();
        let got = run(&mut sink, &mut fx.ws, |ws| {
            case.command.apply_to(ws, target)
        });
        if let Err(complaint) = golden::verify(case, &fx, &got) {
            bad.push(complaint);
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} golden cases disagree through the editor's seam:\n\n{}",
        bad.len(),
        CASES.len(),
        bad.join("\n\n")
    );
}

/// What the seam leaves behind is the *outcome's* activity, in the outcome's
/// order, under the causation id the **command** minted — never one the editor
/// made up, because that id is what makes desktop and MCP group a cascade
/// identically.
#[test]
fn the_sink_carries_the_commands_own_causation_and_order() {
    let case = golden::case("complete.cascades_over_checkables_and_nested_tasks")
        .expect("the cascade case is in the table");
    let mut fx = golden::Fixture::build(case);
    let target = fx.node(case.command.target());
    let mut sink = ActivitySink::new();
    let outcome = run(&mut sink, &mut fx.ws, |ws| {
        case.command.apply_to(ws, target)
    })
    .expect("the cascade applies")
    .expect("and answers an outcome");

    assert_eq!(sink.len(), 1, "one command, one thing owed");
    assert_eq!(sink[0].causation, outcome.causation);
    assert_eq!(sink[0].intents, outcome.activity);
    assert!(
        sink[0].intents.len() > 1,
        "a cascade owes more than one event"
    );
}

/// A guarded no-op owes nothing. A repeated gesture leaves no phantom Ctrl+Z
/// and no phantom event either.
#[test]
fn a_guarded_no_op_owes_no_activity() {
    let case = golden::case("ensure.explicit").expect("the first case is in the table");
    let mut fx = golden::Fixture::build(case);
    let target = fx.node(case.command.target());
    let mut sink = ActivitySink::new();

    run(&mut sink, &mut fx.ws, |ws| {
        case.command.apply_to(ws, target)
    })
    .unwrap();
    assert_eq!(sink.len(), 1);
    // Again: already marked, so `Unchanged`, so nothing owed.
    run(&mut sink, &mut fx.ws, |ws| {
        case.command.apply_to(ws, target)
    })
    .unwrap();
    assert_eq!(sink.len(), 1, "the second run owed nothing");
}

/// A refusal produces no outcome, so there is nothing to record: the seam never
/// invents an intent a command did not name.
#[test]
fn a_refusal_records_nothing() {
    let case = CASES
        .iter()
        .find(|c| matches!(c.expect, golden::Expect::Refused(_)))
        .expect("the table carries a refusal");
    let mut fx = golden::Fixture::build(case);
    let target = fx.node(case.command.target());
    let mut sink = ActivitySink::new();
    let got = run(&mut sink, &mut fx.ws, |ws| {
        case.command.apply_to(ws, target)
    });
    assert!(got.is_err());
    assert!(sink.is_empty());
}

/// **The seam performs no I/O, and it cannot**: it takes a `&mut WorkspaceDoc`
/// and a closure, and everything it does with the outcome is push it onto a
/// `Vec` the host drains later. `koru-editor` links no events crate, no store
/// client and no gateway — check `Cargo.toml` rather than believing this
/// sentence — so a task gesture on the event thread has nothing to block on.
/// The empirical half is every case above running with no host of any kind.
#[test]
fn the_seam_only_records_and_the_host_drains() {
    let case = golden::case("ensure.explicit").expect("the first case is in the table");
    let mut fx = golden::Fixture::build(case);
    let target = fx.node(case.command.target());
    let mut sink = ActivitySink::new();
    run(&mut sink, &mut fx.ws, |ws| {
        case.command.apply_to(ws, target)
    })
    .unwrap();

    let drained = std::mem::take(&mut sink);
    assert_eq!(drained.len(), 1);
    assert!(sink.is_empty(), "what a host takes, it takes whole");
}

// ------------------------------------------------------- the content-burst note
//
// All this crate owes `task.content.changed` is the note. The coalescing, the
// boundaries and the emission are the host wrapper's (ADR-273), and none of
// them is reachable from here.

/// A workspace with one task row and one plain row.
fn a_task_and_a_paragraph() -> (WorkspaceDoc, koru_core::NodeId, koru_core::NodeId) {
    use koru_core::ids::WorkspaceId;
    use koru_core::node::Node;
    use koru_core::schema::Parent;
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "Tasks").unwrap();
    let task = ws
        .insert(Parent::Root, Node::paragraph("post the form").unwrap())
        .unwrap();
    let plain = ws
        .insert(Parent::Root, Node::paragraph("a note to self").unwrap())
        .unwrap();
    koru_core::tasks::ensure_task(&mut ws, task, koru_core::attributes::TaskReason::Explicit)
        .unwrap();
    (ws, task, plain)
}

/// **Only a task's own text is noted.** The generic edit path calls this on
/// every row it touches, so the test that a plain paragraph records nothing is
/// the one keeping the burst set from becoming a log of the whole document.
#[test]
fn only_a_task_is_noted() {
    let (ws, task, plain) = a_task_and_a_paragraph();
    let mut edits = TaskTextEdits::default();
    edits.note(&ws, plain);
    assert!(edits.is_empty(), "a plain row is not a task");
    edits.note(&ws, task);
    assert_eq!(edits.len(), 1);
    assert_eq!(edits.take()[0].node_id(), task);
}

/// **Never per keystroke, one level down.** The note is a set of subjects: a
/// hundred edits on one task are one entry, and the host is handed one thing to
/// coalesce rather than a hundred.
#[test]
fn a_hundred_notes_on_one_task_are_one_entry() {
    let (ws, task, _plain) = a_task_and_a_paragraph();
    let mut edits = TaskTextEdits::default();
    for _ in 0..100 {
        edits.note(&ws, task);
    }
    assert_eq!(edits.len(), 1);
}

/// A node that has stopped resolving records nothing rather than a reference to
/// something nobody can open. The task test is a read, and a read that fails is
/// an answer: an edit path may hand this a row that a concurrent delete has
/// already taken away.
#[test]
fn a_node_that_no_longer_resolves_records_nothing() {
    let (mut ws, task, _plain) = a_task_and_a_paragraph();
    ws.delete(task).unwrap();
    let mut edits = TaskTextEdits::default();
    edits.note(&ws, task);
    assert!(edits.is_empty());
}

/// What the host takes, it takes whole — the same rule the activity sink keeps,
/// and for the same reason: a note left behind would be a second event for one
/// burst.
#[test]
fn the_host_drains_the_note_whole() {
    let (ws, task, _plain) = a_task_and_a_paragraph();
    let mut edits = TaskTextEdits::default();
    edits.note(&ws, task);
    assert_eq!(edits.take().len(), 1);
    assert!(edits.is_empty());
    assert!(edits.take().is_empty());
}
