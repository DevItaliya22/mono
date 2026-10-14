// SPDX-License-Identifier: MPL-2.0

//! Screen bounds into capture-local pixels.
//!
//! The 4.1 sample subtracted the captured window's top-left from UIA screen
//! rectangles. The maintainers accepted that clip with no geometry corrections, so this
//! is the contract: UIA bounds are already physical screen pixels (the process
//! is DPI-aware). Do not multiply by a scale factor. `title_bar_height` is
//! recorded and not applied. A shortcut's target is the focused control, not
//! chrome.

/// A rectangle in physical screen pixels. Capture geometry and UIA bounds
/// share this one type, so pairing a delivered input with its frame never
/// converts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}

/// Window and capture rectangles used to map UIA screen bounds into the clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureGeometry {
    pub window: Rect,
    pub width: u32,
    pub height: u32,
    pub title_bar_height: u32,
    /// 1000 = 100%. Recorded; [`to_capture_local`] does not multiply by it.
    pub dpi_scale_milli: u32,
}

impl CaptureGeometry {
    pub fn physical(window: Rect, width: u32, height: u32, title_bar_height: u32) -> Self {
        Self {
            window,
            width,
            height,
            title_bar_height,
            dpi_scale_milli: 1000,
        }
    }

    pub fn with_dpi(mut self, dpi_scale_milli: u32) -> Self {
        self.dpi_scale_milli = dpi_scale_milli.max(1);
        self
    }
}

/// Capture-local rectangle: UIA screen pixels minus the captured window origin.
/// Negative monitor coordinates are legal (`window.left` may be negative).
pub fn to_capture_local(target: Rect, window: Rect) -> Rect {
    Rect {
        left: target.left - window.left,
        top: target.top - window.top,
        right: target.right - window.left,
        bottom: target.bottom - window.top,
    }
}

pub fn center(bounds: Rect) -> (i32, i32) {
    (
        bounds.left + (bounds.right - bounds.left) / 2,
        bounds.top + (bounds.bottom - bounds.top) / 2,
    )
}

/// H.264 encode sizes are even. Graphics Capture can still report the odd
/// window height (this sitting: 667 vs encoder 666). That is not a resize.
pub fn even_dim(value: u32) -> u32 {
    value & !1
}

/// A later frame whose size is not the capture geometry would silently warp
/// every box if we kept pairing. Refuse that instead of guessing.
pub fn same_capture_size(geometry: CaptureGeometry, width: u32, height: u32) -> bool {
    even_dim(geometry.width) == even_dim(width) && even_dim(geometry.height) == even_dim(height)
}

#[cfg(test)]
mod tests;
