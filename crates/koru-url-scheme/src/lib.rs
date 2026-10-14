// SPDX-License-Identifier: MPL-2.0

//! Whether this build is the one that opens `koru://` links.
//!
//! Registration is **per user** — `HKEY_CURRENT_USER\Software\Classes` — which
//! needs no elevation and no installer. Koru ships as a zip extracted anywhere
//! ([ADR-112](../../../docs/decisions/README.md)), so there is no install step to hang
//! this off; it is a thing a person switches on from the agent's tray icon.
//!
//! **Last registered wins, and nothing is ever registered without being
//! asked.** Every channel installs side by side and shares nothing on disk, but
//! a URL scheme has exactly one owner per user — so whichever build most
//! recently registered is the one Windows launches. That is the same accepted
//! cost as the machine-wide hotkey, and the tray is expected to *say* which
//! build owns it rather than leaving somebody to guess.
//!
//! A link is written into a document and outlives any one build, so the scheme
//! is `koru` for every channel. A `koru-nightly://` in exported Markdown would
//! be a bug that outlived the nightly that wrote it.

use std::path::Path;

/// Where the four keys live, relative to `HKEY_CURRENT_USER`.
///
/// A parameter rather than a constant everywhere below, so a test can build and
/// read a tree under a root of its own instead of the one the person's browser
/// is using.
pub const CLASSES: &str = "Software\\Classes";

/// Who currently opens `koru://` links.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Registration {
    /// Nobody: the scheme is not registered.
    Absent,
    /// This executable.
    ThisBuild,
    /// Another Koru build — a different channel, or a copy somewhere else.
    /// Carries the path so the tray can name it rather than saying "elsewhere".
    Other { exe: String },
    /// Something that is not Koru at all. Registering over it is allowed;
    /// *unregistering* it is not ours to do.
    Foreign { command: String },
    /// Not Windows.
    Unsupported,
}

/// One registry value to write: subkey below the classes root, value name
/// (empty for the key's default), and data.
pub type Entry = (String, &'static str, String);

/// Everything registration writes, given the executable to launch.
///
/// Pure, and separately testable, which is most of why it exists: asserting
/// what would be written costs nothing and touches no registry.
pub fn entries(exe: &Path) -> Vec<Entry> {
    let exe = exe.display().to_string();
    vec![
        // The default value is a description; `URL Protocol` is the flag, and
        // its *presence* is what makes Windows treat this as a scheme. Its
        // content is ignored, which is why it is empty.
        ("koru".to_owned(), "", "URL:Koru Protocol".to_owned()),
        ("koru".to_owned(), "URL Protocol", String::new()),
        ("koru\\DefaultIcon".to_owned(), "", format!("\"{exe}\",0")),
        (
            "koru\\shell\\open\\command".to_owned(),
            "",
            // **Quoted, both of them.** The exe path has spaces in it more
            // often than not, and `%1` arrives holding whatever a web page put
            // in the URL. The parser refuses a quote as well; this is the other
            // half of that pair.
            format!("\"{exe}\" open-url \"%1\""),
        ),
    ]
}

/// Read the registered command back and say whose it is.
///
/// Pure, so the classification is testable without a registry: the caller
/// supplies whatever `shell\open\command` holds.
pub fn classify(command: Option<&str>, this_exe: &Path) -> Registration {
    let Some(command) = command.map(str::trim).filter(|c| !c.is_empty()) else {
        return Registration::Absent;
    };
    let exe = first_argument(command);
    if same_file(Path::new(exe), this_exe) {
        return Registration::ThisBuild;
    }
    // A Koru build somewhere else, or something else entirely. The distinction
    // decides whether *unregistering* is ours to offer.
    let looks_like_koru = Path::new(exe)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("koru"));
    if looks_like_koru {
        Registration::Other {
            exe: exe.to_owned(),
        }
    } else {
        Registration::Foreign {
            command: command.to_owned(),
        }
    }
}

/// The executable out of a command line.
///
/// Quoted is what we write; unquoted is what a person editing the registry by
/// hand leaves behind, and reading it is cheaper than being confused by it.
fn first_argument(command: &str) -> &str {
    match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.split(' ').next().unwrap_or(command),
    }
}

/// Whether two paths name the same file.
///
/// Canonicalized when both exist — a registration may name this same binary by
/// a different route — and compared case-insensitively otherwise, because a
/// build that has been moved or deleted still has to classify as *something*
/// rather than erroring.
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}

/// Make this executable the one that opens `koru://` links.
pub fn register(exe: &Path) -> std::io::Result<()> {
    platform::write_all(CLASSES, &entries(exe))
}

/// Stop handling `koru://` links.
///
/// **Only ever this build's own registration.** Removing somebody else's from a
/// tray menu is a footgun with no use case; the caller checks [`state`] first
/// and this refuses anything else.
pub fn unregister(exe: &Path) -> std::io::Result<()> {
    match state(exe) {
        Registration::ThisBuild | Registration::Absent => platform::delete_tree(CLASSES, "koru"),
        Registration::Unsupported => platform::unsupported(),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "koru:// is registered to another program",
        )),
    }
}

/// Who opens `koru://` links today.
pub fn state(this_exe: &Path) -> Registration {
    if !platform::SUPPORTED {
        return Registration::Unsupported;
    }
    classify(
        platform::read(CLASSES, "koru\\shell\\open\\command", "").as_deref(),
        this_exe,
    )
}

#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
mod platform {
    use std::io;

    pub const SUPPORTED: bool = false;

    pub fn unsupported() -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "handling koru:// links is currently supported on Windows",
        ))
    }

    pub fn write_all(_root: &str, _entries: &[super::Entry]) -> io::Result<()> {
        unsupported()
    }

    pub fn delete_tree(_root: &str, _subkey: &str) -> io::Result<()> {
        unsupported()
    }

    pub fn read(_root: &str, _subkey: &str, _value: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests;
