// SPDX-License-Identifier: MPL-2.0

//! Plan 4.2 recording contract: first encoded frame is the time origin.
//!
//! The capture mechanism lives in `koru-recording` (Windows only), re-exported
//! here so callers see one `recording` module. This module keeps the runner's
//! half: the artifact contract, the outcome logic, and the event pairing.
//! Serialization stays in the runner (`serde_json`); there is no protocol crate.

mod artifact;
#[cfg(windows)]
pub use koru_recording::capture;
mod run;
mod timeline;

pub use artifact::{
    Artifact, CONTRACT, PairedEvent, VERSION, from_results, pair, unpaired_json, unpaired_manifest,
};
pub use koru_recording::{CaptureGeometry, FrameStamp, Origin, center, to_capture_local};
pub use run::{
    Attempt, Conclusion, Outputs, conclude, refuse_existing, refuse_existing_paths, result_path,
};
pub use timeline::{
    CHORD_ALIGNMENT_MS, CLICK_ALIGNMENT_MS, Event, EventKind, TIMING_SAMPLE, alignment_limit,
    events_from_results, geometry_unchanged, intervals_ms, nearest_frame, origin_ok, wait_timeout,
    within_acceptance,
};

#[cfg(test)]
mod tests;
