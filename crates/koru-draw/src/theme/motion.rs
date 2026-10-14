// SPDX-License-Identifier: MPL-2.0

//! Shared tooltip timing. Interaction delays are distinct from animation time.
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tooltip {
    pub hover_delay: Duration,
    pub enter_duration: Duration,
    pub exit_duration: Duration,
    pub warm_window: Duration,
    /// Initial vertical offset in desktop logical units.
    pub rise: f32,
}

/// Current desktop recipe; consumers retain their existing easing and lifecycle.
pub const fn tooltip() -> Tooltip {
    Tooltip {
        hover_delay: Duration::from_millis(400),
        enter_duration: Duration::from_millis(100),
        exit_duration: Duration::from_millis(75),
        warm_window: Duration::from_millis(300),
        rise: 2.0,
    }
}
