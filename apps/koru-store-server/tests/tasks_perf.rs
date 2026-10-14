// SPDX-License-Identifier: MPL-2.0

//! **What `/v1/tasks/query` costs over the wire** — plan P-Base (roadmap rev6
//! §5), case §6 (a) at the endpoint rather than at the statement.
//!
//! ```sh
//! cargo test --release -p koru-store-server --test tasks_perf -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `#[ignore]`d, and the reason is the same one `crates/koru-projection/tests/perf.rs`
//! gives: this starts a real daemon and publishes five thousand nodes through a
//! sync session, which is a minute nobody working on something else should pay.
//! The **structural** claims about this route already run unignored, next door in
//! `tasks_query.rs` — a warm query walks no document, a repeat accumulates no
//! rows — and this adds only the arithmetic they deliberately refuse to carry.
//!
//! ## What this measures, and what it cannot
//!
//! The corpus here is **one workspace, one document, N tasks**, not the roadmap
//! fixture. That is a limitation with a cause rather than a shortcut:
//! `koru_projection::fixtures` writes `projection.db` directly, and its own
//! documentation says why a corpus seeded that way cannot be shown to a running
//! daemon — the projector retires a workspace the authoritative store does not
//! have, so the first query that names it deletes it. Publishing 55,000 rows
//! through a real sync session to get the same shape would take a quarter of an
//! hour and would still be measuring Loro rather than the query.
//!
//! So the division of labour is: **this file prices the hop** — the HTTP
//! round trip, the JSON, the port and the process boundary — by comparing the
//! same endpoint at fifty tasks and at five thousand, and
//! `crates/koru-projection/tests/perf.rs` prices the **statement** at corpus
//! scale. The report adds them up and says which is which.
//!
//! ## The ratio is a gate now
//!
//! Gate (a.4), approved by the owner on 2026-09-01 at the value P-Base proposed
//! (`docs/features/tasks/PERF_BASELINE.md` §7). It is the only assertion here that is a
//! number; the counter beside it — `projected` unmoved across the whole timing
//! loop — is the load-bearing one and was always asserted. Loosening the ratio is
//! a decision to be argued in the report, not a fix to be applied here.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use koru_core::attributes::KEY_PRIORITY;
use koru_core::node::{AttrValue, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::tasks;
use koru_core::time::{DateOnly, Temporal};
use koru_store_client::{Client, TaskQuery};
use koru_workspace::store::StoreSession;

fn segment() -> &'static str {
    koru_channel::segment()
}

// ------------------------------------------------------------------ protocol

/// Three warmups, median of nine — roadmap §6, and the same protocol the
/// projection baseline uses so the two tables can sit beside each other.
const WARMUPS: usize = 3;
const RUNS: usize = 9;

struct Bench {
    median: Duration,
    min: Duration,
    max: Duration,
}

impl Bench {
    fn of(mut f: impl FnMut() -> usize) -> (Self, usize) {
        for _ in 0..WARMUPS {
            f();
        }
        let mut samples = Vec::with_capacity(RUNS);
        let mut answered = None;
        for _ in 0..RUNS {
            let at = Instant::now();
            let rows = f();
            samples.push(at.elapsed());
            match answered {
                None => answered = Some(rows),
                Some(first) => assert_eq!(first, rows, "the same query answered differently"),
            }
        }
        samples.sort();
        (
            Self {
                median: samples[samples.len() / 2],
                min: samples[0],
                max: samples[samples.len() - 1],
            },
            answered.unwrap_or(0),
        )
    }

    fn ms(&self) -> f64 {
        self.median.as_secs_f64() * 1000.0
    }

    fn spread(&self) -> f64 {
        (self.max.as_secs_f64() - self.min.as_secs_f64()) / self.median.as_secs_f64().max(1e-9)
    }
}

fn report(what: &str, bench: &Bench, rows: usize) {
    println!(
        "  {what:<44} {:>9.3} ms   (min {:>8.3}  max {:>8.3}  spread {:>5.1}%)   rows={rows}",
        bench.ms(),
        bench.min.as_secs_f64() * 1000.0,
        bench.max.as_secs_f64() * 1000.0,
        bench.spread() * 100.0,
    );
}

// ------------------------------------------------------------------- harness

struct Store {
    child: Option<Child>,
    root: PathBuf,
}

impl Store {
    fn start(root: &Path) -> Self {
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
        let child = command.spawn().expect("spawn koru-store-server");
        let deadline = Instant::now() + Duration::from_secs(30);
        while common::discovery(root).map(|d| Client::new(d).status().is_ok()) != Some(true) {
            assert!(
                Instant::now() < deadline,
                "the store never became reachable"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        Self {
            child: Some(child),
            root: root.to_path_buf(),
        }
    }

    fn client(&self) -> Client {
        Client::new(common::discovery(&self.root).expect("discover"))
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn root_for(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "koru-store-tasksperf-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(300);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn counter(client: &Client, route: &str, name: &str) -> u64 {
    client
        .debug(route)
        .unwrap_or_default()
        .get(name)
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// A workspace, a document, and `count` tasks in it — the shape `tasks_query.rs`
/// builds, at a size a baseline needs.
///
/// One publish, so the projector walks the document once rather than once per
/// task; consecutive dues from a fixed date, so the default sort is total and
/// nothing here reads a clock.
struct Fixture {
    #[allow(dead_code)]
    session: StoreSession,
    workspace: koru_store_core::ids::WorkspaceId,
}

impl Fixture {
    fn build(store: &Store, root: &Path, count: usize) -> Self {
        let client = store.client();
        let profile = client
            .create_profile("Tester", None, None, None)
            .expect("profile");
        let workspace = client
            .create_workspace(profile.id, "Acme")
            .expect("workspace");
        let document = client
            .create_document(workspace.id, "Tasks", "page")
            .expect("document");

        let (mut session, snapshot) =
            StoreSession::join(segment(), Some(root), workspace.id, document).expect("join");
        let mut doc = match WorkspaceDoc::from_snapshot(&snapshot) {
            Ok(doc) if doc.id().is_some() => doc,
            _ => WorkspaceDoc::create(
                koru_core::WorkspaceId::from_uuid(workspace.id.as_uuid()),
                "Acme",
            )
            .expect("create"),
        };
        let page = doc
            .insert(Parent::Root, Node::document("Tasks").unwrap())
            .unwrap();
        let built = Instant::now();
        for i in 0..count {
            let node = doc
                .insert(
                    Parent::Node(page),
                    Node::task(&format!("Task {:05}", i + 1)).unwrap(),
                )
                .unwrap();
            if i % 4 == 0 {
                doc.set_attribute(node, KEY_PRIORITY, AttrValue::Text("urgent".into()))
                    .expect("priority");
            }
            // A year of consecutive days, wrapped, so the corpus spans every
            // horizon a bucket window can name.
            let day = DateOnly::parse(&format!("2026-{:02}-{:02}", 1 + (i / 28) % 12, 1 + i % 28))
                .expect("a real day");
            tasks::set_due(&mut doc, node, Temporal::Date(day)).expect("due");
        }
        println!(
            "  built {count} tasks in {:.1} s",
            built.elapsed().as_secs_f64()
        );

        let published = Instant::now();
        if let Some(batch) = session.publish(&doc).expect("publish") {
            let deadline = Instant::now() + Duration::from_secs(300);
            while !session.is_durable(batch) {
                let _ = session.drain(&mut doc);
                assert!(Instant::now() < deadline, "the write never became durable");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        println!("  published in {:.1} s", published.elapsed().as_secs_f64());

        let projected = Instant::now();
        wait_for("the tasks to reach the read model", || {
            let page = client
                .query_tasks(&TaskQuery {
                    workspaces: vec![workspace.id],
                    limit: 1,
                    ..TaskQuery::default()
                })
                .expect("q");
            page.projected && !page.rows.is_empty()
        });
        println!(
            "  first projection in {:.1} s",
            projected.elapsed().as_secs_f64()
        );

        // The first projection answering is not the projector being done: a
        // debounce window closing behind the publish can legitimately project
        // the same corpus a second time moments later, and the timing loops
        // below assert the `projected` counter never moves. Wait for the
        // counter to hold still across a window longer than the projector's
        // own quiet period before handing the fixture out.
        let settled = Instant::now();
        let deadline = settled + Duration::from_secs(60);
        let mut last = counter(&client, "projection", "projected");
        loop {
            std::thread::sleep(Duration::from_millis(750));
            let now = counter(&client, "projection", "projected");
            if now == last {
                break;
            }
            last = now;
            assert!(Instant::now() < deadline, "the projector never settled");
        }
        println!("  settled in {:.1} s", settled.elapsed().as_secs_f64());

        Self {
            session,
            workspace: workspace.id,
        }
    }
}

// -------------------------------------------------------------- the baseline

/// **The endpoint's own median, at two sizes.**
///
/// Roadmap §6 case (a), asked of `/v1/tasks/query` rather than of the statement
/// behind it. Two claims come out of it and the report separates them: the
/// *floor* — what a page of a hundred rows costs when there is nothing to search
/// — is the hop, and it is the same at both sizes; and the *growth* is the
/// statement, which the projection baseline measures at corpus scale.
///
/// The counter check is here too rather than only next door, because a timing
/// run is exactly the place a document walk would hide: five thousand queries'
/// worth of milliseconds looks like a busy machine and not like a projector
/// re-reading a document.
#[test]
#[ignore = "a baseline run: cargo test --release -p koru-store-server --test tasks_perf -- --ignored"]
fn what_the_tasks_endpoint_costs_at_fifty_tasks_and_at_five_thousand() {
    let mut measured = Vec::new();
    for count in [50usize, 5_000] {
        println!("\n/v1/tasks/query @ {count} tasks");
        let root = root_for(&format!("n{count}"));
        let store = Store::start(&root);
        let client = store.client();
        let fixture = Fixture::build(&store, &root, count);

        let walked = counter(&client, "projection", "projected");
        let page = |limit: usize| TaskQuery {
            workspaces: vec![fixture.workspace],
            limit,
            ..TaskQuery::default()
        };

        let (one, one_rows) = Bench::of(|| client.query_tasks(&page(1)).expect("query").rows.len());
        report("one row (the hop, near enough)", &one, one_rows);

        let (hundred, hundred_rows) =
            Bench::of(|| client.query_tasks(&page(100)).expect("query").rows.len());
        report("a page of 100", &hundred, hundred_rows);

        let (five_hundred, five_hundred_rows) =
            Bench::of(|| client.query_tasks(&page(500)).expect("query").rows.len());
        report(
            "a page of 500 (the wire maximum)",
            &five_hundred,
            five_hundred_rows,
        );

        assert_eq!(
            counter(&client, "projection", "projected"),
            walked,
            "a timed run of warm queries walked a document — the numbers above are \
             measuring a projection, not a query"
        );
        measured.push((count, one.ms(), hundred.ms(), five_hundred.ms()));
    }

    let (_, small_one, small_hundred, _) = measured[0];
    let (_, big_one, big_hundred, _) = measured[1];
    println!(
        "\n  hop (1 row): {small_one:.2} ms at 50 tasks, {big_one:.2} ms at 5,000 \
         ({:.2}x)",
        big_one / small_one.max(1e-9)
    );
    println!(
        "  page of 100: {small_hundred:.2} ms at 50 tasks, {big_hundred:.2} ms at 5,000 \
         ({:.2}x)",
        big_hundred / small_hundred.max(1e-9)
    );
    // **Gate (a.4)**, approved 2026-09-01 (`docs/features/tasks/PERF_BASELINE.md` §7).
    // A hundred times the tasks; what it catches is the endpoint doing something
    // per *task* rather than per row — opening the document, or building the
    // whole list before paging it.
    //
    // Observed 8.2-16.9x over four runs, two of them after `attrs_probe` landed.
    // The ceiling is 40x and the margin is deliberately enormous, because **both
    // ends of this ratio are the noisy kind**: the 5,000 figures moved 28%
    // between the first two runs, and this harness builds its corpus by
    // publishing five thousand tasks through a real sync session, so a slow
    // first projection is inside the same process that then does the timing.
    //
    // It is also the one gate the covering index did **not** move — 14.3x and
    // 8.2x after, against 13.7x and 16.9x before — which is worth knowing before
    // reading a change here as a regression. This corpus is one document of
    // 5,000 tasks rather than the roadmap's 55,000 rows across 250, so the
    // candidate scan the index shortens is a much smaller share of it.
    assert!(
        big_hundred / small_hundred.max(1e-9) < 40.0,
        "a hundred times the tasks cost {:.0}x a page of a hundred — gate (a.4) is 40x, \
         against 8.2-16.9x observed. The endpoint is no longer answering a page",
        big_hundred / small_hundred.max(1e-9)
    );
}
