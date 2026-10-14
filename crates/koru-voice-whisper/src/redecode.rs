// SPDX-License-Identifier: MPL-2.0

//! **The counter behind chunk-and-redecode — the same body as
//! `koru_voice_sherpa::redecode`, kept so by hand.**
//!
//! Two identical types in two crates is a drift hazard and it is taken
//! deliberately, for the reason `koru_ui::keys::ctrl_letter` and
//! `koru_keys::letter` are the same function twice: the alternative is a
//! dependency edge that says something false.
//!
//! **Neither adapter may depend on the other.** They wrap two unrelated
//! inference runtimes, they are provisioned differently, and ADR-253 deletes at
//! least one of them — so `koru-voice-whisper` importing a type from
//! `koru-voice-sherpa` would make deleting either a change to the other. The
//! other option is a fourth crate below both, for forty lines of arithmetic and
//! no second reader.
//!
//! What is shared and must stay shared is the *interval*: `H` is declared once,
//! in the benchmark's candidate table, and handed to both adapters. That is the
//! number a person would tune, and it is in one place. What is duplicated is
//! the counting, which nobody tunes.
//!
//! `docs/features/voice/archive/QUESTIONS.md` Q-V49 records the trade and what would collapse
//! it.
//!
//! The three rules the arithmetic exists for, restated because a reader of this
//! copy should not have to find the other one:
//!
//! - the interval is **audio time, not wall time**, so the paced and unpaced
//!   passes redecode the same number of times over the same utterance;
//! - a call carrying several intervals is **one** redecode, because the decode
//!   is over the whole buffer;
//! - the remainder carries, so a schedule driven by 20 ms frames does not drift.

use std::time::Duration;

use koru_voice::SAMPLE_RATE;

/// Counts audio toward the next redecode.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Redecode {
    every: u64,
    since: u64,
    total: u64,
}

impl Redecode {
    /// Every `interval` of audio. A sub-sample interval means never.
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

    pub fn is_enabled(&self) -> bool {
        self.every > 0
    }

    pub fn interval_samples(&self) -> u64 {
        self.every
    }

    pub fn total_samples(&self) -> u64 {
        self.total
    }

    pub fn elapsed_audio(&self) -> Duration {
        Duration::from_secs_f64(self.total as f64 / f64::from(SAMPLE_RATE))
    }

    /// Account for `samples` more, and say whether a redecode is due. At most
    /// one `true` per call.
    pub fn push(&mut self, samples: usize) -> bool {
        let samples = samples as u64;
        self.total += samples;
        if !self.is_enabled() {
            return false;
        }
        self.since += samples;
        if self.since >= self.every {
            self.since %= self.every;
            true
        } else {
            false
        }
    }

    pub fn restart(&mut self) {
        self.since = 0;
        self.total = 0;
    }
}

#[cfg(test)]
mod tests;
