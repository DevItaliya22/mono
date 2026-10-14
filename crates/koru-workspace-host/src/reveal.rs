// SPDX-License-Identifier: MPL-2.0

//! Showing a path in the OS file manager.
//!
//! A few lines and three `cfg`s, but worth its own module because the
//! alternative is this appearing inline in `app.rs` and growing a second copy
//! later.
//!
//! **The menu item is hidden where this cannot work rather than dimmed.** An
//! item that is there but does nothing is worse than one that is not there; the
//! tray has the same policy for the same reason. [`is_supported`] is what the
//! menu asks before offering the row.
//!
//! So `is_supported` and `reveal` must never disagree — a platform answering
//! `true` and then returning `Unsupported` puts a row on the menu that fails
//! when pressed, which is the outcome the policy exists to prevent. **They are
//! therefore written as one pair per platform rather than as two independent
//! `cfg` lists.** Adding a platform means adding both in the same block;
//! forgetting one is a missing-function or duplicate-definition error at
//! compile time, which is a stronger guarantee than a test and needs no process
//! spawned to check it.
//!
//! **Windows.** A folder is `explorer <path>`. A **file** must be
//! `explorer /select,"path"` as a *raw* command-line fragment: ordinary
//! `Command::arg` wraps the whole `/select,…` token in quotes, which explorer
//! then treats as a path and **opens the file** with the default app instead of
//! selecting it in its parent. `explorer.exe` famously returns a non-zero exit
//! code on success, so nothing here inspects the status — only whether the
//! process could be started at all.
//!
//! **macOS.** `open -R <path>` reveals the item *in its parent*, selected,
//! which is the platform convention for both files and folders. Plain
//! `open <path>` would open the folder as a new Finder window or launch the
//! file — the Windows-folder behaviour, and not what a Mac user means by
//! "show in Finder". Nothing waits on the result: this is fire-and-forget from
//! a menu row, and blocking the event loop on a Finder launch would be a worse
//! bug than a silent no-op.
//!
//! **Linux** would be `xdg-open`, which has no reveal-and-select form. Left
//! unsupported rather than guessed at.

use std::path::Path;

// ------------------------------------------------------------------- windows

/// Whether this platform can show a folder at all. What the menu asks before it
/// offers the item.
#[cfg(windows)]
pub const fn is_supported() -> bool {
    true
}

/// Open `path` in the file manager. A file is selected in its parent; a
/// directory is opened.
#[cfg(windows)]
pub fn reveal(path: &Path) -> std::io::Result<()> {
    // **Not `is_file()`.** A just-written path that `is_file` cannot see (or a
    // path that is only a file by extension) must still use `/select`. The
    // folder branch is only for real directories; everything else is selected,
    // because `explorer <file>` opens the file with its default app — which is
    // how Show was opening the exported markdown instead of its folder.
    if path.is_dir() {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map(|_| ())
    } else {
        // Prefer a canonical path so explorer is not handed a relative form it
        // cannot select. Fall back to the original if canonicalize fails.
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        // Strip the `\\?\` prefix canonicalize adds on Windows — explorer's
        // `/select` does not accept it.
        let display = strip_verbatim_prefix(&path);
        // Raw: one command-line fragment explorer actually understands. Do not
        // use `.arg` — it would quote the whole `/select,"…"` and break parsing.
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{display}\""))
            .spawn()
            .map(|_| ())
    }
}

/// `canonicalize` on Windows yields `\\?\C:\…`; explorer wants `C:\…`.
#[cfg(windows)]
fn strip_verbatim_prefix(path: &Path) -> String {
    let s = path.display().to_string();
    s.strip_prefix(r"\\?\")
        .or_else(|| s.strip_prefix("//?/"))
        .unwrap_or(&s)
        .to_owned()
}

// --------------------------------------------------------------------- macos

#[cfg(target_os = "macos")]
pub const fn is_supported() -> bool {
    true
}

/// Reveal `path` in Finder, selected inside its parent.
#[cfg(target_os = "macos")]
pub fn reveal(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(|_| ())
}

// ------------------------------------------------------------------ elsewhere

#[cfg(not(any(windows, target_os = "macos")))]
pub const fn is_supported() -> bool {
    false
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn reveal(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "showing a path is not implemented on this platform",
    ))
}

#[cfg(all(test, windows))]
mod tests {
    use super::strip_verbatim_prefix;
    use std::path::Path;

    #[test]
    fn verbatim_prefix_is_stripped_for_explorer() {
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\C:\dev\out.md")),
            r"C:\dev\out.md"
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"C:\dev\out.md")),
            r"C:\dev\out.md"
        );
    }
}
