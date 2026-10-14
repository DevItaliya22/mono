// SPDX-License-Identifier: MPL-2.0

//! The drain, driven by scripted sinks: no network, no vendor, no waiting on
//! a thread. [`cycle`] is the unit under test — it is the whole policy, and
//! the thread around it is a `wait` and a `match`.

use std::sync::atomic::{AtomicUsize, Ordering};

use koru_events::{
    Draft, EmitContext, Envelope, EventId, ImportCompleted, ImportOutcome, ImportSource, bucket,
};
use koru_metrics_sink::{Blackhole, SinkId, code};

use super::*;
use crate::state::{EventsHandle, WorkerConfig};

/// The opt-in contract of 2026-10-11, held by a sink that records every export
/// attempt.
mod consent_contract;

const T0: i64 = 1_756_000_000_000;

fn ctx() -> EmitContext {
    EmitContext::system("test")
}

/// An `AnonymousUsage` event, so it projects into the outbox.
fn exportable(id: u128) -> Envelope {
    Draft::from(ImportCompleted {
        source: ImportSource::Markdown,
        document_count_bucket: bucket(3),
        outcome: ImportOutcome::Ok,
    })
    .build(&ctx(), EventId(uuid::Uuid::from_u128(id)), T0)
}

/// A worker on a temporary journal whose person has said **yes**, with
/// `count` events already spooled. These tests are about what a drain does
/// with an outbox; consent itself is held by `consent_contract.rs`, and
/// without the yes there would be no outbox to drain.
fn worker(sink: Option<SinkFactory>, count: u128) -> (tempfile::TempDir, EventsHandle) {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = EventsHandle::open_with(WorkerConfig::quiet(dir.path().to_path_buf()), sink)
        .expect("a journal");
    handle
        .state
        .with_journal(
            |journal| journal.set_consent(true, koru_events::telemetry::DEFAULT_CONSENT_VERSION),
            || (),
        )
        .expect("consent recorded");
    let events: Vec<Envelope> = (1..=count).map(exportable).collect();
    if !events.is_empty() {
        handle.state.append(&events).expect("appended");
    }
    (dir, handle)
}

fn outbox(handle: &EventsHandle) -> usize {
    handle
        .state
        .with_journal(|journal| journal.telemetry_pending(1_000), Vec::new)
        .expect("readable")
        .len()
}

/// What a scripted sink answers a batch with.
type Answer = Box<dyn Fn(&[SinkEvent]) -> Delivery + Send + Sync>;

/// A sink that answers whatever it was told to, and remembers what it saw.
struct Scripted {
    answer: Answer,
    calls: AtomicUsize,
    seen: Mutex<Vec<String>>,
    max_batch: usize,
}

impl Scripted {
    fn new(
        max_batch: usize,
        answer: impl Fn(&[SinkEvent]) -> Delivery + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            answer: Box::new(answer),
            calls: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
            max_batch,
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl Sink for Scripted {
    fn id(&self) -> SinkId {
        SinkId("scripted")
    }
    fn max_batch(&self) -> usize {
        self.max_batch
    }
    fn deliver(&self, events: &[SinkEvent]) -> Delivery {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.seen
            .lock()
            .expect("seen")
            .extend(events.iter().map(|event| event.name.clone()));
        (self.answer)(events)
    }
}

#[test]
fn a_drain_delivers_and_forgets() {
    let sink = Arc::new(Blackhole::new());
    let (_dir, handle) = worker(None, 3);
    assert_eq!(outbox(&handle), 3);

    let outcome = cycle(&handle.state, sink.as_ref());
    assert!(matches!(outcome, Cycle::Sent { .. }));
    assert_eq!(sink.delivered(), 3);
    assert_eq!(outbox(&handle), 0, "delivered events leave the outbox");

    assert!(matches!(cycle(&handle.state, sink.as_ref()), Cycle::Idle));
    assert_eq!(sink.batches(), 1, "an empty outbox costs no request");
}

/// Every event that leaves carries the dimensions and the two anonymous ids —
/// and the installation id is the same one for every event, which is what
/// makes an installation one person downstream.
#[test]
fn what_reaches_a_sink_is_the_projection_plus_the_build() {
    let seen: Arc<Mutex<Vec<SinkEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&seen);
    let sink = Scripted::new(500, move |events| {
        captured.lock().expect("seen").extend_from_slice(events);
        Delivery::Ok
    });
    let (_dir, handle) = worker(None, 2);
    cycle(&handle.state, sink.as_ref());

    let seen = seen.lock().expect("seen");
    assert_eq!(seen.len(), 2);
    let installation = seen[0].installation_id.clone().expect("minted");
    for event in seen.iter() {
        assert_eq!(event.name, "import.completed");
        assert_eq!(event.occurred_at_ms, T0, "when it happened");
        assert_eq!(
            event.installation_id.as_deref(),
            Some(installation.as_str())
        );
        assert_eq!(
            event.consent_version,
            koru_events::telemetry::DEFAULT_CONSENT_VERSION
        );
        assert_eq!(event.platform, std::env::consts::OS);
        assert!(!event.app_version.is_empty());
    }
}

#[test]
fn a_retry_deletes_nothing() {
    let sink = Scripted::new(500, |_| Delivery::Retry {
        after: Some(Duration::from_secs(7)),
        code: code::SERVER_ERROR,
    });
    let (_dir, handle) = worker(None, 4);

    let outcome = cycle(&handle.state, sink.as_ref());
    match outcome {
        Cycle::Failed { after, code } => {
            assert_eq!(after, Some(Duration::from_secs(7)), "the sink's own wait");
            assert_eq!(code, code::SERVER_ERROR);
        }
        _ => panic!("a failing sink must report a failure"),
    }
    assert_eq!(outbox(&handle), 4, "nothing is lost to a failure");
}

/// The one place data is deliberately dropped, and the reason it is safe: a
/// batch is refused as a whole, so the split is what tells one poison event
/// apart from the 3 good ones travelling with it.
#[test]
fn a_permanent_refusal_splits_until_the_offender_is_alone() {
    let poison = EventId(uuid::Uuid::from_u128(3)).to_string();
    let refused = poison.clone();
    let sink = Scripted::new(500, move |events| {
        if events.iter().any(|event| event.event_id == refused) {
            Delivery::Permanent {
                code: code::BAD_REQUEST,
            }
        } else {
            Delivery::Ok
        }
    });
    let (_dir, handle) = worker(None, 4);

    let outcome = cycle(&handle.state, sink.as_ref());
    assert!(matches!(outcome, Cycle::Sent { .. }));
    assert_eq!(
        outbox(&handle),
        0,
        "the three good events go and the refused one is dropped rather than left to block them"
    );
    // 1 whole batch + 2 halves + 2 quarters at most: bounded, not one request
    // per event.
    assert!(sink.calls() <= 7, "the split is binary, not linear");
}

/// A sink that stops answering mid-split leaves the rest of the batch alone —
/// pressing on would be guessing about events it never saw.
#[test]
fn a_retry_during_a_split_stops_the_split() {
    let calls = AtomicUsize::new(0);
    let sink = Scripted::new(500, move |_| {
        if calls.fetch_add(1, Ordering::Relaxed) == 0 {
            Delivery::Permanent {
                code: code::BAD_REQUEST,
            }
        } else {
            Delivery::Retry {
                after: None,
                code: code::TIMEOUT,
            }
        }
    });
    let (_dir, handle) = worker(None, 4);
    assert!(matches!(
        cycle(&handle.state, sink.as_ref()),
        Cycle::Failed { .. }
    ));
    assert_eq!(outbox(&handle), 4);
}

#[test]
fn a_full_batch_says_there_is_more_behind_it() {
    let sink = Scripted::new(2, |_| Delivery::Ok);
    let (_dir, handle) = worker(None, 5);
    assert!(matches!(
        cycle(&handle.state, sink.as_ref()),
        Cycle::Sent { remaining: true }
    ));
    assert_eq!(
        outbox(&handle),
        3,
        "one batch per cycle, at the sink's size"
    );
    cycle(&handle.state, sink.as_ref());
    assert!(matches!(
        cycle(&handle.state, sink.as_ref()),
        Cycle::Sent { remaining: false }
    ));
    assert_eq!(outbox(&handle), 0);
}

#[test]
fn a_withdrawn_consent_sends_nothing() {
    let sink = Arc::new(Blackhole::new());
    let (_dir, handle) = worker(None, 3);
    handle
        .state
        .with_journal(|journal| journal.set_consent(false, "consent-1"), || ())
        .expect("set");
    assert!(matches!(cycle(&handle.state, sink.as_ref()), Cycle::Idle));
    assert_eq!(sink.batches(), 0, "not even an empty request");
}

/// The invariant the whole module exists for: the journal is usable while a
/// slow sink is in flight. Held across `deliver`, this deadlocks and the test
/// times out.
#[test]
fn the_journal_is_not_locked_while_a_sink_is_working() {
    let state: Arc<OnceLock<Arc<EventsState>>> = Arc::new(OnceLock::new());
    let during = Arc::clone(&state);
    let sink = Scripted::new(500, move |_| {
        let state = during.get().expect("state is set before the cycle runs");
        // A publish, from this thread, in the middle of a delivery. It is the
        // same lock the drain took to read the outbox.
        state
            .append(&[exportable(500)])
            .expect("the journal answers during a delivery");
        Delivery::Ok
    });
    let (_dir, handle) = worker(None, 2);
    state.set(Arc::clone(&handle.state)).ok();

    assert!(matches!(
        cycle(&handle.state, sink.as_ref()),
        Cycle::Sent { .. }
    ));
    assert_eq!(
        outbox(&handle),
        1,
        "the two delivered rows are gone and the one published mid-flight remains"
    );
}

/// **A test run never sends.** Internal builds now report without being
/// configured, so this is no longer "nothing is set" — it is the guard that
/// says a suite is exempt. `cfg!(test)` covers this crate; the `no-sink`
/// feature covers every dependent, and each of those asserts its own.
#[test]
fn a_test_run_never_constructs_a_sink() {
    assert!(
        !sends_by_default(),
        "a suite must be exempt from the default"
    );
    assert!(
        configured_sink().is_none(),
        "a test run must never construct a real sink"
    );
    let (_dir, handle) = worker(None, 1);
    // Appending still wakes the SSE streams and simply has no drain to signal.
    handle.state.append(&[exportable(77)]).expect("appended");
    assert_eq!(outbox(&handle), 2);
}

#[test]
fn the_backoff_climbs_and_stays_inside_its_jitter() {
    let mut previous = Duration::ZERO;
    for attempt in 1..=6 {
        let delay = backoff(attempt);
        let base = LADDER[attempt as usize - 1];
        assert!(
            delay >= Duration::from_millis(base * 800)
                && delay <= Duration::from_millis(base * 1_200),
            "attempt {attempt}: {delay:?} is outside ±20% of {base}s"
        );
        assert!(delay > previous, "the ladder climbs");
        previous = delay;
    }
    // Past the end it caps rather than panicking on the index.
    assert!(backoff(50) <= Duration::from_millis(3_600 * 1_200));
}

/// **The gateway opens this worker from inside `#[tokio::main]`.** A
/// `reqwest::blocking::Client` built there takes the whole process down with
/// *"Cannot drop a runtime in a context where blocking is not allowed"* — it
/// stands a runtime up and tears it down, which tokio refuses inside an async
/// context. That is why a sink arrives as a closure and not as a value, and
/// this asserts where the closure actually runs.
#[tokio::test]
async fn a_sink_is_built_on_the_drains_thread_and_never_on_the_callers() {
    let (tx, rx) = std::sync::mpsc::channel();
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = EventsHandle::open_with(
        WorkerConfig::quiet(dir.path().to_path_buf()),
        Some(Box::new(move || {
            let _ = tx.send(
                std::thread::current()
                    .name()
                    .unwrap_or("<unnamed>")
                    .to_string(),
            );
            None
        })),
    )
    .expect("a journal");

    let built_on = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the factory runs");
    assert_eq!(
        built_on, "koru-metrics",
        "a sink is constructed on the drain's own thread, never on the caller's"
    );
    drop(handle);
}
