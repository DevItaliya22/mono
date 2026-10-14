// SPDX-License-Identifier: MPL-2.0

//! Headless terminal emulation behind Koru-owned, bounded types.
//!
//! Alacritty types do not cross this crate. A caller can read only a requested
//! row window or scrollback page; there is deliberately no retained-grid
//! iterator or whole-history snapshot helper.

mod convert;
mod emulator;
mod event;
mod input;
mod parser;
mod style;
mod window;

pub use emulator::*;
pub use event::*;
pub use input::*;
pub use style::*;
pub use window::*;

/// The compatibility contract exercised by the fixture suite and ADR-228.
pub const EMULATOR_VERSION: &str = "alacritty_terminal 0.26.0";
/// Alacritty 0.26.0's pinned width table.
pub const UNICODE_WIDTH_VERSION: &str = "unicode-width 0.2.2";
/// One terminal retains at most this many rows above its active screen.
pub const MAX_SCROLLBACK_ROWS: usize = 100_000;
/// One API call may materialize at most this many complete rows.
pub const MAX_PAGE_ROWS: u16 = 512;
/// Window titles are UI hints, not an unbounded output channel.
pub const MAX_TITLE_CHARS: usize = 1_024;
/// Maximum OSC payload forwarded into VTE's otherwise growable `std` buffer.
pub const MAX_OSC_BYTES: usize = 4_096;
/// Maximum side-effect records produced by one parse batch.
pub const MAX_EVENTS_PER_BATCH: usize = 256;
