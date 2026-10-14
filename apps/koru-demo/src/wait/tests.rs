// SPDX-License-Identifier: MPL-2.0

//! **A fake clock and a scripted sequence of looks.**
//!
//! No thread sleeps and nothing is timed, so these finish instantly and mean
//! the same thing on a busy machine as on an idle one. They prove the wait's
//! *policy*: when it stops, what it reports, and that an interruption is not
//! mistaken for an absence. They prove nothing about a real tree changing, which
//! is the one thing a wait exists for and the one thing only the interactive
//! proof can establish.

use std::cell::{Cell, RefCell};

use super::*;
use crate::inspect::{ElementFacts, Handle, Rect, Role};
use crate::select::{Candidate, Unusable};

/// Time that only moves when a wait rests.
struct FakeClock {
    now: Cell<Instant>,
    rests: Cell<u32>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            now: Cell::new(Instant::now()),
            rests: Cell::new(0),
        }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.now.get()
    }

    fn rest(&self, interval: Duration) {
        self.rests.set(self.rests.get() + 1);
        self.now.set(self.now.get() + interval);
    }
}

fn patience() -> Patience {
    Patience {
        limit: Duration::from_millis(500),
        interval: Duration::from_millis(100),
    }
}

fn found() -> Selection {
    Selection::One(ElementFacts {
        handle: Handle {
            generation: 1,
            slot: 1,
        },
        locator: Some("koru:w1/v1/s:save#news".to_owned()),
        role: Role::Text,
        enabled: true,
        offscreen: false,
        bounds: Some(Rect {
            left: 0,
            top: 0,
            right: 10,
            bottom: 10,
        }),
        process_id: Some(4242),
    })
}

fn disabled() -> Selection {
    Selection::Unusable {
        candidate: Candidate {
            locator: Some("koru:w1/v1/s:save#retry".to_owned()),
            role: Role::Button,
            enabled: false,
            offscreen: false,
            bounds: None,
            process_id: Some(4242),
        },
        why: Unusable::Disabled,
    }
}

/// Hands back a scripted answer per look, repeating the last one forever.
fn script(looks: Vec<Look>) -> impl FnMut() -> Look {
    let looks = RefCell::new(looks.into_iter());
    let last = RefCell::new(Ok(Selection::NotFound));
    move || {
        if let Some(next) = looks.borrow_mut().next() {
            *last.borrow_mut() = next.clone();
        }
        last.borrow().clone()
    }
}

#[test]
fn a_control_already_in_the_wanted_state_costs_one_look_and_no_waiting() {
    let clock = FakeClock::new();
    let waited = until(Goal::Present, patience(), &clock, script(vec![Ok(found())]));
    assert!(waited.reached().is_some(), "{waited}");
    assert_eq!(clock.rests.get(), 0, "nothing should have rested");
}

#[test]
fn a_control_that_turns_up_on_a_later_pass_is_waited_for() {
    // The case the loop exists for: the tree changes on the window's pass, not
    // the client's, so the first look legitimately misses.
    let clock = FakeClock::new();
    let waited = until(
        Goal::Present,
        patience(),
        &clock,
        script(vec![
            Ok(Selection::NotFound),
            Ok(Selection::NotFound),
            Ok(found()),
        ]),
    );
    assert!(waited.reached().is_some(), "{waited}");
    assert_eq!(clock.rests.get(), 2);
}

#[test]
fn absence_is_a_goal_a_wait_can_reach() {
    // Screen retirement, seen from outside: the control that was there is no
    // longer findable by its old locator. There is nothing to poll *for*, so a
    // wait that could only wait for presence could not express it at all.
    let clock = FakeClock::new();
    let waited = until(
        Goal::Absent,
        patience(),
        &clock,
        script(vec![Ok(found()), Ok(Selection::NotFound)]),
    );
    assert_eq!(waited.reached(), Some(&Selection::NotFound), "{waited}");
}

#[test]
fn a_timeout_reports_the_last_thing_it_saw_and_not_merely_that_it_gave_up() {
    // *Not found for twenty seconds* and *found but disabled for twenty
    // seconds* send a person to different places. A wait that reported only
    // "timed out" would make them the same failure.
    let clock = FakeClock::new();
    let waited = until(
        Goal::Present,
        patience(),
        &clock,
        script(vec![Ok(disabled())]),
    );
    let Waited::TimedOut { looks, last, .. } = &waited else {
        panic!("expected a timeout: {waited}");
    };
    assert_eq!(*looks, 6, "five rests over a 500 ms limit at 100 ms");
    assert!(matches!(
        last,
        Selection::Unusable {
            why: Unusable::Disabled,
            ..
        }
    ));
    assert!(waited.to_string().contains("it is disabled"), "{waited}");
}

#[test]
fn a_cancelled_run_stops_waiting_and_is_not_reported_as_an_absence() {
    // The distinction that matters: a cancelled look is *no evidence about the
    // tree*. Folding it into "not found" would make a stopped run look like a
    // failed assertion about the product.
    let clock = FakeClock::new();
    let waited = until(
        Goal::Present,
        patience(),
        &clock,
        script(vec![Ok(Selection::NotFound), Err(Interrupted::Cancelled)]),
    );
    assert!(
        matches!(waited, Waited::Cancelled { looks: 2, .. }),
        "{waited}"
    );
    assert!(
        clock.rests.get() < 5,
        "a cancel must not sit out the rest of the deadline"
    );
}

#[test]
fn a_wedged_apartment_ends_the_wait_instead_of_being_retried_until_the_deadline() {
    let clock = FakeClock::new();
    let waited = until(
        Goal::Present,
        patience(),
        &clock,
        script(vec![Err(Interrupted::Wedged(
            "two apartments are not answering".to_owned(),
        ))]),
    );
    let Waited::Wedged { looks, why, .. } = &waited else {
        panic!("expected a wedge: {waited}");
    };
    assert_eq!(*looks, 1);
    assert!(why.contains("not answering"));
}

#[test]
fn every_wait_looks_at_least_once() {
    // A zero-length patience is still one look. The alternative — reporting a
    // timeout having never asked — is a failure that says nothing about the
    // tree and reads exactly like one that does.
    let clock = FakeClock::new();
    let waited = until(
        Goal::Present,
        Patience {
            limit: Duration::ZERO,
            interval: Duration::ZERO,
        },
        &clock,
        script(vec![Ok(found())]),
    );
    assert!(waited.reached().is_some(), "{waited}");
}
