// SPDX-License-Identifier: MPL-2.0

//! Getting a console back, for the invocations that need one.
//!
//! The binary is `windows_subsystem = "windows"` in every build — without that,
//! a Jump List launch of `koru.exe` flashes a console window, which is the
//! opposite of a taskbar action. But the setting applies to the whole
//! executable, so `koru --help` typed into a terminal would print into nothing.
//!
//! So the console is *attached* rather than owned: [`attach`] joins the console
//! of whatever launched this process, which for a terminal is the terminal you
//! typed in. Started from Explorer there is no parent console, the call fails,
//! and the process runs windowless — which is the no-console case the subsystem
//! was set for.
//!
//! ## The trap, which is the whole reason this is not two lines
//!
//! **Attach only when the standard handles are not already valid.** A test
//! spawns the binary with piped stdio; the harness is reading those pipes;
//! `AttachConsole` would take stdout and stderr away from it and hand them to a
//! console the harness cannot see. So: ask what we were given first, and leave a
//! valid handle exactly as it is. A parent that arranged where our output goes
//! has already answered the question this module is asking.
//!
//! ## Everywhere else
//!
//! There is nothing to do. Only Windows has a subsystem flag, and only Windows
//! detaches a process from its terminal by default.

/// Attach to the launching process's console, if this process has none.
///
/// Best effort and never fatal: a program with nowhere to print is a program
/// whose output is lost, not one that should refuse to run. The caller has
/// already decided it wants a console; this either finds one or does not.
pub fn attach() {
    platform::attach();
}

#[cfg(windows)]
mod platform {
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };

    pub fn attach() {
        if already_connected() {
            // A pipe, a file, or a console we already own. Whoever launched us
            // said where the output goes, and that answer outranks this one.
            return;
        }
        // SAFETY: no arguments to get wrong, and the return is checked by the
        // caller's next write rather than here — failing means there was no
        // parent console, which is not an error.
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }

    /// Whether stdout already goes somewhere.
    ///
    /// A GUI-subsystem process launched from Explorer has `NULL` here; one
    /// launched from a terminal *also* has `NULL`, because the subsystem flag
    /// means no console was allocated. One spawned with a pipe has a real
    /// handle — which is the case this exists to protect.
    fn already_connected() -> bool {
        // SAFETY: `GetStdHandle` takes a constant and returns a handle or an
        // error; nothing is dereferenced.
        match unsafe { GetStdHandle(STD_OUTPUT_HANDLE) } {
            // `NULL` is a windows-subsystem process with no console and no
            // redirected output; `INVALID_HANDLE_VALUE` is the error sentinel.
            Ok(handle) => !handle.is_invalid() && !handle.0.is_null(),
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
mod platform {
    /// Nothing to attach to and nothing to attach: a process keeps its
    /// terminal, and there is no subsystem flag to have taken it away.
    pub fn attach() {}
}
