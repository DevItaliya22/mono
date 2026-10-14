// SPDX-License-Identifier: MPL-2.0

//! One captured/encoded frame relative to the first encoded frame, and the
//! origin that anchors it: the Instant the first frame was *successfully sent
//! to the encoder* (ADR-349) — the moment an encoded frame is known to exist.

use std::time::Instant;

/// One captured/encoded frame relative to the first encoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameStamp {
    pub index: u32,
    pub arrived_ms: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub dpi_scale_milli: Option<u32>,
}

/// The recording's time origin. Every `arrived_ms` and every `at_ms` in a
/// recording's artifacts measures from this Instant.
pub struct Origin {
    pub instant: Instant,
}

/// Whether every frame kept the capture's size. `windows-capture` answers
/// `Ok` to a frame whose size changed mid-clip — it pads or crops the frame
/// into the configured dimensions and the clip warps *silently* — so the
/// only way to know is to compare the stamps. The first frame is the
/// reference: it is what the origin and the telemetry geometry were taken
/// from.
pub fn frames_hold_size(frames: &[FrameStamp]) -> bool {
    let Some(first) = frames.first() else {
        return true;
    };
    frames
        .iter()
        .all(|frame| frame.width == first.width && frame.height == first.height)
}

#[cfg(test)]
mod tests;
