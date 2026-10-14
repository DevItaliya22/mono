// SPDX-License-Identifier: MPL-2.0

//! The worker lifecycle as a pure value: events in, actions out.
//!
//! Nothing here spawns, sleeps, or touches a handle — the supervisor's
//! control thread owns all of that and feeds this machine what happened.
//! That split is what makes the contract testable: every ordering the plan
//! names (stop beats backoff, no restart inside an update window, an abort
//! that resumes, a budget that exhausts visibly) is a unit test on values.
//!
//! One vocabulary note: **hold** is the update window. `hold` suppresses
//! restart while leaving a live child running — the host is preparing to
//! shut down, and replacing it mid-preparation would be the supervisor
//! fighting the update. `resume` is the aborted update's exit: back to
//! `Ready` when the child survived, a fresh spawn when it did not. A plain
//! `stop` ends the child and the supervision both.

use std::time::{Duration, Instant};

/// Where the worker stands. `Backoff` and `Exhausted` carry no child;
/// `Stopping` may (an update hold) or may not (a stop in progress).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// A child is spawned and its readiness probe has not yet passed.
    Starting,
    /// The child answered its readiness probe and is being health-checked.
    Ready,
    /// The child exited; a respawn is scheduled after a delay.
    Backoff,
    /// Restart is suppressed: an intentional stop or an update hold.
    Stopping,
    /// The restart budget ran out. Visible, never silent; `resume` retries.
    Exhausted,
    /// Stopped on purpose and reaped. Terminal.
    Stopped,
}

/// What the control thread observed or was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The readiness probe passed for the current child.
    Ready,
    /// The current child exited and was reaped (the waiter's `wait()`
    /// returned). Every kill funnels through this — the machine never
    /// respawns before the old child is provably gone.
    Exited,
    /// The readiness deadline passed or a health probe failed; the child is
    /// alive but not serving.
    Unhealthy,
    /// The scheduled backoff delay elapsed.
    BackoffElapsed,
    /// Stop the worker for good.
    Stop,
    /// Enter the update window: keep the child, suppress restart.
    Hold,
    /// Leave the update window (the update finished aborting).
    Resume,
}

/// What the control thread must now do. At most one per event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Spawn a fresh child (a new generation).
    Spawn,
    /// Kill the current child's job; an `Exited` event will follow.
    Kill,
    /// Ask the role for a graceful stop, then kill on the stop deadline;
    /// an `Exited` event will follow.
    StopChild,
    /// Sleep for the given delay, then feed `BackoffElapsed`.
    ScheduleBackoff(Duration),
}

/// How many crash-triggered respawns are allowed inside a rolling window.
#[derive(Clone, Copy, Debug)]
pub struct RestartBudget {
    pub max_restarts: u32,
    pub window: Duration,
}

/// Exponential delay between a crash and its respawn.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Backoff {
    fn delay(&self, consecutive_failures: u32) -> Duration {
        let doubled = self
            .initial
            .saturating_mul(1u32 << consecutive_failures.min(16));
        doubled.min(self.max)
    }
}

pub struct Machine {
    state: State,
    /// `Stopping` came from `Hold` (child kept) rather than `Stop`.
    holding: bool,
    /// Whether a child process currently exists (spawned and not reaped).
    child_alive: bool,
    budget: RestartBudget,
    backoff: Backoff,
    /// Crash-triggered respawn instants inside the budget window.
    respawns: Vec<Instant>,
    /// Crashes since the last successful `Ready`, for backoff growth.
    consecutive_failures: u32,
}

impl Machine {
    /// A machine whose first child has just been spawned.
    pub fn starting(budget: RestartBudget, backoff: Backoff) -> Self {
        Self {
            state: State::Starting,
            holding: false,
            child_alive: true,
            budget,
            backoff,
            respawns: Vec::new(),
            consecutive_failures: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn child_alive(&self) -> bool {
        self.child_alive
    }

    /// Feed one observation; do what comes back.
    pub fn on(&mut self, event: Event, now: Instant) -> Action {
        match (self.state, event) {
            // ------------------------------------------------ the happy path
            (State::Starting, Event::Ready) => {
                self.state = State::Ready;
                self.consecutive_failures = 0;
                Action::None
            }

            // ------------------------------------------------------- crashes
            (State::Starting | State::Ready, Event::Exited) => {
                self.child_alive = false;
                self.crash(now)
            }
            (State::Starting | State::Ready, Event::Unhealthy) => {
                // Alive but not serving: end it. The `Exited` that follows
                // the kill is what schedules the respawn, so a replacement
                // can never race a corpse that was not yet reaped.
                Action::Kill
            }
            (State::Backoff, Event::BackoffElapsed) => {
                self.state = State::Starting;
                self.child_alive = true;
                Action::Spawn
            }

            // ------------------------------------------------- stop for good
            (State::Starting | State::Ready, Event::Stop) => {
                self.state = State::Stopping;
                self.holding = false;
                Action::StopChild
            }
            (State::Backoff | State::Exhausted, Event::Stop) => {
                // Stop beats a pending backoff: cancel the respawn outright.
                self.state = State::Stopped;
                self.holding = false;
                Action::None
            }
            (State::Stopping, Event::Stop) if self.holding => {
                // A stop arriving during an update hold ends the held child.
                self.holding = false;
                if self.child_alive {
                    Action::StopChild
                } else {
                    // **There is nothing to stop, so the stop is finished.**
                    // The child died inside the hold — it crashed, or it was
                    // never respawned because the hold suppressed the backoff.
                    // Emitting `StopChild` here was the bug: the control
                    // thread found neither payload nor live handle, did
                    // nothing, no `Exited` ever followed, `Stopped` was never
                    // reached, the acknowledgement timed out, and the public
                    // stop's control-thread join then blocked for good. A
                    // `Resume` afterwards fell through to `Action::None`, so
                    // an aborted update could not restore service either.
                    //
                    // Reaching `Stopped` directly is what the caller is
                    // waiting for, and the supervisor reports the *absence*
                    // and the previous exit rather than a graceful stop —
                    // **absence of a child is never evidence of a clean Store
                    // join.**
                    self.state = State::Stopped;
                    Action::None
                }
            }
            (State::Stopping, Event::Exited) if !self.holding => {
                self.child_alive = false;
                self.state = State::Stopped;
                Action::None
            }

            // -------------------------------------------- the update window
            (State::Starting | State::Ready, Event::Hold) => {
                self.state = State::Stopping;
                self.holding = true;
                Action::None
            }
            (State::Backoff | State::Exhausted, Event::Hold) => {
                // Nothing is running; the hold still suppresses the respawn.
                self.state = State::Stopping;
                self.holding = true;
                Action::None
            }
            (State::Stopping, Event::Exited) if self.holding => {
                // The child died inside the update window. No restart here —
                // an update may be replacing the executable. Resume decides.
                self.child_alive = false;
                Action::None
            }
            (State::Stopping, Event::Resume) if self.holding => {
                self.holding = false;
                if self.child_alive {
                    self.state = State::Ready;
                    Action::None
                } else {
                    self.state = State::Starting;
                    self.child_alive = true;
                    self.respawns.clear();
                    self.consecutive_failures = 0;
                    Action::Spawn
                }
            }
            (State::Exhausted | State::Stopped, Event::Resume) => {
                // An explicit resume is a person or the Agent deciding to
                // serve again — out of an exhausted budget, or after a stop
                // whose reason went away (an install that stopped the child
                // and then aborted). Either way the budget starts over, so
                // an aborted update can never leave the service suppressed.
                self.state = State::Starting;
                self.child_alive = true;
                self.respawns.clear();
                self.consecutive_failures = 0;
                Action::Spawn
            }

            // ----------------------------------------------------- leftovers
            // A stale timer or probe for a child that no longer matters.
            _ => Action::None,
        }
    }

    fn crash(&mut self, now: Instant) -> Action {
        self.consecutive_failures += 1;
        self.respawns
            .retain(|instant| now.duration_since(*instant) < self.budget.window);
        if self.respawns.len() as u32 >= self.budget.max_restarts {
            self.state = State::Exhausted;
            return Action::None;
        }
        self.respawns.push(now);
        self.state = State::Backoff;
        Action::ScheduleBackoff(self.backoff.delay(self.consecutive_failures - 1))
    }
}

#[cfg(test)]
mod tests;
