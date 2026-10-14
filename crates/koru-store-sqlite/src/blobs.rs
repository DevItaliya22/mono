// SPDX-License-Identifier: MPL-2.0

//! Bytes, outside Loro and outside SQLite.
//!
//! ## Why blobs are not in the CRDT
//!
//! A CRDT keeps history. A ten-megabyte image inserted and deleted twice is
//! thirty megabytes of history that never goes away, replicated to every peer,
//! and re-encoded into every snapshot. Blobs live beside the document and the
//! document holds a hash ([ADR-12](../../../docs/decisions/README.md)).
//!
//! ## One object per store root; the association is the permission
//!
//! ```text
//! <root>/blobs/objects/b3-<digest>      the bytes, once
//! <root>/blobs/staging/…part            work nobody may read yet
//! <root>/blobs/quarantine/<stamp>/…     evidence, copied never moved
//! workspace.db.blobs[hash]              the association, per Workspace
//! ```
//!
//! This type answers only the first question — *are these bytes here* — and it
//! is never authority. Filing bytes per Workspace instead duplicated a picture
//! used twice, made a cross-Workspace move copy bytes, and made the renderer
//! ask the selected navigation Workspace for a document tab's image
//! (`ADR-000-blobobjectworker`, "One local object, and the association is the
//! permission"). The Workspace row is checked one layer up, in the service,
//! because only the service can see a `workspace.db` at all.
//!
//! ## Identity is content, never a filename
//!
//! The on-disk name is `b3-<64 hex>`. The person's filename is metadata in the
//! `blobs` table — arbitrary Unicode, quite possibly containing a path
//! separator, a colon, or a right-to-left override. Building a path from one is
//! how a store writes outside its own directory.
//!
//! **BLAKE3, not the FNV that `koru-docstore` uses.** `BlobHash` is already
//! `<algorithm>:<digest>`, so `b3:` sits beside the legacy bare digests rather
//! than replacing them — which is the whole reason the algorithm is in the
//! value. FNV addresses content; it does not authenticate it, and a collision
//! in a store that will one day dedup across profiles is somebody else's
//! picture in your document.
//!
//! ## Nothing damaged is deleted
//!
//! [`quarantine`](SqliteBlobs::quarantine) **copies**. Recovery must not be
//! able to destroy the thing it was called to preserve, and a blob that fails
//! verification is evidence. The same rule governs
//! [`promote`](SqliteBlobs::promote): a legacy per-Workspace file is *copied*
//! into the object store and left exactly where it was, so this rung is
//! reversible by running an older build.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use koru_store_core::errors::{Code, Error, Result};
use koru_store_core::model::{BlobHash, Timestamp};
use koru_store_core::ports::{
    BLOB_UPLOAD_BODY_LIMIT, BlobStore, CompletedBlobUpload, LegacyPromotion, PendingBlobUpload,
};

static NEXT_UPLOAD: AtomicU64 = AtomicU64::new(0);

/// Where the bytes themselves live, under the object store's root.
pub const OBJECTS_DIR: &str = "objects";
/// Work in progress. Nothing here is ever served, at any age.
pub const STAGING_DIR: &str = "staging";
/// Copies of bytes that failed verification. Evidence, never a source.
pub const QUARANTINE_DIR: &str = "quarantine";

/// How long a pending file may sit in staging before a sweep considers it
/// abandoned. Generous on purpose: a 2 GiB upload over a slow link is hours,
/// and the cost of being wrong in the other direction is somebody's picture.
pub const PENDING_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;

/// Reading a candidate to hash it costs one buffer, not one file. The whole
/// point of the object store is that a 2 GiB blob never becomes a 2 GiB `Vec`.
const VERIFY_CHUNK: usize = 256 * 1024;

/// Content-addressed bytes under one store root.
///
/// One instance per **store**, not per Workspace — that is the change B1 makes.
pub struct SqliteBlobs {
    root: PathBuf,
}

struct SqlitePendingUpload {
    objects: PathBuf,
    staging: PathBuf,
    file: Option<std::fs::File>,
    hasher: blake3::Hasher,
    size: u64,
    limit: u64,
    published: bool,
}

/// What a sweep of the staging directory did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PendingSweep {
    /// Abandoned partial uploads removed, and promotion candidates whose
    /// object is already installed. Nothing referenced either: an upload
    /// publishes its address only on `finish`.
    pub removed: usize,
    /// Stale promotion candidates whose object was still missing and whose
    /// bytes were installed by the sweep instead of being thrown away.
    pub recovered: usize,
    /// Stale promotion candidates that could neither be matched to their name
    /// nor confirmed as already installed. **Left on disk**, and reported.
    pub retained: usize,
}

impl SqliteBlobs {
    /// `root` is the object store's own directory — `<store root>/blobs`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The object store's root. Quarantine locations are relative to this.
    pub fn dir(&self) -> &Path {
        &self.root
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join(OBJECTS_DIR)
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.root.join(STAGING_DIR)
    }

    /// The hash of some bytes, as this build computes it.
    pub fn hash_of(bytes: &[u8]) -> BlobHash {
        BlobHash::from_digest(BlobHash::BLAKE3, &blake3::hash(bytes).to_hex())
            .expect("blake3 produces hex")
    }

    fn path_of(&self, hash: &BlobHash) -> PathBuf {
        // `file_name` is `<alg>-<digest>`: ASCII, no colon (illegal in a Windows
        // path), and derived from the content rather than from anything a
        // person typed.
        self.objects_dir().join(hash.file_name())
    }

    fn quarantine_dir(&self) -> PathBuf {
        self.root.join(QUARANTINE_DIR)
    }

    fn stage(&self, prefix: &str) -> Result<(PathBuf, std::fs::File)> {
        let staging = self.staging_dir();
        std::fs::create_dir_all(&staging)
            .map_err(|e| Error::new(Code::StorageFailed, format!("blob staging directory: {e}")))?;
        std::fs::create_dir_all(self.objects_dir())
            .map_err(|e| Error::new(Code::StorageFailed, format!("blob directory: {e}")))?;
        loop {
            let sequence = NEXT_UPLOAD.fetch_add(1, Ordering::Relaxed);
            let path = staging.join(format!("{prefix}.{}.{sequence}.part", std::process::id()));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(Error::new(
                        Code::StorageFailed,
                        format!("create pending blob: {error}"),
                    ));
                }
            }
        }
    }

    fn begin_with_limit(&self, limit: u64) -> Result<Box<dyn PendingBlobUpload>> {
        let (staging, file) = self.stage(pending::UPLOAD_PREFIX)?;
        Ok(Box::new(SqlitePendingUpload {
            objects: self.objects_dir(),
            staging,
            file: Some(file),
            hasher: blake3::Hasher::new(),
            size: 0,
            limit,
            published: false,
        }))
    }

    /// Copy a candidate file into the object store under the name it claims.
    ///
    /// This is the lazy layout migration, and every rule in it exists to make
    /// the rung reversible:
    ///
    /// - the candidate is **hashed** before anything is installed, so a file
    ///   whose bytes do not match the name it sits under is refused visibly
    ///   rather than published as something it is not;
    /// - the copy lands in staging first, so a reader never sees a partial
    ///   object under a name that promises those exact bytes;
    /// - the install is a rename, and a concurrent identical install winning
    ///   the race is harmless — the bytes are the same bytes by construction;
    /// - **the candidate is never moved and never deleted.**
    pub fn promote(&self, candidate: &Path, hash: &BlobHash) -> Result<LegacyPromotion> {
        if self.path_of(hash).exists() {
            return Ok(LegacyPromotion::Installed);
        }
        let mut source = match std::fs::File::open(candidate) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LegacyPromotion::NoCandidate);
            }
            Err(error) => {
                return Err(Error::new(
                    Code::StorageFailed,
                    format!("open legacy blob: {error}"),
                ));
            }
        };

        let (staging, mut file) = self.stage(&pending::promotion_prefix(hash))?;
        let outcome = (|| -> Result<LegacyPromotion> {
            let mut hasher = blake3::Hasher::new();
            let mut buffer = vec![0u8; VERIFY_CHUNK];
            loop {
                let read = source.read(&mut buffer).map_err(|e| {
                    Error::new(Code::StorageFailed, format!("read legacy blob: {e}"))
                })?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                file.write_all(&buffer[..read]).map_err(|e| {
                    Error::new(Code::StorageFailed, format!("stage legacy blob: {e}"))
                })?;
            }
            file.flush()
                .map_err(|e| Error::new(Code::StorageFailed, format!("stage legacy blob: {e}")))?;
            drop(file);

            let found = BlobHash::from_digest(BlobHash::BLAKE3, &hasher.finalize().to_hex())
                .expect("blake3 produces hex");
            if found != *hash {
                // Visibly refused. The candidate stays exactly where it is:
                // it is the only copy of whatever it actually holds, and a
                // file nobody can name is still better than no file.
                return Ok(LegacyPromotion::Mismatch { found });
            }
            self.install(&staging, hash)?;
            Ok(LegacyPromotion::Installed)
        })();
        if !matches!(outcome, Ok(LegacyPromotion::Installed)) {
            let _ = std::fs::remove_file(&staging);
        }
        outcome
    }

    /// Publish a staged file at the object's final path.
    ///
    /// **The final object path is checked before the staged file goes**, in
    /// both branches: a rename that lands is the install, and a rename refused
    /// because the object is already there means some other writer installed
    /// the identical bytes first — which is harmless, because content
    /// addressing makes the two files the same file.
    fn install(&self, staging: &Path, hash: &BlobHash) -> Result<()> {
        std::fs::create_dir_all(self.objects_dir())
            .map_err(|e| Error::new(Code::StorageFailed, format!("blob directory: {e}")))?;
        let destination = self.path_of(hash);
        match std::fs::rename(staging, &destination) {
            Ok(()) => Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists || destination.exists() =>
            {
                std::fs::remove_file(staging).map_err(|remove| {
                    Error::new(
                        Code::StorageFailed,
                        format!("discard duplicate pending blob after {error}: {remove}"),
                    )
                })
            }
            Err(error) => Err(Error::new(
                Code::StorageFailed,
                format!("publish blob: {error}"),
            )),
        }
    }

    /// Clear staging of work a crash left behind.
    ///
    /// A pending file is a candidate for removal only when its name matches the
    /// bounded format this store writes **and** it is older than `max_age_ms`.
    /// A promotion candidate names the object it was copying, so the final
    /// object path is checked first: if the object never landed, the bytes are
    /// hashed and installed rather than deleted, and one that matches neither
    /// is kept and reported. Nothing outside the format is touched at any age —
    /// staging is ours, and a file we did not write is somebody else's.
    pub fn sweep_staging(&self, now: Timestamp, max_age_ms: i64) -> Result<PendingSweep> {
        let mut swept = PendingSweep::default();
        let entries = match std::fs::read_dir(self.staging_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(swept),
            Err(error) => {
                return Err(Error::new(
                    Code::StorageFailed,
                    format!("read blob staging: {error}"),
                ));
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(kind) = pending::classify(&name) else {
                continue;
            };
            if !older_than(&path, now, max_age_ms) {
                continue;
            }
            match kind {
                pending::Pending::Upload => {
                    // Bytes with no hash and no row: an upload publishes its
                    // address only on `finish`, so nothing can reference these.
                    if std::fs::remove_file(&path).is_ok() {
                        swept.removed += 1;
                    }
                }
                pending::Pending::Promotion(hash) => {
                    // **The final object path first**, always: a pending file
                    // whose object landed is finished work, and one whose
                    // object did not is the only copy of those bytes.
                    if self.path_of(&hash).exists() {
                        if std::fs::remove_file(&path).is_ok() {
                            swept.removed += 1;
                        }
                    } else if self.hash_file(&path).is_ok_and(|found| found == hash)
                        && self.install(&path, &hash).is_ok()
                    {
                        swept.recovered += 1;
                    } else {
                        // Truncated, unreadable, or not what its name says.
                        // One file nobody can account for does not end the
                        // sweep, and it certainly does not get deleted.
                        swept.retained += 1;
                        tracing::warn!(
                            object = %hash,
                            file = %path.display(),
                            "a stale blob promotion could not be accounted for and was kept"
                        );
                    }
                }
            }
        }
        if swept.removed + swept.recovered + swept.retained > 0 {
            tracing::info!(
                removed = swept.removed,
                recovered = swept.recovered,
                retained = swept.retained,
                "swept abandoned blob staging files"
            );
        }
        Ok(swept)
    }

    /// The hash of a file on disk, read in bounded chunks.
    fn hash_file(&self, path: &Path) -> Result<BlobHash> {
        let mut file = std::fs::File::open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::bare(Code::BlobNotFound),
            _ => Error::new(Code::StorageFailed, format!("read blob: {e}")),
        })?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; VERIFY_CHUNK];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|e| Error::new(Code::StorageFailed, format!("read blob: {e}")))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(
            BlobHash::from_digest(BlobHash::BLAKE3, &hasher.finalize().to_hex())
                .expect("blake3 produces hex"),
        )
    }
}

/// Is a file older than `max_age_ms`?
///
/// A filesystem that will not say is treated as *not* stale: a clock that
/// cannot be read is not a reason to remove somebody's bytes.
fn older_than(path: &Path, now: Timestamp, max_age_ms: i64) -> bool {
    let Ok(modified) = std::fs::metadata(path).and_then(|meta| meta.modified()) else {
        return false;
    };
    let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return false;
    };
    now.millis().saturating_sub(since_epoch.as_millis() as i64) > max_age_ms
}

/// The bounded names this store writes into staging, and how to read one back.
///
/// A sweep acts on a filename, so the grammar is narrow on purpose: ASCII,
/// bounded length, a known prefix and a known suffix. Anything else in staging
/// belongs to somebody else and is left alone.
mod pending {
    use koru_store_core::model::BlobHash;

    pub const UPLOAD_PREFIX: &str = ".pending";
    pub const PROMOTE_PREFIX: &str = ".promote";
    const SUFFIX: &str = ".part";
    /// `.promote.` + `b3-` + 64 hex + `.` + pid + `.` + sequence + `.part`,
    /// with generous room for both numbers.
    const MAX_NAME: usize = 128;

    pub enum Pending {
        Upload,
        Promotion(BlobHash),
    }

    pub fn promotion_prefix(hash: &BlobHash) -> String {
        format!("{PROMOTE_PREFIX}.{}", hash.file_name())
    }

    pub fn classify(name: &str) -> Option<Pending> {
        if name.len() > MAX_NAME || !name.is_ascii() || !name.ends_with(SUFFIX) {
            return None;
        }
        if let Some(rest) = name
            .strip_prefix(PROMOTE_PREFIX)
            .and_then(|r| r.strip_prefix('.'))
        {
            let (file_name, _) = rest.split_once('.')?;
            let (algorithm, digest) = file_name.split_once('-')?;
            return BlobHash::from_digest(algorithm, digest)
                .ok()
                .map(Pending::Promotion);
        }
        name.strip_prefix(UPLOAD_PREFIX)
            .and_then(|rest| rest.strip_prefix('.'))
            .map(|_| Pending::Upload)
    }
}

impl PendingBlobUpload for SqlitePendingUpload {
    fn write_chunk(&mut self, bytes: &[u8]) -> Result<()> {
        let next = self
            .size
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| Error::bare(Code::PayloadTooLarge))?;
        if next > self.limit {
            return Err(Error::new(
                Code::PayloadTooLarge,
                format!("blob exceeds the {} byte limit", self.limit),
            ));
        }
        self.file
            .as_mut()
            .expect("unfinished upload owns its file")
            .write_all(bytes)
            .map_err(|e| Error::new(Code::StorageFailed, format!("write blob: {e}")))?;
        self.hasher.update(bytes);
        self.size = next;
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<CompletedBlobUpload> {
        let mut file = self.file.take().expect("unfinished upload owns its file");
        file.flush()
            .map_err(|e| Error::new(Code::StorageFailed, format!("flush blob: {e}")))?;
        drop(file);

        let hash = BlobHash::from_digest(
            BlobHash::BLAKE3,
            &self.hasher.finalize().to_hex().to_string(),
        )
        .expect("blake3 produces hex");
        let destination = self.objects.join(hash.file_name());
        match std::fs::rename(&self.staging, &destination) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists || destination.exists() =>
            {
                std::fs::remove_file(&self.staging).map_err(|remove| {
                    Error::new(
                        Code::StorageFailed,
                        format!("discard duplicate pending blob after {error}: {remove}"),
                    )
                })?;
            }
            Err(error) => {
                return Err(Error::new(
                    Code::StorageFailed,
                    format!("publish blob: {error}"),
                ));
            }
        }
        self.published = true;
        Ok(CompletedBlobUpload {
            hash,
            size: self.size,
        })
    }
}

impl Drop for SqlitePendingUpload {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.staging);
        }
    }
}

impl BlobStore for SqliteBlobs {
    fn begin(&self) -> Result<Box<dyn PendingBlobUpload>> {
        self.begin_with_limit(BLOB_UPLOAD_BODY_LIMIT)
    }

    fn get(&self, hash: &BlobHash) -> Result<Vec<u8>> {
        std::fs::read(self.path_of(hash)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::bare(Code::BlobNotFound),
            _ => Error::new(Code::StorageFailed, format!("read blob: {e}")),
        })
    }

    fn open(&self, hash: &BlobHash) -> Result<Box<dyn std::io::Read + Send>> {
        // A file handle, not the file: the caller reads it in bounded chunks
        // and the Store lock is long gone by the time the first one is read.
        std::fs::File::open(self.path_of(hash))
            .map(|file| Box::new(file) as Box<dyn std::io::Read + Send>)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::bare(Code::BlobNotFound),
                _ => Error::new(Code::StorageFailed, format!("open blob: {e}")),
            })
    }

    fn read_range(&self, hash: &BlobHash, offset: u64, len: usize) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(self.path_of(hash)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::bare(Code::BlobNotFound),
            _ => Error::new(Code::StorageFailed, format!("read blob: {e}")),
        })?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| Error::new(Code::StorageFailed, format!("seek blob: {e}")))?;
        // Seeking past the end is legal and yields nothing, so the short read
        // here is the answer rather than an error to distinguish.
        let mut bytes = Vec::new();
        file.take(len as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::new(Code::StorageFailed, format!("read blob: {e}")))?;
        Ok(bytes)
    }

    fn has(&self, hash: &BlobHash) -> Result<bool> {
        Ok(self.path_of(hash).exists())
    }

    fn verify(&self, hash: &BlobHash) -> Result<bool> {
        // Re-hash what is on the disk and compare with the name it is filed
        // under. A blob is the one thing in the store whose identity *is* its
        // checksum, so this needs no separate column.
        //
        // **Streamed, not read.** A streaming caller verifies once before it
        // starts; materialising 2 GiB to answer a question about 32 bytes is
        // the memory event `open` exists to avoid.
        Ok(self.hash_file(&self.path_of(hash))? == *hash)
    }

    fn release(&self, hash: &BlobHash) -> Result<()> {
        match std::fs::remove_file(self.path_of(hash)) {
            Ok(()) => Ok(()),
            // Already gone is the state the caller asked for.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::new(
                Code::StorageFailed,
                format!("release blob: {e}"),
            )),
        }
    }

    fn quarantine(&self, hash: &BlobHash) -> Result<String> {
        let from = self.path_of(hash);
        if !from.exists() {
            return Err(Error::bare(Code::BlobNotFound));
        }
        let at = Timestamp::now().for_filename();
        let dir = self.quarantine_dir().join(&at);
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::new(Code::StorageFailed, format!("quarantine: {e}")))?;
        let to = dir.join(hash.file_name());
        // **Copied, never moved.** The damaged blob stays where the document
        // references it: a reference to nothing is worse than a reference to
        // something broken, and the copy is what somebody diagnoses.
        std::fs::copy(&from, &to)
            .map_err(|e| Error::new(Code::StorageFailed, format!("quarantine: {e}")))?;
        // An opaque location for a report — not a path a client may parse.
        Ok(format!("{QUARANTINE_DIR}/{at}/{}", hash.file_name()))
    }
}

#[cfg(test)]
mod tests;
