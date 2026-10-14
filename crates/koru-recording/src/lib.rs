// SPDX-License-Identifier: MPL-2.0

//! Window capture and H.264 encoding, and the physical-pixel geometry a
//! recording is measured in. The mechanism half of Koru recording: what a
//! recording is *of* — a compiled case, a person's session — stays with its
//! consumer (`koru-demo`'s runner, the in-app session control).
//!
//! The time origin of any recording is the Instant the first frame was
//! *successfully sent to the encoder* (ADR-349), because that is the moment
//! an encoded frame is known to exist. A configured frame rate does not
//! guarantee a constant one; times are recorded per frame, never assumed.
//!
//! Capture I/O stays behind [`capture`] (Windows only). Nothing here is
//! constructed by `cargo test --workspace`, so an ordinary test run cannot
//! record the desktop.

pub mod frame;
pub mod geometry;
pub mod telemetry;

#[cfg(windows)]
pub mod capture;

pub use frame::{FrameStamp, Origin, frames_hold_size};
pub use geometry::{CaptureGeometry, Rect, center, even_dim, same_capture_size, to_capture_local};
pub use telemetry::{Action, ActionKind, Heartbeat, SessionTelemetry};

use std::sync::atomic::AtomicBool;

/// **One recording per process** (SESSIONRECORD §4 dec 4). Set by the shell
/// the moment a start is *requested* — before the worker exists, so a second
/// window's start landing while the first is still binding is refused, not
/// raced — kept through the capture, and cleared when a start fails or the
/// finalize answers; read by every window's menu row and F9, which dim
/// while *another* window holds the recording. The shell refuses a second
/// `StartRecording` regardless — this flag is the early, visible half of
/// that rule, not the enforcement.
pub static RECORDING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// **Test-only serialization** for suites that flip [`RECORDING_ACTIVE`]:
/// take this lock or the flip races parallel readers under cargo's runner —
/// the theme's `THEME_LOCK` precedent (guide/contributing/TESTING.md). The
/// app itself never touches it.
#[doc(hidden)]
pub static RECORDING_ACTIVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The H.264 bitrate for a frame size, derived rather than asked
/// (SESSIONRECORD §4 dec 5): keyed on **pixel count**, so a portrait or
/// square size tiers beside its landscape equivalent — 1080×1920 gets what
/// 1920×1080 gets.
///
/// Computed from the final even dimensions at capture start, never from the
/// preset somebody picked, so a display-scale surprise cannot silently
/// change the encoder's budget.
pub fn bitrate_bps(width: u32, height: u32) -> u32 {
    const TIER_1080P: u64 = 2_100_000;
    const TIER_1440P: u64 = 3_700_000;
    let pixels = width as u64 * height as u64;
    if pixels <= TIER_1080P {
        20_000_000
    } else if pixels <= TIER_1440P {
        30_000_000
    } else {
        50_000_000
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_bitrate_tiers_key_on_pixel_count() {
        use super::bitrate_bps;
        // The presets tier where their landscape equivalents tier.
        assert_eq!(bitrate_bps(1920, 1080), 20_000_000);
        assert_eq!(bitrate_bps(1080, 1920), 20_000_000);
        assert_eq!(bitrate_bps(1080, 1080), 20_000_000);
        assert_eq!(bitrate_bps(1080, 1350), 20_000_000);
        assert_eq!(bitrate_bps(1024, 768), 20_000_000);
        // 1440p is the middle tier, either orientation.
        assert_eq!(bitrate_bps(2560, 1440), 30_000_000);
        assert_eq!(bitrate_bps(1440, 2560), 30_000_000);
        // 4K and beyond tops out.
        assert_eq!(bitrate_bps(3840, 2160), 50_000_000);
        // The boundaries themselves.
        assert_eq!(bitrate_bps(2100, 1000), 20_000_000);
        assert_eq!(bitrate_bps(2101, 1000), 30_000_000);
        assert_eq!(bitrate_bps(3700, 1000), 30_000_000);
        assert_eq!(bitrate_bps(3701, 1000), 50_000_000);
    }
}
