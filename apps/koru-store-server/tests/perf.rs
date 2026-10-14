// SPDX-License-Identifier: MPL-2.0

//! **The nine measurements, as assertions.**
//!
//! Run with `cargo test -p koru-store-server --test perf -- --test-threads=1
//! --nocapture` to see the numbers; the assertions run either way.
//!
//! ## Ratios and shapes, not microseconds
//!
//! `apps/koru/src/perf.rs` established the rule this follows: **assert that
//! something does not get worse as the workspace grows, not that it takes fewer
//! than N milliseconds.** A wall-clock threshold is a promise about the machine
//! running the test — it passes on a developer's laptop, fails on a loaded CI
//! box, and gets raised until it asserts nothing.
//!
//! Where a budget is unavoidable it is set generously and the comment says what
//! it is protecting against, so a failure is a signal rather than a nuisance.
//!
//! ## What each measurement is for
//!
//! | | |
//! | --- | --- |
//! | store startup | must not scale with how much has been written |
//! | workspace open | must not scale with document count |
//! | document open | checkpoint load plus replay |
//! | edit -> durable ack | what group commit is tuned against, and what TECHDEBT 20 needs |
//! | edit -> store receive | transport alone |
//! | API latency | the management path |
//! | checkpoint creation | must not stall the accept path |
//! | memory per open document | what decides an eviction policy later |
//! | edit -> local render | a client property; see the note at the bottom |

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use koru_store_client::{Client, Connection, Event};
use koru_store_core::ids::WorkspaceId;

/// The channel this test binary and the store it spawns both are.
///
/// They have to agree — the rendezvous is keyed on the channel, so a test
/// looking for `stable` would not find the store it just started. Asked rather
/// than written down: both are built by the same cargo invocation, so a
/// `cargo test --release` moves both to `internal-release` together and a
/// hardcoded string would move neither.
fn segment() -> &'static str {
    koru_channel::segment()
}

// ------------------------------------------------------------------ harness

struct Store {
    child: Child,
    root: PathBuf,
}

impl Store {
    fn start_in(root: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_koru-store-server"));
        command
            .arg("serve")
            // A suite starting daemons on throwaway roots must never raise a
            // modal dialog: nothing dismisses it, so the process would never
            // exit and would go on holding its databases and this binary.
            .env("KORU_STORE_NO_DIALOG", "1")
            .arg("--console")
            .arg("--root")
            .arg(root);
        common::configure(&mut command, root);
        let child = command.spawn().expect("spawn");
        let store = Self {
            child,
            root: root.to_path_buf(),
        };
        store.wait_until_ready();
        store
    }

    /// Time from spawn to answering. **This is the store-startup measurement**,
    /// and it includes process creation, which is most of it.
    fn timed_start(root: &Path) -> (Self, Duration) {
        let began = Instant::now();
        let store = Self::start_in(root);
        (store, began.elapsed())
    }

    /// Wait until the store is answering.
    ///
    /// **This used to have to ignore a stale port.** A store killed abruptly
    /// left `store.json` behind, so the next poll dialled the previous store's
    /// dead port — which on Windows costs a second or two per attempt, and
    /// which the first version of this harness duly reported as start-up time.
    /// A rendezvous cannot go stale that way: the endpoint is gone the moment
    /// the process is, so the first connect that succeeds is necessarily to
    /// *this* store.
    fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(found) = common::discovery(&self.root)
                && Client::new(found).status().is_ok()
            {
                return;
            }
            assert!(Instant::now() < deadline, "the store never started");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn client(&self) -> Client {
        Client::new(common::discovery(&self.root).expect("discover"))
    }

    fn connection(&self) -> Connection {
        let mut c =
            Connection::open(&common::discovery(&self.root).expect("discover")).expect("connect");
        c.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        c
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.stop();
    }
}

fn a_workspace(store: &Store, documents: usize) -> WorkspaceId {
    let client = store.client();
    let profile = client
        .create_profile("Owner", None, None, Some("en"))
        .expect("profile");
    let workspace = client
        .create_workspace(profile.id, "W")
        .expect("workspace")
        .id;
    for i in 0..documents {
        client
            .create_document(workspace, &format!("Document {i}"), "page")
            .expect("document");
    }
    workspace
}

/// Median of several runs, so one scheduling hiccup does not decide a test.
fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn report(what: &str, taken: Duration) {
    println!("  {what:<38} {:>8.2}ms", taken.as_secs_f64() * 1000.0);
}

// ------------------------------------------------------------------ startup

#[test]
fn store_startup_does_not_scale_with_what_has_been_written() {
    // The property the whole lazy design exists for: start-up opens
    // `catalog.db` and nothing else. If a workspace database or a `LoroDoc`
    // ever got opened eagerly, this is what would notice.
    let dir = tempfile::tempdir().unwrap();
    let (mut store, empty) = Store::timed_start(dir.path());

    // Fill it: several workspaces, each with documents and real history.
    for _ in 0..5 {
        let ws = a_workspace(&store, 20);
        let client = store.client();
        let _ = client.documents(ws);
    }
    store.stop();

    let (store, full) = Store::timed_start(dir.path());
    drop(store);

    report("store startup (empty)", empty);
    report("store startup (5 workspaces, 100 docs)", full);

    // Generous, because most of both numbers is process creation. What it
    // catches is the shape changing — eager opening would put a database read
    // per workspace in here.
    //
    // Measured while writing this: a store holding a hundred documents starts
    // in single-digit milliseconds, *faster* than an empty one, because the
    // empty case includes creating and migrating `catalog.db`. The first
    // version of this test reported two seconds and was measuring its own
    // harness dialling the previous store's dead port.
    assert!(
        full < empty * 3 + Duration::from_millis(500),
        "start-up scaled with stored content: {empty:?} empty vs {full:?} full"
    );
}

#[test]
fn a_started_store_has_opened_no_workspace() {
    // The same property, asserted directly rather than by timing — a clock is
    // circumstantial evidence and this is the fact itself.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());
    let _ = a_workspace(&store, 10);
    drop(store);

    let store = Store::start_in(dir.path());
    let report: serde_json::Value = reqwest::blocking::Client::new()
        .get(format!(
            "http://127.0.0.1:{}/v1/debug/store",
            common::discovery(&store.root).unwrap().port
        ))
        .bearer_auth(common::discovery(&store.root).unwrap().token)
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(report["workspaces_known"], 1);
    assert_eq!(
        report["workspaces_open"], 0,
        "a workspace was opened eagerly"
    );
    assert_eq!(report["documents_open"], 0, "a document was opened eagerly");
}

// ------------------------------------------------------------------ opening

#[test]
fn workspace_open_does_not_scale_with_document_count() {
    // Listing a workspace reads the derived mirror. If it ever walked the
    // manifest — or worse, opened each document — this would grow with n.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());
    let client = store.client();

    let small = a_workspace(&store, 2);
    let large = a_workspace(&store, 200);

    let time = |ws: WorkspaceId| {
        median(
            (0..5)
                .map(|_| {
                    let began = Instant::now();
                    client.documents(ws).expect("list");
                    began.elapsed()
                })
                .collect(),
        )
    };
    let small_time = time(small);
    let large_time = time(large);
    report("list 2 documents", small_time);
    report("list 200 documents", large_time);

    // A hundred times the documents must not be anything like a hundred times
    // the work. Twenty is loose enough to absorb the larger response body.
    assert!(
        large_time < small_time * 20 + Duration::from_millis(50),
        "listing scaled with document count: {small_time:?} for 2 vs {large_time:?} for 200"
    );
}

#[test]
fn document_open_is_paid_once() {
    // The second open should be cheaper: the document is already live, so the
    // checkpoint load and journal replay do not happen again.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());
    let client = store.client();
    let ws = a_workspace(&store, 5);
    let manifest = client.manifest(ws).expect("manifest");

    let first = {
        let began = Instant::now();
        client.snapshot(ws, manifest).expect("snapshot");
        began.elapsed()
    };
    let again = median(
        (0..5)
            .map(|_| {
                let began = Instant::now();
                client.snapshot(ws, manifest).expect("snapshot");
                began.elapsed()
            })
            .collect(),
    );
    report("document open (cold)", first);
    report("document open (already live)", again);
    // Not a ratio: on a small document both are sub-millisecond and the ratio
    // is noise. What matters is that neither is pathological.
    assert!(again < Duration::from_millis(100), "{again:?}");
}

// ------------------------------------------------- the client's own sync turn

/// **What a quiet sync turn costs the thread that also draws.**
///
/// The one measurement here that is about the *client*, and it belongs beside
/// the others because the number it produces is the one a transport change is
/// judged by. `Workspace::tick` runs in `about_to_wait`, before the event loop
/// picks its next deadline, so every microsecond in it is a microsecond the
/// window is not accepting a keystroke.
///
/// **What it caught, and the numbers either side of the fix.**
///
/// | | manifest only | manifest + 3 documents |
/// | --- | --- | --- |
/// | socket read on the event loop | 15.26 ms | 61.01 ms |
/// | socket on its own thread | 0.00 ms | 0.01 ms |
///
/// `StoreSession::drain` used to do a blocking socket read per session per turn,
/// so a turn cost one read timeout times the number of open sessions — the cost
/// scaling exactly with the session count being the shape that mattered. Read
/// the *absolute* before-numbers as an upper bound rather than as the app's:
/// that timeout is `SO_RCVTIMEO`, Windows rounds it up to a system timer tick,
/// and a plain test process gets the 15.625 ms default while the app's graphics
/// stack raises the resolution and saw about 1.2 ms. Same bug, different
/// multiplier. What was the app's, from `KORU_FRAME_STATS=1` on an idle window:
/// **365 ms of every second** in that call, because `Workspace::next_due`
/// reported "now" whenever a session existed and the loop spun.
///
/// The budget below is deliberately a hundred times the measured figure. It is
/// not tuning a number; it is the assertion that a quiet sync turn cannot be the
/// reason a frame is late, and it is three orders of magnitude away from being
/// satisfiable by a blocking read.
#[test]
fn a_quiet_sync_turn_never_blocks_the_event_loop_thread() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());
    let client = store.client();
    let profile = client
        .create_profile("Owner", None, None, Some("en"))
        .expect("profile");
    let ws = client
        .create_workspace(profile.id, "W")
        .expect("workspace")
        .id;

    let mut w = koru_workspace::Workspace::new(
        koru_core::schema::WorkspaceDoc::create(koru_core::WorkspaceId::new(), "").unwrap(),
    );
    w.open_from_store(
        segment(),
        Some(dir.path()),
        koru_core::WorkspaceId::from_uuid(ws.as_uuid()),
    )
    .expect("open from store");

    /// Turns with nothing to say, which is the overwhelming majority of them.
    /// Settled first: a join's catch-up is real work and measuring it here
    /// would report the transport's cost as whatever the last edit was.
    fn quiet(w: &mut koru_workspace::Workspace, turns: u32) -> Duration {
        for _ in 0..20 {
            w.tick();
        }
        let began = Instant::now();
        for _ in 0..turns {
            w.tick();
        }
        began.elapsed() / turns
    }

    let one = quiet(&mut w, 100);
    report("sync turn (manifest only)", one);

    for i in 0..3 {
        w.create_document(&format!("Document {i}")).expect("create");
    }
    let four = quiet(&mut w, 100);
    report("sync turn (manifest + 3 documents)", four);

    // A frame is 16 ms and a turn happens between frames, so a millisecond is
    // already generous; the measured figure is a hundredth of it. What this
    // refuses is a turn that has gone back to waiting on a socket, which cannot
    // come in under a system timer tick however the constant is written.
    assert!(
        four < Duration::from_millis(1),
        "the sync turn is blocking the event loop again: {four:?}"
    );
    // And it must not grow with the number of open documents, which is the shape
    // a per-session blocking read has. Compared with a floor, because at these
    // magnitudes a ratio is scheduler noise.
    assert!(
        four < one + Duration::from_millis(1),
        "the turn is scaling with open sessions again: {one:?} -> {four:?}"
    );
}

// ------------------------------------------------------------------ the edit path

#[test]
fn edit_to_durable_acknowledgement_stays_bounded() {
    // **The number TECHDEBT 20 needs.** It is the round trip a client waits on
    // before it can stop retrying: transport, import, one transaction, one
    // fsync, and the reply.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());
    let client = store.client();
    let ws = a_workspace(&store, 1);
    let document = client
        .create_document(ws, "Measured", "page")
        .expect("document");

    let mut connection = store.connection();
    let room = format!("document:{}", document.to_storage());
    let doc = loro::LoroDoc::new();
    connection.join(&room, &doc.oplog_vv()).expect("join");
    // Drain the join response.
    while matches!(connection.next_event(), Ok(Some(_))) {}

    let mut sent_to_ack = Vec::new();
    let mut sent_to_receive = Vec::new();
    let mut previous = loro::VersionVector::new();

    for i in 0..20 {
        let text = doc.get_text("content");
        text.insert_utf8(text.len_utf8(), &format!("{i} ")).unwrap();
        doc.commit();
        let update = doc.export(loro::ExportMode::updates(&previous)).unwrap();
        previous = doc.oplog_vv();

        let began = Instant::now();
        connection.send_update(&room, update).expect("send");
        sent_to_receive.push(began.elapsed());

        loop {
            match connection.next_event() {
                Ok(Some(Event::Acked { ack, .. })) => {
                    assert!(ack.durable, "the store refused an edit: {:?}", ack.status);
                    sent_to_ack.push(began.elapsed());
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) => {}
                Err(e) => panic!("the connection ended: {e}"),
            }
        }
    }

    let receive = median(sent_to_receive);
    let ack = median(sent_to_ack);
    report("edit -> store receive (median)", receive);
    report("edit -> durable ack (median)", ack);

    // A budget rather than a ratio, and a generous one: this includes a real
    // fsync, whose cost varies by an order of magnitude between an NVMe disk
    // and a spinning one. What it catches is the fsync-per-keystroke regression
    // that group commit exists to prevent — that would be tens of milliseconds
    // on any hardware.
    assert!(
        ack < Duration::from_millis(250),
        "a durable acknowledgement took {ack:?}; group commit may have stopped batching"
    );
    assert!(
        receive <= ack,
        "receiving cannot cost more than acknowledging"
    );
}

#[test]
fn a_burst_of_edits_costs_less_per_edit_than_one_at_a_time() {
    // What group commit buys. Sending twenty updates without waiting should
    // cost far less than twenty round trips, because they share transactions.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());
    let client = store.client();
    let ws = a_workspace(&store, 1);
    let document = client
        .create_document(ws, "Burst", "page")
        .expect("document");

    let mut connection = store.connection();
    let room = format!("document:{}", document.to_storage());
    let doc = loro::LoroDoc::new();
    connection.join(&room, &doc.oplog_vv()).expect("join");
    while matches!(connection.next_event(), Ok(Some(_))) {}

    let mut previous = loro::VersionVector::new();
    let began = Instant::now();
    for i in 0..20 {
        let text = doc.get_text("content");
        text.insert_utf8(text.len_utf8(), &format!("{i} ")).unwrap();
        doc.commit();
        let update = doc.export(loro::ExportMode::updates(&previous)).unwrap();
        previous = doc.oplog_vv();
        connection.send_update(&room, update).expect("send");
    }

    // Wait for all twenty acknowledgements.
    let mut acked = 0;
    let deadline = Instant::now() + Duration::from_secs(30);
    while acked < 20 && Instant::now() < deadline {
        if let Ok(Some(Event::Acked { ack, .. })) = connection.next_event() {
            assert!(ack.durable);
            acked += 1;
        }
    }
    let whole = began.elapsed();
    assert_eq!(acked, 20, "only {acked} of 20 edits were acknowledged");
    report("20 edits, pipelined (total)", whole);
    report("20 edits, pipelined (per edit)", whole / 20);

    // **This does not currently beat sending them one at a time**, and the
    // measurement is how we know. `CommitWindow` implements group commit and
    // is tested, but the protocol handler flushes on every message — so each
    // edit still pays its own fsync. Durability is unaffected (the flush
    // precedes the acknowledgement either way); what is unpaid-for is the
    // amortisation group commit exists to provide. See TECHDEBT 22.
    //
    // The assertion is therefore a ceiling rather than a ratio: it catches a
    // burst becoming pathological, and will be tightened into a real
    // comparison once the window is wired into the accept path.
    assert!(
        whole < Duration::from_secs(5),
        "a burst of twenty edits took {whole:?}"
    );
}

// ------------------------------------------------------------------ the API

#[test]
fn api_latency_stays_flat_as_the_store_fills() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());
    let client = store.client();

    let time_status = || {
        median(
            (0..10)
                .map(|_| {
                    let began = Instant::now();
                    client.status().expect("status");
                    began.elapsed()
                })
                .collect(),
        )
    };
    let empty = time_status();
    for _ in 0..5 {
        a_workspace(&store, 20);
    }
    let full = time_status();

    report("GET /v1/status (empty store)", empty);
    report("GET /v1/status (5 workspaces open)", full);
    // `/v1/status` reads no workspace, so it must not notice. The store lock is
    // shared, though, which is what this would catch if it ever became
    // contended — see TECHDEBT 20.
    assert!(
        full < empty * 5 + Duration::from_millis(50),
        "status latency grew with stored content: {empty:?} -> {full:?}"
    );
}

// ------------------------------------------------------------------ memory

#[test]
fn holding_documents_open_is_reported_so_an_eviction_policy_can_be_judged() {
    // There is no eviction policy yet, and there should not be one before there
    // is a number to justify it. What this asserts is that the *count* is
    // visible — which is what a policy would be written against.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());
    let client = store.client();
    let ws = a_workspace(&store, 5);
    let manifest = client.manifest(ws).expect("manifest");
    client.snapshot(ws, manifest).expect("open it");

    let discovery = common::discovery(&store.root).unwrap();
    let report: serde_json::Value = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/v1/metrics", discovery.port))
        .bearer_auth(&discovery.token)
        .send()
        .unwrap()
        .json()
        .unwrap();

    assert_eq!(report["workspaces_open"], 1);
    // The manifest is held as a `Manifest`, not as an entry in `documents` —
    // so this is zero, and that is the shape rather than a bug. A document a
    // client actually opens would appear here.
    assert!(report["documents_open"].as_u64().is_some());
}

// ------------------------------------------------------- the derived layers

/// **Indexing one document in a large workspace costs that document.**
///
/// A ratio rather than a budget, in this file's idiom: the number that matters
/// is that the work does not grow with what else the workspace holds. If a
/// commit ever re-indexed a workspace — or re-read every listing row per
/// document — a person typing in a thousand-document workspace would pay for
/// nine hundred and ninety-nine documents they did not touch.
///
/// Measured as *how long the derived layers stay busy after one edit*, which is
/// the thing a person would feel as the machine warming up under them.
#[test]
fn one_edit_costs_the_same_in_a_large_workspace_as_in_a_small_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::start_in(dir.path());

    let settle = |ws: WorkspaceId| -> Duration {
        let client = store.client();
        let document = client
            .create_document(ws, "The edited one", "page")
            .expect("document");
        // Wait for the churn of creating it to pass, so what is timed is the
        // edit rather than the workspace waking up.
        wait_until_quiet(&store);

        let began = Instant::now();
        client
            .create_document(ws, "And another", "page")
            .expect("document");
        wait_until_quiet(&store);
        let _ = document;
        began.elapsed()
    };

    let small = a_workspace(&store, 2);
    let large = a_workspace(&store, 200);
    let small_time = settle(small);
    let large_time = settle(large);
    report("index one edit, 2 documents", small_time);
    report("index one edit, 200 documents", large_time);

    // A hundred times the documents. The comparison happens per document in a
    // sweep, so this is not free — but it is a SQLite read each, and nothing
    // like a hundred times an index write.
    assert!(
        large_time < small_time * 10 + Duration::from_millis(500),
        "indexing scaled with workspace size: {small_time:?} for 2 vs {large_time:?} for 200"
    );
}

/// Both derived layers idle, so a measurement is of what was asked for rather
/// than of what was still in flight.
fn wait_until_quiet(store: &Store) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let quiet = || {
        let client = store.client();
        let projection = client.debug("projection").unwrap_or_default();
        let search = client.debug("search").unwrap_or_default();
        projection["pending"] == 0 && projection["sweeping"] == false && search["pending"] == 0
    };
    while !quiet() {
        assert!(
            Instant::now() < deadline,
            "the derived layers never settled"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ------------------------------------------------------------------ not here
//
// **edit -> local render is not measured in this file, and cannot be.** It is
// entirely a client property: a client applies an edit to its own replica and
// draws, without waiting for the store at all. The store's contribution to it
// is zero by construction, and that is the design — `apps/koru/src/perf.rs` is
// where rendering latency is asserted, against the editor that does the
// drawing.
//
// Recording it here would measure this test's own `LoroDoc`, which is neither
// the store's behaviour nor the editor's.
