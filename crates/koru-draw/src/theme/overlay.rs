// SPDX-License-Identifier: MPL-2.0

//! Semantic geometry for anchored desktop popovers, in logical units.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Popover {
    pub viewport_inset: f32,
    pub anchor_gap: f32,
    pub radius: f32,
}

/// Defaults used by PopoverShell::anchored; callers can still supply custom shells.
pub const fn popover() -> Popover {
    Popover {
        viewport_inset: 8.0,
        anchor_gap: 6.0,
        radius: 10.0,
    }
}
