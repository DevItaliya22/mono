// SPDX-License-Identifier: MPL-2.0

//! The drain: outbox → sink, on one thread that sleeps when there is nothing.
//!
//! **This drain is the uploader.** Telemetry leaves the machine addressed to
//! Koru's own hosted metrics service through `koru-telemetry-client`; the
//! service, not this binary, holds every vendor credential
//! (`docs/architecture/eventbus/README.md`). It began as a PostHog spike with a
//! project token compiled in, and the plan for it was once "deleted when an
//! Agent uploader lands" — the decision stands the other way: the drain's
//! mechanics were never the problem, the credential was, and the credential
//! is gone.
//!
//! ## Why a thread, when the module beside it says there is deliberately none
//!
//! [`crate::state`] is right that a journal wrapped in a Mutex needs no worker
//! thread: every operation is a small synchronous SQLite call. A *delivery* is
//! not — it is an HTTP request that can take fifteen seconds, and there are
//! exactly two places it must not happen. Not on a request handler, which owes
//! an answer now; and **never while holding the journal lock**, which would
//! stop every publish and every SSE reader for the length of a network
//! timeout. So: read pending, drop the lock, deliver, re-take the lock, ack.
//! That is the whole reason this file exists, and it is the one invariant not
//! to lose while editing it.
//!
//! ## Nothing here polls
//!
//! The thread waits on a condition variable that [`crate::state::EventsState`]
//! signals at the same moment it wakes the SSE streams — after an append that
//! accepted something. The only timed wait is a retry deadline, plus a long
//! idle floor as a safety net against a missed signal. An idle installation
//! costs one parked thread.
//!
//! ## What a failure may never do
//!
//! Delete. `Retry` means the same bytes may work later, so the outbox is left
//! exactly as it was and the backoff grows. The single exception is an event a
//! sink refuses *permanently and alone*: it is dropped, loudly, because one
//! poison row must not stall every event behind it forever.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use koru_events::telemetry::Consent;
use koru_metrics_sink::{Delivery, Sink, SinkEvent};

use crate::state::EventsState;

/// The retry ladder, in seconds. Capped at an hour: a sink that has been down
/// longer is not coming back inside one, and asking every hour is enough to
/// notice when it does.
const LADDER: [u64; 6] = [5, 30, 120, 600, 1_800, 3_600];

/// The longest the thread sleeps with nothing to do. Purely a safety net — a
/// wake is signalled on every accepted append — so it is long enough to cost
/// nothing and short enough that a missed signal is a delay, not a stall.
const IDLE: Duration = Duration::from_secs(300);

/// After a delivery that emptied its batch, look again immediately: a backlog
/// drains at the sink's pace rather than one batch per wake.
const IMMEDIATE: Duration = Duration::from_millis(50);

/// `KORU_METRICS_DEBUG` — free when off, one `OnceLock<bool>` and a branch.
pub fn debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_METRICS_DEBUG))
}

/// The signal the journal's writer sends and the forwarder waits on.
///
/// Held by both, which is why it is its own type rather than a field on
/// either: the state must be able to wake a thread it does not own, and the
/// thread must be able to notice the state has gone.
pub struct Waker {
    signalled: Mutex<bool>,
    condvar: Condvar,
    stopped: AtomicBool,
}

impl Waker {
    fn new() -> Self {
        Self {
            signalled: Mutex::new(false),
            condvar: Condvar::new(),
            stopped: AtomicBool::new(false),
        }
    }

    /// There may be something to send. Cheap, and safe to call from the append
    /// path — it never blocks on the sink or on the network.
    pub fn wake(&self) {
        *self.signalled.lock().expect("metrics waker") = true;
        self.condvar.notify_one();
    }

    /// The worker is going away. Called from `EventsState::drop`, so a test
    /// that drops its handle does not leave a thread parked for five minutes.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wake();
    }

    /// Wait for a signal or the deadline. `false` means stop.
    fn wait(&self, timeout: Duration) -> bool {
        let mut signalled = self.signalled.lock().expect("metrics waker");
        if !*signalled {
            let (guard, _) = self
                .condvar
                .wait_timeout(signalled, timeout)
                .expect("metrics waker");
            signalled = guard;
        }
        *signalled = false;
        !self.stopped.load(Ordering::Acquire)
    }
}

/// How a sink comes into existence, deferred to the thread that will use it.
///
/// **A sink is never built on the caller's thread**, and that is not
/// fastidiousness. `EventsHandle::open` is called from inside the gateway's
/// `#[tokio::main]`, and building a `reqwest::blocking::Client` there panics
/// the process — *"Cannot drop a runtime in a context where blocking is not
/// allowed"* — because the blocking client stands a runtime up and tears it
/// down, which tokio refuses inside an async context. Handing over a closure
/// instead of a built sink moves both the construction and the eventual drop
/// onto `koru-metrics`, an ordinary thread that has never heard of a runtime.
pub type SinkFactory = Box<dyn FnOnce() -> Option<Arc<dyn Sink>> + Send>;

/// Whether this build has a destination without being configured.
///
/// Public so a dependent's suite can assert its own `no-sink` guard is armed —
/// see the tests in `koru-store-server` and `koru-api-gateway-core`.
///
/// **A destination is not permission.** Every channel — dev, nightly, beta
/// and stable alike — has the built-in destination, and none of them sends a
/// thing until this device's consent record says yes: the drain reads that
/// record before every batch ([`cycle`]), and the journal spools nothing while
/// it says anything else. That is the opt-in contract of 2026-10-11, which
/// amends ADR-176: a release channel classifies a binary and never stands in
/// for consent. Before it, this answered `true` for internal and nightly only,
/// and consent defaulted on, so the channel *was* the permission.
pub fn sends_by_default() -> bool {
    // **A test run never sends, and `cfg!(test)` alone does not say that.**
    // A dependency is never compiled with `cfg(test)`, so this covers this
    // crate's own suite and nothing else — `koru-store-server` and
    // `koru-api-gateway-core` both open a real worker from a `#[cfg(test)]`
    // block, and without the feature they would post their fixtures into the
    // hosted service. That is the `no-reader` lesson applied a third time:
    // the feature is what covers a dependent, and it is turned on in their
    // `[dev-dependencies]`.
    if cfg!(test) || cfg!(feature = "no-sink") {
        return false;
    }
    has_destination(koru_channel::segment())
}

/// The channel's part in the decision, which is none: every channel gets the
/// same destination, and consent alone decides whether anything reaches it.
/// Spelled as a function of the segment so the suite can hold every channel
/// to that, rather than trusting that nobody re-adds a `matches!`.
pub(crate) fn has_destination(_segment: &str) -> bool {
    true
}

/// Whether this process is configured to send anywhere, and how to build it.
///
/// `KORU_TELEMETRY_ENABLED=0` removes the destination, and setting a base URL
/// replaces the built-in one — which is how a developer points at a metrics
/// service of their own. Unset, every build has `https://m.koru.do`
/// as its destination. **Neither the environment nor the channel can grant
/// consent**: `KORU_TELEMETRY_ENABLED=1` builds a sink and the drain still
/// sends nothing until the device's record says yes.
///
/// Reading the environment is cheap and happens here; **constructing** the
/// sink is what the returned closure does, later, elsewhere — see
/// [`SinkFactory`].
pub fn configured_sink() -> Option<SinkFactory> {
    let enabled = match std::env::var_os(koru_env::name::KORU_TELEMETRY_ENABLED) {
        // Said explicitly, either way: that answer wins.
        Some(_) => koru_env::switch(koru_env::name::KORU_TELEMETRY_ENABLED),
        None => sends_by_default(),
    };
    if !enabled {
        return None;
    }
    let base_url = non_empty(koru_env::name::KORU_TELEMETRY_BASE_URL)
        .unwrap_or_else(|| koru_telemetry_client::DEFAULT_BASE_URL.into());
    let config = koru_telemetry_client::Config {
        base_url,
        batch: number(koru_env::name::KORU_TELEMETRY_BATCH_EVENTS, 250),
        timeout: Duration::from_millis(
            number(koru_env::name::KORU_TELEMETRY_TIMEOUT_MS, 15_000) as u64
        ),
    };
    Some(Box::new(move || {
        match koru_telemetry_client::TelemetryClient::new(config) {
            Ok(sink) => {
                // The endpoint, the batch size, the sink's name. There is no
                // credential left to keep out of this line — that is the
                // point of the service it names.
                tracing::info!(
                    target: "koru_metrics",
                    sink = %sink.id(),
                    endpoint = %sink.endpoint(),
                    batch = sink.max_batch(),
                    "the telemetry sink is running"
                );
                Some(Arc::new(sink) as Arc<dyn Sink>)
            }
            Err(error) => {
                tracing::error!(target: "koru_metrics", %error, "no telemetry sink is running");
                None
            }
        }
    }))
}

/// Wrap an already-built sink as a factory, for the tests that inject one.
pub fn ready(sink: Arc<dyn Sink>) -> SinkFactory {
    Box::new(move || Some(sink))
}

/// A variable's value when it is set to something other than blank.
///
/// A blank override is treated as absent rather than as an empty host — the
/// shape a `.env` with a commented-out line leaves behind.
fn non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A `Value` variable read as a number, falling back rather than failing: a
/// mistyped batch size must not be the reason a worker refuses to start.
fn number(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(fallback)
}

/// Start the drain. The state holds the [`Waker`]; the thread holds only a
/// [`Weak`] back, so the two do not keep each other alive and dropping the
/// handle ends the thread.
///
/// The factory is **run on the new thread**, not here — see [`SinkFactory`].
pub fn start(state: Weak<EventsState>, waker: Arc<Waker>, sink: SinkFactory) {
    let spawned = std::thread::Builder::new()
        .name("koru-metrics".into())
        .spawn(move || run(state, waker, sink));
    if let Err(error) = spawned {
        tracing::error!(target: "koru_metrics", %error, "the telemetry forwarder could not start");
    }
}

fn run(state: Weak<EventsState>, waker: Arc<Waker>, sink: SinkFactory) {
    // On this thread, and nowhere else. A sink that will not build says so and
    // the thread ends; the journal keeps spooling, and the outbox is drained
    // by whatever is configured next time.
    let Some(sink) = sink() else { return };
    let mut failures = 0u32;
    let mut last_code = "";
    let mut wait = IMMEDIATE;
    loop {
        if !waker.wait(wait) {
            return;
        }
        let Some(state) = state.upgrade() else { return };
        match cycle(&state, sink.as_ref()) {
            Cycle::Idle => {
                failures = 0;
                last_code = "";
                wait = IDLE;
            }
            Cycle::Sent { remaining } => {
                failures = 0;
                last_code = "";
                wait = if remaining { IMMEDIATE } else { IDLE };
            }
            Cycle::Failed { after, code } => {
                failures += 1;
                // Report news, not activity: the first failure after a healthy
                // run and every change of reason are worth a line; the
                // fiftieth identical timeout is not.
                if failures == 1 || code != last_code || failures.is_multiple_of(10) {
                    tracing::warn!(
                        target: "koru_metrics",
                        sink = %sink.id(),
                        code,
                        attempt = failures,
                        "telemetry delivery failed; nothing was deleted"
                    );
                }
                last_code = code;
                wait = after.unwrap_or_else(|| backoff(failures));
            }
        }
    }
}

enum Cycle {
    /// Nothing to do: consent withdrawn, or the outbox is empty.
    Idle,
    Sent {
        /// The batch was full, so there is probably more behind it.
        remaining: bool,
    },
    Failed {
        after: Option<Duration>,
        code: &'static str,
    },
}

/// One pass. **The lock is held for the two journal steps and never across
/// `deliver`** — see the module note.
fn cycle(state: &EventsState, sink: &dyn Sink) -> Cycle {
    let limit = sink.max_batch();
    let prepared = state.with_journal(
        |journal| {
            let consent = journal.consent()?;
            if !consent.granted {
                return Ok(None);
            }
            let pending = journal.telemetry_pending(limit as u32)?;
            if pending.is_empty() {
                return Ok(None);
            }
            let installation_id = journal.installation_id()?;
            Ok(Some((pending, consent.consent_version, installation_id)))
        },
        || None,
    );
    let prepared = match prepared {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return Cycle::Idle,
        Err(error) => {
            tracing::error!(target: "koru_metrics", %error, "the events journal did not answer");
            return Cycle::Failed {
                after: None,
                code: "journal_failed",
            };
        }
    };
    let (pending, consent_version, installation_id) = prepared;
    let info = koru_channel::build_info();
    let events: Vec<SinkEvent> = pending
        .iter()
        .map(|event| SinkEvent {
            event_id: event.event_id.to_string(),
            name: event.name.clone(),
            version: event.version,
            occurred_at_ms: event.occurred_at_ms,
            session_id: event.session_id.to_string(),
            installation_id: Some(installation_id.clone()),
            app_version: info.version.to_string(),
            build_channel: info.channel.to_string(),
            platform: std::env::consts::OS.to_string(),
            consent_version: consent_version.clone(),
            properties: event.properties.clone(),
        })
        .collect();

    let at = Instant::now();
    let mut acked = Vec::with_capacity(events.len());
    // **Asked again before every request**, not only when the batch was read:
    // a withdrawal that lands between reading the outbox and sending it has
    // already deleted those rows, and this is what keeps them from leaving
    // anyway. A short lock each time, never held across `deliver`.
    let still_allowed = || {
        state
            .with_journal(|journal| journal.consent(), Consent::undecided)
            .is_ok_and(|consent| consent.granted)
    };
    let settled = deliver(sink, &events, &mut acked, &still_allowed);
    let took = at.elapsed();

    if !acked.is_empty() {
        match state.with_journal(|journal| journal.telemetry_ack(&acked), || 0) {
            Ok(_) => {}
            Err(error) => {
                // Delivered but not forgotten. Harmless — every sink dedupes
                // on `event_id` — but worth saying, because the outbox not
                // shrinking is otherwise a mystery.
                tracing::error!(
                    target: "koru_metrics",
                    %error,
                    "delivered events could not be removed from the outbox; they will be sent again"
                );
            }
        }
    }
    report(sink, &events, acked.len(), &settled, took);

    match settled {
        Settled::Done => Cycle::Sent {
            remaining: events.len() == limit,
        },
        Settled::Retry { after, code } => Cycle::Failed { after, code },
        Settled::Withdrawn => Cycle::Idle,
    }
}

enum Settled {
    /// Every event in the batch is resolved — delivered, or refused alone and
    /// dropped.
    Done,
    /// Nothing was resolved past this point; try again.
    Retry {
        after: Option<Duration>,
        code: &'static str,
    },
    /// Consent was withdrawn mid-cycle; nothing past this point was sent, and
    /// the withdrawal has already deleted it.
    Withdrawn,
}

/// Deliver, splitting on a permanent refusal until the offending event is
/// alone.
///
/// **A batch is refused as a whole, so a `Permanent` names no event.** Sending
/// the halves separately finds it in log₂n extra requests, and only the event
/// that is refused by itself is dropped — the other 249 in a batch of 250 are
/// delivered rather than lost with it.
fn deliver(
    sink: &dyn Sink,
    events: &[SinkEvent],
    acked: &mut Vec<String>,
    allowed: &dyn Fn() -> bool,
) -> Settled {
    if !allowed() {
        return Settled::Withdrawn;
    }
    match sink.deliver(events) {
        Delivery::Ok => {
            acked.extend(events.iter().map(|event| event.event_id.clone()));
            Settled::Done
        }
        Delivery::Retry { after, code } => Settled::Retry { after, code },
        Delivery::Permanent { code } if events.len() == 1 => {
            let event = &events[0];
            // The name and the code, never a property: what is wanted is
            // "which event does this sink refuse", and the answer is a name.
            tracing::warn!(
                target: "koru_metrics",
                sink = %sink.id(),
                name = %event.name,
                version = event.version,
                code,
                "a sink refuses this event permanently; dropping it so the rest can go"
            );
            acked.push(event.event_id.clone());
            Settled::Done
        }
        Delivery::Permanent { .. } => {
            let middle = events.len() / 2;
            match deliver(sink, &events[..middle], acked, allowed) {
                // The second half waits: a retry means the sink stopped
                // answering mid-split, and pressing on would be guessing.
                retry @ Settled::Retry { .. } => retry,
                Settled::Withdrawn => Settled::Withdrawn,
                Settled::Done => deliver(sink, &events[middle..], acked, allowed),
            }
        }
    }
}

/// One line per attempt. `debug` normally, `info` under `KORU_METRICS_DEBUG`,
/// which also adds the event names and the property **keys** — the thing you
/// want while deciding whether the projection carries enough, and never a
/// value.
fn report(sink: &dyn Sink, events: &[SinkEvent], acked: usize, settled: &Settled, took: Duration) {
    let outcome = match settled {
        Settled::Done => "ok",
        Settled::Retry { code, .. } => code,
        Settled::Withdrawn => "consent_withdrawn",
    };
    if !debug_enabled() {
        tracing::debug!(
            target: "koru_metrics",
            sink = %sink.id(),
            events = events.len(),
            acked,
            outcome,
            took_ms = took.as_secs_f64() * 1000.0,
            "telemetry delivered"
        );
        return;
    }
    let mut names: Vec<&str> = events.iter().map(|event| event.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let mut keys: Vec<&str> = events
        .iter()
        .flat_map(|event| event.properties.keys().map(String::as_str))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    tracing::info!(
        target: "koru_metrics",
        sink = %sink.id(),
        events = events.len(),
        acked,
        outcome,
        took_ms = took.as_secs_f64() * 1000.0,
        names = %names.join(","),
        property_keys = %keys.join(","),
        "telemetry delivered"
    );
}

/// The ladder, with ±20% of jitter.
///
/// The jitter matters for the same reason a thundering herd does: every
/// installation that was offline during an outage comes back when the sink
/// does, and a ladder without it schedules them all on the same second. The
/// seed is the attempt and the process, so no `rand` dependency is needed and
/// two installations differ.
fn backoff(attempts: u32) -> Duration {
    let step = LADDER[(attempts.max(1) as usize - 1).min(LADDER.len() - 1)];
    let spread = jitter(attempts) % 41; // 0..=40
    let percent = 80 + spread; // 80%..=120%
    Duration::from_millis(step * 10 * percent)
}

fn jitter(attempts: u32) -> u64 {
    // FNV-1a over the attempt and this process's id: enough spread for a
    // retry schedule, and deterministic within a run so a test can assert the
    // bounds rather than the value.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in (attempts as u64)
        .to_le_bytes()
        .iter()
        .chain(u64::from(std::process::id()).to_le_bytes().iter())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Build the waker a state will hold, so the caller can put it in place before
/// the thread that signals it exists.
pub fn waker() -> Arc<Waker> {
    Arc::new(Waker::new())
}

#[cfg(test)]
mod tests;
