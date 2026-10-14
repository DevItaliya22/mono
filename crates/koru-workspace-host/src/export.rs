// SPDX-License-Identifier: MPL-2.0

//! Writing a Store workspace bundle to a file somebody can keep elsewhere.
//!
//! The Store owns the bundle bytes: manifest, every document snapshot and
//! every blob. The shell owns only the destination picker and the last durable
//! filesystem step. Bytes go through a sibling, process-named `.part`; only a
//! complete, flushed bundle replaces the chosen path.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// A filename for the export, from the workspace's name and the moment.
pub fn suggested_name(workspace: &str, stamp: &str) -> String {
    let safe: String = workspace
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim().trim_matches('-').trim();
    let safe = if safe.chars().any(|c| c.is_alphanumeric()) {
        safe
    } else {
        "workspace"
    };
    format!("{safe} {stamp}.koru")
}

/// Durably write `bytes`, then atomically replace `to` with the completed file.
pub fn write_bundle(to: &Path, bytes: &[u8]) -> io::Result<()> {
    let part = part_path(to);
    let result = (|| {
        let mut file = fs::File::create(&part)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace(&part, to)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

fn part_path(to: &Path) -> PathBuf {
    let name = to
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace.koru");
    to.with_file_name(format!("{name}.{}.part", std::process::id()))
}

#[cfg(not(windows))]
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(io::Error::other)
}

#[cfg(test)]
mod tests;
