// SPDX-License-Identifier: MPL-2.0

//! **What a run pill says**, and the thread that finds out (D8, D16).
//!
//! The pill stores one thing: the Run document's id. Everything a person reads
//! off it — *Working…*, *Finished*, and on any device that is not executing
//! the run, the name of the device that is — is **projected from the Run
//! document by that id**, never stored beside it. That is the whole reason a
//! pill cannot go stale or disagree with the run it names.
//!
//! **Read from the Run document, not from the runs transport.** The header a
//! pill needs is in the document itself, so a device that is not executing the
//! run — the very device D16 asks to name the executor on, which has no
//! delegate transport of its own — reads the same answer as the one that is.
//!
//! **Nothing here runs on the window thread.** Fetching a snapshot is a
//! request over the gateway, so it goes to a worker of its own; the window
//! keeps a resolved caption per run and draws that. A pill whose run has not
//! been resolved yet draws the label the delegating gesture stored, which is
//! also exactly what an older build draws — not a broken pill, because a run
//! that cannot be reached from here may simply live on a device this one has
//! not heard from yet.
//!
//! **An idle surface asks nothing.** The worker is started only while a run
//! pill that is *not* in a terminal state is on screen, and a run that has
//! finished, failed or been cancelled is never asked about again: nothing can
//! move it. So a document full of finished delegations costs one request each,
//! once, and then nothing for ever.
//!
//! **What the run produced rides along.** A right-click on a pill lists the
//! run's results and opens the one that is picked as a tab (ADR-369), and
//! that list is already in the header this worker reads — so the menu is
//! built from what is in hand and **nothing reads the artifact database on
//! the window thread**. A settled run is read once and its results are final,
//! which is why one read is enough.
//!
//! **A canvas's runs are read here too, not by a second poller.** An agent
//! node on a canvas shows the same run a pill would: its runs join the set
//! this worker reads (`MainView::maintain_run_pills`), and the same read
//! keeps the newest step and the elapsed time a node shows beside the word
//! (`super::super::canvas_runs`). A `delegate.*` event about one of them
//! [nudges](RunPills::nudge) the next read forward; the three-second clock is
//! the fallback when no event arrives.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use koru_delegate_core::{ResultReference, RunFailureReason, RunStatus, StepKind};
use koru_device::DeviceId;
use koru_i18n::Key as I18nKey;

/// How often a run that is still moving is asked about again.
///
/// Slow on purpose: a pill is a word in a paragraph, not a progress bar, and
/// the run's own views are where somebody watching a run watches it.
const REFRESH: Duration = Duration::from_secs(3);

/// One run, as the pill needs it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct RunPillState {
    /// The run itself. The pill stores the *document*, because that is what
    /// an attribute reference names; opening the run needs the id the run
    /// views are keyed by, and the header this state was read from carries
    /// it — so the mapping is made once, here, rather than by a route that
    /// does not exist.
    pub(crate) run: koru_delegate_core::RunId,
    pub(crate) status: RunStatus,
    /// The device the run is executing on, when the header names one. An id:
    /// **a name is resolved at display time**, because a device's name is a
    /// thing a person changes and an identity never derives from one.
    pub(crate) device: Option<DeviceId>,
    /// Every result the run has registered, in registration order — the rows
    /// of the pill's right-click menu and a canvas node's results. The
    /// header's own references: what each is called, how big it is, and
    /// **which device holds the bytes** (D27) — an id, resolved to a name at
    /// display time. A title's `Debug` is redacted (standing requirement 7).
    pub(crate) results: Vec<ResultReference>,
    /// **The newest step of the run's history**, when it has one — what a
    /// canvas node says the run is doing. A step kind is content-free (D45)
    /// and changes once per agent turn, so reading it from the document this
    /// worker already fetched costs no second request.
    pub(crate) latest_step: Option<StepKind>,
    /// How long the run has worked (§17.2), up to the moment it was read.
    /// `None` until it has started.
    pub(crate) elapsed_ms: Option<u64>,
    /// Why it failed, when it failed and kept a content-free cause.
    pub(crate) failure_reason: Option<RunFailureReason>,
}

/// How many of the newest timeline entries are looked through for the newest
/// *step*: usage facts sit between steps, one or two per turn.
const STEP_WINDOW: usize = 8;

impl RunPillState {
    /// **Everything a pill or a canvas node says about a run, from its
    /// document** — the one projection both read. Pure: the worker fetches
    /// and decodes, and this reads what was decoded.
    ///
    /// The elapsed time of a run still moving is folded up to `now_ms`, since
    /// the header's copy is refreshed only at the end of a turn; a settled
    /// run's header already holds its final figure.
    pub(crate) fn of(
        run: &koru_delegate_document::RunDocument,
        header: &koru_delegate_core::RunHeader,
        now_ms: i64,
    ) -> Self {
        let latest_step = run
            .timeline_window(0, STEP_WINDOW)
            .iter()
            .find_map(|item| item.step().map(|entry| entry.kind));
        let elapsed_ms = match header.status.is_terminal() {
            true => header.economics.elapsed_ms,
            false => run.economics(now_ms).elapsed_ms,
        };
        Self {
            run: header.run,
            status: header.status.clone(),
            device: header.executing_device(),
            results: results_of(header),
            latest_step,
            elapsed_ms,
            failure_reason: header.failure_reason,
        }
    }
}

impl RunPillState {
    /// Whether nothing can move this run again.
    fn settled(&self) -> bool {
        self.status.is_terminal()
    }

    /// **What the pill says**, in this language, on this device.
    ///
    /// **D16**: the executing device is named when it is not this one. On the
    /// machine actually running the work, saying so would be noise — the
    /// person is looking at it. A device this window cannot name — one it has
    /// not been introduced to — leaves the state alone rather than showing a
    /// uuid, because an unfamiliar id tells a person nothing.
    pub(crate) fn caption(
        &self,
        lang: koru_i18n::Lang,
        here: Option<DeviceId>,
        name_of: impl Fn(DeviceId) -> Option<String>,
    ) -> String {
        // The word is `koru_runs_ui::summary`'s, so a pill and a canvas node
        // showing the same run cannot say different things.
        let state = koru_runs_ui::summary(lang, &self.status, None, None).status;
        let state = state.as_str();
        let elsewhere = self
            .device
            .filter(|device| Some(*device) != here)
            .and_then(name_of);
        match elsewhere {
            Some(name) => koru_i18n::fill(
                koru_i18n::t(lang, I18nKey::DelegateRunOnDevice),
                &[state, &name],
            ),
            None => state.to_owned(),
        }
    }
}

/// What a run has registered, as the pill's menu needs it.
///
/// The run's own header, unchanged — **no second read**. The list is already
/// there because a run records what it delivered (D14), and reading it again
/// from the artifact database would be a second answer to the same question
/// with a chance of disagreeing.
pub(crate) fn results_of(header: &koru_delegate_core::RunHeader) -> Vec<ResultReference> {
    header.artifacts.clone()
}

/// One run resolved, on its way back to the window.
pub(crate) struct Resolved {
    /// The Run document's storage id — the pill's entity key.
    pub(crate) document: String,
    pub(crate) state: RunPillState,
}

/// The wall clock, for folding a moving run's elapsed time on the worker.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// The worker that answers "how is this run going".
#[derive(Default)]
pub(crate) struct RunPills {
    /// The answers of the request in flight, if there is one.
    inflight: Option<Receiver<Vec<Resolved>>>,
    /// When the next request may be made. `None` means "as soon as something
    /// is asked for" — the first pill on screen resolves immediately.
    next_due: Option<Instant>,
    /// **Something said a run moved**: the next read goes out without
    /// waiting for [`REFRESH`] — at once, or the moment the read in flight
    /// has answered. The event bus is the prompt; the clock is the fallback.
    nudged: bool,
}

impl RunPills {
    /// Ask about every run that is on screen and not settled.
    ///
    /// `wanted` maps the pill's entity key (the Run document's storage id) to
    /// the run it names. Answers `true` when a request went out.
    pub(crate) fn refresh(
        &mut self,
        now: Instant,
        client: Option<&koru_api_client::Client>,
        workspace: koru_store_core::ids::WorkspaceId,
        wanted: &HashMap<String, koru_store_core::ids::DocumentId>,
        wake: Option<winit::event_loop::EventLoopProxy<koru_workspace_host::app::Wake>>,
    ) -> bool {
        if wanted.is_empty() || self.inflight.is_some() {
            return false;
        }
        if !self.nudged && self.next_due.is_some_and(|due| now < due) {
            return false;
        }
        let Some(client) = client else {
            // No store client on this window: the pills keep their stored
            // labels, which is the honest answer and not an error.
            return false;
        };
        let client = client.clone();
        let asking: Vec<(String, koru_store_core::ids::DocumentId)> = wanted
            .iter()
            .map(|(key, run)| (key.clone(), *run))
            .collect();
        let (send, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("koru-run-pills".into())
            .spawn(move || {
                let mut answers = Vec::new();
                for (document, run) in asking {
                    // **The Run document itself**, not the runs transport.
                    // Two reasons, and the second is the one that matters: it
                    // is the same answer wherever it is read, and a device that
                    // is *not* executing the run has no delegate transport to
                    // ask — which is exactly the device D16 asks to name the
                    // executor on.
                    let Ok(bytes) = client.snapshot(workspace, run) else {
                        continue;
                    };
                    let Ok(run) = koru_delegate_document::RunDocument::from_snapshot(&bytes) else {
                        continue;
                    };
                    let Ok(header) = run.header() else {
                        continue;
                    };
                    answers.push(Resolved {
                        document,
                        state: RunPillState::of(&run, &header, now_ms()),
                    });
                }
                // Queue first, wake second: the other order lets the loop find
                // an empty channel and sleep past the answer.
                if send.send(answers).is_ok()
                    && let Some(proxy) = wake
                {
                    let _ = proxy.send_event(koru_workspace_host::app::Wake::Store);
                }
            });
        if spawned.is_err() {
            return false;
        }
        self.inflight = Some(receiver);
        self.next_due = Some(now + REFRESH);
        self.nudged = false;
        true
    }

    /// **Read again soon, not in three seconds.** A run on screen moved —
    /// the event bus said so — and waiting out the clock would show the old
    /// state for no reason. Costs nothing while nothing is on screen: an
    /// empty set still asks nothing.
    pub(crate) fn nudge(&mut self) {
        self.nudged = true;
    }

    /// The worker's answers, if it has any. Never blocks.
    pub(crate) fn take(&mut self) -> Option<Vec<Resolved>> {
        let answers = self.inflight.as_ref()?.try_recv().ok()?;
        self.inflight = None;
        Some(answers)
    }

    /// When the window should look again, while runs on screen are still
    /// moving. `None` when nothing is in flight and nothing is due — an idle
    /// surface contributes no clock.
    pub(crate) fn deadline(&self, anything_unsettled: bool) -> Option<Instant> {
        if self.inflight.is_some() {
            return Some(Instant::now() + Duration::from_millis(16));
        }
        anything_unsettled.then(|| match self.nudged {
            true => Instant::now(),
            false => self.next_due.unwrap_or_else(Instant::now),
        })
    }
}

/// Which of the runs on screen still have something to say.
pub(crate) fn unsettled(
    wanted: &HashMap<String, koru_store_core::ids::DocumentId>,
    known: &HashMap<String, RunPillState>,
) -> HashMap<String, koru_store_core::ids::DocumentId> {
    wanted
        .iter()
        .filter(|(key, _)| !known.get(*key).is_some_and(RunPillState::settled))
        .map(|(key, run)| (key.clone(), *run))
        .collect()
}

#[cfg(test)]
mod tests;
