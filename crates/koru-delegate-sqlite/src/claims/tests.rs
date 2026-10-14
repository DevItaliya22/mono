// SPDX-License-Identifier: MPL-2.0

//! One slot, one run: keyed by the occurrence, never the revision.

use super::*;
use crate::fixtures::{Root, request, schedule, workspace};
use crate::{CrashPoint, Moment, WriteOp};

#[test]
fn a_slot_is_claimed_once_whatever_the_revision_and_mints_its_run() {
    let root = Root::new();
    let mut db = root.open();
    let claimed = db.claim_slot(schedule(1), 900_000, 3).unwrap();
    assert_eq!(claimed.schedule, schedule(1));
    assert_eq!(claimed.occurrence_ms, 900_000);
    assert_eq!(claimed.revision, 3);
    let slot = db.slot(schedule(1), 900_000).unwrap().unwrap();
    assert_eq!(slot.run, claimed.run);
    assert_eq!(slot.state, SlotState::Claimed);
    assert_eq!(slot.claimed_at_ms, root.now_ms());

    // A newer revision does not make a second slot; the recorded revision
    // is the one in force when it was claimed.
    assert_eq!(
        db.claim_slot(schedule(1), 900_000, 4),
        Err(LedgerError::SlotAlreadyClaimed {
            run: claimed.run,
            revision: 3
        })
    );
    assert_eq!(db.slot(schedule(1), 900_000).unwrap().unwrap().revision, 3);
    // Another occurrence, or another schedule, is another slot and another
    // run.
    let next = db.claim_slot(schedule(1), 960_000, 4).unwrap();
    let other = db.claim_slot(schedule(2), 900_000, 1).unwrap();
    assert_ne!(next.run, claimed.run);
    assert_ne!(other.run, claimed.run);
    let slots: Vec<_> = db
        .slots_of(schedule(1))
        .unwrap()
        .into_iter()
        .map(|s| s.occurrence_ms)
        .collect();
    assert_eq!(slots, vec![900_000, 960_000]);
    assert_eq!(
        db.claim_slot(schedule(3), 1, 0),
        Err(LedgerError::RevisionZero)
    );
}

#[test]
fn scheduler_supplied_claim_time_is_the_durable_event_authority() {
    let root = Root::new();
    let mut db = root.open();
    let launch = request(22);
    let authoritative = root.now_ms().saturating_sub(7_000);

    let claimed = db
        .claim_scheduled_run_at(
            schedule(1),
            900_000,
            3,
            &launch,
            workspace(2),
            authoritative,
        )
        .unwrap();
    assert_eq!(claimed.claimed_at_ms, authoritative);
    assert_eq!(
        db.intent(launch.run).unwrap().unwrap().created_at_ms,
        authoritative
    );

    let skipped = db
        .claim_skip_at(
            schedule(1),
            960_000,
            3,
            request(23).run,
            SlotOutcome::SkippedMissed,
            None,
            authoritative,
        )
        .unwrap();
    assert_eq!(skipped.claim.claimed_at_ms, authoritative);
}

#[test]
fn an_outcome_is_recorded_once_and_a_slot_is_never_rewritten() {
    let root = Root::new();
    let mut db = root.open();
    assert_eq!(
        db.record_outcome(schedule(1), 900_000, SlotOutcome::Fired),
        Err(LedgerError::UnknownSlot)
    );
    db.claim_slot(schedule(1), 900_000, 1).unwrap();
    assert_eq!(
        db.record_outcome(schedule(1), 900_000, SlotOutcome::Fired)
            .unwrap(),
        WriteOutcome::Applied
    );
    assert_eq!(
        db.record_outcome(schedule(1), 900_000, SlotOutcome::Fired)
            .unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert_eq!(
        db.record_outcome(schedule(1), 900_000, SlotOutcome::SkippedMissed),
        Err(LedgerError::InvalidSettlementOutcome)
    );
    assert_eq!(
        db.slot(schedule(1), 900_000).unwrap().unwrap().state,
        SlotState::Settled(SlotOutcome::Fired)
    );
}

#[test]
fn scheduled_claim_and_complete_pending_intent_are_both_or_neither() {
    for moment in Moment::ALL {
        let root = Root::new();
        let mut db = root.open();
        let mut scheduled = request(20);
        scheduled.requested_model = Some(koru_delegate_core::ModelName::new("gpt-5.4").unwrap());
        db.arm_crash(CrashPoint {
            op: WriteOp::ClaimScheduledRun,
            at: moment,
        });
        assert!(
            db.claim_scheduled_run(schedule(1), 900_000, 3, &scheduled, workspace(2))
                .is_err()
        );
        drop(db);
        let mut restarted = root.open();
        let claim = restarted.slot(schedule(1), 900_000).unwrap();
        let intent = restarted.intent(scheduled.run).unwrap();
        assert_eq!(claim.is_some(), intent.is_some(), "{moment:?}");
        if moment == Moment::AfterCommit {
            assert_eq!(
                restarted.pending_intents().unwrap()[0]
                    .payload
                    .requested_model,
                scheduled.requested_model
            );
            let mut retry = request(21);
            retry.requested_model =
                Some(koru_delegate_core::ModelName::new("another-model").unwrap());
            let original = restarted
                .claim_scheduled_run(schedule(1), 900_000, 4, &retry, workspace(3))
                .unwrap();
            assert_eq!(original.run, scheduled.run);
            assert_eq!(original.revision, 3);
            assert!(restarted.intent(retry.run).unwrap().is_none());
        }
    }
}

#[test]
fn due_run_and_representative_miss_commit_as_one_crash_boundary() {
    for moment in Moment::ALL {
        let root = Root::new();
        let mut db = root.open();
        let launch = request(60);
        db.arm_crash(CrashPoint {
            op: WriteOp::ClaimScheduledRun,
            at: moment,
        });
        assert!(
            db.claim_scheduled_run_with_missed(
                schedule(6),
                3_600_000,
                2,
                &launch,
                workspace(2),
                0,
                request(61).run,
            )
            .is_err()
        );
        drop(db);

        let restarted = root.open();
        let slots = restarted.slots_of(schedule(6)).unwrap();
        if moment == Moment::AfterCommit {
            assert_eq!(slots.len(), 2, "{moment:?}");
            assert!(matches!(
                slots[0].state,
                SlotState::Settled(SlotOutcome::SkippedMissed)
            ));
            assert!(matches!(slots[1].state, SlotState::Claimed));
            assert_eq!(restarted.pending_intents().unwrap()[0].run, launch.run);
        } else {
            assert!(slots.is_empty(), "{moment:?}");
            assert!(
                restarted.pending_intents().unwrap().is_empty(),
                "{moment:?}"
            );
        }
    }
}

#[test]
fn skipped_claim_is_atomic_settled_and_never_creates_an_intent() {
    for moment in Moment::ALL {
        let root = Root::new();
        let mut db = root.open();
        db.arm_crash(CrashPoint {
            op: WriteOp::ClaimSkip,
            at: moment,
        });
        let planned = request(40).run;
        assert!(
            db.claim_skip(
                schedule(2),
                600_000,
                1,
                planned,
                SlotOutcome::SkippedMissed,
                None,
            )
            .is_err()
        );
        drop(db);
        let mut restarted = root.open();
        let slot = restarted.slot(schedule(2), 600_000).unwrap();
        if moment == Moment::AfterCommit {
            let slot = slot.unwrap();
            assert_eq!(slot.state, SlotState::Settled(SlotOutcome::SkippedMissed));
            assert!(restarted.intent(slot.run).unwrap().is_none());
            let retry = restarted
                .claim_skip(
                    schedule(2),
                    600_000,
                    9,
                    request(41).run,
                    SlotOutcome::SkippedMissed,
                    None,
                )
                .unwrap();
            assert_eq!(retry.claim.run, slot.run);
            assert_eq!(retry.claim.run, planned);
            assert_eq!(retry.claim.revision, 1);
            let launch = request(22);
            assert_eq!(
                restarted.claim_scheduled_run(schedule(2), 600_000, 9, &launch, workspace(2),),
                Err(LedgerError::SlotOutcomeRecorded {
                    outcome: SlotOutcome::SkippedMissed,
                })
            );
            assert!(restarted.intent(launch.run).unwrap().is_none());
        } else {
            assert!(slot.is_none(), "{moment:?}");
        }
    }
}

#[test]
fn busy_skip_retains_the_holder_across_every_crash_boundary_and_restart() {
    for moment in Moment::ALL {
        let root = Root::new();
        let mut db = root.open();
        db.arm_crash(CrashPoint {
            op: WriteOp::ClaimSkip,
            at: moment,
        });
        let planned = request(42).run;
        let holder = request(43).run;
        assert!(
            db.claim_skip(
                schedule(4),
                720_000,
                2,
                planned,
                SlotOutcome::SkippedBusy,
                Some(holder),
            )
            .is_err()
        );
        drop(db);
        let mut restarted = root.open();
        let slot = restarted.slot(schedule(4), 720_000).unwrap();
        if moment == Moment::AfterCommit {
            let slot = slot.unwrap();
            assert_eq!(slot.run, planned);
            assert_eq!(slot.busy_holder, Some(holder));
            assert_eq!(slot.state, SlotState::Settled(SlotOutcome::SkippedBusy));
            assert_eq!(
                restarted.claim_skip(
                    schedule(4),
                    720_000,
                    3,
                    request(44).run,
                    SlotOutcome::SkippedBusy,
                    Some(request(45).run),
                ),
                Err(LedgerError::SlotClaimConflict)
            );
        } else {
            assert!(slot.is_none(), "{moment:?}");
        }
    }
}

#[test]
fn reverse_lookup_unsettled_reads_and_observation_cursor_survive_restart() {
    let root = Root::new();
    let mut db = root.open();
    let launch = request(30);
    let claimed = db
        .claim_scheduled_run(schedule(3), 120_000, 1, &launch, workspace(2))
        .unwrap();
    assert_eq!(db.unsettled_claims().unwrap().len(), 1);
    assert_eq!(
        db.slot_for_run(claimed.run).unwrap().unwrap().schedule,
        schedule(3)
    );
    db.record_outcome(schedule(3), 120_000, SlotOutcome::Fired)
        .unwrap();
    assert!(db.unsettled_claims().unwrap().is_empty());
    root.tick(3_000_000);
    db.claim_skip(
        schedule(3),
        180_000,
        1,
        request(31).run,
        SlotOutcome::SkippedMissed,
        None,
    )
    .unwrap();
    let cursor = db.observed_through_ms(schedule(3)).unwrap().unwrap();
    assert_eq!(cursor, root.now_ms());
    drop(db);
    assert_eq!(
        root.open().observed_through_ms(schedule(3)).unwrap(),
        Some(cursor)
    );
}

#[test]
fn only_missed_and_busy_are_valid_atomic_skips() {
    let root = Root::new();
    let mut db = root.open();
    assert_eq!(
        db.claim_skip(schedule(1), 1, 1, request(32).run, SlotOutcome::Fired, None,),
        Err(LedgerError::InvalidSkipOutcome)
    );
    assert_eq!(
        db.claim_skip(
            schedule(1),
            3,
            1,
            request(34).run,
            SlotOutcome::SkippedBusy,
            None,
        ),
        Err(LedgerError::InvalidSkipOutcome)
    );
    assert_eq!(
        db.claim_skip(
            schedule(1),
            2,
            1,
            request(33).run,
            SlotOutcome::SkippedRetired,
            None,
        ),
        Err(LedgerError::InvalidSkipOutcome)
    );
    assert!(db.slot(schedule(1), 1).unwrap().is_none());
}
