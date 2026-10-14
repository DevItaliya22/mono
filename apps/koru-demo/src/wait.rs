// SPDX-License-Identifier: MPL-2.0

//! **Waiting for a control to be in a state, without sleeping through a hang.**
//!
//! A tree changes on the window's next pass, not on the client's. So a runner
//! that asks once is a runner that fails intermittently — which is the exact
//! shape `guide/contributing/deps/ACCESSKIT.md` records for the placeholder root, and every
//! interactive proof in the tree carries its own copy of the same loop.
//!
//! This is that loop, with two things the hand-written ones do not have.
//!
//! **The deadline is not the only way out.** A wait ends when the goal is
//! reached, when the deadline passes, when the run is cancelled, or when the
//! supervisor gives up on a wedged apartment — and the last two arrive from
//! another thread, through [`crate::supervise::CancelToken`], rather than by
//! being noticed on the next pass. A wait whose only exit was its own deadline
//! would be exactly the instrument-inside-the-failure that
//! [PRINCIPLES.md](../../../guide/contributing/PRINCIPLES.md) warns about, because the pass
//! that would notice the deadline is the pass that is blocked.
//!
//! **The clock is injected.** [`Clock`] is a trait with a fake behind it, so the
//! timeout and cancellation branches are ordinary tests that finish instantly
//! and never sleep. A `Duration` in a test is a machine-specific number that
//! gets relaxed until it asserts nothing.
//!
//! ## What a wait reports when it fails
//!
//! The last thing it saw. *Not found for twenty seconds* and *found, but
//! disabled, for twenty seconds* are different failures and send somebody to
//! different places — the first to the locator or the fixture, the second to
//! whether the workflow really reached the state the case assumed.

use std::fmt;
use std::time::{Duration, Instant};

use crate::select::Selection;

/// Time, so a test does not have to spend any.
pub trait Clock {
    fn now(&self) -> Instant;
    /// Wait between attempts. A fake does not.
    fn rest(&self, interval: Duration);
}

/// The real one.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn rest(&self, interval: Duration) {
        std::thread::sleep(interval);
    }
}

/// How long, and how often to look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Patience {
    pub limit: Duration,
    pub interval: Duration,
}

impl Default for Patience {
    fn default() -> Self {
        Self {
            // The same twenty seconds every interactive proof in the tree
            // budgets, and for the same reason: the first `WM_GETOBJECT`
            // against a window starts UI Automation itself.
            limit: Duration::from_secs(20),
            // 50 ms is what the proofs poll the placeholder root at. Faster
            // buys nothing — the tree changes on a window pass — and costs a
            // cross-process call each time.
            interval: Duration::from_millis(50),
        }
    }
}

/// What the wait is waiting for.
///
/// Stated as a predicate over a [`Selection`] rather than over an element, so
/// *absence* is a goal a wait can reach. It has to be: a screen that has been
/// retired is proved gone by its control no longer being findable, which is the
/// assertion `accessibility_onboarding_native.rs` makes and the only way to see
/// screen retirement from outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal {
    /// Exactly one match, meeting the requirements the search was given.
    Present,
    /// No match at all — a control that has gone, rather than one that is
    /// merely disabled.
    Absent,
}

impl Goal {
    fn reached(self, selection: &Selection) -> bool {
        match self {
            Self::Present => matches!(selection, Selection::One(_)),
            Self::Absent => matches!(selection, Selection::NotFound),
        }
    }
}

/// Why a look could not be taken at all.
///
/// Distinct from *the control was not there*: a wedged apartment or a cancelled
/// run is not evidence about the tree, and a wait that treated it as "not yet"
/// would sit out its whole deadline reporting the wrong thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Interrupted {
    Cancelled,
    Wedged(String),
}

/// One look.
pub type Look = Result<Selection, Interrupted>;

/// How a wait ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Waited {
    Reached(Selection),
    /// The deadline passed. `last` is what the final look saw, which is the
    /// part somebody reads.
    TimedOut {
        goal: Goal,
        looks: u32,
        last: Selection,
    },
    Cancelled {
        goal: Goal,
        looks: u32,
    },
    Wedged {
        goal: Goal,
        looks: u32,
        why: String,
    },
}

impl fmt::Display for Waited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reached(selection) => write!(f, "{selection}"),
            Self::TimedOut { goal, looks, last } => write!(
                f,
                "waited for the control to be {goal:?} and looked {looks} times; the last look \
                 said: {last}"
            ),
            Self::Cancelled { goal, looks } => {
                write!(
                    f,
                    "cancelled while waiting for {goal:?}, after {looks} looks"
                )
            }
            Self::Wedged { goal, looks, why } => {
                write!(f, "gave up waiting for {goal:?} after {looks} looks: {why}")
            }
        }
    }
}

impl Waited {
    pub fn reached(&self) -> Option<&Selection> {
        match self {
            Self::Reached(selection) => Some(selection),
            _ => None,
        }
    }
}

/// Look until the goal is reached, the deadline passes, or somebody stops it.
///
/// The first look is taken **before** any rest, so a control that is already in
/// the wanted state costs nothing. Every wait ends having looked at least once,
/// which is what makes `looks` in the failure a number somebody can reason
/// about: zero would mean the deadline was already gone when the wait started.
pub fn until(
    goal: Goal,
    patience: Patience,
    clock: &dyn Clock,
    mut look: impl FnMut() -> Look,
) -> Waited {
    let deadline = clock.now() + patience.limit;
    let mut looks = 0;
    loop {
        looks += 1;
        let seen = match look() {
            Ok(selection) => selection,
            Err(Interrupted::Cancelled) => return Waited::Cancelled { goal, looks },
            Err(Interrupted::Wedged(why)) => return Waited::Wedged { goal, looks, why },
        };
        if goal.reached(&seen) {
            return Waited::Reached(seen);
        }
        // The deadline is examined *after* a look, never before one, which is
        // what makes `last` the thing that was actually seen rather than a
        // placeholder nobody looked for.
        if clock.now() >= deadline {
            return Waited::TimedOut {
                goal,
                looks,
                last: seen,
            };
        }
        clock.rest(patience.interval);
    }
}

#[cfg(test)]
mod tests;
