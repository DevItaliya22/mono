// SPDX-License-Identifier: MPL-2.0

//! The durability ordering of §7.1, proved by crashing it.
//!
//! Each test arms one point, drives the ledger to it, lets it die, drops
//! the handle (the process is gone), reopens the file (the restart), and
//! asserts the state the roadmap promises: a pending intent completes
//! forward, a satisfied one is not redone, a claim either exists with its
//! `RunId` or does not exist at all, a queue entry either has its id or is
//! absent, and a retry of a request whose answer was lost finds the fact
//! rather than making it twice.

use koru_delegate_core::{AttemptGeneration, PublicationKind};

use super::*;
use crate::{
    AttemptPhase, IntentState, LookupAnswer, Projection, RestartVerdict, RunsDb, SlotOutcome,
    SlotState, WriteOutcome, classify_at_restart,
    fixtures::{
        BODY_SENTINEL, Root, created_run, document, event, host, request, reserved, run_id,
        running_attempt, schedule, workspace,
    },
};

const WS: u8 = 0xa2;

fn at(op: WriteOp, at: Moment) -> CrashPoint {
    CrashPoint { op, at }
}

fn body() -> String {
    format!("{{\"sentinel\":\"{BODY_SENTINEL}\"}}")
}

/// Drive `db` into `point` through `act`, assert it died there, and assert
/// it stays dead — a crashed process answers nothing more.
fn crash(
    db: &mut RunsDb,
    point: CrashPoint,
    act: impl FnOnce(&mut RunsDb) -> Result<(), LedgerError>,
) {
    db.arm_crash(point);
    assert_eq!(act(db), Err(LedgerError::Crashed(point)));
    assert!(
        db.pending_intents().is_ok(),
        "reads still answer; only writes are gone"
    );
    assert_eq!(
        db.advance_cursor(run_id(0xee), Projection::Bus, 0),
        Err(LedgerError::Crashed(point)),
        "dead from here on"
    );
}

// ------------------------------------------------------------ the intent

/// Crash before the intent write: nothing exists; the person's retry is a
/// first request.
#[test]
fn a_crash_before_the_intent_write_leaves_no_request() {
    let root = Root::new();
    let mut db = root.open();
    crash(
        &mut db,
        at(WriteOp::RecordIntent, Moment::BeforeWrite),
        |db| db.record_intent(&request(1), workspace(WS)).map(|_| ()),
    );
    drop(db);
    let mut restarted = root.open();
    assert!(restarted.pending_intents().unwrap().is_empty());
    assert_eq!(
        restarted.record_intent(&request(1), workspace(WS)).unwrap(),
        WriteOutcome::Applied
    );
}

/// Crash after the intent's statements but before its commit: the file
/// shows nothing of it.
#[test]
fn a_crash_inside_the_intent_transaction_rolls_it_back() {
    let root = Root::new();
    let mut db = root.open();
    crash(
        &mut db,
        at(WriteOp::RecordIntent, Moment::AfterWriteBeforeCommit),
        |db| db.record_intent(&request(1), workspace(WS)).map(|_| ()),
    );
    drop(db);
    let restarted = root.open();
    assert!(restarted.intent(run_id(1)).unwrap().is_none());
}

/// Crash after the intent committed, before the caller heard: on restart
/// it is a pending request with its whole payload, listed for completion;
/// the caller's retry is already applied and makes no second row.
#[test]
fn a_crash_after_the_intent_commit_leaves_a_pending_request_to_complete_forward() {
    let root = Root::new();
    let mut db = root.open();
    crash(
        &mut db,
        at(WriteOp::RecordIntent, Moment::AfterCommit),
        |db| db.record_intent(&request(1), workspace(WS)).map(|_| ()),
    );
    drop(db);
    let mut restarted = root.open();
    let pending = restarted.pending_intents().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].run, run_id(1));
    assert_eq!(
        pending[0].payload,
        request(1),
        "the payload a restart completes from"
    );
    assert_eq!(
        restarted.record_intent(&request(1), workspace(WS)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert_eq!(
        restarted.pending_intents().unwrap().len(),
        1,
        "never a second row"
    );
    // Completing forward: the Run document is committed, the intent
    // satisfied, and it is no longer a request.
    restarted.satisfy_intent(run_id(1), document(9)).unwrap();
    assert!(restarted.pending_intents().unwrap().is_empty());
}

/// Crash after the intent but before the satisfy (between §7.1 steps 1 and
/// 4): the request is still pending and completes forward.
#[test]
fn a_crash_before_the_satisfy_leaves_the_request_pending() {
    let root = Root::new();
    let mut db = root.open();
    db.record_intent(&request(1), workspace(WS)).unwrap();
    crash(
        &mut db,
        at(WriteOp::SatisfyIntent, Moment::BeforeWrite),
        |db| db.satisfy_intent(run_id(1), document(9)).map(|_| ()),
    );
    drop(db);
    let mut restarted = root.open();
    assert_eq!(
        restarted.intent(run_id(1)).unwrap().unwrap().state,
        IntentState::Pending
    );
    assert_eq!(restarted.pending_intents().unwrap().len(), 1);
    assert_eq!(
        restarted.satisfy_intent(run_id(1), document(9)).unwrap(),
        WriteOutcome::Applied
    );
}

/// Crash after the satisfy committed: the run exists, is not a request,
/// and is not redone — the retry naming the same document is already
/// applied, a different document is a conflict.
#[test]
fn a_crash_after_the_satisfy_commit_is_not_redone() {
    let root = Root::new();
    let mut db = root.open();
    db.record_intent(&request(1), workspace(WS)).unwrap();
    crash(
        &mut db,
        at(WriteOp::SatisfyIntent, Moment::AfterCommit),
        |db| db.satisfy_intent(run_id(1), document(9)).map(|_| ()),
    );
    drop(db);
    let mut restarted = root.open();
    let record = restarted.intent(run_id(1)).unwrap().unwrap();
    assert_eq!(record.state, IntentState::Satisfied);
    assert_eq!(record.run_document, Some(document(9)));
    assert!(
        restarted.pending_intents().unwrap().is_empty(),
        "not a request"
    );
    assert_eq!(
        restarted.satisfy_intent(run_id(1), document(9)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert_eq!(
        restarted.satisfy_intent(run_id(1), document(10)),
        Err(LedgerError::IntentConflict),
        "there is no interval in which two records both claim to be the run"
    );
}

// ----------------------------------------------------------- the attempt

/// Before and after every attempt advance: before, the phase is unchanged
/// and the restart classifies from the evidence it has; after, the phase
/// is durable and the retry is already applied.
#[test]
fn a_crash_around_each_attempt_advance_leaves_a_classifiable_attempt() {
    let ladder = [
        AttemptPhase::Starting,
        AttemptPhase::Running,
        AttemptPhase::Ended,
    ];
    for (index, target) in ladder.iter().enumerate() {
        for moment in Moment::ALL {
            let root = Root::new();
            let mut db = root.open();
            let run = created_run(&mut db, 1);
            let opened = db.open_attempt(run).unwrap();
            let generation = opened.generation;
            for earlier in &ladder[..index] {
                db.advance(run, generation, *earlier).unwrap();
            }
            let before = db.current_attempt(run).unwrap().unwrap().phase;
            crash(&mut db, at(WriteOp::AdvanceAttempt, moment), |db| {
                db.advance(run, generation, *target).map(|_| ())
            });
            drop(db);
            let mut restarted = root.open();
            let attempt = restarted.current_attempt(run).unwrap().unwrap();
            assert_eq!(
                attempt.attempt, opened.attempt,
                "the same attempt after a restart"
            );
            let expected = match moment {
                Moment::BeforeWrite | Moment::AfterWriteBeforeCommit => before,
                Moment::AfterCommit => *target,
            };
            assert_eq!(attempt.phase, expected, "{target:?} at {moment:?}");
            // The retry is harmless whichever side of the commit the crash
            // fell on: same phase is already applied, the earlier phase
            // still advances.
            let retried = restarted.advance(run, generation, *target).unwrap();
            assert_eq!(retried.phase, *target);
            // And the restart never fails the run: whatever the host says,
            // the verdict is one of §7.6's four.
            let verdict = classify_at_restart(&attempt, LookupAnswer::NoEvidence, host(1));
            assert_ne!(verdict.name(), "failed");
            assert!(matches!(verdict, RestartVerdict::Interrupted { .. }));
        }
    }
}

/// A crash between reserving the key here and hearing from the host: the
/// key is durable, so the restart can ask the non-creating lookup about it
/// instead of retrying create.
#[test]
fn a_crash_after_binding_the_key_leaves_it_to_look_up() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    let generation = db.open_attempt(run).unwrap().generation;
    db.advance(run, generation, AttemptPhase::Starting).unwrap();
    crash(
        &mut db,
        at(WriteOp::BindSession, Moment::AfterCommit),
        |db| {
            db.bind_session(run, generation, reserved(1), host(1))
                .map(|_| ())
        },
    );
    drop(db);
    let restarted = root.open();
    let attempt = restarted.current_attempt(run).unwrap().unwrap();
    assert_eq!(attempt.provider_session, Some(reserved(1)));
    assert_eq!(attempt.host_instance, Some(host(1)));
    assert_eq!(
        classify_at_restart(&attempt, LookupAnswer::Creating, host(1)),
        RestartVerdict::WaitThenReask,
        "a process may exist right now; never the nothing-running branch"
    );
    assert!(matches!(
        classify_at_restart(&attempt, LookupAnswer::NoEvidence, host(1)),
        RestartVerdict::Interrupted {
            resume: crate::ResumePolicy::PersonDecides,
            ..
        }
    ));
}

/// The fence survives a crash: an unretired attempt still blocks a fresh
/// one after the restart, and a retire that committed still authorises it.
#[test]
fn the_retirement_fence_is_durable_on_both_sides_of_the_commit() {
    for moment in [Moment::AfterWriteBeforeCommit, Moment::AfterCommit] {
        let root = Root::new();
        let mut db = root.open();
        let run = created_run(&mut db, 1);
        let generation = running_attempt(&mut db, run, 1).generation;
        db.advance(run, generation, AttemptPhase::Interrupted)
            .unwrap();
        crash(&mut db, at(WriteOp::RetireAttempt, moment), |db| {
            db.retire(run, generation).map(|_| ())
        });
        drop(db);
        let mut restarted = root.open();
        match moment {
            Moment::AfterCommit => {
                assert!(restarted.current_attempt(run).unwrap().unwrap().retired);
                let fresh = restarted.open_attempt(run).unwrap();
                assert_eq!(fresh.generation, AttemptGeneration::FIRST.next());
            }
            _ => {
                assert!(!restarted.current_attempt(run).unwrap().unwrap().retired);
                assert_eq!(
                    restarted.open_attempt(run),
                    Err(LedgerError::AttemptLive { generation })
                );
            }
        }
    }
}

// ------------------------------------------------------------- the queue

/// After enqueue, before acknowledge: the entry has its id and its body on
/// restart; the retried enqueue is already applied; the acknowledgement
/// that was lost is given once and then already applied.
#[test]
fn a_crash_after_enqueue_before_acknowledge_keeps_the_entry_under_its_id() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    crash(&mut db, at(WriteOp::Enqueue, Moment::AfterCommit), |db| {
        db.enqueue(event(7), run, PublicationKind::Lifecycle, &body())
            .map(|_| ())
    });
    drop(db);
    let mut restarted = root.open();
    let open = restarted.open_publications(10).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(
        open[0].event,
        event(7),
        "the same EventId, so the journal dedupes"
    );
    assert_eq!(open[0].body.as_str(), body());
    assert_eq!(
        restarted
            .enqueue(event(7), run, PublicationKind::Lifecycle, &body())
            .unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert_eq!(restarted.open_publications(10).unwrap().len(), 1);
    assert_eq!(
        restarted.acknowledge(event(7)).unwrap(),
        WriteOutcome::Applied
    );
    assert_eq!(
        restarted.acknowledge(event(7)).unwrap(),
        WriteOutcome::AlreadyApplied,
        "a lost acknowledgement is retried, not repeated"
    );
    assert!(restarted.open_publications(10).unwrap().is_empty());
}

/// An enqueue that never committed is absent: either the entry has its id
/// or there is no entry.
#[test]
fn a_crash_inside_enqueue_leaves_no_entry() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    crash(
        &mut db,
        at(WriteOp::Enqueue, Moment::AfterWriteBeforeCommit),
        |db| {
            db.enqueue(event(7), run, PublicationKind::Usage, &body())
                .map(|_| ())
        },
    );
    drop(db);
    let mut restarted = root.open();
    assert!(restarted.open_publications(10).unwrap().is_empty());
    assert_eq!(
        restarted.acknowledge(event(7)),
        Err(LedgerError::UnknownPublication)
    );
}

/// A lost acknowledgement's answer: the ack committed, the caller retries,
/// the retry is already applied and the entry stays closed.
#[test]
fn a_crash_after_the_acknowledge_commit_is_answered_by_the_retry() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    db.enqueue(
        event(7),
        run,
        PublicationKind::ArtifactRegistration,
        &body(),
    )
    .unwrap();
    crash(
        &mut db,
        at(WriteOp::Acknowledge, Moment::AfterCommit),
        |db| db.acknowledge(event(7)).map(|_| ()),
    );
    drop(db);
    let mut restarted = root.open();
    assert!(restarted.open_publications(10).unwrap().is_empty());
    assert_eq!(
        restarted.acknowledge(event(7)).unwrap(),
        WriteOutcome::AlreadyApplied
    );
}

// ------------------------------------------------------------- the claim

/// The claim and its `RunId` are one transaction: after the insert but
/// before the commit there is no claim at all; after the commit but before
/// the caller heard, the claim exists with its run id and the retry finds
/// it rather than minting a second run.
#[test]
fn a_claim_either_exists_with_its_run_id_or_does_not_exist_at_all() {
    for moment in Moment::ALL {
        let root = Root::new();
        let mut db = root.open();
        crash(&mut db, at(WriteOp::ClaimSlot, moment), |db| {
            db.claim_slot(schedule(3), 900_000, 2).map(|_| ())
        });
        drop(db);
        let mut restarted = root.open();
        match moment {
            Moment::AfterCommit => {
                let claim = restarted.slot(schedule(3), 900_000).unwrap().unwrap();
                assert_eq!(claim.state, SlotState::Claimed);
                assert_eq!(claim.revision, 2);
                assert_eq!(
                    restarted.claim_slot(schedule(3), 900_000, 2),
                    Err(LedgerError::SlotAlreadyClaimed {
                        run: claim.run,
                        revision: 2
                    }),
                    "the retry finds the run the crash left to look up"
                );
                assert_eq!(restarted.slots_of(schedule(3)).unwrap().len(), 1);
            }
            _ => {
                assert!(restarted.slot(schedule(3), 900_000).unwrap().is_none());
                let claimed = restarted.claim_slot(schedule(3), 900_000, 2).unwrap();
                assert_eq!(restarted.slots_of(schedule(3)).unwrap()[0].run, claimed.run);
            }
        }
    }
}

/// The outcome of a slot, on both sides of its commit.
#[test]
fn a_crash_around_the_outcome_leaves_the_slot_claimed_or_settled_never_lost() {
    for moment in Moment::ALL {
        let root = Root::new();
        let mut db = root.open();
        let claimed = db.claim_slot(schedule(3), 900_000, 1).unwrap();
        crash(&mut db, at(WriteOp::RecordOutcome, moment), |db| {
            db.record_outcome(schedule(3), 900_000, SlotOutcome::Fired)
                .map(|_| ())
        });
        drop(db);
        let mut restarted = root.open();
        let slot = restarted.slot(schedule(3), 900_000).unwrap().unwrap();
        assert_eq!(slot.run, claimed.run, "the run id never changes");
        let expected = match moment {
            Moment::AfterCommit => SlotState::Settled(SlotOutcome::Fired),
            _ => SlotState::Claimed,
        };
        assert_eq!(slot.state, expected);
        restarted
            .record_outcome(schedule(3), 900_000, SlotOutcome::Fired)
            .unwrap();
        assert_eq!(
            restarted.slot(schedule(3), 900_000).unwrap().unwrap().state,
            SlotState::Settled(SlotOutcome::Fired)
        );
    }
}

// ---------------------------------------------------------------- the seam

/// Every write has all three moments, and an armed point that is not
/// reached leaves the ledger whole.
#[test]
fn every_write_op_can_be_armed_at_every_moment() {
    let mut arm = CrashArm::default();
    for op in WriteOp::ALL {
        for moment in Moment::ALL {
            arm.arm(CrashPoint { op, at: moment });
            for other in WriteOp::ALL.into_iter().filter(|o| *o != op) {
                assert_eq!(arm.check(other, moment), Ok(()), "another op passes");
            }
            assert_eq!(
                arm.check(op, moment),
                Err(LedgerError::Crashed(CrashPoint { op, at: moment }))
            );
            assert_eq!(
                arm.check(op, Moment::BeforeWrite),
                Err(LedgerError::Crashed(CrashPoint { op, at: moment })),
                "dead stays dead"
            );
            arm = CrashArm::default();
        }
    }
    let root = Root::new();
    let mut db = root.open();
    db.arm_crash(at(WriteOp::ClaimSlot, Moment::AfterCommit));
    let run = created_run(&mut db, 1);
    assert!(
        db.intent(run).unwrap().is_some(),
        "an unreached point changes nothing"
    );
}
