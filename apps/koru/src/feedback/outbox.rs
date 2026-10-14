// SPDX-License-Identifier: MPL-2.0

//! The pending feedback send, owned by the process rather than by the view.
//!
//! **A send outlives the surface that started it.** Send draws a thank-you
//! page and the person closes it; the tab — or the tray's own feedback window —
//! is gone while the POST is still in flight. A receiver living on the view
//! would deliver that failure to nobody. So the in-flight send, the means to
//! retry it and the draft that would restore the form live here, and whichever
//! window is up drains it.
//!
//! **One automatic retry, then the person decides.** The route has no
//! idempotency key and rate-limits three sends per ten minutes per address, so
//! a POST that failed *after* the server accepted it must not be replayed
//! forever — a second copy of somebody's report is worse than a toast asking
//! them to press Send again.
//!
//! Memory only, like [`super::draft`]: quitting Koru forgets an unsent report.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::draft::Draft;
use super::submit::{self, Payload, SubmitError};

/// How long a failed send waits before its one automatic retry. Long enough
/// for a flapping connection to come back, short enough that somebody watching
/// the thank-you page never learns there was a first attempt.
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// Attempts in total: the first send, plus the one automatic retry.
const MAX_ATTEMPTS: u32 = 2;

/// How often a send in flight is looked at. A background POST is not latency
/// critical, and this is the shell's wake interval while one is running, so it
/// is deliberately not a frame.
const POLL: Duration = Duration::from_millis(50);

type Answer = Receiver<Result<(), SubmitError>>;

/// What draining the outbox produced.
pub enum Outcome {
    /// Delivered. Nothing is shown — the thank-you page already said so.
    Sent,
    /// Every attempt failed. The draft comes back so the form can be restored
    /// exactly as it was written.
    Failed { draft: Box<Draft> },
}

enum State {
    /// A worker is running; this is its answer channel.
    InFlight(Answer),
    /// An attempt failed and the retry is due at this instant.
    Waiting(Instant),
}

struct Pending {
    /// Starts one attempt. Held rather than the url and payload because the
    /// retry must reach the network the same way the first attempt did, and
    /// because it is the seam tests replace.
    start: Box<dyn FnMut() -> Answer + Send>,
    draft: Draft,
    attempts: u32,
    state: State,
}

#[cfg(not(test))]
mod slot {
    use super::Pending;
    use std::sync::{Mutex, MutexGuard};

    static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

    pub fn lock() -> MutexGuard<'static, Option<Pending>> {
        PENDING.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[cfg(test)]
mod slot {
    use super::Pending;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static PENDING: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();

    pub fn lock() -> MutexGuard<'static, Option<Pending>> {
        PENDING
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// Answers every post with success and touches nothing.
///
/// **The seam that keeps a test build off the network.** Three view tests once
/// called [`super::FeedbackView::send`] to check that the thank-you page
/// appears, and each posted its sample report to the production route — the
/// team was mailed "The tab would not close" three times over. Pressing Send
/// in a test was the mistake; a `send` that could only ever reach the real
/// endpoint is what turned it into outgoing mail.
struct Offline;

impl submit::Transport for Offline {
    fn post(&self, _url: &str, _payload: &Payload) -> Result<(), SubmitError> {
        Ok(())
    }
}

/// Begin a send. Replaces anything already pending: the form cannot be open
/// twice, so a second Send is the same report being tried again.
pub fn send(url: String, payload: Payload, draft: Draft, include_logs: bool) {
    let start = move || {
        let prepare = move |payload: &mut Payload| {
            if include_logs {
                payload.logs_zip = super::collect_logs_zip();
            }
        };
        // **`cfg!`, not `#[cfg]`.** Both arms compile either way, so the real
        // transport cannot rot unnoticed in a test build and the offline one
        // cannot rot in a release — and no test can reach the network however
        // it arrives here.
        if cfg!(test) {
            submit::spawn_prepared(url.clone(), payload.clone(), Offline, prepare)
        } else {
            submit::spawn_prepared(url.clone(), payload.clone(), submit::HttpTransport, prepare)
        }
    };
    begin(Box::new(start), draft);
}

fn begin(mut start: Box<dyn FnMut() -> Answer + Send>, draft: Draft) {
    let rx = start();
    *slot::lock() = Some(Pending {
        start,
        draft,
        attempts: 1,
        state: State::InFlight(rx),
    });
}

/// Whether a send is in flight or waiting to retry.
#[cfg(test)]
pub fn is_pending() -> bool {
    slot::lock().is_some()
}

/// When the shell should next look at the outbox, if at all.
pub fn deadline() -> Option<Instant> {
    match slot::lock().as_ref()?.state {
        State::InFlight(_) => Some(Instant::now() + POLL),
        State::Waiting(at) => Some(at),
    }
}

/// Drive the pending send. Returns what finished, if anything did.
///
/// A dropped channel counts as a failed attempt: the worker could not be
/// spawned, or it died before answering, and either way nothing was delivered.
pub fn poll(now: Instant) -> Option<Outcome> {
    let mut guard = slot::lock();
    let pending = guard.as_mut()?;
    match &pending.state {
        State::Waiting(at) => {
            if now < *at {
                return None;
            }
            pending.attempts += 1;
            let rx = (pending.start)();
            pending.state = State::InFlight(rx);
            None
        }
        State::InFlight(rx) => match rx.try_recv() {
            Ok(Ok(())) => {
                *guard = None;
                Some(Outcome::Sent)
            }
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                if pending.attempts < MAX_ATTEMPTS {
                    pending.state = State::Waiting(now + RETRY_AFTER);
                    return None;
                }
                let draft = pending.draft.clone();
                *guard = None;
                Some(Outcome::Failed {
                    draft: Box::new(draft),
                })
            }
            Err(TryRecvError::Empty) => None,
        },
    }
}

#[cfg(test)]
mod tests;
