// SPDX-License-Identifier: MPL-2.0

//! The download's staging file, and the quarantine beside it.
//!
//! One file per hash, named `<hash.file_name()>.download.part`, under
//! `<root>/blobs/staging`. Named by the *hash* rather than by the transfer so
//! that two Workspaces wanting the same picture share one job and one file,
//! and so that a download that got 300 MB from one path resumes at 300 MB on
//! another (plan §8, §10).
//!
//! **`hash.file_name()` is `b3-<digest>`, never `b3:<digest>`.** The colon is
//! the NTFS alternate-data-stream delimiter and is illegal in a Windows
//! filename; an earlier revision of the plan regressed to the raw spelling in
//! three places regardless (D248). The address spelling belongs in a URL path
//! segment and the filename spelling belongs on a disk, and the two never
//! cross.
//!
//! Three properties everything below exists for:
//!
//! - **The synced length is the truth.** A chunk is appended, flushed and
//!   synced before the next one is requested, so the file's length on disk is
//!   the contiguous durable prefix and a resume needs no record anywhere else.
//! - **A pending file is never renderable.** It is not at the object address
//!   and it carries a suffix nothing else reads.
//! - **An unrecognised pending file is quarantined, not interpreted.** A name
//!   in this module's own namespace (it ends in [`DOWNLOAD_SUFFIX`]) that does
//!   not parse is not a blob it may guess about.
//!
//! **The directory is shared.** The Store's own writes stage here too
//! (`koru-store-sqlite`'s `.pending.*` and `.promote.*` files), and so does
//! every Workspace on the Store root, because a part file is named by hash.
//! So a sweep leaves a name it did not write alone — the Store's sweep holds
//! the same rule the other way round — and it never decides from one
//! Workspace's references that a shared download is unwanted.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use koru_store_core::{errors::Code, model::BlobHash};

use crate::ports::TransferError;

/// The suffix that makes a partial download unmistakable and unopenable by
/// anything that reads objects.
pub const DOWNLOAD_SUFFIX: &str = ".download.part";

/// The longest a staging file name can be: `b3-` plus a BLAKE3 hex digest
/// plus the suffix. Anything longer is not one of ours, which is the bound
/// that lets a sweep refuse to interpret a name rather than parse it.
pub const MAX_STAGING_NAME: usize = 3 + 64 + DOWNLOAD_SUFFIX.len();

/// How long an unreferenced partial download must sit untouched before a
/// sweep removes it. A live download rewrites its file every chunk, and the
/// sweep sees only one Workspace's references while the file may be another
/// Workspace's — so "nothing I know of wants this" is not enough, and
/// "nobody has written it for a day" is. The Store's own staging sweep uses
/// the same day for the same reason. Being wrong costs a re-download.
pub const ABANDONED_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// `<root>/blobs/staging` and `<root>/blobs/quarantine`.
#[derive(Clone, Debug)]
pub struct StagingArea {
    staging: PathBuf,
    quarantine: PathBuf,
}

impl StagingArea {
    /// `root` is the Store's `<root>/blobs/staging`, as the port reports it;
    /// quarantine is its sibling, because a quarantined file must not be
    /// swept by the thing that sweeps staging.
    pub fn new(staging: PathBuf) -> Self {
        let quarantine = staging
            .parent()
            .map(|parent| parent.join("quarantine"))
            .unwrap_or_else(|| staging.join("quarantine"));
        Self {
            staging,
            quarantine,
        }
    }

    pub fn staging_dir(&self) -> &Path {
        &self.staging
    }

    pub fn quarantine_dir(&self) -> &Path {
        &self.quarantine
    }

    pub fn part_path(&self, hash: &BlobHash) -> PathBuf {
        self.staging.join(part_name(hash))
    }

    pub fn ensure(&self) -> Result<(), TransferError> {
        fs::create_dir_all(&self.staging).map_err(local)?;
        fs::create_dir_all(&self.quarantine).map_err(local)
    }

    /// The contiguous durable prefix: the file's length on disk, or zero when
    /// there is no file. **Never a remembered number** — a length a process
    /// held in memory is a length a power cut disagrees with.
    pub fn synced_length(&self, hash: &BlobHash) -> Result<u64, TransferError> {
        match fs::metadata(self.part_path(hash)) {
            Ok(metadata) => Ok(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(local(error)),
        }
    }

    /// Append one bounded chunk at `offset`, flush it and sync it **before
    /// returning**. The length this answers with is on the disk.
    ///
    /// `offset` must be the current length; anything else is a caller that
    /// lost track, and writing it would leave a hole the hash would later
    /// blame on the network.
    pub fn append(&self, hash: &BlobHash, offset: u64, bytes: &[u8]) -> Result<u64, TransferError> {
        self.ensure()?;
        let path = self.part_path(hash);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(local)?;
        let length = file.metadata().map_err(local)?.len();
        if length != offset {
            // A caller that lost track of the length would otherwise leave a
            // hole the final hash would blame on the network.
            return Err(TransferError::Local(Code::Internal));
        }
        file.write_all(bytes).map_err(local)?;
        file.flush().map_err(local)?;
        // The one place in this crate that syncs, and it is the place where
        // durability *is* the meaning of the operation: the next request is
        // made on the strength of this length.
        file.sync_all().map_err(local)?;
        Ok(length + bytes.len() as u64)
    }

    /// Whether this volume can hold the rest of a download **and** the copy
    /// that publishing it makes, without being driven to zero.
    ///
    /// Asked once, before the first range, because "this device cannot fit
    /// that picture" is a fact a person can act on and a hundred `ENOSPC`s
    /// spread across an afternoon is not. It is deliberately **not** asked
    /// per chunk: that would put a `statvfs` on the transfer path for a
    /// question whose answer moved by two megabytes.
    ///
    /// **Available means what the system would give this write**
    /// ([`koru_free_space::available`]): on a Mac or an iPhone that counts the
    /// purgeable space `statvfs` leaves out, so a nearly full device is not
    /// told it cannot fit a picture it would have made room for.
    ///
    /// **The bound is derived rather than guessed.** `install_downloaded`
    /// streams the finished staging file into the object store, so at the
    /// moment of publication both copies exist: `remaining` still to write
    /// here, plus `total` for the object. The slack is one
    /// [`koru_cloud_blob::CHUNK`], so a transfer does not fail on its last
    /// append because something else on the device wrote a byte while it ran.
    pub fn will_fit(&self, remaining: u64, total: u64) -> Result<bool, TransferError> {
        self.ensure()?;
        let available = koru_free_space::available(&self.staging).map_err(local)?;
        let needed = remaining
            .saturating_add(total)
            .saturating_add(koru_cloud_blob::CHUNK);
        Ok(available >= needed)
    }

    /// Hash the whole staging file and compare it to the address it claims.
    ///
    /// Read in fixed buffers, so a 2 GiB download is hashed with a 64 KiB
    /// allocation. `blake3::Hasher` cannot be serialised, which is why this is
    /// one read at the end rather than state carried across chunks — the same
    /// reason the worker hashes once (§4.3).
    pub fn hashes_to(&self, hash: &BlobHash) -> Result<bool, TransferError> {
        use std::io::Read;
        let mut file = fs::File::open(self.part_path(hash)).map_err(local)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(local)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hasher.finalize().to_hex().as_str() == hash.digest())
    }

    /// Set a pending download aside **by moving it out of staging**, and
    /// return the opaque location for the report.
    ///
    /// Moved rather than copied, which is the opposite of the Store's rule for
    /// a *stored* blob — and deliberately: a stored blob is the only copy of
    /// something a document references, while this is a partial download whose
    /// bytes are known to be wrong. Leaving it in staging would have the next
    /// resume append to bytes that already failed.
    pub fn quarantine(&self, hash: &BlobHash) -> Result<String, TransferError> {
        self.ensure()?;
        let from = self.part_path(hash);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default();
        let to = self.quarantine.join(format!("{}.{stamp}", part_name(hash)));
        fs::rename(&from, &to).map_err(local)?;
        Ok(to.to_string_lossy().into_owned())
    }

    /// The staging file is no longer wanted — the object is installed, or the
    /// reference is gone. Absent is success.
    pub fn discard(&self, hash: &BlobHash) -> Result<(), TransferError> {
        match fs::remove_file(self.part_path(hash)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(local(error)),
        }
    }

    /// Startup recovery over the staging directory.
    ///
    /// **Touches only its own names.** A name that does not end in
    /// [`DOWNLOAD_SUFFIX`] belongs to somebody else — the Store's in-flight
    /// imports stage here — and is left exactly where it is. A name that ends
    /// in the suffix but is not `b3-<digest>.download.part` is moved to
    /// quarantine rather than parsed, deleted or read.
    ///
    /// A recognised file whose hash `referenced` does not name is removed only
    /// once nothing has written it for `abandoned_after`
    /// ([`ABANDONED_AFTER`] in production): `referenced` is one Workspace's
    /// set, and the file may be another's download in flight. Returns
    /// `(kept, removed, quarantined)`.
    pub fn sweep(
        &self,
        referenced: &std::collections::BTreeSet<String>,
        abandoned_after: std::time::Duration,
    ) -> Result<(usize, usize, usize), TransferError> {
        self.ensure()?;
        let (mut kept, mut removed, mut quarantined) = (0, 0, 0);
        for entry in fs::read_dir(&self.staging).map_err(local)? {
            let entry = entry.map_err(local)?;
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(DOWNLOAD_SUFFIX) {
                continue;
            }
            match parse_part_name(&name) {
                Some(hash) if referenced.contains(hash.as_str()) => kept += 1,
                Some(_) if !untouched_for(&entry, abandoned_after) => kept += 1,
                Some(_) => {
                    fs::remove_file(entry.path()).map_err(local)?;
                    removed += 1;
                }
                None => {
                    let to = self.quarantine.join(&name);
                    fs::rename(entry.path(), to).map_err(local)?;
                    quarantined += 1;
                }
            }
        }
        Ok((kept, removed, quarantined))
    }
}

pub fn part_name(hash: &BlobHash) -> String {
    format!("{}{DOWNLOAD_SUFFIX}", hash.file_name())
}

/// The inverse, refusing anything that is not exactly the format this crate
/// writes. Bounded before it is parsed, so a hostile name costs a length
/// check rather than an allocation.
pub fn parse_part_name(name: &str) -> Option<BlobHash> {
    if name.len() > MAX_STAGING_NAME {
        return None;
    }
    let stem = name.strip_suffix(DOWNLOAD_SUFFIX)?;
    let (algorithm, digest) = stem.split_once('-')?;
    // The filename spelling and the address spelling differ by exactly this
    // one character, and nowhere else.
    BlobHash::from_digest(algorithm, digest)
        .ok()
        .filter(|hash| hash.file_name() == stem)
}

/// Whether nothing has written this file for `age`. An unreadable time
/// answers no: a file the sweep cannot date is not one it may call abandoned.
fn untouched_for(entry: &fs::DirEntry, age: std::time::Duration) -> bool {
    entry
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= age)
}

/// Every filesystem refusal in this module becomes one Store code, with the
/// operating system's own words kept behind `KORU_CLOUD_DEBUG`'s level rather
/// than thrown away: the code is what a caller branches on, and the `errno` is
/// what a person reads when a disk fills.
fn local(error: std::io::Error) -> TransferError {
    tracing::debug!(
        target: "koru_blob_sync",
        event = "cloud.blob.staging_refused",
        error = %error,
    );
    TransferError::Local(Code::StorageFailed)
}

#[cfg(test)]
mod tests;
