// SPDX-License-Identifier: MPL-2.0

//! The register path rule: which files a run may register, and the bytes
//! read from one (ROADMAP §7.3).
//!
//! `Register` accepts only canonicalized paths inside
//! `<run-workspace>/.artifacts/<RunId>/`, rejecting traversal, symlinks,
//! junctions, and a file changed between listing and registration. Each of
//! those is its own [`StageError`] with its own stable code, and none of
//! the messages names the path: a path is content (standing requirement 7).
//!
//! The order is deliberate. The lexical check (`..`) comes first because it
//! needs no filesystem; the reparse-point walk comes before
//! canonicalization because canonicalization *resolves* a link — a
//! symlinked file inside the folder canonicalizes to wherever it points,
//! and if that is inside the folder too the containment check alone would
//! pass it. Then containment, then the file's shape, then the read with
//! its two stats around it.
//!
//! **The Windows branch is the reparse tag.** `std` reports a junction as
//! a directory symlink, and the roadmap wants the two refused with distinct
//! codes, so on Windows the tag is read from the open handle:
//! `IO_REPARSE_TAG_MOUNT_POINT` is a junction, `IO_REPARSE_TAG_SYMLINK` a
//! symlink, and any other tag is refused as a symlink too rather than
//! followed. On the other platforms a symlink is the only reparse there is.

use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use koru_artifact_core::{BlobHash, Filename, RunAuthorityRecord};

/// Why a path was refused. Every variant is a stable code; none of the
/// messages names the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageError {
    /// `<run-workspace>/.artifacts/<RunId>/` does not exist — the agent
    /// registered before it wrote anything.
    RunFolderMissing,
    /// A `..` component.
    Traversal,
    /// A symbolic link on the way to the file, or the file itself.
    Symlink,
    /// A junction (Windows) on the way to the file.
    Junction,
    /// The canonical path is not under the run's folder.
    Outside,
    FileNotFound,
    /// A directory, a device — not a regular file.
    NotAFile,
    /// The file's size or modification time changed between the listing
    /// and the read, or the read returned a different length.
    Changed,
    /// The file could not be read.
    Unreadable,
}

impl StageError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::RunFolderMissing => "run_folder_missing",
            Self::Traversal => "path_traversal",
            Self::Symlink => "path_symlink",
            Self::Junction => "path_junction",
            Self::Outside => "path_outside_run_folder",
            Self::FileNotFound => "file_not_found",
            Self::NotAFile => "not_a_file",
            Self::Changed => "file_changed",
            Self::Unreadable => "file_unreadable",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::RunFolderMissing => {
                "The run's .artifacts/<RunId>/ folder does not exist; write the result there first."
            }
            Self::Traversal => "The path contains a parent-directory component.",
            Self::Symlink => {
                "The path goes through a symbolic link, which a registration never follows."
            }
            Self::Junction => {
                "The path goes through a junction, which a registration never follows."
            }
            Self::Outside => "The path resolves outside the run's .artifacts/<RunId>/ folder.",
            Self::FileNotFound => "No file exists at that path under the run's folder.",
            Self::NotAFile => "The path is not a regular file.",
            Self::Changed => {
                "The file changed while it was being registered; write it completely, then register it."
            }
            Self::Unreadable => "The file could not be read.",
        }
    }
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for StageError {}

/// A file read under the rule: its canonical path inside the folder with
/// `/` separators (the registration's identity), the name it had, and its
/// bytes with their hash. `Debug` names the hash and the length only.
#[derive(Clone, PartialEq, Eq)]
pub struct Staged {
    pub relative_path: String,
    pub filename: Option<Filename>,
    pub bytes: Vec<u8>,
    pub hash: BlobHash,
}

impl fmt::Debug for Staged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Staged")
            .field("hash", &self.hash)
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// What kind of reparse point a path component is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reparse {
    Symlink,
    /// NTFS only: the unix arm below reports every link as a symlink.
    #[cfg_attr(not(windows), allow(dead_code))]
    Junction,
}

#[cfg(windows)]
fn reparse_of(path: &Path) -> std::io::Result<Option<Reparse>> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FileAttributeTagInfo, GetFileInformationByHandleEx,
    };

    // The tags themselves, spelled here rather than through a further
    // `windows-sys` feature: both are fixed by the platform.
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_symlink() {
        return Ok(None);
    }
    // Open the reparse point itself — not what it points at — and ask for
    // its tag. `BACKUP_SEMANTICS` is what lets a directory be opened.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut info = FILE_ATTRIBUTE_TAG_INFO {
        FileAttributes: 0,
        ReparseTag: 0,
    };
    // SAFETY: the handle is open for the duration of the call, the buffer
    // is exactly the structure the information class fills, and the size
    // passed is that structure's size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileAttributeTagInfo,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        return Ok(None);
    }
    Ok(Some(match info.ReparseTag {
        IO_REPARSE_TAG_MOUNT_POINT => Reparse::Junction,
        IO_REPARSE_TAG_SYMLINK => Reparse::Symlink,
        // Any other reparse point — a deduplicated file, a cloud
        // placeholder, an AppExecLink — is not followed either.
        _ => Reparse::Symlink,
    }))
}

#[cfg(not(windows))]
fn reparse_of(path: &Path) -> std::io::Result<Option<Reparse>> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata
        .file_type()
        .is_symlink()
        .then_some(Reparse::Symlink))
}

fn refuse_reparse(path: &Path) -> Result<(), StageError> {
    match reparse_of(path) {
        Ok(None) => Ok(()),
        Ok(Some(Reparse::Symlink)) => Err(StageError::Symlink),
        Ok(Some(Reparse::Junction)) => Err(StageError::Junction),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(StageError::FileNotFound),
        Err(_) => Err(StageError::Unreadable),
    }
}

/// The run's folder, canonical, with neither `.artifacts` nor `<RunId>`
/// being a link — otherwise a folder pointed elsewhere would make every
/// file under it "inside".
fn run_folder(authority: &RunAuthorityRecord) -> Result<PathBuf, StageError> {
    let workspace = authority
        .run_workspace
        .as_path()
        .canonicalize()
        .map_err(|_| StageError::RunFolderMissing)?;
    let artifacts = workspace.join(koru_artifact_core::ARTIFACTS_DIR);
    let folder = artifacts.join(authority.run.to_storage());
    for step in [&artifacts, &folder] {
        match refuse_reparse(step) {
            Ok(()) => {}
            Err(StageError::FileNotFound) => return Err(StageError::RunFolderMissing),
            Err(other) => return Err(other),
        }
    }
    if !folder.is_dir() {
        return Err(StageError::RunFolderMissing);
    }
    folder
        .canonicalize()
        .map_err(|_| StageError::RunFolderMissing)
}

fn fingerprint(metadata: &fs::Metadata) -> (u64, Option<SystemTime>) {
    (metadata.len(), metadata.modified().ok())
}

/// Resolve `requested` under the run's folder and read it under the rule.
///
/// `between` runs after the file's size and modification time are taken
/// and before its bytes are read — the seam the "changed between listing
/// and registration" test uses; production passes `|| {}`.
pub fn stage(
    authority: &RunAuthorityRecord,
    requested: &str,
    between: impl FnOnce(),
) -> Result<Staged, StageError> {
    let folder = run_folder(authority)?;
    let requested = Path::new(requested);
    if requested
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(StageError::Traversal);
    }
    // A relative path is under the folder; an absolute one replaces it
    // when joined and must then prove containment like any other.
    let candidate = folder.join(requested);
    // Every component from the folder down: a link anywhere on the way is
    // refused before canonicalization can resolve it.
    let mut walked = folder.clone();
    for component in candidate
        .strip_prefix(&folder)
        .map(Path::components)
        .into_iter()
        .flatten()
    {
        walked.push(component);
        refuse_reparse(&walked)?;
    }
    if !candidate.starts_with(&folder) {
        // Absolute and elsewhere: its own components were never walked,
        // and it may not be a link either.
        refuse_reparse(&candidate)?;
    }
    let canonical = candidate.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StageError::FileNotFound
        } else {
            StageError::Unreadable
        }
    })?;
    let relative = canonical
        .strip_prefix(&folder)
        .map_err(|_| StageError::Outside)?;
    if relative.as_os_str().is_empty() {
        return Err(StageError::NotAFile);
    }
    let before = fs::metadata(&canonical).map_err(|_| StageError::Unreadable)?;
    if !before.is_file() {
        return Err(StageError::NotAFile);
    }
    let listed = fingerprint(&before);
    between();
    let bytes = fs::read(&canonical).map_err(|_| StageError::Unreadable)?;
    let after = fs::metadata(&canonical).map_err(|_| StageError::Unreadable)?;
    if fingerprint(&after) != listed || bytes.len() as u64 != listed.0 {
        return Err(StageError::Changed);
    }
    let relative_path = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    let filename = canonical
        .file_name()
        .and_then(|name| Filename::new(name.to_string_lossy()).ok());
    let hash = BlobHash::from_digest("b3", &blake3::hash(&bytes).to_hex())
        .expect("a BLAKE3 hex digest is a well-formed blob hash");
    Ok(Staged {
        relative_path,
        filename,
        bytes,
        hash,
    })
}
