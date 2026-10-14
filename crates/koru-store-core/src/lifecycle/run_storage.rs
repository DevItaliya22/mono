// SPDX-License-Identifier: MPL-2.0

//! **The storage harness for the delegated Run document** — what a run's history
//! costs through the one seam the store loads and transports a document by:
//! [`LiveDocument::from_snapshot`], [`LiveDocument::export_snapshot`] and
//! [`LiveDocument::export_updates_since`]. The Run storage contract is in
//! `docs/architecture/DATAMODEL.md`; this file measures its physical
//! representation.
//!
//! ```sh
//! cargo test --release -p koru-store-core run_storage -- --ignored --nocapture --test-threads=1
//! ```
//!
//! ## The two candidates
//!
//! - **One document.** A `run` map (identity, status, executing device, the
//!   economics summary) and a `timeline` list of lean entries in one `LoroDoc`.
//!   The document is the load and transport unit, so every cost is the whole
//!   run's.
//! - **Header plus pages.** The same `run` map in a small header document
//!   whose `pages` list names history-page documents, each holding a bounded
//!   slice of the timeline. Opening costs the header and one page; a first
//!   replication costs every page.
//!
//! ## An entry is a lean step (D45)
//!
//! An id, a step type, references to what it touched, a time and bounded
//! counts — never content. Two spellings are measured because they differ by
//! more than a factor of two: **string** ids (the 36-character storage form
//! every other Koru identity is written in) and **binary** ids (16 bytes,
//! short keys). The proposal picks one on the figures.
//!
//! ## Ratios, not wall clocks
//!
//! The unignored tests assert bytes, which are deterministic: an appended step
//! stays bounded whatever the history, and a snapshot grows no worse than
//! linearly. Durations are printed by the ignored harness and never asserted —
//! `apps/koru-store-server/tests/perf.rs` set that rule.

use std::time::{Duration, Instant};

use loro::{ExportMode, LoroDoc, LoroValue};

use super::LiveDocument;
use crate::ids::DocumentId;
use crate::manifest::Manifest;
use crate::model::Timestamp;

/// The history lengths the roadmap asks for.
const SIZES: [usize; 4] = [100, 1_000, 10_000, 50_000];
/// Entries per history page in the paged candidate.
const PAGE: usize = 1_000;
/// Entries a surface draws at once — the "first activity page" of §7.9.
const WINDOW: usize = 50;

const K_RUN: &str = "run";
const K_TIMELINE: &str = "timeline";
const K_PAGES: &str = "pages";

/// A step type from a closed vocabulary; the mix stands in for a real run's.
const STEPS: [&str; 6] = [
    "turn_started",
    "tool_completed",
    "tool_completed",
    "tool_completed",
    "turn_completed",
    "usage",
];

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Ids {
    /// `DocumentId::to_storage()` — 36 characters, as every id Koru stores.
    Text,
    /// The 16 raw bytes, and one-letter keys.
    Binary,
}

impl Ids {
    fn name(self) -> &'static str {
        match self {
            Self::Text => "text ids",
            Self::Binary => "binary ids",
        }
    }
}

fn id_value(ids: Ids) -> LoroValue {
    let id = uuid::Uuid::new_v4();
    match ids {
        Ids::Text => LoroValue::from(id.hyphenated().to_string()),
        Ids::Binary => LoroValue::from(id.as_bytes().to_vec()),
    }
}

/// One lean timeline entry: id, step type, references, time, bounded counts.
fn entry(i: usize, ids: Ids) -> LoroValue {
    let step = STEPS[i % STEPS.len()];
    // Most steps touch one thing; a turn boundary touches none.
    let refs: Vec<LoroValue> = if step.starts_with("turn") {
        Vec::new()
    } else {
        vec![id_value(ids)]
    };
    let at = 1_770_000_000_000_i64 + i as i64 * 1_500;
    let pairs: Vec<(String, LoroValue)> = match ids {
        Ids::Text => vec![
            ("id".into(), id_value(ids)),
            ("step".into(), step.into()),
            ("refs".into(), refs.into()),
            ("at".into(), at.into()),
            ("tokens_in".into(), LoroValue::from((i % 900) as i64)),
            ("tokens_out".into(), LoroValue::from((i % 300) as i64)),
        ],
        Ids::Binary => vec![
            ("i".into(), id_value(ids)),
            ("s".into(), step.into()),
            ("r".into(), refs.into()),
            ("t".into(), at.into()),
            (
                "c".into(),
                vec![
                    LoroValue::from((i % 900) as i64),
                    LoroValue::from((i % 300) as i64),
                ]
                .into(),
            ),
        ],
    };
    let map: std::collections::HashMap<String, LoroValue> = pairs.into_iter().collect();
    LoroValue::from(map)
}

/// The header every candidate carries: identity, status, executing device
/// and the economics summary a projection keeps current.
fn write_header(doc: &LoroDoc, ids: Ids) {
    let run = doc.get_map(K_RUN);
    run.insert("id", id_value(ids)).unwrap();
    run.insert("status", "running").unwrap();
    run.insert("device", id_value(ids)).unwrap();
    run.insert("agent", "codex").unwrap();
    run.insert("scope", id_value(ids)).unwrap();
    run.insert("tokens_in", 0_i64).unwrap();
    run.insert("tokens_out", 0_i64).unwrap();
    run.insert("turns", 0_i64).unwrap();
    doc.commit();
}

/// Append one step **as a run does it** — one commit per step, because each
/// step is flushed to the store on its own and the op log keeps a change per
/// commit. Batching commits would flatter the figures.
fn append(doc: &LoroDoc, list: &str, i: usize, ids: Ids) {
    doc.get_list(list).push(entry(i, ids)).unwrap();
    doc.commit();
}

/// Candidate A: the whole run in one document.
fn one_document(n: usize, ids: Ids) -> LoroDoc {
    let doc = LoroDoc::new();
    write_header(&doc, ids);
    for i in 0..n {
        append(&doc, K_TIMELINE, i, ids);
    }
    doc
}

/// Candidate B: a header naming its pages, and the pages.
fn header_and_pages(n: usize, ids: Ids) -> (LoroDoc, Vec<LoroDoc>) {
    let header = LoroDoc::new();
    write_header(&header, ids);
    let mut pages = Vec::new();
    for start in (0..n).step_by(PAGE) {
        let page = LoroDoc::new();
        for i in start..(start + PAGE).min(n) {
            append(&page, K_TIMELINE, i, ids);
        }
        header.get_list(K_PAGES).push(id_value(ids)).unwrap();
        header.commit();
        pages.push(page);
    }
    (header, pages)
}

fn snapshot_of(doc: &LoroDoc) -> Vec<u8> {
    doc.export(ExportMode::Snapshot).unwrap()
}

/// The bytes one appended step costs a replica that already has everything
/// before it — what §7.9 calls the bounded, incremental case.
fn appended_step_bytes(doc: &LoroDoc, list: &str, i: usize, ids: Ids) -> usize {
    let before = doc.oplog_vv();
    append(doc, list, i, ids);
    doc.export(ExportMode::updates(&before)).unwrap().len()
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn timed<T>(f: impl Fn() -> T) -> (T, Duration) {
    let mut samples = Vec::new();
    let mut last = None;
    for _ in 0..3 {
        let began = Instant::now();
        last = Some(f());
        samples.push(began.elapsed());
    }
    (last.unwrap(), median(samples))
}

/// Through the store's own seam, so the number is the store's number.
fn store_import(snapshot: &[u8]) -> LiveDocument {
    LiveDocument::from_snapshot(DocumentId::new(), snapshot).unwrap()
}

/// Read the newest `WINDOW` entries the way a surface would: by index, from
/// the end, without exporting anything.
fn read_window(doc: &LoroDoc) -> usize {
    let list = doc.get_list(K_TIMELINE);
    let len = list.len();
    let mut seen = 0;
    for i in len.saturating_sub(WINDOW)..len {
        if list.get(i).is_some() {
            seen += 1;
        }
    }
    seen
}

struct Row {
    entries: usize,
    snapshot: usize,
    all_updates: usize,
    export: Duration,
    import: Duration,
    window: Duration,
    step: usize,
}

fn measure_one_document(n: usize, ids: Ids) -> Row {
    let doc = one_document(n, ids);
    let (snapshot, export) = timed(|| snapshot_of(&doc));
    let all_updates = doc.export(ExportMode::all_updates()).unwrap().len();
    let (live, import) = timed(|| store_import(&snapshot));
    let (seen, window) = timed(|| read_window(live.doc()));
    assert_eq!(seen, WINDOW.min(n));
    let step = appended_step_bytes(&doc, K_TIMELINE, n, ids);
    Row {
        entries: n,
        snapshot: snapshot.len(),
        all_updates,
        export,
        import,
        window,
        step,
    }
}

struct PagedRow {
    entries: usize,
    header: usize,
    newest_page: usize,
    all_pages: usize,
    open: Duration,
    step: usize,
    manifest_growth: usize,
}

fn measure_header_and_pages(n: usize, ids: Ids) -> PagedRow {
    let (header, pages) = header_and_pages(n, ids);
    let header_bytes = snapshot_of(&header);
    let page_bytes: Vec<Vec<u8>> = pages.iter().map(snapshot_of).collect();
    let newest = page_bytes.last().map_or(0, Vec::len);
    let all_pages: usize = page_bytes.iter().map(Vec::len).sum();
    // Opening a run: the header and its newest page, both through the store's
    // seam, then the first window read.
    let (_, open) = timed(|| {
        let _header = store_import(&header_bytes);
        let page = store_import(page_bytes.last().unwrap());
        read_window(page.doc())
    });
    let newest_len = n - (pages.len() - 1) * PAGE;
    let step = if newest_len < PAGE {
        appended_step_bytes(pages.last().unwrap(), K_TIMELINE, n, ids)
    } else {
        // A full page: the next step opens a new page, which costs a page
        // document plus a header edit. Measured as the page's first entry
        // plus the header's list push.
        let page = LoroDoc::new();
        let page_cost = appended_step_bytes(&page, K_TIMELINE, n, ids);
        let before = header.oplog_vv();
        header.get_list(K_PAGES).push(id_value(ids)).unwrap();
        header.commit();
        page_cost + header.export(ExportMode::updates(&before)).unwrap().len()
    };
    // Every page is a manifest entry; that is a cost the one-document
    // candidate does not pay.
    let manifest_growth = manifest_bytes(1 + pages.len()) - manifest_bytes(1);
    PagedRow {
        entries: n,
        header: header_bytes.len(),
        newest_page: newest,
        all_pages,
        open,
        step,
        manifest_growth,
    }
}

/// A manifest listing `entries` documents of the run kind, in bytes.
fn manifest_bytes(entries: usize) -> usize {
    let mut manifest = Manifest::create(
        crate::ids::WorkspaceId::new(),
        "W",
        Timestamp::from_millis(0),
    )
    .unwrap();
    for _ in 0..entries {
        manifest
            .add_document(DocumentId::new(), "Run", "koru-run", None)
            .unwrap();
    }
    manifest.export_snapshot().unwrap().len()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

fn ratio(a: Duration, b: Duration) -> f64 {
    if b.is_zero() {
        f64::NAN
    } else {
        a.as_secs_f64() / b.as_secs_f64()
    }
}

// ------------------------------------------------------------------ the figures

/// The table the proposal is built from. ASCII only, for the same reason
/// `stats::report` is.
#[ignore = "P1.0's measurement: cargo test --release -p koru-store-core run_storage -- --ignored --nocapture --test-threads=1"]
#[test]
fn run_storage_figures() {
    for ids in [Ids::Text, Ids::Binary] {
        eprintln!();
        eprintln!("one document, {} (window = {WINDOW} entries)", ids.name());
        eprintln!(
            "  {:>8} {:>10} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>7}",
            "entries",
            "snapshot",
            "updates",
            "B/entry",
            "export",
            "x100",
            "import",
            "x100",
            "window",
            "step B"
        );
        let mut base: Option<Row> = None;
        for n in SIZES {
            let row = measure_one_document(n, ids);
            let b = base.as_ref().unwrap_or(&row);
            eprintln!(
                "  {:>8} {:>10} {:>10} {:>9.1} {:>7.2}ms {:>9.1} {:>7.2}ms {:>9.1} {:>7.3}ms {:>7}",
                row.entries,
                row.snapshot,
                row.all_updates,
                row.snapshot as f64 / row.entries as f64,
                ms(row.export),
                ratio(row.export, b.export),
                ms(row.import),
                ratio(row.import, b.import),
                ms(row.window),
                row.step
            );
            if base.is_none() {
                base = Some(row);
            }
        }

        eprintln!();
        eprintln!(
            "header + pages of {PAGE}, {} (open = header + newest page + window)",
            ids.name()
        );
        eprintln!(
            "  {:>8} {:>8} {:>10} {:>10} {:>9} {:>9} {:>7} {:>10}",
            "entries", "header", "newest pg", "all pages", "open", "x100", "step B", "manifest+"
        );
        let mut base: Option<PagedRow> = None;
        for n in SIZES {
            let row = measure_header_and_pages(n, ids);
            let b = base.as_ref().unwrap_or(&row);
            eprintln!(
                "  {:>8} {:>8} {:>10} {:>10} {:>7.2}ms {:>9.1} {:>7} {:>10}",
                row.entries,
                row.header,
                row.newest_page,
                row.all_pages,
                ms(row.open),
                ratio(row.open, b.open),
                row.step,
                row.manifest_growth
            );
            if base.is_none() {
                base = Some(row);
            }
        }
    }
}

// ------------------------------------------------------------------ the properties

/// The criterion the proposal accepted for one document (§1.6), as the
/// constants the assertions below and `koru-store-sqlite`'s
/// `run_document.rs` — which asserts the same over the real shape — read.
const OPEN_BYTES_PER_ENTRY: usize = 256;
const OPEN_BYTES_HEADER: usize = 4 * 1024;
const STEP_BYTES: usize = 512;

/// The bytes a second replica imports for one appended step do not grow with
/// the history behind it, and stay under the criterion's 512. This is §7.9's
/// "bounded — incremental" row, and it is what makes a long run cheap to
/// keep in sync once it has been synced.
#[test]
fn an_appended_step_costs_the_same_bytes_on_a_long_run_as_on_a_short_one() {
    for ids in [Ids::Text, Ids::Binary] {
        let short = one_document(100, ids);
        let long = one_document(5_000, ids);
        let on_short = appended_step_bytes(&short, K_TIMELINE, 100, ids);
        let on_long = appended_step_bytes(&long, K_TIMELINE, 5_000, ids);
        assert!(
            on_long <= on_short * 2 && on_long <= STEP_BYTES,
            "{}: one step cost {on_long} bytes after 5,000 entries and {on_short} after 100",
            ids.name()
        );
    }
}

/// A snapshot grows no worse than linearly in its entries, so "keep every
/// step" (D32) is a growing store and never a growing per-step tax — and
/// the bytes to open are within 256 an entry plus 4 KiB at every size.
#[test]
fn a_snapshot_grows_no_worse_than_linearly_with_its_entries() {
    for ids in [Ids::Text, Ids::Binary] {
        let bytes = |n: usize| snapshot_of(&one_document(n, ids)).len();
        for n in [100usize, 1_000, 5_000] {
            let budget = OPEN_BYTES_PER_ENTRY * n + OPEN_BYTES_HEADER;
            assert!(
                bytes(n) <= budget,
                "{}: {} bytes to open {n} entries against a budget of {budget}",
                ids.name(),
                bytes(n)
            );
        }
        let small = bytes(100) as f64 / 100.0;
        let large = bytes(5_000) as f64 / 5_000.0;
        assert!(
            large <= small * 1.25,
            "{}: {large:.1} bytes per entry at 5,000 against {small:.1} at 100",
            ids.name()
        );
    }
}

/// Import is no worse than linear above a thousand entries: ten times the
/// entries cost at most twenty times the time, measured in one process —
/// a ratio, loose because the fixed cost below a thousand still shows, and
/// never a wall-clock budget.
#[test]
fn import_is_no_worse_than_linear_above_a_thousand_entries() {
    let small = snapshot_of(&one_document(1_000, Ids::Text));
    let large = snapshot_of(&one_document(10_000, Ids::Text));
    let (_, one) = timed(|| store_import(&small));
    let (_, ten) = timed(|| store_import(&large));
    let ratio = ratio(ten, one);
    assert!(
        ratio <= 20.0,
        "importing 10,000 entries cost {ratio:.1}x importing 1,000 ({ten:?} against {one:?})"
    );
}

/// A window read costs no bytes once the document is open: the snapshot it
/// was imported from is gone, nothing is exported or imported again, and
/// every window across the whole history is served from the live document.
#[test]
fn a_window_read_costs_no_bytes_once_the_document_is_open() {
    let live = {
        let snapshot = snapshot_of(&one_document(300, Ids::Text));
        store_import(&snapshot)
    };
    let version = live.version();
    let list = live.doc().get_list(K_TIMELINE);
    let mut seen = 0;
    for window in 0..(300 / WINDOW) {
        let end = list.len() - window * WINDOW;
        for i in end - WINDOW..end {
            assert!(list.get(i).is_some());
            seen += 1;
        }
    }
    assert_eq!(seen, 300);
    assert_eq!(live.version(), version, "reading changed nothing");
}

/// The store's own load path round-trips a run-shaped document byte for
/// byte, and reads its header and newest window without touching the rest.
/// A `LiveDocument` is opaque, and this is what opaque buys.
#[test]
fn a_run_shaped_document_round_trips_through_the_store_seam() {
    let doc = one_document(300, Ids::Text);
    let snapshot = snapshot_of(&doc);
    let live = store_import(&snapshot);
    assert_eq!(live.doc().get_list(K_TIMELINE).len(), 300);
    assert_eq!(read_window(live.doc()), WINDOW);
    let run = live.doc().get_map(K_RUN);
    assert!(matches!(
        run.get("status").and_then(|v| v.into_value().ok()),
        Some(LoroValue::String(s)) if s.as_str() == "running"
    ));
    // Re-exported from a replica that changed nothing, the snapshot is the
    // same document: nothing was rewritten by being read.
    let again = store_import(&live.export_snapshot().unwrap());
    assert_eq!(again.version(), live.version());
    assert_eq!(again.doc().get_list(K_TIMELINE).len(), 300);
}

/// The paged candidate's promise — opening costs one page, not the history —
/// holds by construction; asserted so the proposal's comparison is against a
/// measured fact rather than a description.
#[test]
fn a_paged_run_opens_its_header_and_one_page() {
    let (header, pages) = header_and_pages(2_500, Ids::Binary);
    assert_eq!(pages.len(), 3);
    assert_eq!(header.get_list(K_PAGES).len(), 3);
    let newest = snapshot_of(pages.last().unwrap());
    let whole: usize = pages.iter().map(|p| snapshot_of(p).len()).sum();
    assert!(
        newest.len() < whole,
        "the newest page is a fraction of the history"
    );
    assert_eq!(read_window(store_import(&newest).doc()), WINDOW);
}

// ------------------------------------------------------------------ economics

/// The fold §17.2 asks of the retained economics facts, over the
/// representation this file measures: usage entries in the run's own
/// timeline, keyed by `(attempt, turn)` and carrying a monotonic `revision`.
/// Delivered 100, then corrected to 140, then the 100 replayed, the answer is
/// 140; a later correction to 90 is 90. The list keeps every delivery — a
/// retry is a duplicate entry, never a lost one — and the projection, not the
/// storage, is what orders them.
#[test]
fn economics_replay_selects_the_highest_revision_regardless_of_delivery_order() {
    fn usage(attempt: i64, turn: &str, revision: i64, total: i64) -> LoroValue {
        let pairs: Vec<(String, LoroValue)> = vec![
            ("step".into(), "usage".into()),
            ("attempt".into(), attempt.into()),
            ("turn".into(), turn.into()),
            ("revision".into(), revision.into()),
            ("tokens_in".into(), total.into()),
            // The model, as an id into the header's `models` map (§17.3):
            // never a name in the entry.
            ("model".into(), "m-1".into()),
        ];
        let map: std::collections::HashMap<String, LoroValue> = pairs.into_iter().collect();
        LoroValue::from(map)
    }
    fn field(entry: &LoroValue, key: &str) -> LoroValue {
        entry[key].clone()
    }
    /// Highest revision per `(attempt, turn)`, whatever order the entries
    /// arrived in.
    fn fold(doc: &LoroDoc) -> std::collections::BTreeMap<(i64, String), (i64, i64)> {
        let mut totals = std::collections::BTreeMap::new();
        for entry in doc.get_list(K_TIMELINE).to_vec() {
            let LoroValue::String(step) = field(&entry, "step") else {
                continue;
            };
            if step.as_str() != "usage" {
                continue;
            }
            let (LoroValue::I64(attempt), LoroValue::String(turn)) =
                (field(&entry, "attempt"), field(&entry, "turn"))
            else {
                continue;
            };
            let (LoroValue::I64(revision), LoroValue::I64(total)) =
                (field(&entry, "revision"), field(&entry, "tokens_in"))
            else {
                continue;
            };
            let slot = totals
                .entry((attempt, turn.to_string()))
                .or_insert((revision, total));
            if revision > slot.0 {
                *slot = (revision, total);
            }
        }
        totals
    }

    let doc = LoroDoc::new();
    let timeline = doc.get_list(K_TIMELINE);
    timeline.push(usage(1, "t1", 1, 100)).unwrap();
    timeline.push(usage(1, "t1", 2, 140)).unwrap();
    // The 100 delivered again, after the correction.
    timeline.push(usage(1, "t1", 1, 100)).unwrap();
    doc.commit();
    assert_eq!(fold(&doc)[&(1, "t1".into())], (2, 140));

    // A legitimate later correction wins; an older attempt's late report
    // still applies to its own key (§7.5) and never touches this one.
    timeline.push(usage(1, "t1", 3, 90)).unwrap();
    timeline.push(usage(0, "t9", 1, 7)).unwrap();
    doc.commit();
    let totals = fold(&doc);
    assert_eq!(totals[&(1, "t1".into())], (3, 90));
    assert_eq!(totals[&(0, "t9".into())], (1, 7));

    // And the same answer on a replica that received the entries in a
    // different order — the fold is order-independent, which is what makes
    // it a projection rather than a log.
    let replica = LoroDoc::new();
    replica.import(&snapshot_of(&doc)).unwrap();
    assert_eq!(fold(&replica), totals);
}
