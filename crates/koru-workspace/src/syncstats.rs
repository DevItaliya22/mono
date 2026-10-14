// SPDX-License-Identifier: MPL-2.0

//! Where the store sync's time goes, on demand.
//!
//! ```sh
//! KORU_FRAME_STATS=1 ./koru.sh run
//! ```
//!
//! The third member of a family: `koru_core::timing` measures the launch,
//! `koru_window::framestats` measures a frame's drawing, and this measures the
//! sync turn that runs *between* frames. Built the same way as both — off, it is
//! a load of a `bool` and a branch, and the `Instant::now` calls do not happen.
//!
//! ## Why it is not part of `framestats`
//!
//! `framestats` lives in `koru-window`, which names no workspace and must not
//! learn about one, and its four numbers are phases of a single draw. The sync
//! turn is not in a frame at all: it happens in `about_to_wait`, before the
//! deadline is chosen, and its cost is paid whether or not anything is drawn.
//! Folding it into the frame table would attribute it to the wrong thing.
//!
//! It reads the **same switch**, because the two are read together and nobody
//! wants to set two variables to answer one question. Each module resolves it
//! into its own `OnceLock`, which is already how `timing` and `framestats`
//! coexist.
//!
//! ## Reading the line
//!
//! ```text
//! sync x287/s  publish 1.2ms  drain 291.4ms (read 288.9ms)  | sessions 3  out 4KB/12  in 7  worst 3.1ms
//! ```
//!
//! - **x287/s** — how many times `tick_store` ran in the last second. A store
//!   session makes `next_due` report *now*, so today this is "as fast as the
//!   loop can go" rather than a cadence anybody chose.
//! - **publish / drain** — total time in each half **per second**, not per turn.
//!   Per-turn averages flatter a loop that is spinning; what is wanted is the
//!   share of every second the event loop is not available for typing.
//! - **read** — the part of `drain` spent blocked in `Connection::next_event`.
//!   This is the number the background thread exists to remove: it should read
//!   ~0 afterwards, and `drain` should fall to roughly `publish`.
//! - **out** — bytes handed to the store, and how many batches. Watch this
//!   against **in**: a client that publishes about as much as it imports is
//!   echoing its peers' operations back, which is a bug rather than a cost.
//! - **worst** — the longest single turn in the second. An average hides the
//!   stall that is actually felt.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Whether the switch is on. Read once, then free.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| koru_env::switch(koru_env::name::KORU_FRAME_STATS))
}

/// A stopwatch that costs nothing when the switch is off.
///
/// Deliberately the same shape as `koru_window::framestats::Phase`: it returns
/// a `Duration` rather than recording one, so a caller can add several together
/// before handing over a turn's total.
#[must_use = "a phase that is never stopped measures nothing"]
pub struct Phase(Option<Instant>);

impl Phase {
    pub fn start() -> Self {
        Self(enabled().then(Instant::now))
    }

    pub fn stop(self) -> Duration {
        self.0.map(|t| t.elapsed()).unwrap_or_default()
    }
}

/// How often the line is printed. **Wall time, not a turn count** — the whole
/// point of the change this measures is that the number of turns per second
/// collapses, and a fixed turn count would make the print rate collapse with it
/// and the two runs incomparable.
const EVERY: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct Totals {
    since: Instant,
    turns: u32,
    publish: Duration,
    drain: Duration,
    read: Duration,
    worst: Duration,
    sessions: usize,
    out_bytes: usize,
    out_batches: u32,
    in_frames: u32,
}

impl Totals {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            turns: 0,
            publish: Duration::ZERO,
            drain: Duration::ZERO,
            read: Duration::ZERO,
            worst: Duration::ZERO,
            sessions: 0,
            out_bytes: 0,
            out_batches: 0,
            in_frames: 0,
        }
    }
}

fn totals() -> &'static Mutex<Totals> {
    static TOTALS: OnceLock<Mutex<Totals>> = OnceLock::new();
    TOTALS.get_or_init(|| Mutex::new(Totals::new()))
}

/// A lock that is never held across anything that can block, and whose failure
/// is a dropped measurement rather than a panic. A poisoned mutex here means a
/// thread died mid-count; the app has better things to do about that than die
/// too.
fn with(f: impl FnOnce(&mut Totals)) {
    if !enabled() {
        return;
    }
    if let Ok(mut t) = totals().lock() {
        f(&mut t);
    }
}

/// One turn of the store sync loop is over.
///
/// `sessions` is how many connections it had to service, so a per-session cost
/// can be read off a table rather than inferred from how many tabs were open.
pub fn turn(publish: Duration, drain: Duration, sessions: usize) {
    with(|t| {
        t.turns += 1;
        t.publish += publish;
        t.drain += drain;
        t.worst = t.worst.max(publish + drain);
        t.sessions = t.sessions.max(sessions);
        if t.since.elapsed() >= EVERY {
            report(t);
            *t = Totals::new();
        }
    });
}

/// Time spent blocked in a socket read. Part of `drain`, reported separately
/// because it is the part that is supposed to disappear.
pub fn read_block(taken: Duration) {
    with(|t| t.read += taken);
}

/// Bytes handed to the store as one batch.
pub fn published(bytes: usize) {
    with(|t| {
        t.out_bytes += bytes;
        t.out_batches += 1;
    });
}

/// Update frames taken from the store and imported.
pub fn imported(frames: usize) {
    with(|t| t.in_frames += frames as u32);
}

/// **ASCII only**, and the reason is written up in `koru_core::timing`: this
/// prints to a console whose codepage is whatever Windows chose, and a
/// box-drawing character arrives as mojibake. `us` rather than `µs` for the same
/// reason.
fn report(t: &Totals) {
    let secs = t.since.elapsed().as_secs_f64().max(f64::EPSILON);
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    println!(
        "sync x{:<4.0}/s  publish {:>7.1}ms  drain {:>7.1}ms (read {:>7.1}ms)  \
         | sessions {}  out {}B/{}  in {}  worst {:.1}ms",
        t.turns as f64 / secs,
        ms(t.publish),
        ms(t.drain),
        ms(t.read),
        t.sessions,
        t.out_bytes,
        t.out_batches,
        t.in_frames,
        ms(t.worst),
    );
}
