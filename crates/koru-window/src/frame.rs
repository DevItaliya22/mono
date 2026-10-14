// SPDX-License-Identifier: MPL-2.0

//! The window's own outline: a 1px border, rounded corners, and a drop shadow
//! while the window is not maximised.
//!
//! ## Where this stops being enough
//!
//! DWM paints the border at the **window** rect, which stops being the client
//! rect the moment an edge reaches the screen — so a maximised or snapped
//! window's border is not missing but off-screen, and the shell draws its own
//! there instead. [`crate::hairline`] explains the mechanism. The border colour
//! here is still what floating edges wear; the hairline stands in once an edge
//! is flush.
//!
//! ## Corners
//!
//! Rounded corners cannot be drawn. A window is a rectangle, and painting a
//! rounded shape inside one leaves the four real corners showing whatever was
//! painted there — the effect is a rounded *outline* on a square window, which
//! is worse than no rounding at all. Only the compositor can actually clip the
//! window's shape, so on Windows 11 that is `DWMWA_WINDOW_CORNER_PREFERENCE`,
//! reached through winit's `WindowExtWindows`. The border is its companion
//! attribute, and follows the same clip.
//!
//! Corners follow maximised state only: a snapped window stays rounded, matching
//! the desktop default. A maximised window is squared — there is nowhere for a
//! shadow to fall, and rounding would cut the corners out of the content for no
//! reason. The drawn hairline may lose ~8px at each arc on a snapped window.
//!
//! `DWMWCP_ROUND` is the system radius — 8px on Windows 11 — rather than a
//! number of ours. That is the point: a window that rounds by a different amount
//! from every other window on the desktop looks broken rather than styled.
//!
//! ## Drop shadow — two paths, one switch
//!
//! A window shadow has to sit *outside* the client; the scene cannot paint it.
//!
//! 1. **Modern (Win11).** Tied to rounded corners. Works on a floating
//!    undecorated window with a full client area. DWM drops it when the window
//!    is aero-snapped, even if corners stay round — so Round alone does not
//!    keep the shadow on snap.
//! 2. **Classic NC strip.** winit's `set_undecorated_shadow(true)` leaves a 1px
//!    non-client tweak in `WM_NCCALCSIZE` so DWM can cast the aero drop shadow.
//!    That path survives snap. Side-effect (winit's own): a thin 1px line on the
//!    top of the window.
//!
//! Both are on whenever the window is not maximised; both off when it is. See
//! [ADR-48](../../../docs/decisions/README.md).
//!
//! ## Elsewhere
//!
//! Border colour and corner preference are Windows 11 and later. On Windows 10
//! those calls fail harmlessly inside winit; the classic shadow still applies.
//! On other platforms this is a no-op. Nothing above needs to know which
//! happened.

use winit::window::Window;

/// Make a decorated Windows caption use the same semantic colors as its page.
///
/// On older Windows builds the compositor rejects these attributes and keeps
/// its native caption unchanged. Other platforms already derive their native
/// title bars from the window theme, so this is intentionally a no-op there.
#[cfg(windows)]
pub fn apply_caption(window: &Window, background: peniko::Color, text: peniko::Color) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmSetWindowAttribute,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Some(hwnd) = window
        .window_handle()
        .ok()
        .and_then(|handle| match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut core::ffi::c_void),
            _ => None,
        })
    else {
        return;
    };
    for (attribute, color) in [
        (DWMWA_CAPTION_COLOR as u32, colorref(background)),
        (DWMWA_TEXT_COLOR as u32, colorref(text)),
    ] {
        // SAFETY: the HWND belongs to this live winit window, the values
        // outlive each synchronous call, and DWM copies four bytes from them.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                attribute,
                std::ptr::addr_of!(color).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}

#[cfg(not(windows))]
pub fn apply_caption(_window: &Window, _background: peniko::Color, _text: peniko::Color) {}

#[cfg(any(windows, test))]
fn colorref(color: peniko::Color) -> u32 {
    let [r, g, b, _] = color.to_rgba8().to_u8_array();
    u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)
}

/// Apply border, corners, and drop shadow for the window's current state.
///
/// `square` is true when the window is maximised. Cheap, but not free — DWM
/// attributes plus a frame refresh for the shadow flag — so callers only invoke
/// it when something it depends on has actually changed: the theme, or
/// maximised state.
#[cfg(windows)]
pub fn apply(window: &Window, border: peniko::Color, square: bool) {
    use winit::platform::windows::{Color, CornerPreference, WindowExtWindows};

    let [r, g, b, _] = border.to_rgba8().to_u8_array();
    window.set_border_color(Some(Color::from_rgb(r, g, b)));
    // Maximised only. A snapped window keeps Round (desktop default) and the
    // classic NC shadow below; squaring on any flush edge was wrong for both.
    window.set_corner_preference(if square {
        CornerPreference::DoNotRound
    } else {
        CornerPreference::Round
    });
    // Classic aero shadow via a 1px NC strip. The modern Round shadow alone does
    // not survive snap; this one does. Off when maximised — nowhere to cast.
    window.set_undecorated_shadow(!square);
}

#[cfg(not(windows))]
pub fn apply(_window: &Window, _border: peniko::Color, _square: bool) {}

/// Ask Windows 11 for its compositor shadow without enabling winit's classic
/// one-pixel non-client strip. A pill-shaped utility window supplies its own
/// native region and draws one outline on that exact edge; only the shadow
/// belongs here. DWM's system-radius border must be disabled because it does
/// not match the capsule radius at the left and right ends.
#[cfg(windows)]
pub(crate) fn apply_utility_shadow(window: &Window) {
    use winit::platform::windows::{CornerPreference, WindowExtWindows};

    window.set_border_color(None);
    window.set_corner_preference(CornerPreference::Round);
}

#[cfg(not(windows))]
pub(crate) fn apply_utility_shadow(_window: &Window) {}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_color_uses_windows_colorref_channel_order() {
        assert_eq!(
            colorref(peniko::Color::from_rgb8(0x12, 0x34, 0x56)),
            0x0056_3412
        );
    }
}
