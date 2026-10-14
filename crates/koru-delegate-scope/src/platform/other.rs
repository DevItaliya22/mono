// SPDX-License-Identifier: MPL-2.0

//! Every platform that is not Windows: there is no volume prefix, and names
//! compare byte for byte — **except on macOS, which folds case.**
//!
//! A default APFS volume compares names case-insensitively, so `~/Work/Repo`
//! and `~/work/repo` are one folder there. `canonicalize` already answers with
//! the on-disk spelling for every component that exists (Darwin's `realpath`
//! reads the stored name), so most identities agree before any fold. Two do
//! not: the child segments `ResolvedScope::planned_under` joins without
//! resolving, and an identity recorded before a case-only rename of its
//! folder. Folding closes both. On a volume a person formatted
//! case-sensitive, two folders differing only in case become one identity —
//! a false *busy* a person can wave past, which is the error this crate
//! prefers to a missed collision. Linux keeps the exact comparison its
//! filesystems make.

use std::{
    ffi::{OsStr, OsString},
    path::Prefix,
};

pub(crate) fn fold_name(name: &OsStr) -> OsString {
    #[cfg(target_os = "macos")]
    return super::simple_uppercase(name);
    #[cfg(not(target_os = "macos"))]
    name.to_owned()
}

pub(crate) fn prefix_segment(prefix: Prefix<'_>) -> OsString {
    // `Path::components` never yields a `Prefix` off Windows; the arm exists
    // so the seam has one shape on every platform. `Prefix` is the typed
    // enum, not the component, so it is spelled out rather than borrowed.
    match prefix {
        Prefix::Verbatim(name) | Prefix::DeviceNS(name) => name.to_owned(),
        Prefix::VerbatimUNC(server, share) | Prefix::UNC(server, share) => {
            let mut segment = server.to_owned();
            segment.push("/");
            segment.push(share);
            segment
        }
        Prefix::VerbatimDisk(letter) | Prefix::Disk(letter) => {
            OsString::from(format!("{}:", char::from(letter)))
        }
    }
}
