// SPDX-License-Identifier: MPL-2.0

//! Where a window goes: half-screen snap, and putting it back to a sensible
//! default.
//!
//! **This sets the window's rectangle; it does not press Windows' own chord.**
//! The OS gesture also raises Snap Assist — the picker offering to fill the
//! other half — and a menu row that summoned a full-screen overlay would be
//! doing considerably more than it said. Half the work area is the part anyone
//! means by "snap left", so that is the part this does. The chord is still on
//! the row, and pressing it still gets the whole OS behaviour.
//!
//! The work area, not the monitor: a window snapped over the taskbar is a
//! window with a strip of itself permanently behind something else.
//! `MonitorHandle` exposes the monitor's full rectangle and not its work area,
//! which is why this asks the OS directly — the same reason and the same call
//! `hairline::edges` makes, including the cast its doc comment warns about.

/// Which half.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

/// The rect a snap should produce, given a work area. Pure arithmetic, so the
/// halving is testable without a monitor.
///
/// The left half takes the odd pixel of an odd width: something has to, and
/// leaving a one-pixel gutter down the middle of the screen is the one outcome
/// nobody wants.
pub fn half(work: (i32, i32, i32, i32), side: Side) -> (i32, i32, u32, u32) {
    let (left, top, right, bottom) = work;
    let w = (right - left).max(0);
    let h = (bottom - top).max(0);
    let left_w = w - w / 2;
    match side {
        Side::Left => (left, top, left_w as u32, h as u32),
        Side::Right => (left + left_w, top, (w / 2) as u32, h as u32),
    }
}

/// A rect of `size` centred in `work`, clamped so it never starts off-screen.
///
/// Pure, for the same reason [`half`] is: the arithmetic is the part worth
/// pinning, and a monitor is the part a test cannot have.
pub fn centred(work: (i32, i32, i32, i32), size: (u32, u32)) -> (i32, i32, u32, u32) {
    let (left, top, right, bottom) = work;
    let (aw, ah) = ((right - left).max(0), (bottom - top).max(0));
    // A window larger than the work area is shrunk to it rather than centred
    // half off each edge.
    let w = (size.0 as i32).min(aw).max(0);
    let h = (size.1 as i32).min(ah).max(0);
    (left + (aw - w) / 2, top + (ah - h) / 2, w as u32, h as u32)
}

/// What *Reset Window Position* produces: the default size from `Settings`,
/// centred on this work area — or the work area itself when that will not fit.
pub fn reset_rect(work: (i32, i32, i32, i32), default: (u32, u32)) -> (i32, i32, u32, u32) {
    centred(work, default)
}

/// The draggable band of a borderless window: its top strip, where the caption
/// the view draws is. Physical pixels, because the window's rectangle is.
const DRAG_STRIP: i32 = 64;

/// How much of that strip must be on a monitor for the window to count as
/// reachable: enough width to get a hand on, enough height that the grip is
/// the caption rather than a seam.
const GRIP_W: i32 = 96;
const GRIP_H: i32 = 24;

/// Whether a window at `pos` with `size` is **reachable** on at least one of
/// `monitors` — visible enough to grab by the caption and move back.
///
/// A remembered position describes a desktop that may no longer exist — a
/// monitor unplugged, a resolution dropped, a dock left behind — and restoring
/// it verbatim opens the window off-screen with nothing to hold. So the test
/// is not "the rects intersect" but "the caption strip's overlap with some
/// monitor is a grip": a window hanging half off the edge is a placement the
/// user can correct, a window with no visible grip is lost.
///
/// Pure, for the same reason [`half`] is: the arithmetic is the part worth
/// pinning, and a monitor is the part a test cannot have.
pub fn reachable(monitors: &[(i32, i32, i32, i32)], pos: (i32, i32), size: (u32, u32)) -> bool {
    // i64: a position near `i32::MAX` plus its width must not wrap into a
    // false positive.
    let (left, top) = (i64::from(pos.0), i64::from(pos.1));
    let (right, bottom) = (left + i64::from(size.0), top + i64::from(size.1));
    let strip_bottom = (top + i64::from(DRAG_STRIP)).min(bottom);
    monitors.iter().any(|&(ml, mt, mr, mb)| {
        let grip_w = right.min(i64::from(mr)) - left.max(i64::from(ml));
        let grip_h = strip_bottom.min(i64::from(mb)) - top.max(i64::from(mt));
        grip_w >= i64::from(GRIP_W) && grip_h >= i64::from(GRIP_H)
    })
}

/// Everything a menu row can ask of a window's *placement*.
///
/// One enum rather than an entry point each, because both end in the same three
/// steps — find a work area, compute a rect, put the window in it — and
/// splitting that is a second chance to forget the un-maximise.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Placement {
    /// Fill the left or right half of the current display's work area.
    Half(Side),
    /// Back to the default size, centred on the current display.
    Reset,
}

/// The work area of the monitor a window is on, as `(left, top, right, bottom)`.
///
/// `MonitorHandle` exposes the monitor's full rectangle and not its work area,
/// which is why this asks the OS directly — the same reason and the same call
/// `hairline::edges` makes.
#[cfg(windows)]
fn work_area(monitor: &winit::monitor::MonitorHandle) -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};
    use winit::platform::windows::MonitorHandleExtWindows;

    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // **`hmonitor()` returns winit's `HMONITOR`, which is `isize`, while
    // `windows-sys` declares it a pointer** — the cast is not decoration.
    let hmonitor = monitor.hmonitor() as *mut core::ffi::c_void;
    // SAFETY: `hmonitor` came from winit's live handle, and `info` is sized as
    // the call requires. A failure leaves it zeroed, which the check discards.
    if unsafe { GetMonitorInfoW(hmonitor, &mut info) } == 0 {
        return None;
    }
    Some((
        info.rcWork.left,
        info.rcWork.top,
        info.rcWork.right,
        info.rcWork.bottom,
    ))
}

/// Put `window` where `what` says.
///
/// A maximised window is restored first: setting a rect on one leaves Windows
/// still believing it is maximised, so the next *Restore* would send it
/// somewhere else entirely.
#[cfg(windows)]
pub fn place(window: &winit::window::Window, what: Placement, default_size: (u32, u32)) {
    use winit::dpi::{PhysicalPosition, PhysicalSize};

    let Some(monitor) = window.current_monitor() else {
        return;
    };
    let Some(work) = work_area(&monitor) else {
        return;
    };
    let size = window.outer_size();

    let _ = size;
    let (x, y, w, h) = match what {
        Placement::Half(side) => half(work, side),
        Placement::Reset => reset_rect(work, default_size),
    };
    if w == 0 || h == 0 {
        return;
    }
    if window.is_maximized() {
        window.set_maximized(false);
    }
    window.set_outer_position(PhysicalPosition::new(x, y));
    let _ = window.request_inner_size(PhysicalSize::new(w, h));
}

/// Elsewhere there is no work-area call to make through this crate's one
/// Windows dependency, and tiling is the window manager's business rather than
/// an app's. The rows are hidden off Windows rather than doing nothing — see
/// `context_menu::window_items`.
#[cfg(not(windows))]
pub fn place(_window: &winit::window::Window, _what: Placement, _default_size: (u32, u32)) {}

/// Put `window` at exactly `size` physical pixels — a recording size.
///
/// Unlike [`place`] this never centres and never shrinks to the work area:
/// the pixels are the point (a 1080×1920 preset stays 1080×1920 on a 1080p
/// landscape display), and only the *position* is nudged, by
/// [`nudge_onscreen`], so the caption grip stays reachable. A maximised
/// window is restored first, same as [`place`].
#[cfg(windows)]
pub fn place_exact(window: &winit::window::Window, size: (u32, u32)) {
    use winit::dpi::{PhysicalPosition, PhysicalSize};

    if size.0 == 0 || size.1 == 0 {
        return;
    }
    let Some(monitor) = window.current_monitor() else {
        return;
    };
    let Some(work) = work_area(&monitor) else {
        return;
    };
    let pos = window
        .outer_position()
        .map(|p| (p.x, p.y))
        .unwrap_or((work.0, work.1));
    let (x, y) = nudge_onscreen(work, pos, size);
    if window.is_maximized() {
        window.set_maximized(false);
    }
    window.set_outer_position(PhysicalPosition::new(x, y));
    let _ = window.request_inner_size(PhysicalSize::new(size.0, size.1));
}

/// Elsewhere there is no work-area call to make; see [`place`].
#[cfg(not(windows))]
pub fn place_exact(_window: &winit::window::Window, _size: (u32, u32)) {}

/// Where an exact-size window goes: its current position, nudged so the
/// caption grip stays on the work area. **The size is never clamped** — the
/// grip is what matters, not the rectangle. Pure, for the same reason
/// [`half`] is.
pub fn nudge_onscreen(work: (i32, i32, i32, i32), pos: (i32, i32), size: (u32, u32)) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    let w = size.0 as i32;
    // Horizontally, at least a grip's width of window survives an edge.
    let min_x = left + GRIP_W - w;
    let max_x = right - GRIP_W;
    let x = if min_x <= max_x {
        pos.0.clamp(min_x, max_x)
    } else {
        left
    };
    // Vertically, the caption — the window's top strip — never leaves the
    // work area. How far the window hangs below is its own business: a tall
    // preset on a short display is a recording, not a mistake.
    let max_y = bottom - GRIP_H;
    let y = if top <= max_y {
        pos.1.clamp(top, max_y)
    } else {
        top
    };
    (x, y)
}

#[cfg(test)]
mod tests;
