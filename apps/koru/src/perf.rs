// SPDX-License-Identifier: MPL-2.0

//! What a keystroke costs, and that it does not grow with the document.
//!
//! Typing was once reported as feeling behind Sublime — see
//! [ADR-26](../../../docs/decisions/README.md). Measuring settled it: the edit path is
//! **70–100 µs and flat in document size**, and everything that felt slow was
//! between the finished frame and the glass. This file is the baseline that
//! established that, kept so the next person does not have to re-derive it.
//!
//! ```sh
//! cargo test -p koru --bin koru perf -- --nocapture --test-threads=1
//! ```
//!
//! It drives `Leaf::key`, so what it measures is the whole edit path the
//! shell drives: resolve the chord, insert into Loro, re-read the node's runs,
//! re-shape that row with parley, re-flow the rest, apply pending carets.
//! Everything except the GPU.
//!
//! **The ratio is the assertion, not the microseconds.** An absolute time says
//! more about the machine than the code, but typing that costs the same at two
//! thousand rows as at one is a property of the *algorithm* — and losing it is
//! the regression worth catching. Making a keystroke rebuild the whole body
//! instead of one row is a one-line mistake that nothing else here would notice.

use std::time::{Duration, Instant};

use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{NodeId, WorkspaceId};
use koru_editor::Editing;
use winit::keyboard::{Key, ModifiersState};

use crate::leaf::Leaf;

/// A non-flat, title-less document view rooted at `node`. The twin of the one
/// in `tests.rs`; see it for why this is no longer a `Leaf` constructor.
///
/// Duplicated rather than shared because these are two `#[cfg(test)]` modules
/// and a helper reachable from both would have to be `pub(crate)` on the
/// non-test build too — which is the dead public API the move was made to end.
fn list_block(ws: &WorkspaceDoc, node: NodeId) -> Leaf {
    Leaf::Document(koru_document::BuiltInDocument::open_list(ws, node))
}

/// Long enough to wrap in a narrow window and to carry a few shaping clusters —
/// a row that costs something to lay out, rather than one that flatters.
const ROW_TEXT: &str = "Task number {} with a reasonable amount of text on it";

/// How many keystrokes each measurement averages over.
const PRESSES: usize = 200;

/// A document of `rows` tasks, laid out, with the caret in the last one.
///
/// The last row deliberately: it is the one furthest down the reflow, so
/// anything that walks the rows on every keystroke shows up here at its worst.
fn typing_into(rows: usize) -> (WorkspaceDoc, Leaf) {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").unwrap();
    let doc = ws
        .insert(Parent::Root, Node::document("Doc").unwrap())
        .unwrap();
    let mut ids: Vec<NodeId> = Vec::new();
    for i in 0..rows {
        let text = ROW_TEXT.replace("{}", &i.to_string());
        ids.push(
            ws.insert(Parent::Node(doc), Node::task(&text).unwrap())
                .unwrap(),
        );
    }
    ws.enable_undo();
    // `list_block` rather than `document`: no title row, so `rows[n]` is the
    // n-th task and the numbers below mean what they always meant.
    let mut s = list_block(&ws, doc);
    s.set_viewport(900.0, 560.0);
    if rows > 0 {
        s.body_mut().focus = rows - 1;
    }
    s.view_mut().active = true;
    (ws, s)
}

/// Mean and worst time for one key press, typing `PRESSES` characters.
///
/// Every seventh is a space: a word boundary is where `checkpoint` closes an
/// undo group, so a run of them is the realistic mix rather than the cheapest.
fn cost_of_typing(rows: usize) -> (Duration, Duration) {
    let (mut ws, mut s) = typing_into(rows);
    let mut worst = Duration::ZERO;
    let start = Instant::now();
    for i in 0..PRESSES {
        let c = if i % 7 == 6 { " " } else { "x" };
        let at = Instant::now();
        s.key(&mut ws, &Key::Character(c.into()), Some(c), NONE);
        worst = worst.max(at.elapsed());
    }
    (start.elapsed() / PRESSES as u32, worst)
}

const NONE: ModifiersState = ModifiersState::empty();

/// The whole point: a keystroke in a big document costs what it costs in a
/// small one, because only the edited row is re-shaped.
///
/// The ceiling is deliberately loose. It is not a target — it is the line
/// between "this machine is busy" and "typing now walks the document".
#[test]
fn typing_does_not_get_slower_as_the_document_grows() {
    let mut measured = Vec::new();
    for rows in [1usize, 10, 100, 500, 2000] {
        let (mean, worst) = cost_of_typing(rows);
        println!(
            "rows={rows:<5} mean={:>7.1}µs  worst={:>7.1}µs",
            mean.as_secs_f64() * 1e6,
            worst.as_secs_f64() * 1e6,
        );
        measured.push((rows, mean));
    }

    let smallest = measured.first().expect("measured the smallest").1;
    let largest = measured.last().expect("measured the largest").1;
    assert!(
        largest < smallest * 8,
        "typing at 2000 rows ({largest:?}) should cost about what it does at one \
         ({smallest:?}) — something now walks the whole body per keystroke"
    );
    assert!(
        largest < Duration::from_millis(2),
        "a keystroke took {largest:?}; the baseline is under 100µs"
    );
}

/// **Opening a tab must not re-read the fonts.**
///
/// Every view owns a `TextEngine`, and a `TextEngine` owns a `FontContext` —
/// so a tab is where the bundled faces get registered. Registration used to
/// `std::fs::read` every file again, ~15 MB of it, which meant the second tab
/// paid exactly what the first did and the eighth paid it eight times over.
/// The blob cache in `koru_draw::text` makes the disk read happen once per
/// *process*; `register_fonts` then gets an `Arc` clone of bytes already in
/// memory.
///
/// **The disk read was never the whole of it, and macOS is what showed that.**
/// With the blobs cached, a tab still paid `FontContext::new()` — parley
/// discovering the system's fonts — once per view. That is 384 µs on Windows and
/// **30–258 ms on macOS**, where it means Core Text enumeration; this test
/// measured 48 ms a tab there and passed, because the budget was 50. The font
/// database is now built once per process and cloned per view
/// (`koru_draw::text::prototype`), which costs 375 ns.
///
/// Measured on the dev machine, debug build, this test run on its own:
/// **7.3 ms for the first tab, 384 µs for every one after it** on Windows before
/// the change; **3.4 µs** for every tab after the first on macOS since. The
/// first tab is still where the discovery happens, so it stays slow and stays
/// un-asserted.
///
/// The first construction is not asserted on, deliberately: it is the one that
/// legitimately reads the disk, and how long that takes says more about the
/// page cache than about this code. The **steady state** is what is asserted —
/// what every tab after the first costs — because that is what the user feels
/// when they press `+`.
#[test]
fn opening_a_tab_does_not_re_read_the_fonts() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").unwrap();
    let doc = ws
        .insert(Parent::Root, Node::document("Doc").unwrap())
        .unwrap();

    let cold = Instant::now();
    drop(Leaf::document(&ws, doc));
    let cold = cold.elapsed();

    const TABS: u32 = 8;
    let warm = Instant::now();
    for _ in 0..TABS {
        drop(Leaf::document(&ws, doc));
    }
    let warm = warm.elapsed() / TABS;

    println!("first tab {cold:?}   every tab after {warm:?}");
    assert!(
        warm < Duration::from_millis(5),
        "opening a tab cost {warm:?}; sharing the font database makes it \
         microseconds — something is either reading the ~15 MB of assets again \
         or calling `FontContext::new` per view (see `koru_draw::text`)"
    );
}

/// What a keystroke costs when a *second* window is open on the same document.
///
/// The shell calls `adopt_others` on every press, and `Editing::adopt` is
/// `Body::rebuild` — which drains every row, re-walks the tree and re-reads
/// every node. The test above measured `Leaf::key` alone and so never saw
/// this; with two windows the per-keystroke cost is the sum.
///
/// Returns the **median** press and the worst, not the mean. A mean over two
/// hundred presses carries every time the thread was preempted mid-press, and
/// under a full `cargo test --workspace` that is most of it: a handful of
/// 6-15 ms scheduling stalls lifted the mean at two thousand rows from about
/// 280 µs to 2.4 ms and failed the ceiling below, with nothing in the edit
/// path changed. The median is the press the scheduler left alone, and a
/// regression that walks the body on every press moves it just as surely.
fn cost_of_typing_with_a_second_window(rows: usize) -> (Duration, Duration) {
    let (mut ws, mut typing) = typing_into(rows);
    // A second window on the same node, so both views cost the same rebuild.
    let root = typing.body().root.expect("typing_into roots its view");
    let mut watching = list_block(&ws, root);
    watching.set_viewport(900.0, 560.0);

    let mut presses = Vec::with_capacity(PRESSES);
    for i in 0..PRESSES {
        let c = if i % 7 == 6 { " " } else { "x" };
        let at = Instant::now();
        typing.key(&mut ws, &Key::Character(c.into()), Some(c), NONE);
        // Exactly what `App::adopt_others` does to every other pane.
        watching.adopt(&ws);
        presses.push(at.elapsed());
    }
    presses.sort();
    (presses[PRESSES / 2], presses[PRESSES - 1])
}

/// **This used to scale — and now does not.** `adopt` was `Body::rebuild`,
/// O(document) by construction; it is now `Body::defer_pending` (drain the Loro
/// event subscription) then `Editing::catch_up` (settle: re-anchor an open
/// picker, refresh only the rows that changed, lay out). So a second window
/// costs about what a first one does, whatever the document size.
///
/// Measured on the dev machine, mean per keystroke (the assertion now takes
/// the median press; see `cost_of_typing_with_a_second_window`):
///
/// | rows | one window | two windows (before) | two windows (after) |
/// | ---- | ---------- | -------------------- | ------------------- |
/// | 1    | 78 µs      | 126 µs               | 125 µs              |
/// | 100  | 76 µs      | 812 µs               | 124 µs              |
/// | 500  | 81 µs      | 4.1 ms               | 135 µs              |
/// | 2000 | 98 µs      | **20 ms**            | **180 µs**          |
///
/// The before-column was a dropped frame per key at two thousand rows; the
/// after-column is flat, the same shape as one window. That is the property the
/// assertion holds: a second window must not make typing scale with the
/// document. ADR-13's "Node is truth, Row is a cache" survives — it stops being
/// a *whole-cache* invalidation.
///
/// Coalescing `adopt` per frame was never the answer, and the arithmetic is
/// why: typing runs at about eight characters a *second*, so at 60 fps there is
/// already at most one keystroke per frame. There was nothing to coalesce; the
/// work itself had to shrink, and it did.
#[test]
fn a_second_window_costs_no_more_than_the_first() {
    let mut measured = Vec::new();
    for rows in [1usize, 10, 100, 500, 2000] {
        let (median, worst) = cost_of_typing_with_a_second_window(rows);
        println!(
            "two windows  rows={rows:<5} median={:>8.1}µs  worst={:>8.1}µs",
            median.as_secs_f64() * 1e6,
            worst.as_secs_f64() * 1e6,
        );
        measured.push((rows, median));
    }

    let smallest = measured.first().expect("measured the smallest").1;
    let largest = measured.last().expect("measured the largest").1;
    assert!(
        largest < smallest * 8,
        "typing in a second window at 2000 rows ({largest:?}) should cost about what it \
         does at one ({smallest:?}) — `adopt` has gone back to walking the whole body"
    );
    assert!(
        largest < Duration::from_millis(2),
        "a keystroke with two windows open took {largest:?}; the baseline is under 200µs"
    );
}

// ----------------------------------------------------- resizing the pane

/// What one step of a width change costs, and what a whole drag costs.
///
/// `set_viewport` invalidates every row when the effective width moves, so a
/// step is a full `reflow` — O(rows) through parley, the most expensive call in
/// the app. That is why the explore pane's drag does **not** re-lay the content
/// per mouse-move: the column redraws at the live width and the content catches
/// up once, on release.
fn cost_of_width_steps(rows: usize, steps: usize) -> Duration {
    let (_, mut s) = typing_into(rows);
    let start = Instant::now();
    for i in 0..steps {
        // Alternating so no step is a no-op: `set_viewport` short-circuits on
        // an unchanged width, which would measure nothing.
        let w = 900.0 - (i % 2) as f32 * 40.0;
        s.set_viewport(w, 560.0);
    }
    start.elapsed() / steps as u32
}

/// **The regression this guards.** A drag of the explore pane emits a
/// mouse-move per frame; if each one re-laid the content, the cost would be a
/// full reflow several times a second, and at two thousand rows that is tens of
/// milliseconds *per step* in debug (ROADMAP measures 72 ms).
///
/// The assertion is the ratio, not the milliseconds: a reflow that has become
/// O(rows²), or a drag that has quietly started reflowing every tab rather than
/// the visible one, is what shows up here. An absolute ceiling would only
/// measure the machine.
#[test]
fn a_width_step_stays_linear_in_the_document() {
    let mut measured = Vec::new();
    for rows in [100usize, 500, 2000] {
        let per_step = cost_of_width_steps(rows, 20);
        println!(
            "  {rows:>5} rows: {:>8.1} µs / width step",
            per_step.as_secs_f64() * 1e6
        );
        measured.push((rows, per_step.as_secs_f64()));
    }
    let (small_rows, small) = measured[0];
    let (big_rows, big) = measured[measured.len() - 1];
    let rows_grew = big_rows as f64 / small_rows as f64;
    let time_grew = big / small.max(f64::EPSILON);
    assert!(
        time_grew < rows_grew * 3.0,
        "a width step grew {time_grew:.1}× over {rows_grew:.0}× the rows — \
         reflow is no longer linear in the document"
    );
}

/// The pane's drag is only affordable because the content is laid out **once**
/// for the whole gesture. This pins the mechanism that makes that true:
/// `set_full` moves what is drawn, and only `commit_width` moves what the
/// layout — and therefore `reflow` — sees.
#[test]
fn dragging_the_pane_does_not_relay_the_content_until_it_is_released() {
    use koru_app_shell::Panel;
    let mut p = Panel::new(232.0, true);
    let before = p.layout_width();
    for w in [250.0, 300.0, 420.0, 560.0, 700.0] {
        p.set_full(w);
        assert_eq!(
            p.layout_width(),
            before,
            "a mid-drag step must not move the layout width"
        );
    }
    assert!(p.commit_width(), "and releasing moves it exactly once");
    assert_eq!(p.layout_width(), 700.0);
}
