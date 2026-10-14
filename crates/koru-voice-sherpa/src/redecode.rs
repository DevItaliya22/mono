// SPDX-License-Identifier: MPL-2.0

//! **The counter behind chunk-and-redecode, and nothing else.**
//!
//! An *offline* recogniser produces no partials. Parakeet and Whisper are in the
//! gate anyway, so partials have to come from somewhere, and the benchmark
//! protocol declares where: maintain the audio since session start and every
//! `H` of audio time decode the whole buffer again and emit the result as a
//! `Partial`.
//!
//! **The value of `H` is not in this file, and that is the point.** The policy
//! is ours rather than the candidate's — a poor latency number for Parakeet is
//! evidence about *this policy*, not about the model — so it is declared once,
//! in the benchmark's candidate table, and handed to an adapter as a
//! [`std::time::Duration`]. An adapter that carried its own default would put a
//! second answer beside it, and the two would disagree the first time one was
//! tuned.
//!
//! What *is* here is the arithmetic, because it is the part that is easy to get
//! subtly wrong and impossible to see wrong from the outside:
//!
//! - the interval is **audio time, not wall time**. Under the unpaced pass wall
//!   time has no relation to the audio, and a redecode schedule driven by
//!   `Instant::now` would fire a different number of times in the two passes —
//!   which would show up as `drift` between them and be blamed on the model.
//! - a call carrying several intervals' worth of samples is **one** redecode,
//!   not several. Anything else decodes the same buffer twice and charges the
//!   candidate for it.
//! - `finish` is not a redecode. The final decode is the caller's and happens
//!   whether or not an interval elapsed.

use std::time::Duration;

use koru_voice::SAMPLE_RATE;

/// Counts audio toward the next redecode.
///
/// Constructed from an interval in *audio* time; fed sample counts; answers
/// whether a partial is due.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Redecode {
    /// The interval, in samples at [`SAMPLE_RATE`]. Zero means "never" and is
    /// how an adapter is told to produce no partials at all — which is the
    /// honest configuration for an offline recogniser being measured *without*
    /// the policy.
    every: u64,
    since: u64,
    total: u64,
}

impl Redecode {
    /// Every `interval` of audio.
    ///
    /// A sub-sample interval rounds **down to zero and therefore to never**,
    /// rather than to one sample, which would be a redecode per frame and would
    /// look like a hang.
    pub fn every(interval: Duration) -> Redecode {
        let every = (interval.as_secs_f64() * f64::from(SAMPLE_RATE)) as u64;
        Redecode {
            every,
            since: 0,
            total: 0,
        }
    }

    /// Never emit a partial.
    pub fn never() -> Redecode {
        Redecode {
            every: 0,
            since: 0,
            total: 0,
        }
    }

    /// Whether this instance will ever answer `true`.
    pub fn is_enabled(&self) -> bool {
        self.every > 0
    }

    /// The interval, back in samples, for a report that has to state it.
    pub fn interval_samples(&self) -> u64 {
        self.every
    }

    /// Total samples this session has seen.
    pub fn total_samples(&self) -> u64 {
        self.total
    }

    /// The session's audio duration so far.
    pub fn elapsed_audio(&self) -> Duration {
        Duration::from_secs_f64(self.total as f64 / f64::from(SAMPLE_RATE))
    }

    /// Account for `samples` more, and say whether a redecode is due.
    ///
    /// **At most one `true` per call**, even when the call carries several
    /// intervals: the redecode is over the whole buffer, so doing it twice
    /// produces the same string twice and charges the candidate for two decodes
    /// of everything.
    pub fn push(&mut self, samples: usize) -> bool {
        let samples = samples as u64;
        self.total += samples;
        if !self.is_enabled() {
            return false;
        }
        self.since += samples;
        if self.since >= self.every {
            // Not `since = 0`: the remainder carries, so a stream of 20 ms
            // frames against a 500 ms interval fires every 25 frames forever
            // rather than drifting by the rounding of each one.
            self.since %= self.every;
            true
        } else {
            false
        }
    }

    /// Start again for a new session, keeping the interval.
    pub fn restart(&mut self) {
        self.since = 0;
        self.total = 0;
    }
}

#[cfg(test)]
mod tests;
