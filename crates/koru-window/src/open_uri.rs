// SPDX-License-Identifier: MPL-2.0

//! Hand a URI to the desktop and let it decide.
//!
//! ## Why this is not `webbrowser`
//!
//! `webbrowser` is the right tool for the one thing it claims to be: opening a
//! **web page**. It is the wrong tool for a `mailto:`, and quietly so. Nothing
//! in this workspace links it any more — `apps/koru` did, for three update
//! links, and those go through `koru_editor::browser` now, which asks the
//! shell the same way this does and refuses a scheme that is not `http`,
//! `https` or `onenote:` besides. That allow-list is the difference between
//! the two openers left: a URL out of a release manifest is remote input, and
//! a `mailto:` typed into a document is not something to allow-list.
//!
//! That sentence has been false once: `koru-import-google-docs` arrived linking
//! `webbrowser` for an `oauth::default_open` nobody could inject past. The fix
//! was not a fourth opener but the seam that crate already had —
//! `GoogleDocsImporter::new` takes a `fn(&str) -> std::io::Result<()>` and
//! `koru-workspace-host` hands it this function ([ADR-172](../../../docs/decisions/README.md)).
//! Anything below the shell that must open a URL takes an opener; it does not
//! link one.
//!
//! Its Windows backend does not ask the shell to open the URI. It calls
//! `AssocQueryStringW` with the protocol hardcoded to `http`, finds the command
//! line of the default *browser*, and runs that browser with the string as an
//! argument. Hand it `mailto:someone@example.com` and it launches a browser with
//! a mail address on its command line. Most browsers hand that back to the shell,
//! so it usually appears to work — which is the worst outcome, because the
//! mechanism is "launch a browser and hope" and it fails on exactly the machines
//! nobody tests on.
//!
//! ## Why it lives here
//!
//! Both binaries that need it — `koru.exe` and `koru-agent.exe` — already depend
//! on this crate directly, and neither may depend on the other. The alternative
//! was a copy in each, which is the shape this crate exists to prevent: every
//! entry on its platform-traps list was a real bug, and a second copy means the
//! next Windows fix has to be found twice.
//!
//! It fits the charter. `koru-window` is the window *and its platform traps* —
//! it already holds `desktop_preferences`, `system_locale` and the undecorated
//! resize border, none of which is a window either. Asking the OS to open
//! something is that kind of fact.

/// Open a URI with whatever the desktop has registered for its scheme.
///
/// `https:`, `mailto:` and a plain path all work; the shell decides, which is
/// the point — an application guessing which program should answer a scheme is
/// how a mail link opens a browser.
///
/// **Success means the desktop accepted the request, not that anything
/// opened.** Nothing can be learned about whether a handler displayed its
/// surface, and nothing should try: on Windows a `mailto:` with no mail client
/// raises the operating system's own "how do you want to open this" dialog,
/// which is the right owner of that conversation. Callers log a failure and
/// carry on.
///
/// **`ShellExecuteW`, not `explorer.exe`.** Handing a URL to Explorer as an
/// argument does open the browser, but Explorer is a file browser first: it
/// also opens a window of its own for the argument it was given, so every
/// shared link a person opened left a stray folder window behind it. That is
/// what `ShellExecuteW` is for, and it is the API Windows documents for this.
///
/// The null verb is deliberate — it means "whatever double-clicking does",
/// which is the right answer both for a URL and for the temp file the caller
/// writes when the bytes are not a link.
#[cfg(windows)]
pub fn open_uri(uri: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide: Vec<u16> = std::ffi::OsStr::new(uri)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // `open_uri` runs on the file-opening worker, which owns no apartment of
    // its own. An already-initialized apartment answers `RPC_E_CHANGED_MODE`
    // and is not an error here: somebody else set it up and the call is still
    // valid. Deliberately never uninitialized — the shell may still be using
    // the apartment after this returns.
    // SAFETY: no arguments, and the result is only inspected.
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }

    // SAFETY: `wide` is a null-terminated UTF-16 string that outlives the
    // call; every other argument is null or a documented constant.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    shell_execute_result(result as isize)
}

#[cfg(windows)]
fn shell_execute_result(result: isize) -> std::io::Result<()> {
    if result > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "the desktop URI handler failed with code {result}"
        )))
    }
}

/// See the Windows arm for what success means.
#[cfg(not(windows))]
pub fn open_uri(uri: &str) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(uri)
        .spawn()
        .map(|_| ())
}

/// Where feedback goes.
///
/// **One spelling, in the crate both callers already link.** It is written on
/// the Welcome tab, in the agent's About submenu, and inside the greeting at the
/// top of a *What's new* tab. Three literals of one address is the kind of drift
/// nobody notices until a person writes to a mailbox that stopped existing.
pub const FEEDBACK_MAILTO: &str = "mailto:hello@koru.do";

/// The address on its own, for prose that names it rather than links it.
pub const FEEDBACK_ADDRESS: &str = "hello@koru.do";

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn shell_execute_uses_the_documented_success_boundary() {
        assert!(shell_execute_result(33).is_ok());
        assert!(shell_execute_result(32).is_err());
    }

    /// The two spellings are one address, so a change to either is a change to
    /// both. Nothing here launches anything — a test that opened a mail client
    /// on a developer's machine would be a worse bug than the one it caught.
    #[test]
    fn the_link_and_the_address_agree() {
        assert_eq!(FEEDBACK_MAILTO, format!("mailto:{FEEDBACK_ADDRESS}"));
        assert!(!FEEDBACK_ADDRESS.contains(':'), "an address, not a URI");
    }
}
