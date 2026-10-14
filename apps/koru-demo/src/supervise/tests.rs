// SPDX-License-Identifier: MPL-2.0

//! **What these prove, and the one thing they carefully do not assert.**
//!
//! They prove that the supervisor comes back from a call that has not returned.
//! The assertion that carries that is **causal, not temporal**: `call` came back
//! with no answer, the worker that owes the answer is still alive and no longer
//! reachable, and a *second* call is answered while the first is still inside
//! `perform`. No test here asserts that anything happened within a number of
//! milliseconds; that would be measuring this machine's scheduler, and
//! [PRINCIPLES.md](../../../../guide/contributing/PRINCIPLES.md) is explicit that a wall-clock
//! budget gets relaxed until it asserts nothing.
//!
//! What they do **not** prove is anything about COM. A `Gate` that blocks on a
//! condvar is not a UI Automation provider that has stopped answering, and a
//! thread abandoned here is not an apartment abandoned there. That is the
//! interactive proof's, against a real provider, and nothing in this file may be
//! cited for it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use super::*;
use crate::inspect::{Handle, InspectError};

/// Something a test opens by hand. Closed until it is not.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn wait(&self) {
        let mut open = self.open.lock().expect("no test poisons this");
        while !*open {
            open = self.changed.wait(open).expect("no test poisons this");
        }
    }

    fn open(&self) {
        *self.open.lock().expect("no test poisons this") = true;
        self.changed.notify_all();
    }
}

/// A mechanism that answers with its own generation, and blocks first if told.
struct Fake {
    generation: u64,
    gate: Option<Arc<Gate>>,
    inside: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}

impl Mechanism for Fake {
    fn perform(&mut self, _request: Request) -> Response {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            self.inside.store(true, Ordering::SeqCst);
            gate.wait();
            self.inside.store(false, Ordering::SeqCst);
        }
        // The generation is the whole answer, so a test can tell which
        // apartment replied.
        Response::Facts(crate::inspect::ElementFacts {
            handle: Handle {
                generation: self.generation,
                slot: 1,
            },
            locator: None,
            role: crate::inspect::Role::Window,
            enabled: true,
            offscreen: false,
            bounds: None,
            process_id: None,
        })
    }
}

struct Harness {
    /// Set before a test starts a call it wants to hang.
    hang_next: Arc<Mutex<Option<Arc<Gate>>>>,
    inside: Arc<AtomicBool>,
    starts: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl Harness {
    fn new() -> (Self, Start) {
        let hang_next: Arc<Mutex<Option<Arc<Gate>>>> = Arc::default();
        let inside = Arc::new(AtomicBool::new(false));
        let starts = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let start: Start = {
            let hang_next = Arc::clone(&hang_next);
            let inside = Arc::clone(&inside);
            let starts = Arc::clone(&starts);
            let calls = Arc::clone(&calls);
            Arc::new(move |generation| {
                starts.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(Fake {
                    generation,
                    gate: hang_next.lock().expect("no test poisons this").take(),
                    inside: Arc::clone(&inside),
                    calls: Arc::clone(&calls),
                }) as Box<dyn Mechanism>)
            })
        };
        (
            Self {
                hang_next,
                inside,
                starts,
                calls,
            },
            start,
        )
    }

    /// The next worker started will block on this gate forever.
    fn hang_the_next_worker(&self) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        *self.hang_next.lock().expect("no test poisons this") = Some(Arc::clone(&gate));
        gate
    }

    fn provider_is_still_inside_the_call(&self) -> bool {
        self.inside.load(Ordering::SeqCst)
    }

    /// Wait until the worker has actually entered the provider call.
    ///
    /// A *wait*, not a sleep, and it runs **after** the supervisor has already
    /// come back — which is what makes the claim causal rather than a race with
    /// the scheduler. The job was sent, so the worker reaches `perform`
    /// eventually; the bound is here so a regression fails the suite instead of
    /// hanging it.
    fn wait_until_the_provider_is_inside(&self) {
        for _ in 0..5_000 {
            if self.provider_is_still_inside_the_call() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the worker never reached the provider call");
    }
}

fn a_question() -> Request {
    Request::Facts {
        element: Handle {
            generation: 1,
            slot: 1,
        },
    }
}

fn tight() -> Limits {
    Limits {
        budget: Duration::from_millis(60),
        max_abandoned: 4,
    }
}

fn answered_by(outcome: &Outcome) -> Option<u64> {
    match outcome {
        Outcome::Answered(Response::Facts(facts)) => Some(facts.handle.generation),
        _ => None,
    }
}

#[test]
fn an_ordinary_call_is_answered_by_the_worker_that_is_already_running() {
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(start, tight());
    let cancel = CancelToken::new();

    assert_eq!(
        answered_by(&supervisor.call(a_question(), &cancel)),
        Some(1)
    );
    assert_eq!(
        answered_by(&supervisor.call(a_question(), &cancel)),
        Some(1)
    );
    assert_eq!(
        harness.starts.load(Ordering::SeqCst),
        1,
        "a healthy apartment is reused; opening one per call would be the cost the whole design \
         exists to avoid"
    );
    assert_eq!(supervisor.abandoned(), 0);
}

#[test]
fn control_comes_back_while_the_provider_is_still_inside_the_call() {
    // The property, stated causally: the supervisor returns, and at that instant
    // the call it was waiting for has not returned and never will.
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(start, tight());
    let cancel = CancelToken::new();
    let gate = harness.hang_the_next_worker();

    let outcome = supervisor.call(a_question(), &cancel);

    // `Lost` is *no answer arrived*, and the worker that owes that answer is
    // still alive and unreachable. Both halves are deterministic: a thread's
    // census mark is dropped only when the thread ends, and it cannot end while
    // it is in the gate.
    assert_eq!(outcome, Outcome::Lost);
    assert_eq!(supervisor.abandoned(), 1);
    harness.wait_until_the_provider_is_inside();
    assert!(
        harness.provider_is_still_inside_the_call(),
        "the supervisor came back before the provider did, and the provider has not come back"
    );

    gate.open();
}

#[test]
fn a_wedged_apartment_costs_one_worker_and_not_the_run() {
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(start, tight());
    let cancel = CancelToken::new();
    let gate = harness.hang_the_next_worker();

    assert_eq!(supervisor.call(a_question(), &cancel), Outcome::Lost);

    harness.wait_until_the_provider_is_inside();

    // Still stuck — and the run carries on, in a fresh apartment, with a fresh
    // generation on everything it mints.
    let outcome = supervisor.call(a_question(), &cancel);
    assert!(
        harness.provider_is_still_inside_the_call(),
        "the first call must still be hanging for this to mean anything"
    );
    assert_eq!(answered_by(&outcome), Some(2), "{outcome:?}");
    assert_eq!(supervisor.abandoned(), 1);

    gate.open();
}

#[test]
fn the_abandoned_workers_answer_never_arrives_in_a_later_call() {
    // The failure this prevents is the quiet one: a provider that unblocks
    // three minutes later and hands its stale answer to whatever is waiting
    // now, which would be a report about a control that was current before the
    // window changed.
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(start, tight());
    let cancel = CancelToken::new();
    let gate = harness.hang_the_next_worker();

    assert_eq!(supervisor.call(a_question(), &cancel), Outcome::Lost);
    gate.open();

    // Whatever the abandoned worker does now, every later answer carries the
    // generation of the apartment that actually answered it.
    for expected in [2, 2, 2] {
        let outcome = supervisor.call(a_question(), &cancel);
        assert_eq!(answered_by(&outcome), Some(expected), "{outcome:?}");
    }
}

#[test]
fn a_handle_from_an_abandoned_apartment_is_refused_rather_than_answered_about() {
    // The other half of the fence, and the reason `Handle` carries a
    // generation at all: slot 1 exists in every apartment, and slot 1 of the
    // worker that wedged is not slot 1 of its successor.
    let stale = Handle {
        generation: 1,
        slot: 1,
    };
    let live = Handle {
        generation: 2,
        slot: 1,
    };
    assert_ne!(stale, live);

    // The refusal itself belongs to whatever holds the table — `uia.rs` in a
    // real run. What is pinned here is that the two are distinguishable and
    // that the error exists to say so.
    assert_eq!(
        InspectError::StaleHandle.to_string(),
        "the handle belongs to a worker that was abandoned"
    );
}

#[test]
fn cancelling_from_another_thread_returns_before_the_provider_does() {
    // Not a flag somebody polls: the cancel *sends*, so the waiting
    // `recv_timeout` wakes immediately rather than at the end of its budget.
    // The budget here is long enough that a poll-based design would visibly
    // fail this test rather than pass it slowly.
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(
        start,
        Limits {
            budget: Duration::from_secs(120),
            max_abandoned: 4,
        },
    );
    let cancel = CancelToken::new();
    let gate = harness.hang_the_next_worker();

    let stopper = {
        let cancel = cancel.clone();
        let inside = Arc::clone(&harness.inside);
        std::thread::spawn(move || {
            while !inside.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            cancel.cancel();
        })
    };

    let outcome = supervisor.call(a_question(), &cancel);
    stopper.join().expect("the stopper thread joined");

    assert_eq!(outcome, Outcome::Cancelled);
    assert!(
        harness.provider_is_still_inside_the_call(),
        "cancellation must not have waited for the provider"
    );
    // A cancelled run stays cancelled: the next call is refused without
    // touching the platform at all.
    assert_eq!(supervisor.call(a_question(), &cancel), Outcome::Cancelled);
    assert_eq!(
        harness.calls.load(Ordering::SeqCst),
        1,
        "no further work may be handed to a provider after a cancel"
    );

    gate.open();
}

#[test]
fn a_token_cancelled_before_the_call_starts_no_work_at_all() {
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(start, tight());
    let cancel = CancelToken::new();
    cancel.cancel();

    assert_eq!(supervisor.call(a_question(), &cancel), Outcome::Cancelled);
    assert_eq!(harness.starts.load(Ordering::SeqCst), 0);
    assert_eq!(harness.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn the_supervisor_stops_opening_apartments_rather_than_leaving_them_behind_without_bound() {
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(
        start,
        Limits {
            budget: Duration::from_millis(40),
            max_abandoned: 2,
        },
    );
    let cancel = CancelToken::new();

    let first = harness.hang_the_next_worker();
    assert_eq!(supervisor.call(a_question(), &cancel), Outcome::Lost);
    let second = harness.hang_the_next_worker();
    assert_eq!(supervisor.call(a_question(), &cancel), Outcome::Lost);

    let outcome = supervisor.call(a_question(), &cancel);
    assert_eq!(outcome, Outcome::Wedged { abandoned: 2 }, "{outcome:?}");
    assert_eq!(
        harness.starts.load(Ordering::SeqCst),
        2,
        "a run that has wedged its whole budget must report it, not open another apartment"
    );

    first.open();
    second.open();
}

#[test]
fn a_provider_that_finally_answers_gives_its_apartment_back() {
    // The leak is bounded by how many providers are stuck *at once*, which is
    // only true if a worker that unblocks actually retires. It does: its send
    // fails, its next `recv` finds a closed channel, and the thread ends.
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(
        start,
        Limits {
            budget: Duration::from_millis(40),
            max_abandoned: 1,
        },
    );
    let cancel = CancelToken::new();

    let gate = harness.hang_the_next_worker();
    assert_eq!(supervisor.call(a_question(), &cancel), Outcome::Lost);
    assert_eq!(
        supervisor.call(a_question(), &cancel),
        Outcome::Wedged { abandoned: 1 }
    );

    gate.open();
    // The thread ends on its own schedule, so this waits for the census rather
    // than for a duration. Bounded, so a regression fails rather than hangs the
    // suite.
    for _ in 0..5_000 {
        if supervisor.abandoned() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(supervisor.abandoned(), 0, "an unblocked worker must retire");

    let outcome = supervisor.call(a_question(), &cancel);
    assert_eq!(answered_by(&outcome), Some(2), "{outcome:?}");
}

#[test]
fn dropping_the_supervisor_does_not_wait_for_a_stuck_provider() {
    // The one moment somebody is trying to get their machine back. A `Drop`
    // that joined would hang here forever, which is exactly what a `JoinHandle`
    // field would have bought.
    let (harness, start) = Harness::new();
    let mut supervisor = Supervisor::new(start, tight());
    let cancel = CancelToken::new();
    let gate = harness.hang_the_next_worker();

    assert_eq!(supervisor.call(a_question(), &cancel), Outcome::Lost);
    harness.wait_until_the_provider_is_inside();
    drop(supervisor);

    assert!(
        harness.provider_is_still_inside_the_call(),
        "the drop must not have waited for the provider"
    );
    gate.open();
}

#[test]
fn a_mechanism_that_cannot_start_says_so_instead_of_sitting_out_the_budget() {
    let start: Start = Arc::new(|_| Err("no apartment".to_owned()));
    let mut supervisor = Supervisor::new(
        start,
        Limits {
            // Long enough that a design which let the caller time out instead
            // would take two minutes to report a failure it knew immediately.
            budget: Duration::from_secs(120),
            max_abandoned: 4,
        },
    );
    assert_eq!(
        supervisor.call(a_question(), &CancelToken::new()),
        Outcome::Unavailable("no apartment".to_owned())
    );
}
