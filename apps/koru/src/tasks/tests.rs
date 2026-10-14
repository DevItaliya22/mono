// SPDX-License-Identifier: MPL-2.0

//! The desktop drain: what a dispatched event owes the burst boundary.
//!
//! The coalescing itself is `koru_workspace_host::tasks::ContentBursts` and is
//! pinned in that crate's own suite. What is left to prove here is the wiring —
//! that this window's one caret decides the boundary, that an edit the caret is
//! not on ends immediately, and that an event which edited no task does no work
//! at all.

use super::*;
use koru_core::ids::{DocumentId, NodeId, WorkspaceId};

/// A task reference nothing else in the process will mint. The burst map is a
/// process global — it has to be, since the drain and the maintenance tick live
/// in crates that cannot see each other's state — so every test keys on its own
/// ids rather than sharing a fixture.
fn a_task() -> EntityRef {
    EntityRef::task(WorkspaceId::new(), DocumentId::new(), NodeId::new())
}

/// **Typing keeps the burst open**, and the caret leaving ends it — once.
///
/// The two halves are one test because the interesting part is the sequence: a
/// run of events that each edit the same task must not end anything, and the
/// first event that finds the caret somewhere else must end it exactly once.
#[test]
fn the_burst_ends_when_the_caret_leaves_and_not_before() {
    let task = a_task();
    let mut caret = TaskCaret::default();

    for _ in 0..10 {
        assert!(
            caret.dispatched(vec![task], || Some(task)).is_empty(),
            "still typing in the row"
        );
    }
    assert_eq!(caret.open(), Some(task));

    assert_eq!(
        caret.dispatched(Vec::new(), || None),
        vec![task],
        "the caret left, so the burst ended"
    );
    assert!(caret.open().is_none());
    assert!(
        caret.dispatched(Vec::new(), || None).is_empty(),
        "a burst ends once; the tick and the tab close find nothing"
    );
}

/// **An edit the caret is not on ends at once.** A menu row or an explore-column
/// action can change a task in a tab nobody is looking at, and no caret will
/// ever leave that row — so waiting for one would strand the event until the
/// idle tick, or forever if the tab closed first.
#[test]
fn an_edit_away_from_the_caret_ends_immediately() {
    let elsewhere = a_task();
    let here = a_task();
    let mut caret = TaskCaret::default();

    assert_eq!(
        caret.dispatched(vec![elsewhere], || Some(here)),
        vec![elsewhere]
    );
    assert_eq!(
        caret.open(),
        Some(here),
        "the caret is still where it was, and its own burst is untouched"
    );
}

/// Moving from one task straight to another ends the first and opens nothing —
/// the second is not a burst until something is typed into it.
#[test]
fn moving_between_two_tasks_ends_only_the_first() {
    let first = a_task();
    let second = a_task();
    let mut caret = TaskCaret::default();

    caret.dispatched(vec![first], || Some(first));
    assert_eq!(caret.dispatched(Vec::new(), || Some(second)), vec![first]);
    assert_eq!(caret.open(), Some(second));
    assert!(
        caret.dispatched(Vec::new(), || None).is_empty(),
        "a task the caret merely passed over owes nothing"
    );
}

/// **A tab closed over the row.** There is no caret left to leave it and no
/// tick will ever see it again, so the burst ends now.
#[test]
fn a_closed_row_ends_the_burst() {
    let task = a_task();
    let mut caret = TaskCaret::default();
    caret.dispatched(vec![task], || Some(task));

    assert_eq!(caret.gone(), vec![task]);
    assert!(caret.open().is_none());
    assert!(caret.gone().is_empty(), "and there is nothing left to end");
}

/// **An event that edited no task does no work**, and the strongest form of
/// that is that it never asks where the caret is: `caret_task` is a document
/// read, and almost every event in a session is a pointer move, an arrow key or
/// a click in chrome.
#[test]
fn an_event_that_edited_nothing_never_asks_where_the_caret_is() {
    let mut caret = TaskCaret::default();
    let mut asked = 0;
    for _ in 0..100 {
        assert!(
            caret
                .dispatched(Vec::new(), || {
                    asked += 1;
                    None
                })
                .is_empty()
        );
    }
    assert_eq!(asked, 0, "no burst is open, so there is nothing to compare");
}

/// The emission door is a no-op on an empty flush: no host is built, no counter
/// moves, no line is printed — the same rule [`attempt`] follows, and the reason
/// the boundary may answer an empty `Vec` without every caller guarding.
#[test]
fn an_empty_flush_emits_nothing() {
    let before = koru_workspace_host::tasks::counters();
    content_changed(&[]);
    assert_eq!(koru_workspace_host::tasks::counters(), before);
}
