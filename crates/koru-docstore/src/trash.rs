// SPDX-License-Identifier: MPL-2.0

//! Deleting a workspace, and what that is allowed to mean.
//!
//! **Nothing here unlinks a workspace's content.** Losing content is the one
//! unrecoverable bug, and a delete is the one gesture whose whole purpose is to
//! destroy something — so it is implemented as a *move*, into
//! `<profile_dir>/trash/<id>-<stamp>/`, and the bytes stay there until somebody
//! empties it by hand or the retention below reaches them.
//!
//! The stamp is what makes the folder collision-free: delete a workspace,
//! restore it by hand, delete it again, and the two live side by side rather
//! than the second silently replacing the first.
//!
//! ## Two things the trash must not become
//!
//! - **Part of a backup.** `--backup` copies the whole profile directory, so a
//!   trash holding two deleted workspaces would double the size of every backup
//!   taken afterwards, silently. [`backup::copy_tree`] skips it by name.
//! - **Unbounded.** A directory of full workspaces that nothing ever prunes is a
//!   disk leak with a friendly name. [`prune_trash`] runs on profile open.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use koru_core::WorkspaceId;

use crate::backup;

/// How long a deleted workspace is kept before it is really removed.
///
/// Long enough that "I deleted the wrong one" is recoverable across a holiday,
/// short enough that the folder does not grow forever.
pub const KEEP_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The directory name, and what `copy_tree` skips.
pub const TRASH_DIR: &str = "trash";

/// Where deleted workspaces go.
pub fn trash_dir(profile_dir: &Path) -> PathBuf {
    profile_dir.join(TRASH_DIR)
}

/// Move a workspace's folder into the trash, and say where it went.
///
/// `fs::rename` first, because it is atomic and instant. It fails across
/// volumes — a profile on a network drive with a temp directory elsewhere — and
/// there the fallback is copy-then-remove. **If the copy fails, nothing is
/// removed and the error is returned**: a half-copied workspace that has also
/// been deleted is the exact outcome this module exists to prevent.
pub fn move_to_trash(profile_dir: &Path, id: WorkspaceId) -> io::Result<PathBuf> {
    let from = profile_dir.join("workspaces").join(id.to_storage());
    if !from.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "that workspace's folder is not there",
        ));
    }
    let to = trash_dir(profile_dir).join(destination_name(id, &backup::stamp_now()));
    fs::create_dir_all(trash_dir(profile_dir))?;
    if fs::rename(&from, &to).is_ok() {
        return Ok(to);
    }
    backup::copy_tree(&from, &to)?;
    // Only now, with a complete copy on disk, is the original expendable. A
    // failure here leaves the workspace in both places, which is untidy and
    // harmless — the opposite ordering leaves it in neither.
    fs::remove_dir_all(&from)?;
    Ok(to)
}

/// `<id>-<stamp>`, so a delete/restore/delete cycle does not collide.
///
/// Split out from [`move_to_trash`] because it is the part worth testing: the
/// `fs` calls around it are the ones the suite must not make.
pub fn destination_name(id: WorkspaceId, stamp: &str) -> String {
    format!("{}-{stamp}", id.to_storage())
}

/// Really remove anything in the trash older than [`KEEP_FOR`].
///
/// Best effort and silent: this runs on open, and a trash entry that will not
/// delete is not a reason to fail a launch. `now` is passed in rather than read
/// so the decision is testable without waiting a month.
pub fn prune_trash(profile_dir: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(trash_dir(profile_dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > KEEP_FOR);
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// What a workspace costs on disk, split by what the two halves mean.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Sizes {
    /// The document, its blobs, and the peer logs not yet folded into it —
    /// everything you would lose.
    pub live: u64,
    /// The timestamped snapshots under `backups/`, which are copies of the
    /// above as earlier sessions left it.
    pub backups: u64,
}

impl Sizes {
    pub fn total(self) -> u64 {
        self.live + self.backups
    }
}

/// Measure a workspace directory.
///
/// **Split, rather than one number.** `backups/` keeps up to
/// [`BACKUPS_KEPT`](backup::BACKUPS_KEPT) snapshots, so a 1.4 MB workspace
/// reports about 45 MB if you simply walk the tree — and a user reading "45 MB"
/// beside a workspace holding a few notes reads it as a bug, because from where
/// they sit it is one.
///
/// Never fails: an unreadable directory contributes nothing rather than making
/// the whole listing an error.
pub fn size_of(ws_dir: &Path) -> Sizes {
    Sizes {
        live: size_tree(&ws_dir.join("snapshot.loro"))
            + size_tree(&ws_dir.join("updates"))
            + size_tree(&ws_dir.join("blobs")),
        backups: size_tree(&ws_dir.join("backups")),
    }
}

/// Bytes under a path, whether it is one file or a tree. Zero for anything that
/// will not read.
fn size_tree(path: &Path) -> u64 {
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().map(|e| size_tree(&e.path())).sum()
}

#[cfg(test)]
mod tests;
