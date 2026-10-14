// SPDX-License-Identifier: MPL-2.0

use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::*;

/// **The outbox is one slot for the whole process, so these tests take turns.**
/// Each holds this for its duration and starts from an empty slot, which is
/// also what makes `is_pending` assertions mean anything.
static SERIAL: Mutex<()> = Mutex::new(());

fn locked() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    *slot::lock() = None;
    guard
}

/// Seed the outbox with canned answers, newest attempt last. An entry of
/// `None` is a worker that died without answering.
fn seed(answers: Vec<Option<Result<(), SubmitError>>>, draft: Draft) {
    let mut answers = answers.into_iter();
    let start = move || {
        let (tx, rx) = mpsc::channel();
        match answers.next() {
            // Dropping `tx` unsent is the dead-worker case.
            Some(Some(result)) => {
                let _ = tx.send(result);
            }
            Some(None) | None => {}
        }
        rx
    };
    begin(Box::new(start), draft);
}

fn drafted(message: &str) -> Draft {
    Draft {
        message: message.to_string(),
        ..Draft::default()
    }
}

#[test]
fn a_delivered_send_reports_sent_and_leaves_nothing_pending() {
    let _serial = locked();
    seed(vec![Some(Ok(()))], drafted("it works"));
    let outcome = poll(Instant::now());
    assert!(matches!(outcome, Some(Outcome::Sent)), "delivered");
    assert!(!is_pending(), "and the outbox is empty");
}

#[test]
fn the_first_failure_waits_rather_than_reporting_it() {
    let _serial = locked();
    seed(
        vec![Some(Err(SubmitError::Network("down".into())))],
        drafted("first"),
    );
    let now = Instant::now();
    assert!(poll(now).is_none(), "nothing is reported yet");
    assert!(is_pending(), "the send is still owned");
    assert!(
        deadline().is_some_and(|at| at > now),
        "and the retry is scheduled ahead"
    );
}

#[test]
fn the_retry_runs_once_and_a_second_failure_hands_the_draft_back() {
    let _serial = locked();
    seed(
        vec![
            Some(Err(SubmitError::Network("down".into()))),
            Some(Err(SubmitError::Network("still down".into()))),
        ],
        drafted("keep me"),
    );
    let start = Instant::now();
    assert!(poll(start).is_none(), "first failure schedules the retry");
    // Reaching the deadline starts attempt two; its answer arrives after.
    let due = start + RETRY_AFTER;
    assert!(poll(due).is_none(), "the retry is spawned, not judged");
    let Some(Outcome::Failed { draft }) = poll(due) else {
        panic!("the second failure gives up");
    };
    assert_eq!(draft.message, "keep me", "with the report intact");
    assert!(!is_pending(), "and nothing is left to drain");
}

#[test]
fn a_retry_that_succeeds_reports_sent_and_keeps_no_draft() {
    let _serial = locked();
    seed(
        vec![Some(Err(SubmitError::Network("blip".into()))), Some(Ok(()))],
        drafted("second time"),
    );
    let start = Instant::now();
    assert!(poll(start).is_none());
    let due = start + RETRY_AFTER;
    assert!(poll(due).is_none(), "the retry is spawned");
    assert!(
        matches!(poll(due), Some(Outcome::Sent)),
        "and it lands the second time"
    );
    assert!(!is_pending());
}

#[test]
fn the_retry_does_not_run_before_it_is_due() {
    let _serial = locked();
    seed(
        vec![Some(Err(SubmitError::Network("down".into()))), Some(Ok(()))],
        drafted("waiting"),
    );
    let start = Instant::now();
    assert!(poll(start).is_none());
    assert!(
        poll(start + RETRY_AFTER - Duration::from_millis(1)).is_none(),
        "a tick before the deadline changes nothing"
    );
    assert!(is_pending(), "and the send is still waiting");
}

#[test]
fn a_worker_that_dies_without_answering_counts_as_a_failed_attempt() {
    let _serial = locked();
    seed(vec![None, None], drafted("nobody answered"));
    let start = Instant::now();
    assert!(poll(start).is_none(), "the first death schedules the retry");
    let due = start + RETRY_AFTER;
    assert!(poll(due).is_none(), "the retry is spawned");
    let Some(Outcome::Failed { draft }) = poll(due) else {
        panic!("a dead worker is a failure, not a hang");
    };
    assert_eq!(draft.message, "nobody answered");
}

#[test]
fn an_empty_outbox_reports_nothing_and_asks_for_no_tick() {
    let _serial = locked();
    assert!(!is_pending());
    assert!(poll(Instant::now()).is_none());
    assert!(deadline().is_none(), "and the shell is not woken for it");
}

/// **The guard that stops `cargo test` mailing the team.** Three view tests
/// once pressed Send and posted to the production route; this asserts the
/// offline transport is what a test build reaches for.
///
/// The URL is deliberately not koru.do: if the seam ever breaks, this test
/// fails against a domain that does not resolve rather than delivering a
/// report to anybody.
#[test]
fn a_send_from_a_test_build_never_reaches_the_network() {
    let _serial = locked();
    send(
        "https://invalid.invalid/never".to_string(),
        Payload {
            kind: super::super::Kind::Bug,
            message: "offline".into(),
            frequency: None,
            expected: None,
            importance: None,
            contact: None,
            screenshots: Vec::new(),
            logs_zip: None,
        },
        drafted("offline"),
        false,
    );
    // The worker answers on its own thread; give it a bounded moment.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match poll(Instant::now()) {
            Some(Outcome::Sent) => break,
            Some(Outcome::Failed { .. }) => {
                panic!("a test build reached the network — the offline seam is gone")
            }
            None if Instant::now() < deadline => std::thread::yield_now(),
            None => panic!("the offline transport never answered"),
        }
    }
    assert!(!is_pending());
}
