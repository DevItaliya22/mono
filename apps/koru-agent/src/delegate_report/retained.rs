// SPDX-License-Identifier: MPL-2.0

//! How much disk one run's retained results are holding.
//!
//! **D37**: an unregistered file under `<run-workspace>/.artifacts/<RunId>/`
//! survives as long as its run, including inside a temporary workspace, so
//! cleanup binds to run deletion rather than to the end of an attempt. That
//! is the right rule — a result nobody registered is still the person's
//! work — and its price is a folder that only ever grows. A temporary
//! workspace that is no longer temporary is exactly the kind of quiet
//! growth nobody finds until it matters, so the size of it is a number the
//! diagnostic reports.
//!
//! The walk is deliberately small and deliberately incurious:
//!
//! - **Iterative, never recursive.** A run's own folder could be nested
//!   arbitrarily deep by the agent that wrote it, and a diagnostic must not
//!   be the thing that overflows a stack.
//! - **Symbolic links are counted and never followed.** Following one
//!   would leave the folder being measured — double-counting a tree, or
//!   walking a cycle forever — and the question is how much disk *this*
//!   folder holds.
//! - **Bounded.** [`ENTRY_BOUND`] entries, then it stops and says so. An
//!   answer that took a minute to produce is not the answer somebody in an
//!   incident wanted.
//! - **Content-free.** It returns two numbers. No filename, no extension,
//!   no path — not even the folder it walked, which the caller already
//!   knows and never prints (§13.7).

use std::path::{Path, PathBuf};

/// The most directory entries one run's walk visits. A folder past this is
/// reported as at-least, which is all a person needs to know that it is
/// the one to look at.
pub const ENTRY_BOUND: u64 = 100_000;

/// What one run's `.artifacts/<RunId>/` folder holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Retained {
    /// The folder exists. `false` is the ordinary case for a run that
    /// registered everything it produced, or never wrote anything.
    pub present: bool,
    pub files: u64,
    pub bytes: u64,
    /// The walk stopped at [`ENTRY_BOUND`]; the figures are lower bounds.
    pub truncated: bool,
    /// Entries that could not be read — a permission, a race with a
    /// deletion. Counted rather than reported one by one, and never a
    /// reason to fail the whole report.
    pub unreadable: u64,
}

impl Retained {
    /// Add another run's figures. Used for the total line.
    pub fn add(&mut self, other: Retained) {
        self.files += other.files;
        self.bytes += other.bytes;
        self.truncated |= other.truncated;
        self.unreadable += other.unreadable;
        self.present |= other.present;
    }
}

/// Measure `folder`, following nothing out of it.
pub fn measure(folder: &Path) -> Retained {
    let mut out = Retained::default();
    let Ok(metadata) = std::fs::symlink_metadata(folder) else {
        return out;
    };
    if !metadata.is_dir() {
        return out;
    }
    out.present = true;
    let mut visited = 0u64;
    let mut stack: Vec<PathBuf> = vec![folder.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            out.unreadable += 1;
            continue;
        };
        for entry in entries {
            if visited >= ENTRY_BOUND {
                out.truncated = true;
                return out;
            }
            visited += 1;
            let Ok(entry) = entry else {
                out.unreadable += 1;
                continue;
            };
            // `symlink_metadata` rather than `metadata`, so a link is
            // measured as the link it is and never as whatever it points
            // at — and a link to a directory is never descended into.
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                out.unreadable += 1;
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                out.files += 1;
                out.bytes += metadata.len();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
