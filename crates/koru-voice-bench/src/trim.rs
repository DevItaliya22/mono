// SPDX-License-Identifier: MPL-2.0

//! **Silence trimming, because a recording's leading silence would otherwise
//! land directly in every latency figure.**
//!
//! Corpus clips carry leading silence whose length is a property of the
//! recording session, not of the recogniser. `t0` is defined as *the instant the
//! first audio frame is pushed after silence trimming*, so this module is what
//! makes `TTFUP` comparable between a Common Voice clip recorded on a laptop
//! microphone and one recorded in a booth.
//!
//! The protocol's rule, exactly:
//!
//! 1. short-time energy over 20 ms windows;
//! 2. the first and last window above **−45 dBFS**;
//! 3. cut to that span, then **put back 200 ms of the original leading
//!    silence**.
//!
//! ## Why the 200 ms goes back
//!
//! A candidate may have a voice-activity detector inside it. Handing one a
//! buffer that begins on the first energetic sample gives it nothing to settle
//! on, and what that measures is the VAD's cold start rather than the
//! recogniser. Two hundred milliseconds is the protocol's decision; it is a
//! constant here so it is one number rather than a `0.2` in an expression.
//!
//! **And it comes from the original file, not from generated zeros.** A
//! recording's silence is its room and its microphone's noise floor; feeding a
//! VAD digital silence and then real audio is a step change nothing in the world
//! produces. When the file has less than 200 ms of lead, what there is is used
//! and the shortfall is reported rather than padded.
//!
//! ## The trimmed duration is the one every other number divides by
//!
//! Eligibility (`[1.5 s, 20.0 s]`), RTF's denominator and the audio-time
//! redecode schedule all use [`Trimmed::duration`]. Using the file's duration
//! anywhere would make two utterances with the same speech and different
//! leading silence score differently.

use std::time::Duration;

use koru_voice::SAMPLE_RATE;

/// The energy floor. A window at or below this is silence.
pub const FLOOR_DBFS: f64 = -45.0;

/// The analysis window.
pub const WINDOW: Duration = Duration::from_millis(20);

/// How much of the original leading silence is put back.
pub const LEAD_IN: Duration = Duration::from_millis(200);

/// One trimmed utterance.
#[derive(Clone, PartialEq, Debug)]
pub struct Trimmed {
    /// The samples that will be pushed — the speech span plus up to
    /// [`LEAD_IN`] of the original lead.
    pub samples: Vec<f32>,
    /// How much lead was actually available. Less than [`LEAD_IN`] is reported
    /// rather than padded; see the module note.
    pub lead_in: Duration,
    /// How much was removed from the front and the back, for a report that
    /// wants to say what it did.
    pub removed: Duration,
}

impl Trimmed {
    /// The duration every other figure divides by.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / f64::from(SAMPLE_RATE))
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// Root-mean-square energy of one window, in dBFS.
///
/// `-inf` for digital silence, which compares below any floor without a special
/// case. Computed in `f64`: a 20 ms window is 320 samples and the sum of their
/// squares in `f32` loses precision exactly where the signal is quiet, which is
/// the region this function exists to classify.
pub fn window_dbfs(window: &[f32]) -> f64 {
    if window.is_empty() {
        return f64::NEG_INFINITY;
    }
    let sum: f64 = window.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    let rms = (sum / window.len() as f64).sqrt();
    if rms <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * rms.log10()
    }
}

/// Trim to the speech span, then put the lead back.
///
/// An utterance with no window above the floor comes back empty, which the
/// eligibility filter then excludes — a file that is silence all the way
/// through is a corpus problem and must be counted as one rather than measured.
pub fn trim(samples: &[f32]) -> Trimmed {
    let window = (WINDOW.as_secs_f64() * f64::from(SAMPLE_RATE)) as usize;
    let lead = (LEAD_IN.as_secs_f64() * f64::from(SAMPLE_RATE)) as usize;

    let windows: Vec<f64> = samples.chunks(window).map(window_dbfs).collect();
    let first = windows.iter().position(|db| *db > FLOOR_DBFS);
    let last = windows.iter().rposition(|db| *db > FLOOR_DBFS);

    let (Some(first), Some(last)) = (first, last) else {
        return Trimmed {
            samples: Vec::new(),
            lead_in: Duration::ZERO,
            removed: Duration::from_secs_f64(samples.len() as f64 / f64::from(SAMPLE_RATE)),
        };
    };

    let speech_start = first * window;
    // The last window is inclusive, and it may be a short final chunk.
    let speech_end = ((last + 1) * window).min(samples.len());

    let available = speech_start.min(lead);
    let start = speech_start - available;

    Trimmed {
        samples: samples[start..speech_end].to_vec(),
        lead_in: Duration::from_secs_f64(available as f64 / f64::from(SAMPLE_RATE)),
        removed: Duration::from_secs_f64(
            (samples.len() - (speech_end - start)) as f64 / f64::from(SAMPLE_RATE),
        ),
    }
}

#[cfg(test)]
mod tests;
