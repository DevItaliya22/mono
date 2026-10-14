// SPDX-License-Identifier: MPL-2.0

//! Where a frame's time actually goes, on demand.
//!
//! ```sh
//! KORU_FRAME_STATS=1 ./koru.sh run
//! ```
//!
//! Off, this is a load of a `bool` and four branches; the `Instant::now` calls
//! do not happen. On, it prints a rolling summary rather than a line per frame,
//! because a line per frame changes what it measures.
//!
//! ## What the four numbers mean
//!
//! - **encode** — building the vello `Scene` from the rows. CPU, and the one
//!   viewport culling is meant to flatten: it should not grow with the size of
//!   the document.
//! - **render** — `Renderer::render_to_texture`, which records the compute
//!   passes. Still CPU; the GPU has not been waited on yet.
//! - **acquire** — `get_current_texture`. **The one that matters for latency.**
//!   With a vsync'd surface and frames already in flight this is where the
//!   thread blocks, so a large number here is the frame queue, not the drawing.
//! - **present** — handing the frame over. Usually trivial.
//!
//! The reason this exists: the presentation half of the typing-latency work —
//! `AutoNoVsync`, a frame latency of one, and not redrawing on every wake — was
//! reasoned from the code and never measured. Flip `PRESENT_MODE` back to
//! `AutoVsync` with this on and the difference is a number.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How many frames go into one printed summary. Large enough that the print
/// itself is noise, small enough to see a change while using the app.
const EVERY: u32 = 120;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| koru_env::switch(koru_env::name::KORU_FRAME_STATS))
}

/// A stopwatch that costs nothing when the switch is off.
pub struct Phase(Option<Instant>);

impl Phase {
    pub fn start() -> Self {
        Self(enabled().then(Instant::now))
    }

    pub fn stop(self) -> Duration {
        self.0.map(|t| t.elapsed()).unwrap_or_default()
    }
}

#[derive(Default)]
pub struct Frames {
    n: u32,
    encode: Duration,
    render: Duration,
    acquire: Duration,
    present: Duration,
    /// The worst single frame in this batch, by total — an average hides the
    /// stall that is actually felt.
    worst: Duration,
}

impl Frames {
    pub fn record(
        &mut self,
        encode: Duration,
        render: Duration,
        acquire: Duration,
        present: Duration,
    ) {
        if !enabled() {
            return;
        }
        self.n += 1;
        self.encode += encode;
        self.render += render;
        self.acquire += acquire;
        self.present += present;
        self.worst = self.worst.max(encode + render + acquire + present);

        if self.n >= EVERY {
            self.report();
            *self = Self::default();
        }
    }

    fn report(&self) {
        let each = |d: Duration| d.as_secs_f64() * 1e6 / self.n as f64;
        println!(
            "frame x{:<4} encode {:>7.1}µs  render {:>7.1}µs  acquire {:>7.1}µs  \
             present {:>7.1}µs  | worst {:>7.1}µs",
            self.n,
            each(self.encode),
            each(self.render),
            each(self.acquire),
            each(self.present),
            self.worst.as_secs_f64() * 1e6,
        );
    }
}
