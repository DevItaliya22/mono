// SPDX-License-Identifier: MPL-2.0

//! **The opt-in contract, held by a sink that records every export attempt.**
//!
//! ADR-176-consenton's 2026-10-11 amendment: each
//! device's consent record is the only thing that allows an export. These
//! tests drive the real worker — the journal, the append path, the drain's
//! [`cycle`] and, where it matters, the drain's own thread — against a
//! [`Recorder`] that counts every call to `deliver`, successful or not. "Zero
//! exports" below means the sink was never *asked*, which is stronger than
//! "nothing arrived" and is not something a log line could show.
//!
//! A restart is simulated the way one happens: the handle is dropped (which
//! stops the drain thread and closes the journal) and a new worker opens the
//! same directory.

use std::collections::BTreeSet;

use rusqlite::Connection;

use super::*;

/// Every channel a build can carry. The contract is the same for each.
const CHANNELS: [&str; 5] = [
    "internal-debug",
    "internal-release",
    "nightly",
    "beta",
    "stable",
];

/// A sink that records every attempt and accepts it.
#[derive(Default)]
struct Recorder {
    attempts: Mutex<Vec<Vec<String>>>,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// How many times `deliver` was called.
    fn attempts(&self) -> usize {
        self.attempts.lock().expect("attempts").len()
    }

    /// Every event id any attempt carried.
    fn exported(&self) -> BTreeSet<String> {
        self.attempts
            .lock()
            .expect("attempts")
            .iter()
            .flatten()
            .cloned()
            .collect()
    }
}

impl Sink for Recorder {
    fn id(&self) -> SinkId {
        SinkId("recorder")
    }
    fn max_batch(&self) -> usize {
        500
    }
    fn deliver(&self, events: &[SinkEvent]) -> Delivery {
        self.attempts
            .lock()
            .expect("attempts")
            .push(events.iter().map(|event| event.event_id.clone()).collect());
        Delivery::Ok
    }
}

fn id(n: u128) -> String {
    EventId(uuid::Uuid::from_u128(n)).to_string()
}

fn ids(range: std::ops::RangeInclusive<u128>) -> BTreeSet<String> {
    range.map(id).collect()
}

/// Boot a worker on `dir` the way the gateway does — with the install
/// announcement and the root claim — but with no drain thread, so the test
/// drives [`cycle`] itself.
fn boot(dir: &std::path::Path) -> EventsHandle {
    EventsHandle::open_with(WorkerConfig::new(dir.to_path_buf()), None).expect("a journal")
}

fn record(handle: &EventsHandle, range: std::ops::RangeInclusive<u128>) {
    let events: Vec<Envelope> = range.map(exportable).collect();
    handle.state.append(&events).expect("appended");
}

/// Run the drain until it has nothing to do — more than enough passes for any
/// outbox these tests build.
fn drain(handle: &EventsHandle, sink: &Recorder) {
    for _ in 0..8 {
        if matches!(cycle(&handle.state, sink), Cycle::Idle) {
            return;
        }
    }
}

fn answer(handle: &EventsHandle, granted: bool) {
    handle
        .state
        .with_journal(
            |journal| journal.set_consent(granted, koru_events::telemetry::DEFAULT_CONSENT_VERSION),
            || (),
        )
        .expect("answer recorded");
}

fn status(handle: &EventsHandle) -> koru_events::wire::EventsStatus {
    handle.state.status().expect("status")
}

/// **No channel is permission.** Every channel has the same destination, so
/// the decision cannot hide in a channel test, and the sink a channel would
/// build is never asked before consent, after withdrawal, or across a
/// restart.
#[test]
fn zero_exports_before_consent_and_after_withdrawal_across_restarts_in_every_channel() {
    for channel in CHANNELS {
        assert!(
            has_destination(channel),
            "{channel}: every channel has the same destination; consent decides"
        );
        let dir = tempfile::tempdir().expect("temp dir");
        let sink = Recorder::new();

        // Fresh install: the person has not been asked.
        let handle = boot(dir.path());
        assert!(!status(&handle).telemetry_consent_answered, "{channel}");
        record(&handle, 1..=3);
        drain(&handle, &sink);
        assert_eq!(sink.attempts(), 0, "{channel}: nothing before consent");
        assert_eq!(status(&handle).outbox_pending, 0, "{channel}");

        // Restart without an answer: still nothing, still asked.
        drop(handle);
        let handle = boot(dir.path());
        assert!(!status(&handle).telemetry_consent_answered, "{channel}");
        record(&handle, 4..=5);
        drain(&handle, &sink);
        assert_eq!(sink.attempts(), 0, "{channel}: a restart is not consent");

        // Yes: only what is recorded after it leaves.
        answer(&handle, true);
        record(&handle, 6..=7);
        drain(&handle, &sink);
        assert_eq!(sink.exported(), ids(6..=7), "{channel}");

        // Restart: the yes persists and nobody is asked again.
        drop(handle);
        let handle = boot(dir.path());
        let after_restart = status(&handle);
        assert!(after_restart.telemetry_consent_answered, "{channel}");
        assert!(after_restart.telemetry_consent, "{channel}");
        record(&handle, 8..=8);
        drain(&handle, &sink);
        assert_eq!(sink.exported(), ids(6..=8), "{channel}");

        // Withdraw: nothing afterwards, and nothing after a restart either.
        answer(&handle, false);
        let attempts = sink.attempts();
        record(&handle, 9..=10);
        drain(&handle, &sink);
        drop(handle);
        let handle = boot(dir.path());
        let withdrawn = status(&handle);
        assert!(
            withdrawn.telemetry_consent_answered,
            "{channel}: no re-prompt"
        );
        assert!(!withdrawn.telemetry_consent, "{channel}");
        record(&handle, 11..=12);
        drain(&handle, &sink);
        assert_eq!(
            sink.attempts(),
            attempts,
            "{channel}: zero attempts after withdrawal, across a restart"
        );
        assert_eq!(sink.exported(), ids(6..=8), "{channel}");
    }
}

/// **Events recorded before consent are discarded, never uploaded**, even
/// when consent comes later — and so are events recorded after a withdrawal
/// when the person says yes again. The local journal keeps them all: only the
/// export is refused.
#[test]
fn events_from_before_a_yes_are_never_uploaded_after_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sink = Recorder::new();
    let handle = boot(dir.path());

    record(&handle, 1..=5);
    answer(&handle, true);
    record(&handle, 6..=6);
    drain(&handle, &sink);
    assert_eq!(sink.exported(), ids(6..=6), "the yes is not retroactive");

    answer(&handle, false);
    record(&handle, 7..=8);
    answer(&handle, true);
    record(&handle, 9..=9);
    drain(&handle, &sink);
    assert_eq!(
        sink.exported(),
        BTreeSet::from([id(6), id(9)]),
        "what was recorded while the answer was no stays home after a new yes"
    );

    let journal_rows = handle
        .state
        .with_journal(
            |journal| journal.read_after(0, 100, None, None),
            || unreachable!(),
        )
        .expect("readable")
        .events
        .len();
    assert!(
        journal_rows >= 9,
        "the local journal keeps its own records and retention: {journal_rows}"
    );
}

/// **Withdrawal takes effect immediately, including for anything queued.**
#[test]
fn withdrawal_drops_what_is_queued() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sink = Recorder::new();
    let handle = boot(dir.path());
    answer(&handle, true);
    record(&handle, 1..=4);
    assert_eq!(status(&handle).outbox_pending, 4);

    answer(&handle, false);
    assert_eq!(
        status(&handle).outbox_pending,
        0,
        "the queue goes with the yes"
    );
    drain(&handle, &sink);
    assert_eq!(
        sink.attempts(),
        0,
        "nothing queued before the withdrawal leaves"
    );

    answer(&handle, true);
    drain(&handle, &sink);
    assert_eq!(sink.attempts(), 0, "and a new yes does not bring it back");
}

/// **A withdrawal that lands while a batch is in flight stops the rest of
/// it.** The batch is read under the lock and delivered without it, so a
/// withdrawal can arrive in between; the drain asks again before every
/// request. Here the first request is refused as a whole (which makes the
/// drain split it and send the halves) and the person withdraws during that
/// first request: no half may follow.
#[test]
fn a_withdrawal_during_delivery_stops_the_batch() {
    let state: Arc<OnceLock<Arc<EventsState>>> = Arc::new(OnceLock::new());
    let during = Arc::clone(&state);
    let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
    let calls = Arc::clone(&seen);
    let sink = Scripted::new(500, move |events| {
        calls.lock().expect("calls").push(events.len());
        let state = during.get().expect("state is set before the cycle runs");
        state
            .with_journal(|journal| journal.set_consent(false, "2"), || ())
            .expect("withdrawn mid-flight");
        Delivery::Permanent { code: "refused" }
    });
    let (_dir, handle) = worker(None, 4);
    state.set(Arc::clone(&handle.state)).ok();

    assert!(matches!(cycle(&handle.state, sink.as_ref()), Cycle::Idle));
    assert_eq!(
        *seen.lock().expect("calls"),
        vec![4],
        "the request already in flight is the only one; no half follows it"
    );
    assert_eq!(outbox(&handle), 0);
}

/// The gate inside the split, on its own: a drain told consent is gone sends
/// nothing at all.
#[test]
fn deliver_asks_before_every_request() {
    let sink = Recorder::new();
    let events: Vec<SinkEvent> = Vec::new();
    let mut acked = Vec::new();
    assert!(matches!(
        deliver(sink.as_ref(), &events, &mut acked, &|| false),
        Settled::Withdrawn
    ));
    assert_eq!(sink.attempts(), 0);
}

/// Write a journal the way a build from before the opt-in contract left it:
/// consent "granted" by ADR-176's default (the key absent, or `"1"` written
/// without anybody choosing), an outbox full of events spooled under that
/// default, and an installation id.
fn synthesize_old_default(dir: &std::path::Path, legacy: Option<&str>) {
    // Create the file at the current schema, then rewrite the consent rows.
    drop(crate::journal::Journal::open(dir).expect("a journal"));
    let conn = Connection::open(dir.join("events.db")).expect("events.db");
    conn.execute(
        "DELETE FROM meta WHERE key IN ('telemetry_opt_in', 'telemetry_consent')",
        [],
    )
    .expect("old consent");
    if let Some(value) = legacy {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('telemetry_consent', ?1)",
            [value],
        )
        .expect("legacy key");
    }
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('consent_version', '1'), \
         ('installation_id', '00000000-0000-4000-8000-0000000000aa')",
        [],
    )
    .expect("old id");
    for n in 101..=103u128 {
        let event = koru_events::telemetry::project(
            &exportable(n),
            &koru_events::telemetry::Consent::granted(),
        )
        .expect("exportable");
        conn.execute(
            "INSERT INTO telemetry_outbox (event_id, body, queued_at_ms) VALUES (?1, ?2, ?3)",
            rusqlite::params![id(n), serde_json::to_string(&event).expect("json"), T0],
        )
        .expect("spooled under the old default");
    }
}

/// **An upgrade from the old default asks again and sends nothing until
/// yes**, and what the old default spooled is never sent — not before the
/// answer, and not after a yes.
#[test]
fn an_upgrade_from_the_old_default_asks_again_and_sends_nothing() {
    for legacy in [None, Some("1")] {
        let dir = tempfile::tempdir().expect("temp dir");
        synthesize_old_default(dir.path(), legacy);
        let sink = Recorder::new();

        let handle = boot(dir.path());
        let upgraded = status(&handle);
        assert!(
            !upgraded.telemetry_consent_answered,
            "{legacy:?}: the old default is not an answer, so the person is asked"
        );
        assert!(!upgraded.telemetry_consent, "{legacy:?}");
        assert_eq!(
            upgraded.outbox_pending, 0,
            "{legacy:?}: what the old default spooled is discarded on open"
        );
        record(&handle, 1..=2);
        drain(&handle, &sink);
        assert_eq!(sink.attempts(), 0, "{legacy:?}: nothing until yes");

        // An older build reopening this journal would read the legacy key:
        // it now refuses rather than defaulting to granted.
        drop(handle);
        let conn = Connection::open(dir.path().join("events.db")).expect("events.db");
        let legacy_now: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'telemetry_consent'",
                [],
                |row| row.get(0),
            )
            .expect("legacy key written");
        assert_eq!(legacy_now, "0", "{legacy:?}: a downgrade stays silent too");
        drop(conn);

        let handle = boot(dir.path());
        answer(&handle, true);
        drain(&handle, &sink);
        assert_eq!(
            sink.attempts(),
            0,
            "{legacy:?}: the old spool never leaves, even after a yes"
        );
        record(&handle, 3..=3);
        drain(&handle, &sink);
        assert_eq!(sink.exported(), ids(3..=3), "{legacy:?}");
        let id_now = handle
            .state
            .with_journal(|journal| journal.installation_id(), String::new)
            .expect("id");
        assert_ne!(
            id_now, "00000000-0000-4000-8000-0000000000aa",
            "{legacy:?}: the old default's installation id is not resumed"
        );
    }
}

/// **The drain's own thread obeys the record too.** The tests above drive
/// [`cycle`] by hand; this one hands the recorder to the worker as its sink,
/// exactly as `configured_sink` would, and lets the real thread run. Events
/// recorded before the yes are already sitting in the journal when the yes
/// arrives — the thread must deliver the later event and only that one.
#[test]
fn the_running_drain_sends_only_what_consent_allowed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sink = Recorder::new();
    let handle = EventsHandle::open_with(
        WorkerConfig::new(dir.path().to_path_buf()),
        Some(ready(Arc::clone(&sink) as Arc<dyn Sink>)),
    )
    .expect("a journal");

    record(&handle, 1..=3);
    answer(&handle, true);
    record(&handle, 4..=4);

    let deadline = Instant::now() + Duration::from_secs(10);
    while sink.exported().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        sink.exported(),
        ids(4..=4),
        "the thread delivered the post-consent event and nothing recorded before it"
    );

    answer(&handle, false);
    let attempts = sink.attempts();
    record(&handle, 5..=6);
    // A further yes-less append wakes the thread; give it ample time to run.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sink.attempts(), attempts, "nothing after the withdrawal");
    drop(handle);
}
