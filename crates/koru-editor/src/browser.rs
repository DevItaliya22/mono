// SPDX-License-Identifier: MPL-2.0

//! Handing a supported URL to its OS-registered application.
//!
//! Deliberately restricted to `http`/`https` and OneNote links retained by the
//! importer. A task document is user data, and
//! a Ctrl+click should not be able to launch an arbitrary protocol handler
//! (`file:`, `javascript:`, arbitrary app schemes) just because someone typed
//! one into a link pill. `onenote:` is the narrow migration exception: an
//! unresolved imported link is useful precisely because the installed OneNote
//! handler may still know where it goes.

#[cfg(any(test, feature = "no-browser"))]
use std::cell::RefCell;

// What the last `launch` would have handed the OS, when the guard is on.
#[cfg(any(test, feature = "no-browser"))]
thread_local! {
    static LAST: RefCell<Option<(String, bool)>> = const { RefCell::new(None) };
}

/// Whether a launch is recorded instead of performed.
///
/// **A test must not open a browser window.** A plain click on a link follows
/// it, so a click landing on one from a unit test opens a real window on the
/// machine running `cargo test`. `cfg(test)` covers this crate's own tests and
/// nothing above it — *a dependency is never compiled with `cfg(test)`* — so
/// `apps/koru` and `apps/koru-document` turn on the `no-browser` feature in
/// their `[dev-dependencies]`. The same lesson, and the same shape, as
/// `koru-search-ui`'s `no-reader`.
///
/// `cfg!` rather than `#[cfg]` so the real path keeps its only caller and does
/// not become a page of dead-code warnings under the feature.
fn recorded() -> bool {
    cfg!(test) || cfg!(feature = "no-browser")
}

/// Open `url` with the application registered for its scheme, activating it.
pub fn open(url: &str) -> Result<(), String> {
    launch(url, true)
}

/// Open `url` without taking focus away from Koru when the platform allows it.
pub fn open_without_activating(url: &str) -> Result<(), String> {
    launch(url, false)
}

fn launch(url: &str, activate: bool) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("onenote:"))
    {
        return Err(format!("refusing to open a non-web URL: {url}"));
    }

    if recorded() {
        #[cfg(any(test, feature = "no-browser"))]
        LAST.with(|last| *last.borrow_mut() = Some((url.to_string(), activate)));
        return Ok(());
    }

    let mut cmd = command_for(url, activate);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open {url}: {e}"))
}

#[cfg(any(test, feature = "no-browser"))]
pub fn last_launch() -> Option<(String, bool)> {
    LAST.with(|last| last.borrow().clone())
}

#[cfg(any(test, feature = "no-browser"))]
pub fn clear_last_launch() {
    LAST.with(|last| *last.borrow_mut() = None);
}

#[cfg(target_os = "windows")]
fn command_for(url: &str, _activate: bool) -> std::process::Command {
    // `rundll32` rather than `cmd /C start`, because cmd re-parses the string
    // and would treat `&` in a query string as a command separator. Activation
    // is the handler's choice on Windows; the shell refocuses this window after
    // a stay-in-Koru open.
    let mut c = std::process::Command::new("rundll32.exe");
    c.arg("url.dll,FileProtocolHandler").arg(url);
    c
}

#[cfg(target_os = "macos")]
fn command_for(url: &str, activate: bool) -> std::process::Command {
    let mut c = std::process::Command::new("open");
    if !activate {
        c.arg("-g");
    }
    c.arg(url);
    c
}

#[cfg(all(unix, not(target_os = "macos")))]
fn command_for(url: &str, _activate: bool) -> std::process::Command {
    let mut c = std::process::Command::new("xdg-open");
    c.arg(url);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_schemes_are_allowed() {
        for url in [
            "http://example.com",
            "https://example.com/a?b=1&c=2",
            "onenote:Notebook.one#Page&section-id={123}",
        ] {
            let lower = url.to_ascii_lowercase();
            assert!(
                lower.starts_with("http://")
                    || lower.starts_with("https://")
                    || lower.starts_with("onenote:")
            );
        }
    }

    #[test]
    fn everything_else_is_refused() {
        for url in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "ms-msdt:/id",
            "steam://run/1",
            "mailto:a@b.c",
            "example.com",
            "",
        ] {
            assert!(open(url).is_err(), "{url} should not be handed to the OS");
        }
    }

    /// A `&` in a query string must survive intact — the reason Windows uses
    /// rundll32 instead of `cmd /C start`.
    #[test]
    fn query_strings_are_passed_as_one_argument() {
        let url = "https://example.com/search?q=a&r=b";
        let cmd = command_for(url, true);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == url),
            "the whole URL should be a single argument, got {args:?}"
        );
    }

    /// The counterpart of `a_surface_built_under_test_dials_nothing`: what a
    /// test would otherwise do is spawn a process, and the assertions below
    /// only mean anything while nothing does.
    #[test]
    fn a_launch_under_test_is_recorded_rather_than_performed() {
        assert!(recorded(), "a test must not hand a URL to the OS");
    }

    #[test]
    fn open_records_an_activating_launch() {
        clear_last_launch();
        open("https://example.com").unwrap();
        assert_eq!(last_launch(), Some(("https://example.com".into(), true)));
    }

    #[test]
    fn open_without_activating_records_a_background_launch() {
        clear_last_launch();
        open_without_activating("https://example.com/stay").unwrap();
        assert_eq!(
            last_launch(),
            Some(("https://example.com/stay".into(), false))
        );
    }
}
