// SPDX-License-Identifier: MPL-2.0

//! The worker: what the routes and the store feed share.
//!
//! **No dedicated thread and no command queue for the journal, deliberately.**
//! The store
//! worker earns both — a single-owner Store, a lock discipline, admission
//! control. This journal is a Mutex over small synchronous SQLite operations,
//! so the worker is state the host's runtime calls into: embedded, the
//! gateway forwards requests straight into the router; standalone, `main.rs`
//! serves the same router on a loopback listener. If admission control is
//! ever needed, [`EventsHandle`] is the seam it goes behind.
//!
//! **The one thread here belongs to the telemetry drain**, and only when a
//! sink is configured — an HTTP delivery is the one operation that must not
//! happen on a handler or under the journal lock. See [`crate::forward`]; the
//! destination is the hosted metrics service, `docs/architecture/eventbus/README.md`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use koru_events::wire::{AppendReceipt, EventsStatus, RejectedEvent};
use koru_events::{EmitContext, Envelope};

use crate::forward;
use crate::journal::{BaselineEntry, FeedNote, Journal, JournalError};

/// Where the journal lives and how the worker runs.
pub struct WorkerConfig {
    /// The directory `events.db` lives in — `koru_channel::events_root_beside`
    /// is the one rule for deriving it from a store root.
    dir: PathBuf,
    /// A journal at a location the worker used before this build; adopted by
    /// rename when the current `dir` holds none, so relocating the layout
    /// does not orphan a live journal.
    legacy_dir: Option<PathBuf>,
    /// `app.installed` is announced on a journal's first open. Off in test
    /// fixtures, whose assertions count sequences from one.
    announce_install: bool,
    /// Whether to hold the root claim. Off in fixtures: several open a
    /// journal per test under one temp root, and a machine-wide endpoint is
    /// not what any of them is testing.
    claim_root: bool,
    /// Hosted operational journals never populate the desktop telemetry
    /// outbox and never announce a user installation.
    local_only: bool,
    system_actor: &'static str,
}

impl WorkerConfig {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            legacy_dir: None,
            announce_install: true,
            claim_root: true,
            local_only: false,
            system_actor: "store",
        }
    }

    /// The pre-relocation journal directory, adopted if the new one is empty.
    pub fn with_legacy_dir(mut self, legacy: PathBuf) -> Self {
        self.legacy_dir = Some(legacy);
        self
    }

    /// For fixtures: no installation announcement, so sequence numbers in
    /// assertions start at the first published event.
    pub fn quiet(dir: PathBuf) -> Self {
        Self {
            dir,
            legacy_dir: None,
            announce_install: false,
            claim_root: false,
            local_only: false,
            system_actor: "store",
        }
    }

    /// A durable hosted journal with one named system actor and no desktop
    /// installation/outbox/network behavior.
    pub fn local_only(dir: PathBuf, system_actor: &'static str) -> Self {
        Self {
            dir,
            legacy_dir: None,
            announce_install: false,
            claim_root: true,
            local_only: true,
            system_actor,
        }
    }
}

/// The private, root-qualified claim held while a journal is open — the
/// `StoreRootClaim` shape, for the events root.
///
/// **One writer per journal.** WAL tolerates a second opener without
/// corrupting anything, so the hazard is quieter than a lost write and worse
/// to debug: two workers each hold their own `Notify`, so a subscriber on
/// one never wakes for the other's appends and a live stream simply goes
/// silent. The claim makes that a refusal at open instead.
struct EventsRootClaim {
    _listener: koru_ipc::Listener,
}

impl EventsRootClaim {
    fn acquire(dir: &std::path::Path) -> Result<Self, JournalError> {
        std::fs::create_dir_all(dir)
            .map_err(|error| JournalError::Storage(format!("events dir: {error}")))?;
        let endpoint =
            koru_ipc::Endpoint::for_service_root(koru_ipc::EVENTS, koru_channel::segment(), dir)
                .map_err(|error| JournalError::Storage(error.to_string()))?;
        koru_ipc::Listener::bind(endpoint)
            .map(|listener| Self {
                _listener: listener,
            })
            .map_err(|error| {
                JournalError::Storage(format!(
                    "another events worker already holds this journal: {error}"
                ))
            })
    }
}

pub struct EventsState {
    /// `None` when `KORU_EVENTS_DISABLED` silenced the worker: nothing is
    /// opened, appends no-op, reads answer empty.
    journal: Option<Mutex<Journal>>,
    /// Wakes every SSE stream after an append lands. Runtime-agnostic: the
    /// streams are polled on whatever runtime serves the response.
    pub notify: tokio::sync::Notify,
    pub sse_subscribers: AtomicU64,
    /// **The host is shutting down; every tail ends rather than being waited
    /// on.** Set once by [`EventsState::close_streams`] and never cleared: a
    /// worker whose host is going away does not start serving tails again.
    closing: AtomicBool,
    /// What the per-subject activity read has cost this run — reads served and
    /// rows answered. Counters, never content: neither a subject reference nor
    /// an event name is recorded, because a reference names a document and a
    /// node. Free when nobody asks; two relaxed adds when somebody does.
    pub activity_queries: AtomicU64,
    pub activity_rows: AtomicU64,
    /// The identity derived events carry: `Actor::System("store")`, one
    /// session per worker run.
    pub ctx: EmitContext,
    /// Signals the telemetry drain. `None` when no sink is configured, which
    /// is every ordinary run and every test — there is then no thread at all.
    metrics: Option<Arc<forward::Waker>>,
    local_only: bool,
}

/// Ending the wait rather than leaving a thread parked for its idle floor.
/// The forwarder holds a `Weak` back, so this is the moment it learns the
/// journal is gone.
impl Drop for EventsState {
    fn drop(&mut self) {
        if let Some(waker) = &self.metrics {
            waker.stop();
        }
    }
}

impl EventsState {
    pub fn disabled(&self) -> bool {
        self.journal.is_none()
    }

    /// **End every open SSE tail, now.**
    ///
    /// A tail is a response body that never completes on its own: it is held
    /// until the client's own deadline cuts it, which for `koru-api-client` is
    /// ninety seconds. A host running a graceful HTTP shutdown waits for
    /// in-flight responses, so one idle window attached to `/v1/events/stream`
    /// made a gateway asked to stop take about ninety seconds to exit —
    /// measured, not argued. **A server that cannot stop because a client is
    /// idling on a tail is the defect**, so the host closes the tails as the
    /// first act of its shutdown rather than lengthening a deadline past them.
    ///
    /// Cutting a tail loses nothing: the cursor is the whole protocol, the
    /// client resumes from `Last-Event-ID` on its next connection, and this is
    /// the same end-of-stream it already handles ninety seconds at a time.
    ///
    /// Idempotent, callable from any thread, and it takes no lock — a shutdown
    /// path must not queue behind a journal read.
    pub fn close_streams(&self) {
        self.closing.store(true, Ordering::Release);
        // Every parked tail arms its waiter *before* it reads, so a tail is
        // either registered here (and woken) or about to re-check the flag.
        self.notify.notify_waiters();
    }

    /// Whether [`EventsState::close_streams`] has been called. The stream body
    /// checks it each time round its loop.
    pub fn closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    pub fn append(&self, events: &[Envelope]) -> Result<AppendReceipt, JournalError> {
        let Some(journal) = &self.journal else {
            return Ok(disabled_receipt(events));
        };
        let now = now_ms();
        let receipt = if self.local_only {
            journal
                .lock()
                .expect("events journal")
                .append_local(events, now)?
        } else {
            journal
                .lock()
                .expect("events journal")
                .append(events, now)?
        };
        if debug_enabled() {
            for envelope in events {
                tracing::info!(
                    target: "koru_events",
                    name = %envelope.name,
                    event_id = %envelope.event_id,
                    latency_ms = now - envelope.occurred_at_ms,
                    "recorded"
                );
            }
        }
        if receipt.accepted > 0 {
            self.notify.notify_waiters();
            // The same moment, for the same reason: something arrived. The
            // wake is a flag and a `notify_one`, so the append path pays
            // nothing for a sink it may not have.
            if let Some(waker) = &self.metrics {
                waker.wake();
            }
        }
        Ok(receipt)
    }

    /// The store feed's door. Derivation happens in the journal; anything it
    /// emitted wakes the streams like a client publish would.
    pub fn feed(&self, notes: &[FeedNote]) -> Result<u32, JournalError> {
        let Some(journal) = &self.journal else {
            return Ok(0);
        };
        let now = now_ms();
        let derived = journal
            .lock()
            .expect("events journal")
            .apply_notes(notes, &self.ctx, now)?;
        if debug_enabled() {
            for envelope in &derived {
                tracing::info!(
                    target: "koru_events",
                    name = %envelope.name,
                    event_id = %envelope.event_id,
                    "derived"
                );
            }
        }
        if !derived.is_empty() {
            self.notify.notify_waiters();
        }
        Ok(derived.len() as u32)
    }

    pub fn seed_baseline(&self, entries: &[BaselineEntry]) -> Result<bool, JournalError> {
        let Some(journal) = &self.journal else {
            return Ok(false);
        };
        journal
            .lock()
            .expect("events journal")
            .seed_baseline(entries)
    }

    pub fn with_journal<R>(
        &self,
        read: impl FnOnce(&mut Journal) -> Result<R, JournalError>,
        empty: impl FnOnce() -> R,
    ) -> Result<R, JournalError> {
        match &self.journal {
            Some(journal) => read(&mut journal.lock().expect("events journal")),
            None => Ok(empty()),
        }
    }

    pub fn status(&self) -> Result<EventsStatus, JournalError> {
        let mut status = self.with_journal(|journal| journal.status(), disabled_status)?;
        status.disabled = self.disabled();
        status.sse_subscribers = self.sse_subscribers.load(Ordering::Relaxed);
        status.activity_queries = self.activity_queries.load(Ordering::Relaxed);
        status.activity_rows = self.activity_rows.load(Ordering::Relaxed);
        Ok(status)
    }
}

/// The one worker type the hosts hold: `Clone`, cheap, and the whole public
/// surface — the gateway forwards HTTP through [`EventsHandle::request`], the
/// store feed calls the typed methods, the standalone binary serves
/// [`EventsHandle::router`].
#[derive(Clone)]
pub struct EventsHandle {
    pub state: Arc<EventsState>,
    router: axum::Router,
    /// Held for as long as any clone of this handle lives, which is as long
    /// as the journal is open. `Arc` because the handle is cloned into the
    /// gateway's router state and the store's feed.
    _claim: Option<Arc<EventsRootClaim>>,
}

impl EventsHandle {
    /// Open (or refuse to open) the journal and build the worker.
    pub fn open(config: WorkerConfig) -> Result<Self, JournalError> {
        let sink = forward::configured_sink();
        Self::open_with(config, sink)
    }

    /// The same, with the sink named rather than read from the environment —
    /// which is how the tests drive a drain without a network and without a
    /// vendor. Production has exactly one caller, [`open`](Self::open).
    pub fn open_with(
        config: WorkerConfig,
        sink: Option<forward::SinkFactory>,
    ) -> Result<Self, JournalError> {
        let sink = if config.local_only { None } else { sink };
        let disabled = worker_disabled(
            config.local_only,
            koru_env::switch(koru_env::name::KORU_EVENTS_DISABLED),
        );
        // The claim first, before anything is opened — a second worker fails
        // here rather than halfway through sharing a journal, which is the
        // order the Store's own claim established and for the same reason.
        let claim = match (config.claim_root, disabled) {
            (true, false) => Some(Arc::new(EventsRootClaim::acquire(&config.dir)?)),
            _ => None,
        };
        let journal = if disabled {
            tracing::info!(target: "koru_events", "KORU_EVENTS_DISABLED is set; the journal stays closed");
            None
        } else {
            // Adopt a journal the previous layout left behind — renames, so
            // its history and cursors survive the move whole. The WAL
            // siblings travel too: a cleanly closed journal leaves none, a
            // crashed one may, and a database moved without its WAL loses
            // the commits still living there.
            if let Some(legacy) = &config.legacy_dir
                && legacy.join("events.db").exists()
                && !config.dir.join("events.db").exists()
            {
                let _ = std::fs::create_dir_all(&config.dir);
                for name in ["events.db-wal", "events.db-shm", "events.db"] {
                    let from = legacy.join(name);
                    if !from.exists() {
                        continue;
                    }
                    if let Err(error) = std::fs::rename(&from, config.dir.join(name)) {
                        tracing::warn!(
                            target: "koru_events",
                            "a legacy events journal file at {} could not be adopted: {error}",
                            from.display()
                        );
                    }
                }
            }
            Some(Mutex::new(Journal::open(&config.dir)?))
        };
        // The waker has to exist before the state that holds it, and the
        // thread has to exist after the `Arc` it will weakly hold — so the
        // three are built in that order and never folded together.
        let waker = sink.as_ref().map(|_| forward::waker());
        let state = Arc::new(EventsState {
            journal,
            notify: tokio::sync::Notify::new(),
            sse_subscribers: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            activity_queries: AtomicU64::new(0),
            activity_rows: AtomicU64::new(0),
            ctx: EmitContext::system(config.system_actor),
            metrics: waker.clone(),
            local_only: config.local_only,
        });
        if let (Some(sink), Some(waker)) = (sink, waker) {
            forward::start(Arc::downgrade(&state), waker, sink);
        }
        // The one-time installation announcement: once per journal, marked
        // only after the append lands so a failure retries next open.
        if config.announce_install
            && !state.disabled()
            && !state.with_journal(|journal| journal.installed_recorded(), || true)?
        {
            let installed = koru_events::AppInstalled {
                build: koru_channel::build_info().build_number,
            };
            let receipt =
                state.append(&[koru_events::Draft::from(installed).into_envelope(&state.ctx)])?;
            if receipt.accepted > 0 || receipt.duplicates > 0 {
                state.with_journal(|journal| journal.mark_installed_recorded(), || ())?;
            }
        }
        let router = crate::routes::router(Arc::clone(&state));
        Ok(Self {
            state,
            router,
            _claim: claim,
        })
    }

    /// Route one already-authenticated request. Infallible at this boundary:
    /// the router answers 404s and errors itself.
    pub async fn request(&self, request: axum::extract::Request) -> axum::response::Response {
        use tower::ServiceExt as _;
        self.router
            .clone()
            .oneshot(request)
            .await
            .expect("infallible events router")
    }

    pub fn router(&self) -> axum::Router {
        self.router.clone()
    }

    pub fn feed(&self, notes: &[FeedNote]) -> Result<u32, JournalError> {
        self.state.feed(notes)
    }

    pub fn seed_baseline(&self, entries: &[BaselineEntry]) -> Result<bool, JournalError> {
        self.state.seed_baseline(entries)
    }

    pub fn status(&self) -> Result<EventsStatus, JournalError> {
        self.state.status()
    }

    /// End every open SSE tail — see [`EventsState::close_streams`]. The host
    /// calls this as the first act of its shutdown.
    pub fn close_streams(&self) {
        self.state.close_streams();
    }
}

const fn worker_disabled(local_only: bool, desktop_disabled: bool) -> bool {
    !local_only && desktop_disabled
}

fn disabled_receipt(events: &[Envelope]) -> AppendReceipt {
    AppendReceipt {
        accepted: 0,
        duplicates: 0,
        rejected: events
            .iter()
            .enumerate()
            .map(|(index, envelope)| RejectedEvent {
                index: index as u32,
                event_id: envelope.event_id.to_string(),
                code: "events_disabled".into(),
                message: "KORU_EVENTS_DISABLED is set on this worker".into(),
            })
            .collect(),
        last_seq: 0,
    }
}

fn disabled_status() -> EventsStatus {
    EventsStatus {
        disabled: true,
        total_events: 0,
        domain_events: 0,
        instrumentation_events: 0,
        last_seq: 0,
        pruned_events: 0,
        outbox_pending: 0,
        telemetry_consent: false,
        // A closed journal has nowhere to record an answer, so a surface must
        // read `disabled` first and not ask a question it cannot keep.
        telemetry_consent_answered: false,
        baseline_seeded: false,
        tracked_documents: 0,
        sse_subscribers: 0,
        journal_bytes: 0,
        activity_queries: 0,
        activity_rows: 0,
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// `KORU_EVENTS_DEBUG` — free when off: one `OnceLock<bool>` and a branch.
pub fn debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_EVENTS_DEBUG))
}

#[cfg(test)]
mod local_only_tests {
    use super::worker_disabled;

    #[test]
    fn local_only_hosted_journal_ignores_the_desktop_disabled_switch() {
        assert!(!worker_disabled(true, true));
        assert!(!worker_disabled(true, false));
        assert!(worker_disabled(false, true));
    }
}
