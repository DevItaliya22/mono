// SPDX-License-Identifier: MPL-2.0

//! **One section, kept current, without ever blocking a window.**
//!
//! [`Pending`](super::Pending) and [`watch`](super::SettingsClient::watch) are
//! the two halves a window needs, and every window needs both in the same
//! shape: discover the Gateway, take the bounded wait, apply the defaults
//! meanwhile, then refetch whenever a change fact says the section moved, and
//! send per-field patches for what this window changed. Four apps were about to
//! write that loop four times.
//!
//! **Nothing here runs on the caller's thread.** Discovery, the bounded wait,
//! every read and every patch happen on one worker; the event thread only does
//! a `try_recv` and a `send`. That is the whole reason this type exists rather
//! than a `settings().global()` at a call site: a settings read on the event
//! thread is a settings read that can freeze the app.
//!
//! **Defaults now, correct later.** A link answers
//! [`authoritative`](SectionLink::authoritative) from the moment it is built,
//! and if the Gateway never answers it goes on doing so — S9, never a hang.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use koru_settings::{GlobalPatch, GlobalSettings, Section, TerminalPatch, TerminalSettings};

use super::SettingsResult;
use crate::Client;

#[cfg(test)]
mod tests;

/// The scopes a window that reads *and* writes one section names at connect.
///
/// **Naming scopes filters rather than adds**, so all three are here: reading
/// the section, writing a patch, and the event stream a change fact arrives on.
/// A link keeps its own client, so the caller's Store client is unaffected.
pub const LINK_SCOPES: &[&str] = &[
    koru_settings::SETTINGS_READ_SCOPE,
    koru_settings::SETTINGS_WRITE_SCOPE,
    crate::EVENTS_READ_SCOPE,
];

/// How long the worker keeps trying to reach the Gateway before giving up.
///
/// Longer than the section's own two-second budget on purpose: that budget is
/// about *drawing without waiting*, which the defaults already answer, while
/// this is about a Gateway the Agent may still be starting. A window that takes
/// eight seconds to find one still corrects itself at eight seconds.
const CONNECT_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// How often the worker asks the bounded wait whether it has an answer yet.
const POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// How long a security-sensitive write may wait in this worker before send.
///
/// Expiry is checked by the worker before it gives the request any mutation
/// authority. Once a request is in flight, no caller-side clock may claim it
/// timed out: a late authority answer could otherwise enable a feature after
/// its surface had reported failure.
const CONFIRMED_WRITE_BUDGET: Duration = Duration::from_secs(30);

/// How long a link that lost touch keeps asking again, and how often.
///
/// **A burst, not a clock.** A healthy link blocks on its channel and costs
/// nothing; this arms only after an exchange has actually failed — which means
/// there *was* a Gateway and it went away — and disarms itself afterwards. What
/// covers the long tail is the change subscription, whose own thread reconnects
/// for as long as it lives and delivers the next real change whenever the
/// Gateway comes back.
///
/// The budget is the connect budget, deliberately: both answer "how long is it
/// worth waiting for a supervisor to put a Gateway back", and two numbers for
/// one question drift apart.
const HEAL_BUDGET: Duration = CONNECT_BUDGET;
const HEAL_RETRY: Duration = Duration::from_secs(1);

/// Whether this link is in touch with the authority.
///
/// **The reason it exists**: a section link used to reach a dead Gateway and
/// then sit blocked on its channel forever. Nothing woke it — the watch that
/// would have sent `Refetch` had died with the same Gateway — so a terminal
/// whose Gateway was restarted underneath it went on drawing what it had read
/// at start-up until somebody quit it. What the appearance heals on is a read
/// that succeeds; what this decides is when one is attempted.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Touch {
    /// The last exchange succeeded.
    In,
    /// The last exchange failed, at this moment.
    Lost(Instant),
}

impl Touch {
    /// How long to wait for the next command. `None` blocks forever, which is
    /// what a healthy link does and what a link that has stopped hoping does.
    fn wait(self, now: Instant) -> Option<Duration> {
        match self {
            Self::In => None,
            Self::Lost(at) if now.duration_since(at) < HEAL_BUDGET => Some(HEAL_RETRY),
            Self::Lost(_) => None,
        }
    }

    /// Record a failure. **The moment is the one touch was lost**, not the
    /// moment of the latest failed retry — otherwise every retry would push the
    /// budget out and the burst would never end.
    fn lose(&mut self, now: Instant) {
        if *self == Self::In {
            *self = Self::Lost(now);
        }
    }
}

/// What the caller asks the worker to do.
enum Command<S, P> {
    /// This section changed somewhere — read it again.
    Refetch,
    /// Persist the fields this window moved.
    Patch {
        patch: Box<P>,
        confirmed: Option<ConfirmedRequest<S>>,
        reply: Option<Sender<PatchReply<S>>>,
    },
    /// The window is going away.
    Stop,
}

struct ConfirmedRequest<S> {
    result: Option<Sender<ConfirmedOutcome<S>>>,
    generation: u64,
    newest_generation: Arc<AtomicU64>,
    phase: Arc<AtomicU8>,
    expires_at: Instant,
    notify: Arc<Mutex<Option<Wake>>>,
}

impl<S> ConfirmedRequest<S> {
    fn settle(&mut self, outcome: ConfirmedOutcome<S>) -> bool {
        self.result
            .take()
            .is_some_and(|result| result.send(outcome).is_ok())
    }

    fn wake(&self) {
        if let Some(wake) = self
            .notify
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .as_ref()
        {
            wake();
        }
    }
}

impl<S> Drop for ConfirmedRequest<S> {
    fn drop(&mut self) {
        if self.settle(ConfirmedOutcome::Disconnected) {
            self.wake();
        }
    }
}

const CONFIRMED_QUEUED: u8 = 0;
const CONFIRMED_IN_FLIGHT: u8 = 1;
const CONFIRMED_CANCELLED: u8 = 2;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ConfirmedStart {
    Send,
    FinishStale,
    FinishTimedOut,
    Skip,
}

/// Give a queued confirmed request mutation authority, or settle it before
/// any route call begins. Keeping this decision in one pure seam makes the
/// cancellation/send race deterministic under test.
fn begin_confirmed<S>(request: &ConfirmedRequest<S>, now: Instant) -> ConfirmedStart {
    let stale = request.newest_generation.load(Ordering::Acquire) != request.generation;
    let expired = now >= request.expires_at;
    if stale || expired {
        match request.phase.compare_exchange(
            CONFIRMED_QUEUED,
            CONFIRMED_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                return if stale {
                    ConfirmedStart::FinishStale
                } else {
                    ConfirmedStart::FinishTimedOut
                };
            }
            Err(_) => return ConfirmedStart::Skip,
        }
    }
    match request.phase.compare_exchange(
        CONFIRMED_QUEUED,
        CONFIRMED_IN_FLIGHT,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => ConfirmedStart::Send,
        Err(_) => ConfirmedStart::Skip,
    }
}

/// The three route calls one section needs, so the worker is written once.
///
/// Function pointers rather than a trait: there are exactly two sections a
/// window links to, they differ only in which method they call, and a trait
/// would be three lines of vocabulary for a thing with no second axis.
struct Routes<S, P> {
    section: Section,
    pending: fn(&Client) -> super::Pending<S>,
    read: fn(&Client) -> SettingsResult<S>,
    patch: fn(&Client, &P) -> SettingsResult<S>,
    echoes: fn(&S, &P) -> bool,
}

/// A live view of one settings section.
///
/// Holds the last section the authority confirmed, which is the baseline a
/// patch is computed against: a field that has not moved since is not named,
/// and a field this window never touched is never written at all.
pub struct SectionLink<S, P> {
    updates: Option<Receiver<S>>,
    commands: Option<Sender<Command<S, P>>>,
    authoritative: S,
    notify: Arc<Mutex<Option<Wake>>>,
    /// The newest confirmed intent. A result for an older intent is stale even
    /// when its authority round trip eventually succeeds.
    confirmed_generation: Arc<AtomicU64>,
}

/// Why a confirmed write did not become the durable truth.
///
/// Stable codes only. The authority's developer message may contain machine
/// details and is already recorded by the worker's warning; a surface must
/// localize from this value instead of showing that message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmedFailure {
    /// The authority understood the request and refused it.
    Refused { status: u16, code: String },
    /// The request could not reach, or could not be answered by, the authority.
    Disconnected,
}

/// The terminal, correlated result of one security-sensitive settings write.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmedOutcome<S> {
    /// The authority accepted the patch and echoed this exact section.
    Accepted(S),
    /// The authority explicitly refused, then a re-read recovered current truth.
    Reconciled {
        authoritative: S,
        failure: ConfirmedFailure,
    },
    /// The authority explicitly refused, and reconciliation could not answer.
    Refused { status: u16, code: String },
    /// The authority answered success without echoing the fields requested.
    ContractMismatch { authoritative: S },
    /// The mutation outcome is unknown; no causally final answer was available.
    Disconnected,
    /// A newer confirmed intent superseded this one before it was observed.
    Stale,
    /// The worker did not begin the authority call before its queue deadline.
    /// No mutation was sent.
    TimedOut,
}

/// A nonblocking receipt for one confirmed write.
///
/// It is intentionally not a `Future`: Koru's window and Agent loops are
/// synchronous winit loops, and awaiting here would put network latency on the
/// thread whose job is to answer a person.
#[must_use = "poll the receipt so a security-sensitive mutation is not silently fire-and-forget"]
pub struct ConfirmedWrite<S> {
    result: Option<Receiver<ConfirmedOutcome<S>>>,
    generation: u64,
    newest_generation: Arc<AtomicU64>,
    phase: Arc<AtomicU8>,
    expires_at: Instant,
    done: bool,
}

impl<S> ConfirmedWrite<S> {
    /// The deadline while the mutation is still queued. Once the worker has
    /// begun the authority call, `None` means a caller must wait for its
    /// correlated result rather than inventing a timeout for an in-flight
    /// mutation.
    pub fn deadline(&self) -> Option<Instant> {
        (!self.done && self.phase.load(Ordering::Acquire) == CONFIRMED_QUEUED)
            .then_some(self.expires_at)
    }

    /// Take the result if it is ready. Never waits.
    pub fn poll(&mut self) -> Option<ConfirmedOutcome<S>> {
        self.poll_at(Instant::now())
    }

    fn poll_at(&mut self, now: Instant) -> Option<ConfirmedOutcome<S>> {
        if self.done {
            return None;
        }
        if self.newest_generation.load(Ordering::Acquire) != self.generation {
            let _ = self.phase.compare_exchange(
                CONFIRMED_QUEUED,
                CONFIRMED_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            self.done = true;
            self.result = None;
            return Some(ConfirmedOutcome::Stale);
        }
        if now >= self.expires_at
            && self
                .phase
                .compare_exchange(
                    CONFIRMED_QUEUED,
                    CONFIRMED_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        {
            self.done = true;
            self.result = None;
            return Some(ConfirmedOutcome::TimedOut);
        }
        let Some(result) = self.result.as_ref() else {
            self.done = true;
            return Some(ConfirmedOutcome::Disconnected);
        };
        match result.try_recv() {
            Ok(outcome) => {
                self.done = true;
                self.result = None;
                Some(outcome)
            }
            Err(TryRecvError::Disconnected) => {
                self.done = true;
                self.result = None;
                Some(ConfirmedOutcome::Disconnected)
            }
            Err(TryRecvError::Empty) => None,
        }
    }
}

/// What a link calls when a section is waiting to be taken.
///
/// **A window's event loop sleeps until something wakes it**, so without this
/// an idle window would sit on stale settings until somebody moved the pointer
/// — the exact failure the agent's search overlay had before `maintain` gained
/// a deadline. It is the same shape as the shell's wake proxy: the link does
/// not know what a window is, so the window hands down the one call that makes
/// its loop take another pass.
type Wake = Box<dyn Fn() + Send + 'static>;

/// The authoritative outcome of one particular patch.
///
/// Ordinary controls use [`SectionLink::save`] and reconcile through
/// [`SectionLink::poll`]. A flow that must report whether *its own* write
/// landed uses this receipt instead: a later refetch from another window is
/// not evidence that the requested patch succeeded.
#[derive(Clone, Debug, PartialEq)]
pub enum PatchReply<S> {
    Applied(S),
    Refused {
        authoritative: Option<S>,
        message: String,
    },
}

/// The Global section — theme, caret, fonts, density, zoom, dictation.
pub type GlobalLink = SectionLink<GlobalSettings, GlobalPatch>;
/// The Terminal section — its own theme, font, zoom, cursor and composer.
pub type TerminalLink = SectionLink<TerminalSettings, TerminalPatch>;

// The six one-line adapters the `Routes` pointers name. Written out rather
// than referenced as `SettingsClient::global` directly because a method on a
// borrowing namespace is not a `for<'a, 'b> fn(&'a SettingsClient<'b>)`, and
// the client is the thing the worker actually owns.
fn global_pending(client: &Client) -> super::Pending<GlobalSettings> {
    client.settings().global_when_ready()
}
fn global_read(client: &Client) -> SettingsResult<GlobalSettings> {
    client.settings().global()
}
fn global_patch(client: &Client, patch: &GlobalPatch) -> SettingsResult<GlobalSettings> {
    client.settings().patch_global(patch)
}
fn global_echoes(section: &GlobalSettings, patch: &GlobalPatch) -> bool {
    let mut expected = section.clone();
    patch.clone().apply(&mut expected);
    expected == *section
}
fn terminal_pending(client: &Client) -> super::Pending<TerminalSettings> {
    client.settings().terminal_when_ready()
}
fn terminal_read(client: &Client) -> SettingsResult<TerminalSettings> {
    client.settings().terminal()
}
fn terminal_patch(client: &Client, patch: &TerminalPatch) -> SettingsResult<TerminalSettings> {
    client.settings().patch_terminal(patch)
}
fn terminal_echoes(section: &TerminalSettings, patch: &TerminalPatch) -> bool {
    let mut expected = section.clone();
    patch.clone().apply(&mut expected);
    expected == *section
}

impl GlobalLink {
    /// Open a link to the Global section and return at once.
    pub fn global(segment: String, root: Option<PathBuf>) -> Self {
        Self::open(
            segment,
            root,
            Routes {
                section: Section::Global,
                pending: global_pending,
                read: global_read,
                patch: global_patch,
                echoes: global_echoes,
            },
        )
    }
}

impl TerminalLink {
    /// Open a link to the Terminal section and return at once.
    pub fn terminal(segment: String, root: Option<PathBuf>) -> Self {
        Self::open(
            segment,
            root,
            Routes {
                section: Section::Terminal,
                pending: terminal_pending,
                read: terminal_read,
                patch: terminal_patch,
                echoes: terminal_echoes,
            },
        )
    }
}

impl<S, P> SectionLink<S, P>
where
    S: Default + Clone + Send + 'static,
    P: Send + 'static,
{
    fn open(segment: String, root: Option<PathBuf>, routes: Routes<S, P>) -> Self {
        let (send_update, updates) = std::sync::mpsc::channel();
        let (commands, take_command) = std::sync::mpsc::channel();
        let wake = commands.clone();
        let notify: Arc<Mutex<Option<Wake>>> = Arc::new(Mutex::new(None));
        let theirs = Arc::clone(&notify);
        let spawned = std::thread::Builder::new()
            .name("koru-settings-link".into())
            .spawn(move || {
                serve(
                    segment,
                    root,
                    routes,
                    send_update,
                    take_command,
                    wake,
                    theirs,
                )
            });
        Self {
            updates: spawned.is_ok().then_some(updates),
            commands: spawned.is_ok().then_some(commands),
            authoritative: S::default(),
            notify,
            confirmed_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A link that never connects — for a test, and for a host that must not
    /// reach a real Gateway. It answers the registry's defaults forever.
    pub fn disabled() -> Self {
        Self {
            updates: None,
            commands: None,
            authoritative: S::default(),
            notify: Arc::new(Mutex::new(None)),
            confirmed_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The section the authority last confirmed, or the registry's defaults.
    pub fn authoritative(&self) -> &S {
        &self.authoritative
    }

    /// Install the authoritative section carried by a non-stale confirmed
    /// receipt before its host settles any dependent runtime transaction.
    ///
    /// The receipt can reach the host before the ordinary section broadcast;
    /// ignoring its section would briefly re-apply stale local truth. Callers
    /// must invoke this only for a receipt whose own generation check has
    /// already completed.
    pub fn install_confirmed(&mut self, section: S) {
        self.authoritative = section;
    }

    /// **Never blocks.** `Some` means an authoritative section arrived and the
    /// caller should apply it; `None` means go on drawing.
    pub fn poll(&mut self) -> Option<S> {
        let updates = self.updates.as_ref()?;
        // Drain: several may be queued after a burst, and only the newest is
        // the truth. Applying the older ones would repaint for nothing.
        let mut newest = None;
        loop {
            match updates.try_recv() {
                Ok(section) => newest = Some(section),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.updates = None;
                    break;
                }
            }
        }
        let section = newest?;
        self.authoritative = section.clone();
        Some(section)
    }
}

impl<S, P> SectionLink<S, P>
where
    S: Clone,
    P: Send + 'static,
{
    /// Send a patch, and take it as this window's answer meanwhile.
    ///
    /// **Applied locally first, and whether or not there is anywhere to send
    /// it.** That is what stops the same patch going out on every tick until
    /// the reply lands, and it means a window with no Gateway still shows the
    /// choice its owner just made. The authority's reply then replaces it, so a
    /// refusal reconciles rather than compounds.
    pub fn save(&mut self, patch: P, apply: impl FnOnce(&P, &mut S)) {
        apply(&patch, &mut self.authoritative);
        if let Some(commands) = self.commands.as_ref() {
            let _ = commands.send(Command::Patch {
                patch: Box::new(patch),
                confirmed: None,
                reply: None,
            });
        }
    }

    /// Send a security-sensitive patch without taking it as durable locally.
    ///
    /// The returned receipt completes only with the authority's correlated
    /// echo or an explicit failure. Unlike [`save`](Self::save), this does not
    /// optimistically change [`authoritative`](Self::authoritative): a failed
    /// enable therefore remains off, and a surface can keep a failed disable
    /// visibly pending while stopping its live behavior immediately.
    pub fn save_confirmed(&mut self, patch: P) -> ConfirmedWrite<S> {
        let generation = self
            .confirmed_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let (send_result, result) = std::sync::mpsc::channel();
        let phase = Arc::new(AtomicU8::new(CONFIRMED_QUEUED));
        let expires_at = Instant::now() + CONFIRMED_WRITE_BUDGET;
        let result = self.commands.as_ref().and_then(|commands| {
            commands
                .send(Command::Patch {
                    patch: Box::new(patch),
                    confirmed: Some(ConfirmedRequest {
                        result: Some(send_result),
                        generation,
                        newest_generation: Arc::clone(&self.confirmed_generation),
                        phase: Arc::clone(&phase),
                        expires_at,
                        notify: Arc::clone(&self.notify),
                    }),
                    reply: None,
                })
                .ok()
                .map(|_| result)
        });
        ConfirmedWrite {
            result,
            generation,
            newest_generation: Arc::clone(&self.confirmed_generation),
            phase,
            expires_at,
            done: false,
        }
    }

    /// Send a patch and return a receipt for this exact authority exchange.
    ///
    /// The local value remains optimistic, as with [`Self::save`]. The reply
    /// is separate from the section update stream so an unrelated refetch can
    /// never be mistaken for this patch landing.
    pub fn save_with_reply(
        &mut self,
        patch: P,
        apply: impl FnOnce(&P, &mut S),
    ) -> Receiver<PatchReply<S>> {
        let previous = self.authoritative.clone();
        apply(&patch, &mut self.authoritative);
        let (reply, take_reply) = std::sync::mpsc::channel();
        let sent = self.commands.as_ref().is_some_and(|commands| {
            commands
                .send(Command::Patch {
                    patch: Box::new(patch),
                    confirmed: None,
                    reply: Some(reply.clone()),
                })
                .is_ok()
        });
        if !sent {
            let _ = reply.send(PatchReply::Refused {
                authoritative: Some(previous),
                message: "the settings link is unavailable".into(),
            });
        }
        take_reply
    }

    /// Whether a worker is running at all.
    pub fn is_connected(&self) -> bool {
        self.commands.is_some()
    }

    /// Hand down the call that makes the caller's loop take another pass.
    ///
    /// Installed after construction because a window has no event-loop proxy
    /// until the loop is running, which is after it has built its app.
    pub fn on_update(&self, wake: impl Fn() + Send + 'static) {
        *self.notify.lock().unwrap_or_else(|held| held.into_inner()) = Some(Box::new(wake));
    }
}

impl<S, P> Drop for SectionLink<S, P> {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.as_ref() {
            let _ = commands.send(Command::Stop);
        }
    }
}

impl<S: std::fmt::Debug, P> std::fmt::Debug for SectionLink<S, P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SectionLink")
            .field("connected", &self.commands.is_some())
            .field("authoritative", &self.authoritative)
            .finish()
    }
}

/// The worker: connect, take the bounded wait, then serve until the caller goes.
fn serve<S: Clone + Send + 'static, P: Send + 'static>(
    segment: String,
    root: Option<PathBuf>,
    routes: Routes<S, P>,
    updates: Sender<S>,
    commands: Receiver<Command<S, P>>,
    wake: Sender<Command<S, P>>,
    notify: Arc<Mutex<Option<Wake>>>,
) {
    // Send, then wake: the section is on the caller's channel before its loop
    // is told to take another pass, so a wake can never arrive ahead of what it
    // is about.
    let wake_caller = || {
        if let Some(wake) = notify
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .as_ref()
        {
            wake();
        }
    };
    let publish = |section: S| {
        let _ = updates.send(section);
        wake_caller();
    };

    let Some(client) = connect(&segment, root.as_deref()) else {
        return;
    };

    // **The two-second helper.** It answers with the authoritative section or
    // with nothing; nothing means the defaults already on screen stand.
    let mut pending = (routes.pending)(&client);
    let answer = loop {
        match pending.take() {
            Some(answer) => break answer,
            None => std::thread::sleep(POLL),
        }
    };
    if let Ok(section) = answer {
        publish(section);
    }

    // Section invalidation: another window committed a change, so this one
    // reads the section again rather than being told what it holds.
    let wanted = routes.section;
    let watch = client
        .settings()
        .watch(move |section| {
            if section == wanted {
                let _ = wake.send(Command::Refetch);
            }
        })
        .ok();

    // **Read again, and say whether we are still in touch.** One place, because
    // every path back into contact is the same path: a read that answers is a
    // section published, and a read that does not is a link still out of touch.
    // The client rediscovers the Gateway itself, so a Gateway that came back on
    // a different port is found here without this loop knowing what a port is.
    let reread = |touch: &mut Touch| match (routes.read)(&client) {
        Ok(section) => {
            *touch = Touch::In;
            Some(section)
        }
        Err(_) => {
            touch.lose(Instant::now());
            None
        }
    };

    let mut touch = Touch::In;
    loop {
        let taken = match touch.wait(Instant::now()) {
            None => match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            },
            Some(wait) => match commands.recv_timeout(wait) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
        };
        match taken {
            // **The heal.** Nobody asked; the link is out of touch and it is
            // still worth asking. A section that arrives here is applied
            // exactly as one that arrived on a change fact would be, which is
            // what makes a restarted Gateway a repaint rather than a restart.
            None => {
                if let Some(section) = reread(&mut touch) {
                    publish(section);
                }
            }
            Some(Command::Stop) => break,
            Some(Command::Refetch) => {
                if let Some(section) = reread(&mut touch) {
                    publish(section);
                }
            }
            Some(Command::Patch {
                patch,
                mut confirmed,
                reply,
            }) => {
                if let Some(request) = confirmed.as_ref() {
                    match begin_confirmed(request, Instant::now()) {
                        ConfirmedStart::Send => {}
                        ConfirmedStart::FinishStale => {
                            finish_confirmed(confirmed.take(), ConfirmedOutcome::Stale, true);
                            continue;
                        }
                        ConfirmedStart::FinishTimedOut => {
                            finish_confirmed(confirmed.take(), ConfirmedOutcome::TimedOut, true);
                            continue;
                        }
                        ConfirmedStart::Skip => continue,
                    }
                }
                match (routes.patch)(&client, &patch) {
                    Ok(section) => {
                        touch = Touch::In;
                        let outcome =
                            successful_outcome(section.clone(), patch.as_ref(), routes.echoes);
                        finish_confirmed(confirmed.take(), outcome, false);
                        if let Some(reply) = reply {
                            let _ = reply.send(PatchReply::Applied(section.clone()));
                        }
                        publish(section);
                    }
                    // **A refused write reconciles rather than pretends.** The
                    // authority re-reads before it answers, so what is stored now
                    // is what this window must go back to showing — and a re-read
                    // that also fails is how a write learns the Gateway is gone.
                    Err(error) => {
                        let failure = confirmed_failure(&error);
                        tracing::warn!(
                            target: "koru::settings",
                            section = wanted.token(),
                            %error,
                            "a settings patch did not land; reconciling to what is stored"
                        );
                        let message = error.to_string();
                        if let Some(section) = reread(&mut touch) {
                            let outcome = failed_outcome(failure, Some(section.clone()));
                            finish_confirmed(confirmed.take(), outcome, false);
                            if let Some(reply) = reply {
                                let _ = reply.send(PatchReply::Refused {
                                    authoritative: Some(section.clone()),
                                    message,
                                });
                            }
                            publish(section);
                        } else {
                            if let Some(request) = confirmed.take() {
                                let outcome = failed_outcome(failure, None);
                                finish_confirmed(Some(request), outcome, true);
                            }
                            if let Some(reply) = reply {
                                let _ = reply.send(PatchReply::Refused {
                                    authoritative: None,
                                    message,
                                });
                                wake_caller();
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(watch) = watch {
        watch.close();
    }
}

fn finish_confirmed<S>(
    request: Option<ConfirmedRequest<S>>,
    outcome: ConfirmedOutcome<S>,
    wake: bool,
) {
    let Some(mut request) = request else {
        return;
    };
    if request.settle(outcome) && wake {
        request.wake();
    }
}

fn confirmed_failure(error: &super::SettingsError) -> ConfirmedFailure {
    match error {
        super::SettingsError::Api(failure) => ConfirmedFailure::Refused {
            status: failure.status,
            code: failure.code.clone(),
        },
        super::SettingsError::Local(_) | super::SettingsError::Transport(_) => {
            ConfirmedFailure::Disconnected
        }
    }
}

fn failed_outcome<S>(failure: ConfirmedFailure, authoritative: Option<S>) -> ConfirmedOutcome<S> {
    match (failure, authoritative) {
        (failure @ ConfirmedFailure::Refused { .. }, Some(authoritative)) => {
            ConfirmedOutcome::Reconciled {
                authoritative,
                failure,
            }
        }
        (ConfirmedFailure::Refused { status, code }, None) => {
            ConfirmedOutcome::Refused { status, code }
        }
        (ConfirmedFailure::Disconnected, _) => ConfirmedOutcome::Disconnected,
    }
}

fn successful_outcome<S, P>(
    authoritative: S,
    patch: &P,
    echoes: fn(&S, &P) -> bool,
) -> ConfirmedOutcome<S> {
    if echoes(&authoritative, patch) {
        ConfirmedOutcome::Accepted(authoritative)
    } else {
        ConfirmedOutcome::ContractMismatch { authoritative }
    }
}

/// Reach the Gateway the Agent ensures, retrying inside [`CONNECT_BUDGET`].
///
/// `open` rather than `ensure`: asking what the theme is must not be a reason
/// to start an agent and a gateway that were not running.
fn connect(segment: &str, root: Option<&Path>) -> Option<Client> {
    let deadline = std::time::Instant::now() + CONNECT_BUDGET;
    loop {
        let attempt = match root {
            Some(root) => Client::open_at_with_scopes(segment, root, LINK_SCOPES),
            None => Client::open_with_scopes(segment, LINK_SCOPES),
        };
        match attempt {
            Ok(client) => return Some(client),
            Err(_) if std::time::Instant::now() >= deadline => return None,
            Err(_) => std::thread::sleep(POLL),
        }
    }
}
