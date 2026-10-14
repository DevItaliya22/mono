// SPDX-License-Identifier: MPL-2.0

//! Machine-wide held-key observation and safe text insertion.
//!
//! This is mechanism owned by Agent, not dictation policy. It observes only
//! the virtual key that RegisterHotKey already accepted, only while a hold is
//! active, and it treats an unavailable input desktop as cancellation rather
//! than release. Text insertion captures an opaque accessibility target and
//! verifies that exact control again at the point of consequence.
//!
//! The Windows adapters can block in UI Automation and SendInput. Plan 1.3
//! must run them on the dictation worker, never on Agent's event thread.

mod insertion;
mod release;

#[cfg(windows)]
pub mod windows;

pub use insertion::{
    CapturedRestoreHint, CapturedTarget, Commit, DirectInsertion, InsertBackend, Refusal,
    TargetDisplay, TargetState, capture, capture_for_hint, capture_restore_hint, commit, restore,
    restore_hint, target_display,
};
pub use release::{
    CancelReason, HoldFlight, HoldNotice, HoldOutcome, KeyState, KeyStateSource, ObserverConfig,
    PressError, PressRegistrar, ReleaseObserver, VirtualKey,
};

#[cfg(test)]
mod tests;
