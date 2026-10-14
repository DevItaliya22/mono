// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn accepted_admission_is_not_terminal_success_and_never_times_out() {
    let mut admissions = Admissions::default();
    let now = Instant::now();
    let submission = CommandSubmissionId::next();
    assert!(admissions.insert(1, submission, 7, now));
    assert_eq!(admissions.deadline(), Some(now + DEADLINE));
    assert!(admissions.response(
        1,
        7,
        &Response::AccountQueued {
            generation: 42,
            owner_scope: 0
        }
    ));
    assert!(!admissions.expire(now + DEADLINE));
    assert!(admissions.take_failure().is_none());
    assert!(admissions.deadline().is_none());
    let accepted = admissions.take_admission().unwrap();
    assert_eq!(accepted.submission, submission);
    assert_eq!((accepted.owner_scope, accepted.generation), (0, 42));
    assert!(admissions.take_admission().is_none());
}

#[test]
fn queue_rejection_is_correlated_and_lost_ack_is_uncertain() {
    let mut admissions = Admissions::default();
    let now = Instant::now();
    let rejected = CommandSubmissionId::next();
    let lost = CommandSubmissionId::next();
    admissions.insert(1, rejected, 7, now);
    admissions.insert(2, lost, 7, now);
    assert!(admissions.response(
        1,
        7,
        &Response::Failed {
            code: "account_queue_full".into(),
            message: "Fixture queue is full".into()
        }
    ));
    assert_eq!(
        admissions.take_failure(),
        Some(CommandFailure {
            submission: rejected,
            uncertain: false
        })
    );
    assert!(!admissions.expire(now + DEADLINE - Duration::from_millis(1)));
    assert!(admissions.expire(now + DEADLINE));
    assert_eq!(
        admissions.take_failure(),
        Some(CommandFailure {
            submission: lost,
            uncertain: true
        })
    );
    assert!(!admissions.response(
        2,
        7,
        &Response::AccountQueued {
            generation: 42,
            owner_scope: 0
        }
    ));
    assert!(admissions.take_failure().is_none());
}

#[test]
fn old_connection_cannot_ack_or_retire_replacement_submission() {
    let mut admissions = Admissions::default();
    let now = Instant::now();
    let old = CommandSubmissionId::next();
    let new = CommandSubmissionId::next();
    admissions.insert(1, old, 7, now);
    assert!(admissions.replace(9));
    assert_eq!(
        admissions.take_failure(),
        Some(CommandFailure {
            submission: old,
            uncertain: true
        })
    );
    admissions.insert(2, new, 9, now);
    assert!(!admissions.response(
        2,
        7,
        &Response::AccountQueued {
            generation: 42,
            owner_scope: 0
        }
    ));
    assert!(!admissions.retire(7));
    assert!(admissions.retire(9));
    assert_eq!(
        admissions.take_failure(),
        Some(CommandFailure {
            submission: new,
            uncertain: true
        })
    );
}

#[test]
fn tracking_and_feedback_are_bounded() {
    let mut admissions = Admissions::default();
    let now = Instant::now();
    for id in 0..CAPACITY as u64 {
        assert!(admissions.insert(id, CommandSubmissionId::next(), 1, now));
    }
    let rejected = CommandSubmissionId::next();
    assert!(!admissions.insert(100, rejected, 1, now));
    assert_eq!(admissions.pending.len(), CAPACITY);
    assert_eq!(admissions.take_failure().unwrap().submission, rejected);
    admissions.expire(now + DEADLINE);
    assert_eq!(admissions.failures.len(), CAPACITY);
}
