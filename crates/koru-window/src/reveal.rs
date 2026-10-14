// SPDX-License-Identifier: MPL-2.0

//! Showing a window that has already painted, without an animation.
//!
//! The caller creates the window hidden and paints its first frame into the
//! hidden surface; this puts it on screen. That ordering is the point — it is
//! what stops a panel appearing as a blank rectangle and filling in afterwards
//! — and it is all that is left of what this module used to do.
//!
//! ## The expand-from-centre animation is gone
//!
//! It was `AnimateWindow(hwnd, 90, AW_ACTIVATE | AW_CENTER)`, chosen because
//! Windows otherwise picks its own effect for a newly shown top-level window
//! and that one can read as motion from a screen edge. Both are wrong for a
//! panel summoned by a keystroke: the whole promise of a global shortcut is
//! that the box is *already there* when your hands arrive, and 90 ms of
//! expansion in front of it is 90 ms of waiting for something you asked for
//! instantly.
//!
//! So the animation is not replaced by the OS default — that would be the same
//! complaint with different motion. `DWMWA_TRANSITIONS_FORCEDISABLED` turns the
//! compositor's own transitions off for this window first, and only then is it
//! shown. What that buys is a panel that is simply present on the frame after
//! the hotkey.

use winit::window::Window;

#[cfg(windows)]
pub fn when_painted(window: &Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = window
        .window_handle()
        .ok()
        .and_then(|handle| match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut core::ffi::c_void),
            _ => None,
        });
    if let Some(hwnd) = handle {
        use windows_sys::Win32::Foundation::TRUE;
        use windows_sys::Win32::Graphics::Dwm::{
            DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute,
        };
        let disable: i32 = TRUE;
        // SAFETY: this runs on the event-loop thread that created `window`, and
        // the handle came from winit moments ago. The value outlives the call.
        // A failure is ignored deliberately: on a desktop with the compositor
        // off there is no transition to disable and nothing to report.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                std::ptr::addr_of!(disable).cast(),
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
    // **Always through winit.** Showing the window behind its back leaves
    // winit's visibility flag stale, and a later path that trusts it leaves
    // Search open but invisible — a hotkey that "does nothing".
    window.set_visible(true);
}

#[cfg(not(windows))]
pub fn when_painted(window: &Window) {
    window.set_visible(true);
}
