// SPDX-License-Identifier: MPL-2.0

//! The window's own outline, on the edges the compositor's cannot reach.
//!
//! ## Why the border disappears when the window is against the screen
//!
//! [`crate::frame`] asks DWM for a 1px border, and DWM paints it at the
//! **window** rect. For a floating window that is the same rectangle as the
//! client area, so the border is visible and nothing else is needed.
//!
//! It stops being the same rectangle the moment the window's edge reaches the
//! screen's. winit removes the frame by answering `WM_NCCALCSIZE`, and for a
//! **maximised** window it answers with the monitor's *work area* rather than
//! the rect Windows proposed — because `WS_SIZEBOX` is still set, so the rect
//! Windows proposed overhangs the work area by the thickness of a frame that is
//! no longer drawn. The client area is therefore correct, and the window rect
//! around it is several pixels larger on every side. The border is painted out
//! there: not missing, off-screen. An edge snapped flush against the work area
//! loses its border the same way.
//!
//! Re-applying the attribute cannot help — winit sets them once at window
//! creation and never touches them again, so they are already what we asked
//! for. The only fix is to paint that edge ourselves, inside the client area.
//!
//! ## What this module is
//!
//! [`flush`] answers which of a window's four edges have nothing left between
//! them and the screen. *Whether there is any* is the signal that [`draw`]
//! should put a hairline around the whole client area. Corners are a separate
//! decision — maximised only, in [`crate::frame::apply`] — because Win11's drop
//! shadow only exists while the window is rounded, and a snapped window must
//! keep that shadow.
//!
//! **The whole of it, not only the flush edges.** Two windows snapped side by
//! side meet along an edge that is flush with neither one's work area, and that
//! seam is where a border is most needed — without it the two documents run
//! together. Which edges are flush stays the more precise answer, and is what
//! the tests pin; `any()` is what the draw path reads.
//!
//! It lives in the shell rather than in a view for the same reason
//! [`crate::resize`] does: it is a fact about the *window*. It is also the only
//! place it can be correct — a view draws under `Affine::scale(dpi * zoom)` and
//! could not produce a one-device-pixel line if it tried.

use vello::Scene;
use vello::kurbo::Affine;

use koru_draw::scene;

/// A rectangle the way Windows reports one: four edges, physical pixels,
/// screen origin. Not extents — `right` and `bottom` are exclusive coordinates.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Which of a window's edges are against the screen's. All false is the
/// ordinary case: a window floating inside the work area, wearing the
/// compositor's border on all four sides.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Edges {
    /// Whether any edge is flush — which is what decides both that there is
    /// something to draw and that the corners are squared off.
    pub fn any(self) -> bool {
        self.left || self.top || self.right || self.bottom
    }
}

/// Which edges of `window` are flush with — or past — the edges of `work`.
///
/// Both are physical and screen-origin. `work` is the **work area**, not the
/// monitor: with a taskbar along the bottom, a maximised window's bottom edge
/// is flush hundreds of pixels above the monitor's own.
///
/// The comparisons are `<=` / `>=` rather than `==` so the maximised case falls
/// out with no special handling — a maximised window's rect overhangs the work
/// area on all four sides.
pub fn flush(window: Bounds, work: Bounds) -> Edges {
    // A window with no extent has no edges to speak of. `Resized(1, 1)` and
    // worse arrive mid-drag; see the same guard in `resize::edge`.
    if window.right <= window.left || window.bottom <= window.top {
        return Edges::default();
    }
    // Nothing to do with this work area. The case that matters is a minimised
    // window, which reports the (-32000, -32000) sentinel and would otherwise
    // read as flush on the left and the top.
    if window.right <= work.left
        || window.left >= work.right
        || window.bottom <= work.top
        || window.top >= work.bottom
    {
        return Edges::default();
    }
    Edges {
        left: window.left <= work.left,
        top: window.top <= work.top,
        right: window.right >= work.right,
        bottom: window.bottom >= work.bottom,
    }
}

/// Paint a one-pixel outline around the whole client area.
///
/// **All four edges, not only the flush ones.** The flush edges are what says
/// the outline is *needed*; they are not what it consists of. Two windows
/// snapped side by side meet along an edge that is flush with neither one's
/// work area, and that seam is exactly where a border has to be — without it
/// the two documents run into each other. Drawing the rest of the rectangle is
/// free. On a snapped (still-rounded) window the compositor's ~8px arcs may
/// clip the hairline's ends; that is accepted so the drop shadow survives.
///
/// A window clear of the screen draws nothing and keeps the compositor's
/// border, which is rounded and correct.
///
/// `w` and `h` are the render target's size — **physical** pixels — and the
/// transform is the identity, so a line of width 1.0 is one device pixel, the
/// same weight as the compositor's border it stands in for. Drawing it in
/// logical pixels under the root transform would give three pixels at 150% DPI.
///
/// Where the compositor's border is still visible this paints the same colour
/// over the same pixel: an overdraw, never a doubled line.
pub fn draw(scene: &mut Scene, w: u32, h: u32, edges: Edges, color: peniko::Color) {
    if !edges.any() || w == 0 || h == 0 {
        return;
    }
    let (w, h) = (w as f32, h as f32);
    for side in [
        scene::rect(0.0, 0.0, 1.0, h),
        scene::rect(0.0, 0.0, w, 1.0),
        scene::rect(w - 1.0, 0.0, 1.0, h),
        scene::rect(0.0, h - 1.0, w, 1.0),
    ] {
        scene::fill(scene, Affine::IDENTITY, color, &side);
    }
}

/// Which edges of this window are against the screen's, asked of the OS.
///
/// Three user32 calls, so it is edge-triggered by the caller and cached — see
/// `Pane::flush`. Never ask this from a draw.
///
/// `MonitorHandle` exposes the monitor's full rectangle and not its work area,
/// so the handle is unwrapped and asked directly. **`hmonitor()` returns
/// winit's own `HMONITOR`, which is `isize`, while `windows-sys` declares it a
/// pointer** — the cast is not decoration.
#[cfg(windows)]
pub fn edges(window: &winit::window::Window) -> Edges {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};
    use winit::platform::windows::MonitorHandleExtWindows;

    let (Ok(pos), Some(monitor)) = (window.outer_position(), window.current_monitor()) else {
        return Edges::default();
    };
    let size = window.outer_size();
    let bounds = Bounds {
        left: pos.x,
        top: pos.y,
        right: pos.x.saturating_add(size.width as i32),
        bottom: pos.y.saturating_add(size.height as i32),
    };

    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let hmonitor = monitor.hmonitor() as *mut core::ffi::c_void;
    // SAFETY: `hmonitor` came from winit's live handle, and `info` is sized as
    // the call requires. A failure leaves it zeroed, which the check discards.
    if unsafe { GetMonitorInfoW(hmonitor, &mut info) } == 0 {
        return Edges::default();
    }
    let work = Bounds {
        left: info.rcWork.left,
        top: info.rcWork.top,
        right: info.rcWork.right,
        bottom: info.rcWork.bottom,
    };
    flush(bounds, work)
}

/// Elsewhere there is no compositor border to be missing, so there is nothing
/// to stand in for. Adding an outline here would be a new piece of chrome
/// rather than a repair; see [ROADMAP](../../../docs/deferred/README.md).
#[cfg(not(windows))]
pub fn edges(_window: &winit::window::Window) -> Edges {
    Edges::default()
}

#[cfg(test)]
mod tests;
