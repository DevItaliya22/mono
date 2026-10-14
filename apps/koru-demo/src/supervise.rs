// SPDX-License-Identifier: MPL-2.0

//! **Getting control back from a call that will never return.**
//!
//! A UI Automation call is a cross-process call into a provider that may be
//! mid-freeze, mid-crash, or waiting on a window loop that is itself waiting on
//! something else. There is no cancel. `IUIAutomation2::put_TransactionTimeout`
//! exists and is a *request* to a provider that is by hypothesis not answering
//! requests. So the only thing that is actually true is: **the thread that made
//! the call may never come back, and nothing you do on that thread can change
//! it.**
//!
//! Which is why the plan says *prove bounded regain of control rather than
//! relying on a polling deadline*, and why
//! [PRINCIPLES.md](../../../guide/contributing/PRINCIPLES.md) says the same thing twice:
//!
//! > **A guard runs off the thread it guards.** The handoff's 120 s no-progress
//! > abort ran on the loop it was meant to outlive, so a blocked loop silenced
//! > its own alarm.
//!
//! > **The instrument must not travel inside the thing that is failing.**
//!
//! A deadline checked *by the caller between calls* is exactly the failure
//! those describe: it is inside the thing that is failing, and a call that never
//! returns never reaches the check.
//!
//! ## The shape
//!
//! Every UIA call happens on a **worker thread** that owns the COM apartment,
//! the `IUIAutomation` and every element. The runner never touches them. It
//! sends a [`Request`] down a channel and blocks on the reply with a budget.
//!
//! When the budget runs out, the supervisor does **not** try to stop the worker,
//! and does **not** join it. It *abandons* it: drops its end of the channel and
//! forgets it exists. Control is back on the runner's thread in bounded time
//! whatever the provider does, because the thing being waited on is a channel
//! this process owns rather than a provider in somebody else's.
//!
//! Three properties follow, and each is a test in `tests.rs`:
//!
//! - **The next call gets a fresh worker.** A wedged provider costs one worker,
//!   not the run.
//! - **The abandoned worker's answer is never used.** Its reply channel is gone,
//!   and every [`crate::inspect::Handle`] it minted carries its generation, so a
//!   handle that outlived it is refused rather than answered about whatever
//!   element the new worker has in that slot.
//! - **An abandoned worker retires itself.** If the provider eventually returns,
//!   the worker's send fails, its next `recv` finds a closed channel and the
//!   thread exits. So the leak is bounded by *how many providers are stuck at
//!   once*, not by how long the run is — and [`Limits::max_abandoned`] caps even
//!   that, because a run that has wedged four apartments is a run that should
//!   say so rather than open a fifth.
//!
//! ## Cancellation is the same mechanism
//!
//! [`CancelToken`] does not set a flag for somebody to poll. It holds the sender
//! for whichever call is waiting and *sends into it*, so the supervisor's
//! `recv_timeout` returns immediately — from another thread, while the call it
//! was waiting for is still running. Polling would put the instrument back
//! inside the failing thing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::inspect::{Request, Response};
use koru_windows_input::DeliveryPermit;

/// How long a single call may take, and how many wedged apartments a run
/// tolerates before it stops opening more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub budget: Duration,
    pub max_abandoned: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Generous on purpose. The first `WM_GETOBJECT` against a window
            // starts UI Automation itself on this machine, and the four
            // interactive proofs in the tree all budget twenty seconds for that
            // one call. A budget tight enough to be "responsive" would abandon
            // healthy workers during a cold attach.
            budget: Duration::from_secs(20),
            // Four, because the number that matters is *how many providers are
            // stuck at the same time*, and a run that has reached four has
            // found something worth reporting rather than working around.
            max_abandoned: 4,
        }
    }
}

/// What a call came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Answered(Response),
    /// The budget ran out. The call is **still running** somewhere; nothing was
    /// cancelled, and nothing may be assumed about what it did or will do.
    Lost,
    Cancelled,
    /// A worker could not be started at all.
    Unavailable(String),
    /// Too many apartments are wedged at once. Nothing was attempted.
    Wedged {
        abandoned: usize,
    },
}

/// The platform side, seen from here: something that answers questions and may
/// stop doing so.
///
/// **Deliberately not `Send`.** It is constructed *on* the worker thread and
/// never leaves it, which is not a limitation to work around — it is the rule
/// COM imposes: an apartment belongs to the thread that initialised it, and so
/// does every interface pointer reachable from it. A `Send` bound here would
/// compile and would be a lie the type system had agreed to.
///
/// What crosses the thread boundary is [`Request`] and [`Response`], which are
/// plain data. That is the whole seam.
pub trait Mechanism: 'static {
    fn perform(&mut self, request: Request) -> Response;
    fn perform_guarded(&mut self, request: Request, _permit: &DeliveryPermit) -> Response {
        self.perform(request)
    }
}

/// Makes one, on the worker thread, for the reason above.
///
/// The generation is handed in rather than kept here because the mechanism is
/// what stamps [`crate::inspect::Handle`]s, and a handle whose generation came
/// from anywhere but the apartment that minted it would not be the fence it is
/// supposed to be.
pub type Start = Arc<dyn Fn(u64) -> Result<Box<dyn Mechanism>, String> + Send + Sync>;

/// Wakes a waiting call from another thread.
///
/// Clone it and hand it to whatever might want to stop the run — a signal
/// handler, a deadline thread, a person's Ctrl+C. `cancel` is idempotent and a
/// token that has been cancelled stays cancelled: a run that was told to stop
/// does not resume because the next call happened to start after it.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<Mutex<CancelState>>);

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    waiting: Vec<(u64, Sender<Reply>, DeliveryPermit)>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop whatever is waiting now, and everything after it.
    pub fn cancel(&self) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        state.cancelled = true;
        for (_, waiter, permit) in state.waiting.drain(..) {
            permit.revoke();
            // The receiver may already be gone: the call it belonged to could
            // have been answered a moment ago. That is not an error.
            let _ = waiter.send(Reply::Cancelled);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.lock().is_ok_and(|state| state.cancelled)
    }

    /// Register a waiter. `false` means the token was already cancelled and no
    /// call should be started.
    fn arm(&self, id: u64, waiter: Sender<Reply>, permit: DeliveryPermit) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        if state.cancelled {
            return false;
        }
        state.waiting.push((id, waiter, permit));
        true
    }

    fn disarm(&self, id: u64) {
        if let Ok(mut state) = self.0.lock() {
            state.waiting.retain(|(waiting, _, _)| *waiting != id);
        }
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CancelToken(cancelled: {})", self.is_cancelled())
    }
}

enum Reply {
    Answered(Response),
    Cancelled,
    Unavailable(String),
}

struct Job {
    permit: DeliveryPermit,
    request: Request,
    reply: Sender<Reply>,
}

/// One live apartment.
struct Worker {
    jobs: Sender<Job>,
}

/// Counts threads that are alive, so an abandoned one that finally returns
/// gives its budget back.
///
/// A plain counter rather than a join handle, because joining is the one thing
/// this module may never do.
#[derive(Clone, Default)]
struct Census(Arc<AtomicUsize>);

impl Census {
    fn mark(&self) -> Mark {
        self.0.fetch_add(1, Ordering::SeqCst);
        Mark(Arc::clone(&self.0))
    }

    fn live(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// Held by a worker thread for its whole life; its `Drop` is the only place a
/// thread is reported as gone.
struct Mark(Arc<AtomicUsize>);

impl Drop for Mark {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs every platform call somewhere it can be walked away from.
pub struct Supervisor {
    start: Start,
    limits: Limits,
    worker: Option<Worker>,
    generation: u64,
    calls: u64,
    census: Census,
}

impl Supervisor {
    pub fn new(start: Start, limits: Limits) -> Self {
        Self {
            start,
            limits,
            worker: None,
            generation: 0,
            calls: 0,
            census: Census::default(),
        }
    }

    /// The generation handles minted now will carry.
    ///
    /// Callers do not need it — the worker stamps it — but a report that says
    /// *which* apartment answered is a report somebody can line up against the
    /// abandonments above it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// How many worker threads are alive but no longer listened to.
    pub fn abandoned(&self) -> usize {
        self.census
            .live()
            .saturating_sub(usize::from(self.worker.is_some()))
    }

    /// Ask the platform something, and come back either way.
    pub fn call(&mut self, request: Request, cancel: &CancelToken) -> Outcome {
        if cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        let abandoned = self.abandoned();
        if abandoned >= self.limits.max_abandoned {
            return Outcome::Wedged { abandoned };
        }
        if self.worker.is_none() {
            self.generation += 1;
            self.worker = Some(self.spawn(self.generation));
        }

        self.calls += 1;
        let id = self.calls;
        let (reply, answers) = channel();
        let permit = DeliveryPermit::default();
        if !cancel.arm(id, reply.clone(), permit.clone()) {
            return Outcome::Cancelled;
        }
        let outcome = self.deliver(
            Job {
                request,
                reply,
                permit: permit.clone(),
            },
            &answers,
        );
        permit.revoke();
        cancel.disarm(id);
        outcome
    }

    fn deliver(&mut self, job: Job, answers: &Receiver<Reply>) -> Outcome {
        let Some(worker) = self.worker.as_ref() else {
            return Outcome::Unavailable("no worker".to_owned());
        };
        if worker.jobs.send(job).is_err() {
            // The thread ended on its own. Nothing is stuck, so this costs no
            // abandonment budget; the next call starts a fresh one.
            self.worker = None;
            return Outcome::Lost;
        }
        match answers.recv_timeout(self.limits.budget) {
            Ok(Reply::Answered(response)) => Outcome::Answered(response),
            Ok(Reply::Unavailable(why)) => {
                self.worker = None;
                Outcome::Unavailable(why)
            }
            // The call is still inside the provider. Walk away from the whole
            // apartment: its elements are unreachable from here anyway, and the
            // generation on its handles is what stops its slots being reused.
            Ok(Reply::Cancelled) => {
                self.worker = None;
                Outcome::Cancelled
            }
            Err(RecvTimeoutError::Timeout) => {
                self.worker = None;
                Outcome::Lost
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.worker = None;
                Outcome::Lost
            }
        }
    }

    fn spawn(&self, generation: u64) -> Worker {
        let (jobs, work) = channel::<Job>();
        let start = Arc::clone(&self.start);
        let mark = self.census.mark();
        std::thread::Builder::new()
            .name(format!("koru-demo-uia-{generation}"))
            .spawn(move || {
                // Held for the thread's whole life: the census is decremented
                // exactly when this frame is dropped, whether that is a clean
                // exit or a panic unwinding out of a provider call.
                let _mark = mark;
                let mut mechanism = match start(generation) {
                    Ok(mechanism) => mechanism,
                    Err(why) => {
                        // One job is taken so the waiting call learns why,
                        // rather than sitting out its whole budget for a
                        // mechanism that never existed.
                        if let Ok(job) = work.recv() {
                            let _ = job.reply.send(Reply::Unavailable(why));
                        }
                        return;
                    }
                };
                while let Ok(job) = work.recv() {
                    let response = mechanism.perform_guarded(job.request, &job.permit);
                    // A failed send is the ordinary end of an abandoned worker:
                    // nobody is listening any more, so the next `recv` finds a
                    // closed channel and this thread retires itself.
                    let _ = job.reply.send(Reply::Answered(response));
                }
            })
            .expect("a thread");
        Worker { jobs }
    }
}

impl Drop for Supervisor {
    /// Drops the channel and returns.
    ///
    /// Deliberately written out, because the obvious thing — keeping a
    /// `JoinHandle` and joining it here — is the bug this whole module exists
    /// to avoid. A `Drop` that joined would make tearing the runner down wait
    /// on the provider that was already not answering, which is the one moment
    /// somebody is trying to get their machine back.
    fn drop(&mut self) {
        self.worker = None;
    }
}

#[cfg(test)]
mod delivery_tests;
#[cfg(test)]
mod tests;
