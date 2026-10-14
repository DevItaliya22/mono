// SPDX-License-Identifier: MPL-2.0

//! **Who a zoom gesture belongs to** — the view under it, or the app.
//!
//! Primary+wheel (a Windows precision-touchpad pinch arrives as the same thing)
//! and the Ctrl+= / Ctrl+- / Ctrl+0 chords are the **display zoom's** — the
//! browser convention, owned by the shell so a welcome tab zooms too. A view
//! with a zoom of its own — a canvas, which zooms its camera around the pointer
//! — claims them first through [`View::zoom_wheel`] and [`View::zoom_key`], and
//! only a view that declines leaves the display zoom to move (CANVAS P1.4).
//!
//! Two pure functions rather than two branches inside `window_event`, because
//! the arm they came from needs a live window and a GPU to reach, and the rule
//! they carry — *the view first, the app only on a refusal* — is the one worth
//! pinning. `window_event` applies what they answer through `rezoom`, which
//! clamps and snaps as it always has.

use koru_draw::theme;
use winit::event::MouseScrollDelta;

use crate::{View, ZoomIntent};

/// Primary+wheel: offer the **raw** delta to the view, at the pointer in the
/// view's own coordinates; answer the display zoom's next value only when the
/// view declines and the wheel actually moved.
///
/// `pointer` is the pane's cursor in logical pixels and `zoom` the display
/// zoom now — the view lives in unzoomed logical pixels, the space
/// `mouse_move` is handed, so the division happens here, once, for every view.
///
/// `None` is *leave the display zoom alone* — the view claimed the gesture, or
/// a sideways-only wheel moved nothing vertical.
pub(super) fn primary_wheel<V: View + ?Sized>(
    view: &mut V,
    delta: MouseScrollDelta,
    pointer: (f32, f32),
    zoom: f32,
) -> Option<f32> {
    if view.zoom_wheel(delta, pointer.0 / zoom, pointer.1 / zoom) {
        return None;
    }
    let y = match delta {
        MouseScrollDelta::LineDelta(_, y) => y,
        MouseScrollDelta::PixelDelta(p) => p.y as f32,
    };
    // **Not `y.signum()`**, which is what this was: `0.0_f32.signum()` is
    // `1.0`, so the `step != 0.0` guard beside it never fired and a sideways
    // touchpad swipe with Ctrl held zoomed *in*. Only vertical travel zooms.
    let step = if y > 0.0 {
        1.0
    } else if y < 0.0 {
        -1.0
    } else {
        return None;
    };
    Some(zoom + step * theme::ZOOM_STEP)
}

/// Ctrl+= / Ctrl+- / Ctrl+0: offer the chord to the view; answer the display
/// zoom's next value only when it declines. `None` means the view claimed it.
pub(super) fn zoom_chord<V: View + ?Sized>(
    view: &mut V,
    intent: ZoomIntent,
    zoom: f32,
) -> Option<f32> {
    if view.zoom_key(intent) {
        return None;
    }
    Some(match intent {
        ZoomIntent::In => zoom + theme::ZOOM_STEP,
        ZoomIntent::Out => zoom - theme::ZOOM_STEP,
        ZoomIntent::ActualSize => 1.0,
    })
}

/// The chord a primary-only key press names, if it is one of the three.
///
/// `=` in, `-` out, `0` back to 100% — not `+` (Shift+=), which the editor binds
/// to superscript.
pub(super) fn chord_intent(letter: Option<char>) -> Option<ZoomIntent> {
    match letter {
        Some('=') => Some(ZoomIntent::In),
        Some('-') => Some(ZoomIntent::Out),
        Some('0') => Some(ZoomIntent::ActualSize),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
