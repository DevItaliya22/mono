// SPDX-License-Identifier: MPL-2.0

//! Where the time between launching Koru and seeing a document goes.
//!
//! ```sh
//! KORU_STARTUP_STATS=1 ./koru.sh run
//! ```
//!
//! The twin of `koru_window::framestats`, and deliberately built the same way:
//! off, this is a load of a `bool` and a branch, and the `Instant::now` calls do
//! not happen. On, it accumulates named spans and prints **one table** when the
//! first frame reaches the glass — a line per span as it happened would change
//! what it measures, and would interleave with the startup messages `startup.rs`
//! already writes.
//!
//! ## Why this lives in `koru-core`
//!
//! It has to be reachable from every crate on the startup path — the store
//! decoding a snapshot, the workspace opening a profile, the editor rebuilding a
//! body, the shell creating a device. `koru-core` is the only crate all of them
//! depend on. It costs nothing: this module names no model type and pulls in no
//! dependency beyond `std`.
//!
//! The one crate it cannot reach is `koru-draw`, which depends on no Koru
//! crate at all — so font registration stays uninstrumented. That is fine, and
//! deliberate: `perf::opening_a_tab_does_not_re_read_the_fonts` already pins it
//! at 7.3 ms for the first tab and 384 µs for every one after.
//!
//! ## Reading the table
//!
//! Spans print in the order they first completed, with a count and a total, so a
//! span entered many times — one per tab, one per peer file — shows both.
//! **Spans nest**, and the table does not indent them: `profile open` contains
//! `snapshot decode`, which contains `reindex`. The number to judge a change by
//! is `first frame`, which is measured from process start and contains
//! everything.
//!
//! A span that grows with the *size* of the document is worth chasing; one that
//! grows with the *history* of the document is the bug this module was written
//! to find. See `docs/contributing/operations/RUNBOOK.md`.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Whether the switch is on. Read once, then free.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| koru_env::switch(koru_env::name::KORU_STARTUP_STATS))
}

/// When this process began, as near as we can get to it.
///
/// Resolved on the first call, which is why [`start`] is called from the top of
/// `run`. The first *reader* would otherwise become the origin, and every number
/// would be measured from whenever the switch happened to be noticed.
fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// Fix the origin for [`started_at`]. Call once, as early as possible.
pub fn start() {
    // The stall watch stamps every line with time-since-launch, so it needs the
    // origin fixed here too — otherwise the first stall becomes the origin and
    // every line reads `+0.000s` from a clock that started when the trouble did.
    if enabled() || stall_floor().is_some() {
        origin();
    }
}

/// When this process began.
///
/// Handed out rather than kept private because "first frame" is measured from a
/// *different* origin depending on what is being timed: process start for a cold
/// launch, the moment the switch was asked for when swapping workspaces.
pub fn started_at() -> Instant {
    origin()
}

/// One row of the table: what it was, how many times, how long in total, and
/// the note from the longest single occurrence — plus how long that one took,
/// which is what decides whether a later note replaces it.
type Row = (&'static str, u32, Duration, Option<String>, Duration);

fn spans() -> &'static Mutex<Vec<Row>> {
    static SPANS: OnceLock<Mutex<Vec<Row>>> = OnceLock::new();
    SPANS.get_or_init(Default::default)
}

/// Fold one measurement into `rows`, merging by name.
///
/// Split out from [`Span::finish`] so the merging and the formatting can be
/// tested without the process-global switch — `enabled()` is a `OnceLock` read
/// of the environment, so a test cannot turn it on.
fn add(rows: &mut Vec<Row>, name: &'static str, taken: Duration, note: Option<String>) {
    match rows.iter_mut().find(|(n, ..)| *n == name) {
        Some((_, count, total, existing, worst)) => {
            *count += 1;
            *total += taken;
            // The note from the **longest** occurrence, not the first. `peer
            // import` runs once for the profile document and once for the
            // workspace; the first is a few bytes and the second is the whole
            // problem, so "first wins" described the wrong one and made the
            // table say `0 KB read`.
            if taken >= *worst || existing.is_none() {
                *worst = taken;
                if note.is_some() {
                    *existing = note;
                }
            }
        }
        None => rows.push((name, 1, taken, note, taken)),
    }
}

/// A stopwatch that costs nothing when the switch is off.
///
/// Held by value and consumed by [`stop`], so a span cannot be recorded twice
/// and cannot be left open by accident.
///
/// [`stop`]: Span::stop
#[must_use = "a span that is never stopped is never recorded"]
pub struct Span {
    name: &'static str,
    at: Option<Instant>,
}

impl Span {
    /// Begin timing `name`. A `&'static str` so the table can group by it
    /// without allocating on the measured path.
    pub fn start(name: &'static str) -> Self {
        Self {
            name,
            at: enabled().then(Instant::now),
        }
    }

    /// End the span and record it.
    pub fn stop(self) {
        self.finish(None);
    }

    /// End the span and record it with a note — a row count, a byte count, the
    /// name of the thing it was working on.
    ///
    /// The closure is only called when the switch is on, so building the note
    /// costs nothing in a normal run even when it allocates.
    pub fn stop_with(self, note: impl FnOnce() -> String) {
        let note = self.at.is_some().then(note);
        self.finish(note);
    }

    fn finish(self, note: Option<String>) {
        let Some(at) = self.at else { return };
        let taken = at.elapsed();
        if let Ok(mut rows) = spans().lock() {
            add(&mut rows, self.name, taken, note);
        }
    }
}

/// Record a span measured some other way — a duration already in hand, or work
/// that does not bracket cleanly.
pub fn record(name: &'static str, taken: Duration) {
    if !enabled() {
        return;
    }
    if let Ok(mut rows) = spans().lock() {
        add(&mut rows, name, taken, None);
    }
}

/// A guard that complains if the span it covers took long enough to be felt.
///
/// The tables above are for work that happens once and is then over. **A stall
/// is different**: it is a single event somebody is trying to explain, in the
/// middle of a session, and what they need is its name and its length the
/// moment it happens. Nested watches all report, so the innermost line names
/// the culprit and the outer ones give the context.
///
/// Free when the switch is off, like everything else here.
///
/// **Its switch is not [`enabled`].** A stall is what somebody reports days
/// after launching — "it froze for a second when I pressed Enter" — so it has
/// `KORU_JANK` of its own rather than riding on a variable named for startup.
/// `KORU_STARTUP_STATS` still turns it on, because it always did and a session
/// that is already measuring wants the stalls too.
#[must_use = "a watch that is dropped immediately measures nothing"]
pub struct Watch {
    what: &'static str,
    at: Option<Instant>,
}

/// Long enough that it cannot be ordinary work, short enough to catch a stutter
/// before it becomes a freeze. A frame is 16 ms; this is several of them.
const STALL_OVER: Duration = Duration::from_millis(50);

/// Whether stalls are being watched, and from how long.
///
/// `KORU_JANK=1` turns it on at [`STALL_OVER`]; `KORU_JANK_MS=120` raises the
/// floor, which is what somebody does after their first log comes back full of
/// 60 ms redraws. **Two variables rather than one that means both** — every
/// other switch in this tree is `=1`, so a single variable would read `1` as a
/// one-millisecond floor and log every event in the session. That is not a
/// hypothetical: it is what the first version did, and the test that named it
/// is `the_stall_floor_reads_its_own_switch`.
pub fn stall_floor() -> Option<Duration> {
    static FLOOR: OnceLock<Option<Duration>> = OnceLock::new();
    *FLOOR.get_or_init(|| {
        floor_from(
            std::env::var(koru_env::name::KORU_JANK).ok().as_deref(),
            std::env::var(koru_env::name::KORU_JANK_MS).ok().as_deref(),
            enabled(),
        )
    })
}

/// The rule, as a pure function — the switches are read once per process, so
/// this is the only part a test can drive.
fn floor_from(jank: Option<&str>, floor_ms: Option<&str>, startup_stats: bool) -> Option<Duration> {
    let on = match jank {
        None => startup_stats,
        // The one truthiness rule, on an already-read value: `floor_from`
        // stays pure so the test can drive it.
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
    };
    if !on {
        return None;
    }
    // A floor that is not a positive number is a typo, and the answer to a typo
    // is the default rather than silence: `KORU_JANK_MS=fast` still watches.
    Some(
        floor_ms
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis)
            .unwrap_or(STALL_OVER),
    )
}

impl Watch {
    pub fn start(what: &'static str) -> Self {
        Self {
            what,
            at: stall_floor().is_some().then(Instant::now),
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let Some(at) = self.at else { return };
        let Some(floor) = stall_floor() else { return };
        let took = at.elapsed();
        if took >= floor {
            let thread = std::thread::current();
            let thread = thread.name().unwrap_or("unnamed");
            stall_line(&format!(
                "stall: {} blocked for {:.1}ms [thread={thread}]",
                self.what,
                took.as_secs_f64() * 1e3
            ));
            if let Ok(mut worst) = worst_stalls().lock() {
                worst.push((self.what, took));
            }
        }
    }
}

/// Write one stall line where the person who turned this on can find it.
///
/// **Both places, not one.** A terminal is where a developer looks and is gone
/// the moment the window closes; a file is what somebody can attach to a bug
/// report — and the whole point of this switch is that the freeze happened
/// yesterday. `KORU_JANK_LOG` names the file; without it, the console alone.
fn stall_line(line: &str) {
    let at = started_at().elapsed().as_secs_f64();
    println!("[+{at:8.3}s] {line}");
    let Some(path) = std::env::var_os("KORU_JANK_LOG") else {
        return;
    };
    let Ok(mut file) = jank_file().lock() else {
        return;
    };
    if file.is_none() {
        // Appended, never truncated: a session that crashes and is started
        // again is the one being investigated, and the run before the crash is
        // the interesting half.
        *file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        if let Some(open) = file.as_mut() {
            let _ = std::io::Write::write_all(
                open,
                format!(
                    "--- koru run, floor {:?}\n",
                    stall_floor().unwrap_or_default()
                )
                .as_bytes(),
            );
        }
    }
    if let Some(open) = file.as_mut() {
        use std::io::Write;
        let _ = writeln!(open, "[+{at:8.3}s] {line}");
        // Flushed per line, because the run that is being investigated is the
        // one that will not be shut down cleanly.
        let _ = open.flush();
    }
}

fn jank_file() -> &'static Mutex<Option<std::fs::File>> {
    static FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
    FILE.get_or_init(|| Mutex::new(None))
}

fn worst_stalls() -> &'static Mutex<Vec<(&'static str, Duration)>> {
    static WORST: OnceLock<Mutex<Vec<(&'static str, Duration)>>> = OnceLock::new();
    WORST.get_or_init(|| Mutex::new(Vec::new()))
}

/// What stalled in this session, worst first. Empty when nothing did, and when
/// the switch is off.
///
/// The per-stall lines are the record; this is the paragraph somebody can read
/// at the end of an hour's use without scrolling through them.
pub fn stall_summary() -> Option<String> {
    stall_floor()?;
    let mut stalls = worst_stalls().lock().ok()?.clone();
    if stalls.is_empty() {
        return None;
    }
    let n = stalls.len();
    stalls.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    let mut out = format!("\n--- {n} stall(s) this session, worst first\n");
    for (what, took) in stalls.iter().take(10) {
        out.push_str(&format!(
            "{what:<28}  {:>9.1}ms\n",
            took.as_secs_f64() * 1e3
        ));
    }
    Some(out)
}

/// Print [`stall_summary`] to the console and the log, if there is one.
pub fn report_stalls() {
    if let Some(summary) = stall_summary() {
        for line in summary.lines() {
            stall_line(line);
        }
    }
}

/// The table, as text. Pure, so it is what the tests assert on.
///
/// **ASCII only.** This prints to a console whose codepage is whatever Windows
/// chose, and a box-drawing rule came out as `â”€â”€â”€`. The same trap the
/// `scripts/*.ps1` files carry a warning about, one layer further out.
fn table(what: &str, rows: &[Row]) -> String {
    let mut out = format!(
        "\n--- {what} {}\n",
        "-".repeat(40usize.saturating_sub(what.len()))
    );
    let width = rows.iter().map(|(n, ..)| n.len()).max().unwrap_or(0);
    for (name, count, total, note, _) in rows {
        let ms = total.as_secs_f64() * 1e3;
        let each = if *count > 1 {
            format!("  ({count} x {:.1}ms)", ms / *count as f64)
        } else {
            String::new()
        };
        let note = note
            .as_deref()
            .map(|n| format!("  {n}"))
            .unwrap_or_default();
        out.push_str(&format!("{name:<width$}  {ms:>9.1}ms{each}{note}\n"));
    }
    out
}

/// Print the table and start again, under a heading saying what was timed —
/// `"cold start"`, or `"switch to <workspace>"`.
///
/// Clearing rather than accumulating is what lets a workspace switch be measured
/// with the same machinery: the second table is that switch, not the launch plus
/// that switch.
pub fn report(what: &str) {
    if !enabled() {
        return;
    }
    let Ok(mut rows) = spans().lock() else { return };
    if rows.is_empty() {
        return;
    }
    print!("{}", table(what, &rows));
    rows.clear();
}

#[cfg(test)]
mod tests;
