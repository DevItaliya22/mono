// SPDX-License-Identifier: MPL-2.0

//! Parent-console attachment for GUI-subsystem binaries.
//!
//! Windows GUI binaries do not allocate a console, which prevents an Explorer
//! or taskbar launch from flashing a black window. A command-line invocation
//! still needs somewhere to print `--help`, `--version`, and early failures.
//! Attach only when stdout is not already a valid pipe, file, or console: a
//! caller that redirected output has already supplied the better destination.

/// Attach to the launching process's console when this process has none.
///
/// Best effort and never fatal. There is no corresponding operation outside
/// Windows because those platforms do not detach based on a subsystem flag.
pub fn attach_parent() {
    platform::attach_parent();
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };

    pub fn attach_parent() {
        if already_connected() {
            return;
        }
        // SAFETY: the constant means "the process which launched this one".
        // Failure only means that parent had no console, which is the desired
        // state for an Explorer or taskbar launch.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    fn already_connected() -> bool {
        // SAFETY: the call returns an opaque handle or sentinel; it dereferences
        // no caller memory.
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        !handle.is_null() && handle != INVALID_HANDLE_VALUE
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn attach_parent() {}
}
