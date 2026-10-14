// SPDX-License-Identifier: MPL-2.0

//! Windows: NTFS compares names case-insensitively, and a canonical path
//! wears a verbatim prefix.
//!
//! **Case.** NTFS folds through the volume's `$UpCase` table — one code
//! unit to one code unit. `str::to_uppercase` is the *full* mapping (`ß`
//! becomes `SS`), which would make `straße` and `strasse` one folder when
//! NTFS keeps them apart. So each scalar folds to its uppercase only when
//! that mapping is a single scalar, and stays itself otherwise — the simple
//! mapping, which is what the table approximates. Where the two still
//! disagree the error runs toward a false *busy* (a warning a person can
//! wave past) and never toward a missed collision, which is the failure this
//! crate exists to prevent. A name that is not Unicode is kept as its raw
//! units.
//!
//! **Prefix.** `canonicalize` answers `\\?\C:\…` or `\\?\UNC\server\share\…`;
//! a caller may hand us `C:\…` or `\\server\share\…`. `Path::components`
//! parses both spellings into a typed `Prefix`, and the segment built here
//! makes the verbatim and ordinary forms of one volume the same segment.

use std::{
    ffi::{OsStr, OsString},
    path::Prefix,
};

pub(crate) fn fold_name(name: &OsStr) -> OsString {
    super::simple_uppercase(name)
}

/// The segment a path's volume prefix becomes. Verbatim and ordinary
/// spellings of one volume produce one segment; the drive letter and a UNC
/// server and share fold like names.
pub(crate) fn prefix_segment(prefix: Prefix<'_>) -> OsString {
    match prefix {
        Prefix::VerbatimDisk(letter) | Prefix::Disk(letter) => {
            OsString::from(format!("{}:", (letter as char).to_ascii_uppercase()))
        }
        Prefix::VerbatimUNC(server, share) | Prefix::UNC(server, share) => {
            let mut segment = OsString::from(r"\\");
            segment.push(fold_name(server));
            segment.push(r"\");
            segment.push(fold_name(share));
            segment
        }
        Prefix::Verbatim(name) => {
            let mut segment = OsString::from(r"\\?\");
            segment.push(fold_name(name));
            segment
        }
        Prefix::DeviceNS(name) => {
            let mut segment = OsString::from(r"\\.\");
            segment.push(fold_name(name));
            segment
        }
    }
}
