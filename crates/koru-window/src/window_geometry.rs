// SPDX-License-Identifier: MPL-2.0

//! Machine-local file paths and one shared window-geometry row store.
//!
//! Callers own row keys and value schemas. This module owns the shared
//! mechanism: root-aware path selection, tolerant keyed-row parsing,
//! read-filter-rewrite merging, and a coalescing writer that keeps filesystem
//! I/O off every window/event thread.
//!
//! There is deliberately no file lock. Several Koru processes may read the
//! same body and the last completed write wins; a lost position is acceptable,
//! and preserving that existing policy avoids turning window teardown into a
//! blocking cross-process protocol.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const HEADER: &str = "# Where Koru's windows are on THIS machine.\n\
                      # Each surface owns one stable keyed row; row values are owned by that surface.\n";

/// `%LOCALAPPDATA%/koru/` on Windows, `~/Library/Application Support/koru/` on
/// macOS, XDG state elsewhere.
///
/// **Machine-local, never roaming.** What lives here is true of *this*
/// computer — where a window was, which store profile was last searched — and
/// carrying it to another machine would be wrong rather than merely useless.
pub fn local_state_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| macos_support_base(PathBuf::from(home)))
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
    }?;
    Some(base.join("koru"))
}

/// Where macOS keeps application files — for the roaming *and* the
/// machine-local half, because macOS has neither a roaming/local split nor an
/// XDG state directory (ADR-116).
pub(crate) fn macos_support_base(home: PathBuf) -> PathBuf {
    home.join("Library").join("Application Support")
}

/// Resolve the one window-geometry file for this launch.
///
/// An explicit channel root is a complete isolation boundary and therefore
/// resolves to `<root>/agent/windows.conf`. An ordinary launch deliberately
/// preserves the existing machine-local Koru path.
pub fn window_geometry_path(root: Option<&Path>) -> Option<PathBuf> {
    match root {
        Some(root) => Some(root.join("agent").join("windows.conf")),
        None => Some(local_state_dir()?.join("windows.conf")),
    }
}

/// `<local state>/search.conf` — what the search surface reopens on.
///
/// Search state is not geometry, so its existing root policy remains with the
/// Agent rather than being folded into [`window_geometry_path`].
pub fn search_state_path() -> Option<PathBuf> {
    Some(local_state_dir()?.join("search.conf"))
}

/// `<local state>/voice-consent.conf` — what somebody agreed to before Koru
/// downloads a speech model or opens a microphone.
///
/// **Here rather than in `preferences.conf`**, and that is the whole decision:
/// preferences follow the person and roam, and consent given on a desktop is
/// not consent given on a laptop — a different machine has a different
/// microphone, a different network and possibly a different person at it. It
/// is per OS user and device, keyed by nothing else: not by profile, not by
/// workspace, because agreeing to spend 600 MB is not a fact about content.
///
/// The record's *shape* is `koru-voice-consent`'s and its *directory* is this
/// one — the split `search.conf` already uses
/// ([ADR-111](../../../docs/decisions/ADR-101-125.md)), so an agent under
/// `--root` and a test naming a temporary directory are ordinary callers
/// rather than special cases.
pub fn voice_consent_path() -> Option<PathBuf> {
    Some(local_state_dir()?.join(koru_voice_consent::CONSENT_FILE))
}

/// `<local state>/renderer.conf` — which GPU backend this machine draws with.
pub fn renderer_state_path() -> Option<PathBuf> {
    Some(local_state_dir()?.join("renderer.conf"))
}

/// The roaming preferences file the settings authority owns.
///
/// **Nothing in this crate reads it any more**, and neither does anything the
/// three deleted parsers used to serve — SP-6b. It survives as a *locator*
/// for the two one-time migrations still on the way out: the shell's
/// `emoji_recents` / `last_export_dir` relocation, and the Agent's dictation
/// read. It resolves the same place it always did, so those migrations find
/// what somebody's existing install actually wrote.
///
/// On macOS "roaming" and "machine-local" collapse into one directory —
/// ADR-116, and the note on [`macos_support_base`].
pub fn user_preferences_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| macos_support_base(PathBuf::from(home)))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("koru").join("preferences.conf"))
}

/// Read the value owned by `key`, ignoring comments, blank lines, and corrupt
/// rows. Value parsing remains the caller's policy.
pub fn read_window_row(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    row_in(&text, key).map(str::to_owned)
}

/// A coalescing, off-thread writer for one stable row.
///
/// `last_row` means comparisons are against what *this process* submitted, not
/// what happens to be on disk. The first save consequently always queues a
/// write and can restore this process's row after another process rewrote the
/// shared file. Later identical saves are no-ops, including caret-blink and
/// idle-maintenance passes.
#[derive(Debug, Default)]
pub struct WindowRowWriter {
    sender: Option<mpsc::Sender<String>>,
    last_row: Option<String>,
}

impl WindowRowWriter {
    /// Start the worker when a path is available. Failure to create a best-
    /// effort geometry worker leaves persistence disabled; it never prevents a
    /// window from opening.
    pub fn open(path: Option<PathBuf>, key: impl Into<String>, thread_name: &str) -> Self {
        let sender = path.and_then(|path| {
            let key = key.into();
            let (sender, receiver) = mpsc::channel();
            std::thread::Builder::new()
                .name(thread_name.into())
                .spawn(move || write_rows(&path, &key, receiver))
                .ok()
                .map(|_| sender)
        });
        Self {
            sender,
            last_row: None,
        }
    }

    /// Whether submitting `row` would be news for this process.
    pub fn would_write(&self, row: &str) -> bool {
        self.last_row.as_deref() != Some(row)
    }

    /// Queue `row` only when it differs from this process's last submission.
    /// Returns whether a submission was new. The value is remembered even
    /// when persistence is disabled or the worker has gone away, so an idle
    /// loop never turns a failed best-effort write into a write storm.
    pub fn write_if_changed(&mut self, row: impl Into<String>) -> bool {
        let row = row.into();
        if !self.would_write(&row) {
            return false;
        }
        if let Some(sender) = &self.sender {
            let _ = sender.send(row.clone());
        }
        self.last_row = Some(row);
        true
    }
}

fn write_rows(path: &Path, key: &str, receiver: mpsc::Receiver<String>) {
    while let Ok(first) = receiver.recv() {
        let row = latest_queued(first, &receiver);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let existing = fs::read_to_string(path).unwrap_or_default();
        let _ = fs::write(path, merge_row(&existing, key, &row));
    }
}

/// Several close/persist paths can meet during teardown. Only the newest row
/// matters, so one read-merge-write consumes everything already queued.
fn latest_queued(mut row: String, receiver: &mpsc::Receiver<String>) -> String {
    while let Ok(newer) = receiver.try_recv() {
        row = newer;
    }
    row
}

fn merge_row(existing: &str, key: &str, row: &str) -> String {
    let mut kept: Vec<(String, String)> = rows(existing)
        .filter(|(existing_key, _)| *existing_key != key)
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    kept.push((key.to_owned(), row.to_owned()));

    let mut out = String::from(HEADER);
    for (key, value) in kept {
        out.push_str(&key);
        out.push_str(" = ");
        out.push_str(&value);
        out.push('\n');
    }
    out
}

fn row_in<'a>(text: &'a str, wanted: &str) -> Option<&'a str> {
    rows(text)
        .find(|(key, _)| *key == wanted)
        .map(|(_, value)| value)
}

fn rows(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .filter(|(key, _)| !key.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_files_under_application_support() {
        assert_eq!(
            macos_support_base(PathBuf::from("/Users/ada")),
            PathBuf::from("/Users/ada/Library/Application Support")
        );
    }

    #[test]
    fn explicit_roots_resolve_independently_beneath_their_agent() {
        let parent = tempfile::tempdir().unwrap();
        let a = parent.path().join("a");
        let b = parent.path().join("b");
        assert_eq!(
            window_geometry_path(Some(&a)),
            Some(a.join("agent/windows.conf"))
        );
        assert_eq!(
            window_geometry_path(Some(&b)),
            Some(b.join("agent/windows.conf"))
        );
        assert_ne!(
            window_geometry_path(Some(&a)),
            window_geometry_path(Some(&b))
        );
        assert_eq!(
            window_geometry_path(None),
            local_state_dir().map(|path| path.join("windows.conf"))
        );
        assert!(!a.exists(), "path resolution performs no I/O");
        assert!(!b.exists(), "path resolution performs no I/O");
    }

    #[test]
    fn replacing_one_row_preserves_every_other_valid_owner() {
        let existing = "# old header\nprofile | default = x=1;y=2\nglobal-search = x=3;y=4;h=500\n";
        let body = merge_row(existing, "profile | default", "x=8;y=9");
        assert_eq!(row_in(&body, "profile | default"), Some("x=8;y=9"));
        assert_eq!(row_in(&body, "global-search"), Some("x=3;y=4;h=500"));
        assert_eq!(body.matches("profile | default =").count(), 1);
    }

    #[test]
    fn corrupt_rows_are_ignored_without_hiding_valid_neighbours() {
        let existing = "missing delimiter\n = empty-key\nterminal = x=4;y=5\n";
        assert_eq!(row_in(existing, "terminal"), Some("x=4;y=5"));
        let body = merge_row(existing, "global-search", "h=600");
        assert!(!body.contains("missing delimiter"));
        assert!(!body.contains("empty-key"));
        assert_eq!(row_in(&body, "terminal"), Some("x=4;y=5"));
    }

    #[test]
    fn the_first_submission_writes_and_idle_submissions_do_not() {
        let mut writer = WindowRowWriter::default();
        assert!(writer.would_write("x=1"));
        assert!(writer.write_if_changed("x=1"));
        assert!(!writer.would_write("x=1"));
        assert!(!writer.write_if_changed("x=1"));
        assert!(writer.write_if_changed("x=2"));
    }

    #[test]
    fn a_queued_burst_coalesces_to_its_newest_row() {
        let (sender, receiver) = mpsc::channel();
        sender.send("second".to_owned()).unwrap();
        sender.send("third".to_owned()).unwrap();
        assert_eq!(latest_queued("first".to_owned(), &receiver), "third");
    }

    #[test]
    fn two_root_files_retain_only_their_own_rows() {
        let parent = tempfile::tempdir().unwrap();
        let a = window_geometry_path(Some(&parent.path().join("a"))).unwrap();
        let b = window_geometry_path(Some(&parent.path().join("b"))).unwrap();
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        fs::write(&a, merge_row("", "global-search", "x=1")).unwrap();
        fs::write(&b, merge_row("", "global-search", "x=2")).unwrap();
        assert_eq!(read_window_row(&a, "global-search").as_deref(), Some("x=1"));
        assert_eq!(read_window_row(&b, "global-search").as_deref(), Some("x=2"));
    }

    #[test]
    fn the_worker_writes_without_discarding_another_owner() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("windows.conf");
        fs::write(&path, "profile | default = x=1;y=2\n").unwrap();
        let mut writer = WindowRowWriter::open(
            Some(path.clone()),
            "global-search",
            "window-row-writer-test",
        );
        assert!(writer.write_if_changed("x=3;y=4;h=600"));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while read_window_row(&path, "global-search").as_deref() != Some("x=3;y=4;h=600") {
            assert!(
                std::time::Instant::now() < deadline,
                "the worker did not write its queued row"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            read_window_row(&path, "profile | default").as_deref(),
            Some("x=1;y=2")
        );
    }
}
