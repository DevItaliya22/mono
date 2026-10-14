// SPDX-License-Identifier: MPL-2.0

//! The platform seam: what a filesystem considers one name.
//!
//! Identity has two platform-shaped steps — resolving a path to the folder
//! it names (`canonicalize`, which follows junctions, symlinks and 8.3 short
//! names) and folding a name's case the way the filesystem does when it
//! compares two. Everything above this module works on the folded segments
//! and never asks which platform it is on.

use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{fold_name, prefix_segment};

#[cfg(not(windows))]
mod other;
#[cfg(not(windows))]
pub(crate) use other::{fold_name, prefix_segment};

/// The one directory a path names, with every alias resolved. `std`'s
/// `canonicalize` on every platform: on Windows it answers through the open
/// handle (`GetFinalPathNameByHandle`), so a junction, a symlink and a short
/// name all come back as the target's real spelling under a verbatim prefix.
pub(crate) fn resolve(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Fold one name the way this platform's filesystem compares names.
pub(crate) fn fold(name: &OsStr) -> OsString {
    fold_name(name)
}

/// Each scalar to its uppercase **only when that mapping is a single
/// scalar** — the simple mapping, which is what NTFS's `$UpCase` table and
/// APFS's case-insensitive comparison approximate. `str::to_uppercase` is the
/// full mapping (`ß` becomes `SS`) and would make `straße` and `strasse` one
/// folder where both filesystems keep them apart. Where the approximation and
/// a filesystem still disagree, the error runs toward a false *busy* and never
/// toward a missed collision. A name that is not Unicode keeps its raw units.
#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn simple_uppercase(name: &OsStr) -> OsString {
    fn fold_char(c: char) -> char {
        let mut upper = c.to_uppercase();
        match (upper.next(), upper.next()) {
            (Some(single), None) => single,
            _ => c,
        }
    }
    match name.to_str() {
        Some(text) => text.chars().map(fold_char).collect::<String>().into(),
        None => name.to_owned(),
    }
}
