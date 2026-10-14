// SPDX-License-Identifier: MPL-2.0

//! The window's own resize border.
//!
//! ## Why this exists at all
//!
//! An undecorated window keeps `WS_SIZEBOX` — winit leaves the style on — but
//! **that is not enough to resize it**. winit removes the frame by answering
//! `WM_NCCALCSIZE` with a client area covering the whole window, which leaves no
//! non-client region for `DefWindowProc` to report as an edge. Probed with
//! `WM_NCHITTEST`, every pixel of an undecorated window — edges and corners
//! included — comes back `HTCLIENT`. So the border has to be ours.
//!
//! It lives in the shell rather than in the chrome because it is a fact about
//! the *window*, not about what the window contains: every view gets it, and
//! none of them has to know. The shell claims a press here before the view sees
//! it, the same way it claims Ctrl+N.
//!
//! Performing the drag is `Window::drag_resize_window`, which winit does support
//! — it posts a real `WM_NCLBUTTONDOWN`, so snapping and the live outline are
//! the OS's, not ours.
//!
//! ## …and why it is off on macOS
//!
//! **winit does not implement `drag_resize_window` there.** Its macOS backend
//! returns `ExternalError::NotSupported` unconditionally
//! (`window_delegate.rs`, winit 0.30.13). The call site discarded that error
//! with `let _ =` and then **`return`ed**, claiming the press — so on macOS the
//! outermost 6 px of every edge, and 16 px of every corner, silently swallowed
//! mouse-downs and did nothing with them.
//!
//! The caption is the visible casualty: the tab strip begins at `y = 0`, so the
//! top band lies across it and a press there never reached the view that would
//! have asked to drag the window.
//!
//! Two things follow, and only the first is fixed here:
//!
//! 1. **The band is not hit-tested at all on macOS** ([`is_ours`]). A press
//!    reaches the view, and the pointer never shows a resize cursor for a
//!    resize that cannot happen — an affordance that does nothing is worse than
//!    none, the same rule the tray and *Reveal in Finder* follow.
//! 2. **A borderless macOS window therefore has no resize border at all.** That
//!    was a real gap, not a decision — and it is closed from the other side:
//!    the macOS window is *decorated*, so AppKit supplies the edge resizing,
//!    the rounded corners and the titlebar drag. It was never worked around
//!    with a hand-rolled `setFrame:` loop, which is why closing it cost
//!    nothing here.

use winit::window::ResizeDirection;

/// Whether this platform's resize border is ours to draw and to act on.
///
/// False on macOS: see the module docs. Asked by `Pane::resize_edge_at`, which
/// is the one place the band is hit-tested — so the cursor and the press get
/// the same answer, and no cursor ever promises a resize that
/// `drag_resize_window` will refuse.
///
/// The geometry in [`edge`] stays platform-neutral and stays tested everywhere:
/// it is correct, it is simply not consulted here.
pub const fn is_ours() -> bool {
    !cfg!(target_os = "macos")
}

/// How close to an edge counts as being on it, in logical pixels.
///
/// Windows' own frame is about this thick. Wider would eat into the caption's
/// buttons, which sit within a few pixels of the top-right corner.
pub const BORDER: f32 = 6.0;

/// Corners are a wider target than edges, because hitting the exact
/// intersection of two 6px bands is a pixel-hunt. Every OS does this.
pub const CORNER: f32 = 16.0;

/// Which edge or corner `(x, y)` is on, in a `w` × `h` window. All logical.
///
/// `None` everywhere else — including *everywhere* when the window is
/// maximised, since a maximised window cannot be resized and offering the
/// cursor for it would be a lie. That is also what leaves the top-right corner
/// free for the close button when it matters most: with the window filling the
/// screen, throwing the pointer into the corner has to close it.
pub fn edge(w: f32, h: f32, x: f32, y: f32, maximized: bool) -> Option<ResizeDirection> {
    if maximized || w <= 0.0 || h <= 0.0 {
        return None;
    }
    // A window narrower than two borders would have every pixel on both edges;
    // splitting it down the middle keeps each side reachable.
    let band = BORDER.min(w / 2.0).min(h / 2.0);
    let corner = CORNER.min(w / 2.0).min(h / 2.0);

    let (l, r) = (x <= band, x >= w - band);
    let (t, b) = (y <= band, y >= h - band);
    let (cl, cr) = (x <= corner, x >= w - corner);
    let (ct, cb) = (y <= corner, y >= h - corner);

    Some(match () {
        _ if (t && cl) || (l && ct) => ResizeDirection::NorthWest,
        _ if (t && cr) || (r && ct) => ResizeDirection::NorthEast,
        _ if (b && cl) || (l && cb) => ResizeDirection::SouthWest,
        _ if (b && cr) || (r && cb) => ResizeDirection::SouthEast,
        _ if t => ResizeDirection::North,
        _ if b => ResizeDirection::South,
        _ if l => ResizeDirection::West,
        _ if r => ResizeDirection::East,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
