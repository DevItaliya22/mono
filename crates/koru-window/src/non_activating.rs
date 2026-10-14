// SPDX-License-Identifier: MPL-2.0

//! Passive window behavior shared by status surfaces.

/// Whether a native window is an input destination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowActivation {
    #[default]
    Focused,
    Passive,
}

impl WindowActivation {
    pub(crate) fn allows_focus(self) -> bool {
        self == Self::Focused
    }
}

#[cfg(windows)]
fn passive_style(styles: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    // winit creates an ownerless top-level window with WS_EX_APPWINDOW, then
    // implements `skip_taskbar` through ITaskbarList::DeleteTab. That removes
    // the current button but leaves Windows free to add it again when the
    // initially hidden HUD is revealed. A passive utility surface is a native
    // tool window instead: it has no application/taskbar identity at all.
    (styles | WS_EX_NOACTIVATE as isize | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize)
}

#[cfg(windows)]
fn is_passive_style(styles: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    styles & WS_EX_NOACTIVATE as isize != 0
        && styles & WS_EX_TOOLWINDOW as isize != 0
        && styles & WS_EX_APPWINDOW as isize == 0
}

#[cfg(windows)]
pub(crate) fn apply(window: &winit::window::Window) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = handle.hwnd.get() as *mut core::ffi::c_void;
    // SAFETY: this runs on the HWND's creator thread and preserves every
    // existing extended style.
    unsafe {
        let styles = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, passive_style(styles));
        is_passive_style(GetWindowLongPtrW(hwnd, GWL_EXSTYLE))
    }
}

#[cfg(not(windows))]
pub(crate) fn apply(_window: &winit::window::Window) -> bool {
    true
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn passive_style_preserves_every_existing_extended_style() {
        let existing = 0x0010_0000_isize;
        let passive = passive_style(existing);
        assert_eq!(passive & existing, existing);
        assert!(is_passive_style(passive));
    }

    #[test]
    fn passive_style_can_be_restored_after_a_full_winit_style_rewrite() {
        use windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_APPWINDOW;

        let rewritten_without_noactivate = WS_EX_APPWINDOW as isize;
        assert!(!is_passive_style(rewritten_without_noactivate));
        let restored = passive_style(rewritten_without_noactivate);
        assert!(is_passive_style(restored));
        assert_eq!(restored & WS_EX_APPWINDOW as isize, 0);
    }

    #[test]
    fn passive_never_allows_a_later_focus_request() {
        assert!(WindowActivation::Focused.allows_focus());
        assert!(!WindowActivation::Passive.allows_focus());
    }
}
