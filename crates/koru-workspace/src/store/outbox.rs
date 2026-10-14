// SPDX-License-Identifier: MPL-2.0

//! Work owed to the store, kept on this machine until it lands.
//!
//! Reconnection covers a store that comes back while the window is open — the
//! session rewinds `sent` to `acked` and re-exports. What it cannot cover is
//! closing Koru while the store is down, because nothing outlives the process.
//! This is that disk.
//!
//! ## The outbox belongs to the store, not to the app
//!
//! What is buffered is work owed to **one store instance**, and
//! [ADR-84](../../../../docs/decisions/ADR-076-100.md#adr-84-rendezvous--a-local-endpoint-is-how-a-store-is-found-and-the-only-long-lived-credential-is-gone)
//! makes a store a singleton per user and per channel. So there is exactly one
//! outbox and every shell sharing that store shares it: two Koru windows today,
//! and koru-email or koru-chat later. Each keeping a private pile the others
//! could neither see nor flush is how one of them quietly becomes the only
//! program that can deliver an edit. It also means the debt travels with the
//! store — deleting the store root takes its outbox with it, which is what a
//! reset should mean.
//!
//! ```text
//! <store root>/outbox/<workspace_id>/<document_id>/<replica>.loro
//! ```
//!
//! **Machine-local, and never a profile folder** — one may be on OneDrive, and
//! [GLOBAL_SEARCH](../../../../docs/features/search/README.md)'s storage reasoning applies
//! here unchanged.
//!
//! ## One file per replica, so there is nothing to lock
//!
//! `<replica>` is the Loro peer id of that document's replica in that process.
//! Two windows on one document are two peer ids and two files; a second program
//! is a third. No writer ever touches another writer's file, so no lock is
//! needed and none is taken — which also means no lock to leak when a process
//! is killed, and killed processes are exactly the case this exists for.
//!
//! **Anyone may replay anyone's file.** The payload is opaque Loro update bytes
//! addressed to a document id, so whoever opens that document replays every
//! file under its directory whatever wrote it. Loro merges, and importing the
//! same update twice is a no-op.
//!
//! ## The frame, and why `acked` is in it
//!
//! ```text
//! "KORUOBX" + u8 format      8 bytes
//! i64 document schema        koru_core::schema::SCHEMA_VERSION
//! u32 length of the acked vector
//!     acked VersionVector::encode()
//!     update bytes, to the end
//! ```
//!
//! The updates are a **delta**, and a delta means nothing without the point it
//! is a delta from. `acked` is that point, and it is what
//! [`Buffered::applies_to`] compares before anything is imported: a delta whose
//! causal ancestors are missing imports `Ok` and applies nothing, which is the
//! Loro trap that looks exactly like success.
//!
//! **The schema stamp is the document's, and only a *newer* one is refused.**
//! Replaying our own older work is ordinary; pushing a delta written by a build
//! that understood more of the document than this one does is the thing
//! [`Compatibility`](koru_core::schema::Compatibility) exists to prevent, and
//! the outbox is a way in that would otherwise go around it.
//!
//! A file this build cannot read is **kept, never deleted**. A build that
//! cannot interpret a buffer has no business destroying it.

use std::path::{Path, PathBuf};

use koru_store_core::ids::{DocumentId as StoreDocumentId, WorkspaceId as StoreWorkspaceId};

/// Magic plus a format version, in one eight-byte read.
const MAGIC: &[u8; 7] = b"KORUOBX";
const FORMAT: u8 = 1;

/// Magic, format, schema, and the length of the vector that follows.
const HEAD: usize = MAGIC.len() + 1 + 8 + 4;

/// The directory name under the store root.
const DIR: &str = "outbox";

/// Why an outbox operation did not happen.
///
/// A string, because every caller does the same two things with it — show it
/// and stop — and an enum whose variants nothing matches on is a shape without
/// a reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxError(pub String);

impl std::fmt::Display for OutboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for OutboxError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

type Result<T> = std::result::Result<T, OutboxError>;

/// One replica's owed work, as it was read back off the disk.
pub struct Buffered {
    /// The file it came from, so a caller can remove exactly what it consumed.
    pub path: PathBuf,
    /// The peer id that wrote it. Kept for diagnostics — replay does not care
    /// whose work it is.
    pub replica: u64,
    /// The point the updates are a delta from.
    pub acked: loro::VersionVector,
    pub updates: Vec<u8>,
}

impl Buffered {
    /// Whether a document holding `version` can actually apply this.
    ///
    /// **The guard that makes replay honest.** Loro parks a delta whose
    /// ancestors are missing and returns `Ok`, so importing one that does not
    /// apply is silent — the buffer would look delivered and the work would be
    /// gone. If this is false the file stays where it is.
    pub fn applies_to(&self, version: &loro::VersionVector) -> bool {
        version.includes_vv(&self.acked)
    }
}

/// The outbox directory for one document, or `None` when there is no root to
/// put it under.
///
/// `store_root` is the `--root` the store was found at, when there is one — the
/// same argument `Discovery::find_at` took. Without it, this build's channel
/// root. A test that spawns a daemon on a temp directory therefore buffers into
/// that directory and never touches `%LOCALAPPDATA%`.
pub fn dir(
    store_root: Option<&Path>,
    workspace: StoreWorkspaceId,
    document: StoreDocumentId,
) -> Option<PathBuf> {
    let root = match store_root {
        Some(root) => root.to_path_buf(),
        None => koru_channel::store_root()?,
    };
    Some(
        root.join(DIR)
            .join(workspace.to_storage())
            .join(document.to_storage()),
    )
}

/// Write one replica's owed work, replacing whatever it wrote before.
///
/// **Through a `.part` and a rename**, because a half-written buffer is worse
/// than none: it would be read back as a truncated delta, and the failure would
/// arrive as a document that quietly lost its tail.
pub fn write(
    store_root: Option<&Path>,
    workspace: StoreWorkspaceId,
    document: StoreDocumentId,
    replica: u64,
    acked: &loro::VersionVector,
    updates: &[u8],
) -> Result<PathBuf> {
    let dir = dir(store_root, workspace, document).ok_or_else(|| {
        OutboxError("this platform has no local application-data directory".into())
    })?;
    std::fs::create_dir_all(&dir)?;

    let acked = acked.encode();
    let mut framed = Vec::with_capacity(HEAD + acked.len() + updates.len());
    framed.extend_from_slice(MAGIC);
    framed.push(FORMAT);
    framed.extend_from_slice(&koru_core::schema::SCHEMA_VERSION.to_le_bytes());
    framed.extend_from_slice(&(acked.len() as u32).to_le_bytes());
    framed.extend_from_slice(&acked);
    framed.extend_from_slice(updates);

    let path = dir.join(format!("{replica:016x}.loro"));
    let part = path.with_extension("part");
    std::fs::write(&part, &framed)?;
    std::fs::rename(&part, &path)?;
    Ok(path)
}

/// Everything buffered for one document, whoever wrote it.
///
/// Returns what could be read and, separately, what could not. **A file that
/// will not decode is named rather than skipped** — the whole point of this
/// directory is that somebody's work is in it, so "there was something here and
/// I could not read it" is the one thing that must never be silent.
pub fn read_all(
    store_root: Option<&Path>,
    workspace: StoreWorkspaceId,
    document: StoreDocumentId,
) -> (Vec<Buffered>, Vec<OutboxError>) {
    let Some(dir) = dir(store_root, workspace, document) else {
        return (Vec::new(), Vec::new());
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // Not an error: no directory is the ordinary case, and a directory that
        // cannot be listed says nothing about whether anything was ever in it.
        return (Vec::new(), Vec::new());
    };

    let mut found = Vec::new();
    let mut refused = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("loro") {
            continue;
        }
        match read_one(&path) {
            Ok(buffered) => found.push(buffered),
            Err(why) => refused.push(OutboxError(format!("{}: {why}", path.display()))),
        }
    }
    // In replica order, so two runs replay in the same order and a test can
    // assert on the result rather than on whatever the filesystem felt like.
    found.sort_by_key(|b| b.replica);
    (found, refused)
}

fn read_one(path: &Path) -> Result<Buffered> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < HEAD || &bytes[..MAGIC.len()] != MAGIC {
        return Err(OutboxError("not an outbox file".into()));
    }
    let format = bytes[MAGIC.len()];
    if format != FORMAT {
        return Err(OutboxError(format!(
            "written by a build using outbox format {format}, and this one understands {FORMAT}"
        )));
    }
    let schema = i64::from_le_bytes(bytes[8..16].try_into().expect("eight bytes"));
    // **Newer only.** Replaying our own older work is ordinary; sending on a
    // delta written by a build that understood more of the document than this
    // one is what `Compatibility` refuses on the save path, and this must not
    // be the way around it.
    if schema > koru_core::schema::SCHEMA_VERSION {
        return Err(OutboxError(format!(
            "written against document schema {schema}, and this build understands {}",
            koru_core::schema::SCHEMA_VERSION
        )));
    }
    let acked_len = u32::from_le_bytes(bytes[16..20].try_into().expect("four bytes")) as usize;
    let end = HEAD + acked_len;
    if bytes.len() < end {
        return Err(OutboxError("truncated before the version vector".into()));
    }
    let acked = loro::VersionVector::decode(&bytes[HEAD..end])
        .map_err(|e| OutboxError(format!("the version vector would not decode: {e}")))?;

    let replica = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| u64::from_str_radix(s, 16).ok())
        .unwrap_or_default();

    Ok(Buffered {
        path: path.to_path_buf(),
        replica,
        acked,
        updates: bytes[end..].to_vec(),
    })
}

/// Forget one buffered file, once the store has confirmed what it held.
///
/// Best-effort: a file that will not delete is a file that gets replayed again
/// next time, and replaying an update the store already has is a no-op. Failing
/// the caller over it would turn a tidy-up into an error path.
pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
    // The document and workspace directories, when they empty out. Ignored the
    // same way, and `remove_dir` refuses a non-empty one, so a sibling
    // replica's file is safe.
    if let Some(document) = path.parent() {
        let _ = std::fs::remove_dir(document);
        if let Some(workspace) = document.parent() {
            let _ = std::fs::remove_dir(workspace);
        }
    }
}

#[cfg(test)]
mod tests;
