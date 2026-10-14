// SPDX-License-Identifier: MPL-2.0

//! The trash's two durable facts: what a person decided about a trashed
//! run's results, and what emptying the trash leaves behind in the ledger.
//!
//! Every ledger here is under a temporary root; nothing touches the shared
//! channel root and nothing here removes a directory.

use super::*;
use crate::fixtures::{Root, created_run, event, run_id, running_attempt, schedule};
use crate::{AttemptPhase, Projection};
use koru_delegate_core::PublicationKind;

/// **Keep is what a run with no answer gets.** Nothing infers *delete* —
/// not from silence, not from a build that predates the question, not from
/// a run this device never created.
#[test]
fn a_run_nobody_answered_for_keeps_what_it_produced() {
    let root = Root::new();
    let db = root.open();
    assert_eq!(db.deletion(run_id(1)).unwrap(), None);
    assert_eq!(ArtifactDisposition::default(), ArtifactDisposition::Keep);
}

/// The answer is recorded with the moment it was given, and reading it back
/// is the same answer.
#[test]
fn the_answer_is_recorded_and_read_back() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    root.tick(500);

    assert_eq!(
        db.record_deletion(run, ArtifactDisposition::Delete)
            .unwrap(),
        WriteOutcome::Applied
    );
    assert_eq!(
        db.deletion(run).unwrap(),
        Some(DeletionRecord {
            run,
            artifacts: ArtifactDisposition::Delete,
            decided_at_ms: root.now_ms(),
        })
    );
}

/// **The same answer twice is one answer.** A retried call after a lost
/// acknowledgement finds the fact it made rather than making a second.
#[test]
fn recording_the_same_answer_twice_is_idempotent() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    db.record_deletion(run, ArtifactDisposition::Keep).unwrap();
    let first = db.deletion(run).unwrap();
    root.tick(1_000);
    assert_eq!(
        db.record_deletion(run, ArtifactDisposition::Keep).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert_eq!(db.deletion(run).unwrap(), first, "and the moment stood");
}

/// **A person may change their mind while the run is still in the trash.**
/// Nothing has happened yet — that is the whole point of a trash — so the
/// second answer replaces the first rather than being refused.
#[test]
fn a_second_different_answer_replaces_the_first() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    db.record_deletion(run, ArtifactDisposition::Delete)
        .unwrap();
    root.tick(1_000);
    assert_eq!(
        db.record_deletion(run, ArtifactDisposition::Keep).unwrap(),
        WriteOutcome::Applied
    );
    let record = db.deletion(run).unwrap().unwrap();
    assert_eq!(record.artifacts, ArtifactDisposition::Keep);
    assert_eq!(record.decided_at_ms, root.now_ms());
}

/// Taking the run back out of the trash takes the decision with it, and
/// doing so twice is not an error.
#[test]
fn restoring_a_run_clears_what_was_decided_about_it() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    db.record_deletion(run, ArtifactDisposition::Delete)
        .unwrap();
    assert_eq!(db.clear_deletion(run).unwrap(), WriteOutcome::Applied);
    assert_eq!(db.deletion(run).unwrap(), None);
    assert_eq!(
        db.clear_deletion(run).unwrap(),
        WriteOutcome::AlreadyApplied
    );
}

/// **A run this device never created can still be answered for.** There is
/// no intent row behind it and nothing device-local to delete; recording
/// the decision is harmless, and refusing it would turn "there is nothing
/// to do" into an error somebody sees.
#[test]
fn a_run_with_no_creation_record_can_still_be_answered_for() {
    let root = Root::new();
    let mut db = root.open();
    let stranger = run_id(9);
    assert_eq!(
        db.record_deletion(stranger, ArtifactDisposition::Keep)
            .unwrap(),
        WriteOutcome::Applied
    );
    assert!(db.deletion(stranger).unwrap().is_some());
    assert!(db.intent(stranger).unwrap().is_none());
}

/// Decisions come back oldest first, whatever order they were made in.
#[test]
fn decisions_list_oldest_first() {
    let root = Root::new();
    let mut db = root.open();
    let second = created_run(&mut db, 2);
    root.tick(10);
    db.record_deletion(second, ArtifactDisposition::Keep)
        .unwrap();
    let first = created_run(&mut db, 1);
    root.tick(10);
    db.record_deletion(first, ArtifactDisposition::Delete)
        .unwrap();
    let order: Vec<RunId> = db.deletions().unwrap().into_iter().map(|d| d.run).collect();
    assert_eq!(order, [second, first]);
}

/// **Emptying the trash takes every row of the run**, across all six
/// tables, in one transaction — and leaves the run beside it untouched.
#[test]
fn forgetting_a_run_removes_every_row_it_has_and_nothing_else() {
    let root = Root::new();
    let mut db = root.open();
    let doomed = created_run(&mut db, 1);
    let neighbour = created_run(&mut db, 2);
    running_attempt(&mut db, doomed, 1);
    running_attempt(&mut db, neighbour, 2);
    db.enqueue(event(1), doomed, PublicationKind::Lifecycle, "{}")
        .unwrap();
    db.advance_cursor(doomed, Projection::Bus, 7).unwrap();
    db.record_deletion(doomed, ArtifactDisposition::Delete)
        .unwrap();

    assert_eq!(db.forget_run(doomed).unwrap(), WriteOutcome::Applied);

    assert!(db.intent(doomed).unwrap().is_none(), "the intent is gone");
    assert!(db.current_attempt(doomed).unwrap().is_none());
    assert!(db.deletion(doomed).unwrap().is_none());
    assert!(
        db.open_publications(10)
            .unwrap()
            .iter()
            .all(|open| open.run != doomed),
        "and its queued publication"
    );
    assert_eq!(counted(&db, "projection_cursors", doomed), 0);

    // The other run kept everything.
    assert!(db.intent(neighbour).unwrap().is_some());
    assert_eq!(
        db.current_attempt(neighbour).unwrap().map(|row| row.phase),
        Some(AttemptPhase::Running)
    );
}

/// **A slot the run held is released with it.** A schedule claim names the
/// run it minted; leaving the row behind would keep an occurrence claimed
/// by a run that no longer exists, and the slot could never fire again.
#[test]
fn forgetting_a_run_releases_the_schedule_slot_it_held() {
    let root = Root::new();
    let mut db = root.open();
    let claimed = db.claim_slot(schedule(1), 4_000, 1).unwrap();

    assert_eq!(db.forget_run(claimed.run).unwrap(), WriteOutcome::Applied);
    assert_eq!(counted(&db, "schedule_claims", claimed.run), 0);
}

/// **Deleting something twice is the same answer, not an error.** A run
/// that arrived by sync has nothing here to forget, and a retry after a
/// lost acknowledgement must not fail.
#[test]
fn forgetting_a_run_this_ledger_never_held_is_already_applied() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    assert_eq!(db.forget_run(run).unwrap(), WriteOutcome::Applied);
    assert_eq!(db.forget_run(run).unwrap(), WriteOutcome::AlreadyApplied);
    assert_eq!(
        db.forget_run(run_id(9)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
}

/// **A forgotten run stays forgotten across a restart**, which is what
/// makes the ledger's half of the delete a delete rather than a hiding.
#[test]
fn a_forgotten_run_is_still_gone_after_a_reopen() {
    let root = Root::new();
    let run = {
        let mut db = root.open();
        let run = created_run(&mut db, 1);
        running_attempt(&mut db, run, 1);
        db.forget_run(run).unwrap();
        run
    };
    let again = root.open();
    assert!(again.intent(run).unwrap().is_none());
    assert!(again.current_attempt(run).unwrap().is_none());
    assert!(again.intents().unwrap().is_empty());
}

fn counted(db: &RunsDb, table: &str, run: RunId) -> i64 {
    db.conn()
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE run_id = ?1"),
            params![run.to_storage()],
            |row| row.get(0),
        )
        .unwrap()
}
