// SPDX-License-Identifier: MPL-2.0

//! Video time origin and action/assertion pairing.
//!
//! Times measure **native input delivery completing** (SendInput consequence
//! finished), not that Koru finished processing. Origin is the Instant the
//! first frame was successfully sent to the encoder.

use crate::{execute::StepResult, inspect::ElementFacts};
use std::time::{Duration, Instant};

use koru_recording::{CaptureGeometry, FrameStamp, same_capture_size};

/// Alignment bound taken from the accepted 4.1 sample
/// `sample-20260912-000036`: clicks were 0–5 ms, chords 16–22 ms (one frame).
/// Two frames (33 ms) is the recorded chord ceiling; not a guessed 16.7 or 50.
pub const CLICK_ALIGNMENT_MS: i64 = 16;
pub const CHORD_ALIGNMENT_MS: i64 = 33;
pub const TIMING_SAMPLE: &str = "sample-20260912-000036";

/// A delivered input or a finished assertion, already on the first-frame origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub step: &'static str,
    pub operation: &'static str,
    pub kind: EventKind,
    pub at_ms: u64,
    pub target: Option<ElementFacts>,
    pub matched: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Input,
    Assertion,
}

pub fn alignment_limit(operation: &str) -> i64 {
    match operation {
        "click" | "right-click" | "focus" => CLICK_ALIGNMENT_MS,
        _ => CHORD_ALIGNMENT_MS,
    }
}

pub fn within_acceptance(operation: &str, alignment_ms: i64) -> bool {
    alignment_ms.abs() <= alignment_limit(operation)
}

/// Nearest captured frame by arrival time. Intervals are not assumed constant.
pub fn nearest_frame(frames: &[FrameStamp], at_ms: u64) -> Option<(u32, i64)> {
    let first = frames.first()?;
    let mut best = first;
    let mut best_delta = (first.arrived_ms as i64) - (at_ms as i64);
    for frame in frames.iter().skip(1) {
        let delta = (frame.arrived_ms as i64) - (at_ms as i64);
        if delta.abs() < best_delta.abs() {
            best = frame;
            best_delta = delta;
        }
    }
    Some((best.index, best_delta))
}

pub fn intervals_ms(frames: &[FrameStamp]) -> Vec<u64> {
    frames
        .windows(2)
        .map(|pair| pair[1].arrived_ms.saturating_sub(pair[0].arrived_ms))
        .collect()
}

pub fn origin_ok(frames: &[FrameStamp]) -> Result<(), &'static str> {
    if frames.is_empty() {
        return Err("capture-not-ready");
    }
    if frames[0].arrived_ms != 0 || frames[0].index != 0 {
        return Err("origin-not-first-encoded-frame");
    }
    Ok(())
}

pub fn geometry_unchanged(
    geometry: CaptureGeometry,
    frames: &[FrameStamp],
) -> Result<(), &'static str> {
    for frame in frames {
        let width = frame.width.unwrap_or(geometry.width);
        let height = frame.height.unwrap_or(geometry.height);
        if !same_capture_size(geometry, width, height) {
            return Err("capture-geometry-changed");
        }
        if let Some(dpi) = frame.dpi_scale_milli {
            if dpi != geometry.dpi_scale_milli {
                return Err("capture-geometry-changed");
            }
        }
    }
    Ok(())
}

pub fn events_from_results(results: &[StepResult], origin: Instant) -> Vec<Event> {
    results
        .iter()
        .map(|result| {
            if let Some(delivered) = result.delivered_at {
                Event {
                    step: result.step,
                    operation: result.operation,
                    kind: EventKind::Input,
                    at_ms: delivered.saturating_duration_since(origin).as_millis() as u64,
                    target: result.target.clone(),
                    matched: result.actual.matched,
                }
            } else {
                let at = result
                    .finished_at
                    .unwrap_or(origin)
                    .saturating_duration_since(origin)
                    .as_millis() as u64;
                Event {
                    step: result.step,
                    operation: result.operation,
                    kind: EventKind::Assertion,
                    at_ms: at,
                    target: result.target.clone(),
                    matched: result.actual.matched.or(Some(result.failure.is_none())),
                }
            }
        })
        .collect()
}

pub fn wait_timeout() -> Duration {
    Duration::from_secs(10)
}

#[cfg(test)]
mod tests;
