// SPDX-License-Identifier: MPL-2.0

//! **What emptying the trash may remove of where a run worked — and what it
//! must not.**
//!
//! A deleted run's working folder is the one place in delegation where
//! getting a rule wrong destroys work that was never Koru's. A
//! [`WorkspaceScope`] has two kinds of folder behind it and they look
//! identical by the time they are a path:
//!
//! - `Temporary` is a directory **Koru made for that run** and owns for as
//!   long as the run lives (D37). Deleting the run is what ends that, so
//!   emptying the trash removes it.
//! - `ProjectRoot` and `ProjectFolder` are a person's **own project**. A run
//!   scoped to a project root ran *inside their repository*. Emptying the
//!   trash leaves it exactly alone: it was theirs before the run and it is
//!   theirs after.
//!
//! So the decision is [`WorkspaceScope::is_koru_owned`] and nothing else,
//! and it is asked **before** the path is looked at rather than after — a
//! rule that reads the filesystem first is a rule with a window in it.
//!
//! [`Removal::Kept`] is not silence. The trash says what it did, because an
//! operation that quietly does less than it offered teaches a person that
//! it did more.

use std::io;
use std::path::Path;

use koru_delegate_core::WorkspaceScope;

/// What became of a deleted run's working folder.
///
/// Four answers and no `bool`: *we left it alone deliberately*, *it was
/// already gone*, *it is gone now* and *the filesystem refused* are four
/// different things to tell somebody, and three of them are not failures.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Removal {
    /// Removed. Koru made it, and it is gone.
    Removed,
    /// **Left alone, because it is the person's own project folder.** The
    /// run is forgotten; the directory is untouched.
    Kept,
    /// Koru made it, and it was not there to remove — an earlier delete, a
    /// tidied temporary directory, a machine that lost it. Reported rather
    /// than retried: there is nothing left to do.
    Absent,
    /// Koru made it and the filesystem would not remove it — a file open in
    /// another process is the ordinary cause on Windows. Reported rather
    /// than retried or ignored, so the run can be deleted again later.
    /// `std`'s closed vocabulary, never a path and never a message.
    Refused(io::ErrorKind),
}

impl Removal {
    /// A stable token, for a wire, a report and a sentence's key. Never a
    /// path and never an error string.
    pub fn code(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Kept => "kept",
            Self::Absent => "absent",
            Self::Refused(_) => "refused",
        }
    }

    /// Whether anything is still owed. Only a refusal is: the folder is
    /// still there and nobody has been able to remove it.
    pub fn is_refusal(self) -> bool {
        matches!(self, Self::Refused(_))
    }
}

/// **Remove a deleted run's working folder, if Koru made it.**
///
/// `folder` is the path the run actually worked in, as this device resolved
/// it when the run started — the run authority record's, never a path
/// re-derived from a scope now. A base a person has since changed, a
/// project that has been moved, a temporary root that was repointed: all of
/// them make a fresh resolution name a *different* directory, and the one
/// directory this run is entitled to remove is the one it wrote in.
///
/// `None` means this device never resolved one, which is [`Removal::Absent`]
/// for a scope Koru owns and [`Removal::Kept`] for one it does not.
///
/// The parent is pruned only when it is **empty**, and only one level:
/// `<base>/koru-runs/<run>/workspace` leaves `<run>/` behind otherwise, and
/// a run's own directory with nothing in it is litter. `remove_dir` fails on
/// a directory that is not empty, which is the guard rather than a check
/// that could be raced.
pub fn remove_working_folder(scope: &WorkspaceScope, folder: Option<&Path>) -> Removal {
    // The decision first, before anything reads a disk.
    if !scope.is_koru_owned() {
        return Removal::Kept;
    }
    let Some(folder) = folder else {
        return Removal::Absent;
    };
    match std::fs::remove_dir_all(folder) {
        Ok(()) => {
            if let Some(parent) = folder.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            Removal::Removed
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Removal::Absent,
        Err(error) => Removal::Refused(error.kind()),
    }
}
