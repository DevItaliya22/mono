// SPDX-License-Identifier: MPL-2.0

//! One document's directory on disk, and the sync bookkeeping for this
//! instance — generic over which kind of document it is.
//!
//! ```text
//! <document_dir>/
//!   snapshot.loro            the whole document — sufficient on its own
//!   snapshot.<peer>.part     transient, and named per instance
//!   updates/peer-<id>.loro   append-only, one file per peer
//!   backups/snapshot-<stamp>.loro
//!   quarantine/<stamp>/      copies of anything that would not decode
//!   blobs/<hash>             content-addressed; never inside the CRDT
//! ```
//!
//! **A workspace and a profile get the same machinery, not two copies of it.**
//! Everything here is bytes apart from two lines — building a fresh document and
//! decoding one — so the recovery path, the quarantine, the backup rotation, the
//! framed append and the resume-from-`consumed` poll are shared by construction.
//! There is exactly one implementation of "the snapshot would not open", and
//! that is the point: it is the code path nobody exercises until the day it is
//! the only thing standing between a person and their work.
//!
//! The type parameter is what keeps it honest. [`WorkspaceStore`] and
//! [`ProfileStore`] are aliases, and their `load` is the only method that knows
//! what it is loading.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use koru_core::profile::ProfileDoc;
use koru_core::schema::WorkspaceDoc;
use koru_core::{ProfileId, WorkspaceId};

use crate::{BlobStore, Opened, Recovery, Result, backup};

/// What the store needs of a document.
///
/// All export/import — deliberately nothing about nodes, trees or names. A store
/// that could read the document would be a store that could grow opinions about
/// it.
pub trait Doc: Sized {
    fn from_snapshot(bytes: &[u8]) -> std::result::Result<Self, koru_core::StoreError>;
    fn export_snapshot(&self) -> std::result::Result<Vec<u8>, koru_core::StoreError>;
    fn export_all_updates(&self) -> std::result::Result<Vec<u8>, koru_core::StoreError>;
    fn export_updates_since(
        &self,
        from: &loro::VersionVector,
    ) -> std::result::Result<Vec<u8>, koru_core::StoreError>;
    fn version(&self) -> loro::VersionVector;
    fn import(&mut self, bytes: &[u8]) -> std::result::Result<(), koru_core::StoreError>;
    /// Import many frames at once, paying whatever per-import bookkeeping the
    /// document has **once** rather than per frame. See
    /// `WorkspaceDoc::import_frames`.
    fn import_frames(
        &mut self,
        frames: &[Vec<u8>],
    ) -> std::result::Result<(), koru_core::StoreError>;
    /// Whether this document already contains everything in `frame`, judged from
    /// the frame's header rather than by applying it. See
    /// `WorkspaceDoc::frame_is_known`.
    fn frame_is_known(&self, frame: &[u8]) -> bool;
}

impl Doc for WorkspaceDoc {
    fn from_snapshot(bytes: &[u8]) -> std::result::Result<Self, koru_core::StoreError> {
        WorkspaceDoc::from_snapshot(bytes)
    }
    fn export_snapshot(&self) -> std::result::Result<Vec<u8>, koru_core::StoreError> {
        WorkspaceDoc::export_snapshot(self)
    }
    fn export_all_updates(&self) -> std::result::Result<Vec<u8>, koru_core::StoreError> {
        WorkspaceDoc::export_all_updates(self)
    }
    fn export_updates_since(
        &self,
        from: &loro::VersionVector,
    ) -> std::result::Result<Vec<u8>, koru_core::StoreError> {
        WorkspaceDoc::export_updates_since(self, from)
    }
    fn version(&self) -> loro::VersionVector {
        WorkspaceDoc::version(self)
    }
    fn import(&mut self, bytes: &[u8]) -> std::result::Result<(), koru_core::StoreError> {
        WorkspaceDoc::import(self, bytes)
    }
    fn import_frames(
        &mut self,
        frames: &[Vec<u8>],
    ) -> std::result::Result<(), koru_core::StoreError> {
        WorkspaceDoc::import_frames(self, frames)
    }
    fn frame_is_known(&self, frame: &[u8]) -> bool {
        WorkspaceDoc::frame_is_known(self, frame)
    }
}

impl Doc for ProfileDoc {
    fn from_snapshot(bytes: &[u8]) -> std::result::Result<Self, koru_core::StoreError> {
        ProfileDoc::from_snapshot(bytes)
    }
    fn export_snapshot(&self) -> std::result::Result<Vec<u8>, koru_core::StoreError> {
        ProfileDoc::export_snapshot(self)
    }
    fn export_all_updates(&self) -> std::result::Result<Vec<u8>, koru_core::StoreError> {
        ProfileDoc::export_all_updates(self)
    }
    fn export_updates_since(
        &self,
        from: &loro::VersionVector,
    ) -> std::result::Result<Vec<u8>, koru_core::StoreError> {
        ProfileDoc::export_updates_since(self, from)
    }
    fn version(&self) -> loro::VersionVector {
        ProfileDoc::version(self)
    }
    fn import(&mut self, bytes: &[u8]) -> std::result::Result<(), koru_core::StoreError> {
        ProfileDoc::import(self, bytes)
    }
    fn import_frames(
        &mut self,
        frames: &[Vec<u8>],
    ) -> std::result::Result<(), koru_core::StoreError> {
        ProfileDoc::import_frames(self, frames)
    }
    fn frame_is_known(&self, frame: &[u8]) -> bool {
        ProfileDoc::frame_is_known(self, frame)
    }
}

/// One document's directory, and the sync bookkeeping for this peer.
pub struct DocStore<D: Doc> {
    root: PathBuf,
    /// Names this peer's own update file and its temp files. Distinct per
    /// instance, so two windows on one folder never write the same bytes.
    peer: String,
    /// How many bytes of each peer's file have already been imported, so a poll
    /// only reads the tail.
    consumed: std::collections::HashMap<String, u64>,
    /// Incremental BLAKE3 state for exactly the prefix recorded in `consumed`.
    /// Persisted digests let a later launch trust an offset only after proving
    /// that the file still has the same bytes under that name.
    digests: std::collections::HashMap<String, blake3::Hasher>,
    /// The document version already handed to the transport. Everything after
    /// it is what the next publish sends, so no change is written twice.
    published: Option<loro::VersionVector>,
    /// When this session last copied the snapshot aside. `None` until the first
    /// save, which is what makes that first save take one.
    last_backup: Option<Instant>,
    /// Peer logs that would not decode and were skipped. Reported once, at the
    /// top level, rather than failing the load — one peer's broken file is no
    /// reason to refuse to open your own document.
    damaged: Vec<String>,
    /// Whether this session has written a snapshot yet, which is the event that
    /// makes [`prune_stale_peers`] safe to run. Once, not every save: the
    /// second save has nothing left to prune.
    ///
    /// [`prune_stale_peers`]: Self::prune_stale_peers
    pruned: bool,
    /// Whether another peer's file was written recently enough that somebody is
    /// probably at a keyboard. Recomputed by every [`sync_in`].
    ///
    /// [`sync_in`]: Self::sync_in
    peer_live: bool,
    _doc: PhantomData<D>,
}

/// The peer-log positions covered by one frozen document state.
///
/// Kept opaque so a background snapshot writer can prove which logs its
/// snapshot contains without exposing transport bookkeeping to the workspace
/// layer. A newer tail is deliberately not covered and therefore cannot be
/// pruned by that write.
#[derive(Clone)]
pub struct SnapshotCoverage {
    consumed: std::collections::HashMap<String, u64>,
    digests: std::collections::HashMap<String, blake3::Hasher>,
}

const COVERAGE_MAGIC: &[u8; 8] = b"KORUCOV\0";
const COVERAGE_VERSION: u32 = 1;

struct StoredCoverage {
    frontier: Vec<u8>,
    logs: Vec<(String, u64, [u8; 32])>,
}

impl<D: Doc> DocStore<D> {
    /// Open (or create) `root` as this document's directory.
    ///
    /// `root` is the document's *own* directory, not the data directory —
    /// working out which is the caller's, because a workspace's is named by its
    /// id and a profile's is not.
    pub fn open_dir(root: PathBuf, peer: &str) -> Result<Self> {
        fs::create_dir_all(root.join("updates"))?;
        fs::create_dir_all(root.join("blobs"))?;
        Ok(Self {
            root,
            peer: peer.to_string(),
            consumed: Default::default(),
            digests: Default::default(),
            published: None,
            last_backup: None,
            damaged: Vec::new(),
            pruned: false,
            peer_live: false,
            _doc: PhantomData,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn blobs(&self) -> BlobStore {
        BlobStore::new(self.root.join("blobs"), &self.peer)
    }

    /// A detached handle for serial snapshot writes.
    ///
    /// It names the same atomic destination and peer-specific temp file but
    /// owns its backup/pruning clocks. The caller must keep it on one worker;
    /// two writers for the same peer would race the temp file.
    pub fn snapshot_writer(&self) -> Self {
        Self {
            root: self.root.clone(),
            peer: self.peer.clone(),
            consumed: Default::default(),
            digests: Default::default(),
            published: None,
            last_backup: None,
            damaged: Vec::new(),
            pruned: false,
            peer_live: false,
            _doc: PhantomData,
        }
    }

    /// Which bytes the document currently in memory has consumed.
    pub fn snapshot_coverage(&self) -> SnapshotCoverage {
        SnapshotCoverage {
            consumed: self.consumed.clone(),
            digests: self.digests.clone(),
        }
    }

    /// Write a frozen state and prune only logs that state is known to cover.
    pub fn save_covered_snapshot(&mut self, doc: &D, coverage: SnapshotCoverage) -> Result<()> {
        self.consumed = coverage.consumed;
        self.digests = coverage.digests;
        self.save_snapshot(doc)
    }

    /// Peer update files that were skipped because they would not decode.
    pub fn damaged(&self) -> &[String] {
        &self.damaged
    }

    /// How recently another peer must have written for it to count as live.
    const PEER_LIVE_FOR: std::time::Duration = std::time::Duration::from_secs(20);

    /// Whether somebody else appears to be editing this document right now.
    ///
    /// The signal a caller uses to decide how often to poll: a window on its
    /// own has nothing to wait for, and one beside a peer wants the other's
    /// keystrokes promptly. Answered from the last [`sync_in`], so it costs
    /// nothing to ask.
    ///
    /// [`sync_in`]: Self::sync_in
    pub fn peer_live(&self) -> bool {
        self.peer_live
    }

    fn snapshot_path(&self) -> PathBuf {
        self.root.join("snapshot.loro")
    }

    fn coverage_path(&self) -> PathBuf {
        self.root.join("snapshot.coverage")
    }

    fn updates_dir(&self) -> PathBuf {
        self.root.join("updates")
    }

    fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    fn my_updates(&self) -> PathBuf {
        self.updates_dir().join(format!("peer-{}.loro", self.peer))
    }

    /// Restore only offsets whose snapshot frontier and exact file prefix can
    /// both be proved. The sidecar is derived state: any parse, version, path,
    /// truncation or digest failure simply leaves that log at offset zero for
    /// the established correctness-first scan.
    fn restore_snapshot_coverage(&mut self, doc: &D) {
        self.consumed.clear();
        self.digests.clear();
        let Ok(bytes) = fs::read(self.coverage_path()) else {
            return;
        };
        let Some(stored) = decode_coverage(&bytes) else {
            return;
        };
        if stored.frontier != doc.version().encode() {
            return;
        }
        for (key, offset, digest) in stored.logs {
            let relative = Path::new(&key);
            if relative.components().count() != 1
                || relative
                    .extension()
                    .is_none_or(|extension| extension != "loro")
            {
                continue;
            }
            let path = self.updates_dir().join(&key);
            let Ok(hasher) = hash_prefix(&path, offset) else {
                continue;
            };
            if hasher.clone().finalize().as_bytes() != &digest {
                continue;
            }
            self.consumed.insert(key.clone(), offset);
            self.digests.insert(key, hasher);
        }
    }

    fn write_snapshot_coverage(&self, doc: &D) -> std::io::Result<()> {
        let bytes = encode_coverage(doc.version().encode(), &self.consumed, &self.digests);
        let tmp = self
            .coverage_path()
            .with_extension(format!("coverage.{}.part", self.peer));
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, self.coverage_path())
    }

    // ------------------------------------------------------------ load / save

    /// Load the snapshot if there is one, then fold in every peer's updates.
    ///
    /// `fresh` builds the empty document for the cases where there is nothing to
    /// load from. It is a closure rather than a `Default` because a new document
    /// needs its id and its name, and this function has no business knowing what
    /// either of those is.
    ///
    /// Returns *how* it loaded alongside the document. A caller that ignores
    /// [`Opened`] is a caller that will one day open a blank window over a
    /// perfectly good backup and say nothing.
    pub fn load_with(
        &mut self,
        fresh: impl Fn() -> std::result::Result<D, koru_core::StoreError>,
    ) -> Result<(D, Opened)> {
        let read = koru_core::timing::Span::start("snapshot read");
        let snapshot = fs::read(self.snapshot_path());
        read.stop();
        // Whether what we end up holding is *also* on this disk in a form any
        // other replica can read. It decides whether this session's first
        // publish is a delta or the whole history — see below.
        let mut from_snapshot = false;
        let (mut doc, opened) = match snapshot {
            Ok(bytes) => match D::from_snapshot(&bytes) {
                Ok(doc) => {
                    from_snapshot = true;
                    self.restore_snapshot_coverage(&doc);
                    (doc, Opened::Normal)
                }
                // The one case this whole module is built around.
                Err(e) => self.recover(&fresh, &format!("the snapshot would not open ({e})"))?,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if backup::list(&self.backups_dir()).is_empty() {
                    // No snapshot and nothing to recover from. Either a first
                    // run, or the update logs alone will rebuild it — which is
                    // what makes a snapshot a compaction rather than the truth.
                    let is_fresh = !self.has_updates();
                    (
                        fresh()?,
                        if is_fresh {
                            Opened::Fresh
                        } else {
                            Opened::Normal
                        },
                    )
                } else {
                    // A snapshot that has gone missing while backups exist is
                    // not a first run, and must not be treated as one.
                    self.recover(&fresh, "the snapshot was missing")?
                }
            }
            Err(e) => return Err(e.into()),
        };
        self.sync_in(&mut doc)?;
        // **A session that loaded a snapshot publishes deltas, not history.**
        // Everything it holds is either in `snapshot.loro` or in a peer log
        // still sitting beside it, so the folder is already a complete history
        // and re-writing one into a brand-new file achieves nothing.
        //
        // `published` used to stay `None` unconditionally, making the first
        // publish an `export_all_updates`. Paired with a per-launch peer name
        // ([ADR-37]) that meant a fresh file holding the *entire history* on
        // every launch: `updates/` reached 91 MB beside a 1.4 MB snapshot, and
        // replaying it was 8.4 of the 9.4 seconds it took to open. See ADR-42.
        //
        // **`from_snapshot` is the whole of the condition, and it is not
        // decoration.** A document built by `fresh()` — a first run, or a
        // recovery with nothing to recover from — has a history that exists
        // *only in this process*. A delta from it is a change whose causal
        // ancestors no reader can obtain, and loro parks such an update as
        // pending rather than applying it: the receiving peer silently sees
        // nothing. `two_peers_converge_through_incremental_publishes` is the
        // test that says so.
        if from_snapshot {
            self.published = Some(doc.version());
        }
        Ok((doc, opened))
    }

    /// Whether any peer has ever written anything here.
    fn has_updates(&self) -> bool {
        fs::read_dir(self.updates_dir()).is_ok_and(|mut d| {
            d.any(|e| {
                e.is_ok_and(|e| {
                    e.metadata().map(|m| m.len()).unwrap_or(0) > 0
                        && e.path().extension().is_some_and(|x| x == "loro")
                })
            })
        })
    }

    /// The live snapshot is unusable. Keep a copy of it, then work back through
    /// the backups until one opens; failing that, rebuild from the peer logs.
    fn recover(
        &mut self,
        fresh: &impl Fn() -> std::result::Result<D, koru_core::StoreError>,
        problem: &str,
    ) -> Result<(D, Opened)> {
        let quarantined = self.quarantine(&self.snapshot_path());

        for candidate in backup::list(&self.backups_dir()) {
            let Ok(bytes) = fs::read(&candidate) else {
                continue;
            };
            if let Ok(doc) = D::from_snapshot(&bytes) {
                return Ok((
                    doc,
                    Opened::Recovered(Recovery {
                        problem: problem.to_string(),
                        quarantined,
                        from: Some(candidate),
                    }),
                ));
            }
        }

        // Nothing to restore from. The update logs may still hold everything,
        // and if they do not, `sync_in` leaves an empty document — which the
        // caller now knows to announce rather than present as a fresh install.
        Ok((
            fresh()?,
            Opened::Recovered(Recovery {
                problem: problem.to_string(),
                quarantined,
                from: None,
            }),
        ))
    }

    /// Copy a file that would not decode somewhere it will not be overwritten.
    /// Best-effort and never fatal — it is a courtesy, not a step.
    fn quarantine(&self, path: &Path) -> Option<PathBuf> {
        if !path.exists() {
            return None;
        }
        let dir = self.root.join("quarantine").join(backup::stamp_now());
        fs::create_dir_all(&dir).ok()?;
        let dest = dir.join(path.file_name()?);
        fs::copy(path, &dest).ok()?;
        Some(dest)
    }

    /// Write the whole document. Via a temp file and a rename, so a crash
    /// mid-write cannot leave a half-snapshot that fails to load.
    ///
    /// **The temp file is named per instance.** Two windows on one folder both
    /// writing `snapshot.loro.part` would interleave into a single file and then
    /// rename it over the only good copy — which is a corrupt snapshot produced
    /// by two processes that were each behaving correctly.
    ///
    /// The outgoing snapshot is copied aside first — see [`backup`] for when
    /// and why.
    pub fn save_snapshot(&mut self, doc: &D) -> Result<()> {
        let export = koru_core::timing::Watch::start("export_snapshot");
        let bytes = doc.export_snapshot()?;
        drop(export);
        let backup = koru_core::timing::Watch::start("rotate_backup");
        self.rotate_backup();
        drop(backup);
        let tmp = self
            .snapshot_path()
            .with_extension(format!("{}.part", self.peer));
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, self.snapshot_path())?;
        // The pair cannot be renamed atomically, so the frontier inside the
        // sidecar binds it to exactly this snapshot. A crash between the two
        // renames leaves a harmless mismatch and the next launch scans logs.
        if let Err(error) = self.write_snapshot_coverage(doc) {
            eprintln!("warning: could not write snapshot coverage ({error})");
        }
        // **Only now.** Everything this session consumed is in the file that was
        // just renamed into place, which is what makes deleting the logs it came
        // from provably lossless — see `prune_stale_peers`. Before the rename
        // there is nothing vouching for them.
        if !self.pruned {
            self.pruned = true;
            self.prune_stale_peers();
        }
        Ok(())
    }

    /// Copy the *existing* snapshot into `backups/` if one is due.
    ///
    /// Deliberately not fatal: a full disk or a read-only backups directory is
    /// a reason to complain, never a reason to refuse to save the work itself.
    /// The clock is advanced even on failure, or a broken backup directory
    /// would retry every 800 ms for as long as the app is open.
    fn rotate_backup(&mut self) {
        let due = self
            .last_backup
            .is_none_or(|t| t.elapsed() >= backup::BACKUP_EVERY);
        if !due {
            return;
        }
        self.last_backup = Some(Instant::now());
        let live = self.snapshot_path();
        if !live.exists() {
            return; // nothing written yet — there is nothing to preserve
        }
        if let Err(e) = backup::take(&self.backups_dir(), &live, &self.peer) {
            eprintln!("warning: could not write a backup ({e})");
        }
    }

    /// Every backup of this document, newest first.
    pub fn backups(&self) -> Vec<PathBuf> {
        backup::list(&self.backups_dir())
    }

    // ------------------------------------------------------------ dev sync

    /// Append this peer's changes for others to pick up.
    ///
    /// Appending rather than rewriting is what removes the need for locking:
    /// every byte written is new, so a reader can never see a partly-rewritten
    /// file, only a shorter one.
    pub fn publish(&mut self, update: &[u8]) -> Result<()> {
        if update.is_empty() {
            return Ok(());
        }
        let path = self.my_updates();
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let key = path
            .file_name()
            .expect("an update path has a file name")
            .to_string_lossy()
            .into_owned();
        let before = f.metadata()?.len();
        if self.consumed.get(&key).copied() != Some(before) || !self.digests.contains_key(&key) {
            let hasher = hash_prefix(&path, before)?;
            self.consumed.insert(key.clone(), before);
            self.digests.insert(key.clone(), hasher);
        }
        // Length-prefixed, so the reader can split a concatenated file back
        // into the individual updates Loro expects.
        let length = (update.len() as u32).to_le_bytes();
        f.write_all(&length)?;
        f.write_all(update)?;
        let hasher = self.digests.get_mut(&key).expect("seeded above");
        hasher.update(&length);
        hasher.update(update);
        self.consumed
            .insert(key, before + length.len() as u64 + update.len() as u64);
        Ok(())
    }

    /// Append everything this replica has learned since the last publish.
    ///
    /// Incremental rather than `export_all_updates`, so an hour of editing does
    /// not rewrite an hour of history on every keystroke. Returns true if there
    /// was anything to send.
    ///
    /// A peer will re-echo ops it imported from someone else, once. Loro's
    /// version vectors make that a no-op on the receiving side, and untangling
    /// it would mean tracking which ops came from where — cost the real
    /// transport, which is per-connection, will not have.
    pub fn publish_new(&mut self, doc: &D) -> Result<bool> {
        let current = doc.version();
        // Compare versions rather than testing the encoded bytes: an update
        // covering no ops still carries a header, so it is never empty.
        if self.published.as_ref() == Some(&current) {
            return Ok(false);
        }
        let update = match &self.published {
            Some(from) => doc.export_updates_since(from)?,
            None => doc.export_all_updates()?,
        };
        self.publish(&update)?;
        self.published = Some(current);
        Ok(true)
    }

    /// Import anything other peers have appended since the last call.
    /// Returns true if the document changed.
    ///
    /// A peer file that will not decode is **skipped and recorded**, not
    /// propagated: one damaged transport file is not a reason to refuse to open
    /// a document that is sitting right there in the snapshot. See [`damaged`].
    ///
    /// [`damaged`]: Self::damaged
    pub fn sync_in(&mut self, doc: &mut D) -> Result<bool> {
        let span = koru_core::timing::Span::start("peer import");
        let started = Instant::now();
        let mine = self.my_updates();
        let mut changed = false;
        let mut read_bytes = 0usize;
        let mut applied = 0usize;
        let mut skipped = 0usize;
        let mut checking = Duration::ZERO;
        let mut importing = Duration::ZERO;

        // Recomputed rather than latched: a peer that stops writing has to be
        // allowed to go quiet again, or one visit from a second window would
        // leave this one polling fast for the rest of the session.
        self.peer_live = false;

        for entry in fs::read_dir(self.updates_dir())? {
            let entry = entry?;
            let path = entry.path();
            if path == mine || path.extension().is_none_or(|e| e != "loro") {
                continue;
            }
            let Some(key) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            // Free: the directory entry already carries this on Windows, and it
            // is what decides whether anyone is worth polling quickly for.
            if entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Self::PEER_LIVE_FOR)
            {
                self.peer_live = true;
            }

            // **Only the tail.** `consumed` is always a frame boundary, so
            // seeking to it is the same resumption point re-reading the whole
            // file gave — without the other 91 MB. This is a poll that runs
            // every 300 ms for the life of the process, not just at load.
            let mut already = *self.consumed.get(&key).unwrap_or(&0);
            if already > 0 && !self.digests.contains_key(&key) {
                match hash_prefix(&path, already) {
                    Ok(hasher) => {
                        self.digests.insert(key.clone(), hasher);
                    }
                    Err(_) => {
                        already = 0;
                        self.consumed.remove(&key);
                    }
                }
            }
            let (base, bytes) = match read_from(&path, already) {
                Ok(read) => read,
                // A peer file that vanished between the read_dir and here is
                // another instance pruning, not a fault. Nothing was consumed,
                // so nothing is lost by carrying on.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if base != already {
                already = base;
                self.consumed.remove(&key);
                self.digests.remove(&key);
            }
            read_bytes += bytes.len();
            if bytes.is_empty() {
                continue;
            }

            // Split first, apply once. Framing is cheap; it is `import` that is
            // expensive, and batching lets the document reindex a single time
            // for the whole file instead of once per frame.
            let mut batch: Vec<Vec<u8>> = Vec::new();
            let mut cursor = 0usize;
            let mut damaged = false;
            while cursor + 4 <= bytes.len() {
                let len =
                    u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
                let start = cursor + 4;
                let Some(end) = start.checked_add(len) else {
                    // A length that overflows is not a frame this build wrote.
                    self.note_damage(&key, "a frame length that cannot be addressed");
                    damaged = true;
                    break;
                };
                if end > bytes.len() {
                    break; // still being written; pick it up next time
                }
                let frame = &bytes[start..end];
                // **The frame the snapshot already contains costs a header
                // parse, not an import.** In the ordinary single-user case that
                // is every frame in every file: the replica that wrote this log
                // also wrote the snapshot beside it.
                let at = Instant::now();
                let known = doc.frame_is_known(frame);
                checking += at.elapsed();
                if known {
                    skipped += 1;
                } else {
                    batch.push(frame.to_vec());
                }
                cursor = end;
            }

            if !batch.is_empty() {
                let at = Instant::now();
                let outcome = doc.import_frames(&batch);
                importing += at.elapsed();
                match outcome {
                    Ok(()) => {
                        applied += batch.len();
                        changed = true;
                    }
                    Err(e) => {
                        // Bad bytes stay bad, so consume to the end of what was
                        // read rather than reporting the same frames forever.
                        // The batch is dropped whole: `import_batch` resolves
                        // frames against each other, so which one was bad is not
                        // a question it answers.
                        self.note_damage(&key, &e.to_string());
                        damaged = true;
                    }
                }
            }

            let advanced = if damaged { bytes.len() } else { cursor };
            self.digests
                .entry(key.clone())
                .or_default()
                .update(&bytes[..advanced]);
            let consumed = already + advanced as u64;
            self.consumed.insert(key, consumed);
        }
        span.stop_with(|| {
            format!(
                "{} KB read, {applied} applied, {skipped} already known",
                read_bytes / 1024
            )
        });
        // A poll is meant to be imperceptible. When it is not, say what it was
        // carrying — the counts *are* the diagnosis, and by the time anyone has
        // noticed the stall the numbers are gone.
        if koru_core::timing::enabled() {
            let took = started.elapsed();
            if took >= std::time::Duration::from_millis(50) {
                println!(
                    "stall: sync_in {:.1}ms — {} KB read, {applied} applied \
                     ({:.1}ms), {skipped} known ({:.1}ms checking) [thread={}]",
                    took.as_secs_f64() * 1e3,
                    read_bytes / 1024,
                    importing.as_secs_f64() * 1e3,
                    checking.as_secs_f64() * 1e3,
                    std::thread::current().name().unwrap_or("unnamed"),
                );
            }
        }
        Ok(changed)
    }

    // ------------------------------------------------------------ pruning

    /// How long a peer log must lie untouched before it may be deleted.
    ///
    /// **This is a safety floor, not a tidiness threshold.** A running peer
    /// learns only from logs — it never re-reads `snapshot.loro` — so deleting a
    /// log something is still catching up on would strand it until it restarts.
    /// What makes a day enough is that [`POLL`] is 300 ms: every instance that
    /// has been running reads every log within a third of a second of it
    /// appearing, so a file untouched for 24 hours has certainly been seen by
    /// everything alive.
    ///
    /// [`POLL`]: ../../koru_workspace/session/constant.POLL.html
    const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

    /// Delete peer logs the snapshot has made redundant.
    ///
    /// **Call this only after a successful [`save_snapshot`].** One rule, and
    /// both halves of it are load-bearing — a file goes when
    ///
    /// 1. it was consumed **to its end** into the document just written, so
    ///    everything it held is in `snapshot.loro`, which is sufficient to load
    ///    from on its own; **and**
    /// 2. it has been untouched for [`STALE_AFTER`], so nothing still running
    ///    is behind on it.
    ///
    /// The name is deliberately **not** part of it. An earlier draft deleted
    /// `peer-i<hex>.loro` at any age on the grounds that an auto-named replica
    /// names one launch ([ADR-37]) and is finished once its work is in the
    /// snapshot — but "finished" describes the *writer*, and the file exists for
    /// the *readers*. A second window open on this folder right now has an
    /// auto-named file too. The clock answers the question the name only seemed
    /// to, so `--peer A` is treated exactly like `peer-i1a2b3c4d`: kept while
    /// fresh, dropped when stale. A peer that returns recreates its file —
    /// `publish` opens with `create(true)` — so this costs it nothing but a
    /// re-send it was going to make anyway.
    ///
    /// This instance's own file is never touched: it is being appended to, so it
    /// is never stale, and `sync_in` skips it so it is never consumed either.
    ///
    /// Best-effort throughout — a file that will not delete is left, because
    /// failing a save over housekeeping would be the worse bug.
    ///
    /// [ADR-37]: ../../../docs/decisions/README.md
    /// [`save_snapshot`]: Self::save_snapshot
    /// [`STALE_AFTER`]: Self::STALE_AFTER
    fn prune_stale_peers(&mut self) {
        let mine = self.my_updates();
        let Ok(entries) = fs::read_dir(self.updates_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == mine || path.extension().is_none_or(|e| e != "loro") {
                continue;
            }
            let Some(key) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };

            // Unread bytes are ops the snapshot cannot vouch for.
            if *self.consumed.get(&key).unwrap_or(&0) < meta.len() {
                continue;
            }
            // Written recently enough that something may still be reading it.
            let stale = meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age >= Self::STALE_AFTER);
            if !stale {
                continue;
            }

            if fs::remove_file(&path).is_ok() {
                // Or a file recreated under the same name would be skipped up to
                // the byte count of the one that is gone.
                self.consumed.remove(&key);
                self.digests.remove(&key);
            }
        }
    }

    /// Record a skipped peer file, once per file.
    fn note_damage(&mut self, key: &str, why: &str) {
        let note = format!("{key}: {why}");
        if !self.damaged.contains(&note) {
            eprintln!("warning: skipping a damaged update file — {note}");
            self.damaged.push(note);
        }
    }
}

/// Read `path` from `offset` to the end.
///
/// The whole point of the offset: `sync_in` polls every peer file three times a
/// second, and re-reading a file to throw away all but its tail is how 91 MB of
/// history became a per-poll cost.
fn read_from(path: &Path, offset: u64) -> std::io::Result<(u64, Vec<u8>)> {
    let f = fs::File::open(path)?;
    #[cfg(test)]
    let f = tests::BodyReadObserver(f);
    let mut f = f;
    let len = f.metadata()?.len();
    // A shorter file was pruned/recreated under the same name. Reset and read
    // it from byte zero in this poll; retaining the old cursor skips it forever.
    let offset = if len < offset { 0 } else { offset };
    if len == offset {
        return Ok((offset, Vec::new()));
    }
    f.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity((len - offset) as usize);
    f.read_to_end(&mut bytes)?;
    Ok((offset, bytes))
}

fn hash_prefix(path: &Path, offset: u64) -> std::io::Result<blake3::Hasher> {
    let file = fs::File::open(path)?;
    #[cfg(test)]
    let file = tests::BodyReadObserver(file);
    if file.metadata()?.len() < offset {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "peer log is shorter than its recorded coverage",
        ));
    }
    let mut reader = file.take(offset);
    let mut remaining = offset;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "peer log changed while verifying coverage",
            ));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(hasher)
}

fn encode_coverage(
    frontier: Vec<u8>,
    consumed: &std::collections::HashMap<String, u64>,
    digests: &std::collections::HashMap<String, blake3::Hasher>,
) -> Vec<u8> {
    let mut logs: Vec<_> = consumed
        .iter()
        .filter_map(|(name, offset)| digests.get(name).map(|digest| (name, offset, digest)))
        .collect();
    logs.sort_by_key(|(name, _, _)| name.as_str());
    let mut out = Vec::new();
    out.extend_from_slice(COVERAGE_MAGIC);
    out.extend_from_slice(&COVERAGE_VERSION.to_le_bytes());
    out.extend_from_slice(&(frontier.len() as u32).to_le_bytes());
    out.extend_from_slice(&frontier);
    out.extend_from_slice(&(logs.len() as u32).to_le_bytes());
    for (name, offset, hasher) in logs {
        let name = name.as_bytes();
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(hasher.clone().finalize().as_bytes());
    }
    out
}

fn decode_coverage(bytes: &[u8]) -> Option<StoredCoverage> {
    fn read<const N: usize>(cursor: &mut std::io::Cursor<&[u8]>) -> Option<[u8; N]> {
        let mut value = [0; N];
        cursor.read_exact(&mut value).ok()?;
        Some(value)
    }
    fn read_vec(cursor: &mut std::io::Cursor<&[u8]>, len: usize) -> Option<Vec<u8>> {
        let mut value = vec![0; len];
        cursor.read_exact(&mut value).ok()?;
        Some(value)
    }

    let mut cursor = std::io::Cursor::new(bytes);
    if &read::<8>(&mut cursor)? != COVERAGE_MAGIC
        || u32::from_le_bytes(read::<4>(&mut cursor)?) != COVERAGE_VERSION
    {
        return None;
    }
    let frontier_len = u32::from_le_bytes(read::<4>(&mut cursor)?) as usize;
    if frontier_len > 1024 * 1024 {
        return None;
    }
    let frontier = read_vec(&mut cursor, frontier_len)?;
    let count = u32::from_le_bytes(read::<4>(&mut cursor)?) as usize;
    if count > 10_000 {
        return None;
    }
    let mut logs = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = u32::from_le_bytes(read::<4>(&mut cursor)?) as usize;
        if name_len == 0 || name_len > 4096 {
            return None;
        }
        let name = String::from_utf8(read_vec(&mut cursor, name_len)?).ok()?;
        let offset = u64::from_le_bytes(read::<8>(&mut cursor)?);
        let digest = read::<32>(&mut cursor)?;
        logs.push((name, offset, digest));
    }
    (cursor.position() == bytes.len() as u64).then_some(StoredCoverage { frontier, logs })
}

/// One workspace's directory: `<profile_dir>/workspaces/<workspace_id>/`.
pub type WorkspaceStore = DocStore<WorkspaceDoc>;

#[cfg(test)]
mod tests;

/// The profile's own directory: `<profile_dir>/profile/`.
///
/// Shaped exactly like a workspace's, and beside rather than above them, so the
/// generic machinery applies unchanged and an older build — which looks only in
/// `workspaces/` — walks straight past it.
pub type ProfileStore = DocStore<ProfileDoc>;

impl WorkspaceStore {
    /// Open (or create) the directory for `id` under `root`.
    pub fn open_at(root: impl AsRef<Path>, id: WorkspaceId, peer: &str) -> Result<Self> {
        Self::open_dir(root.as_ref().join("workspaces").join(id.to_storage()), peer)
    }

    /// Open under an explicitly chosen profile folder.
    ///
    /// There is no variant that picks one for you — the caller has to have
    /// decided first (see `koru_workspace_host::startup`).
    pub fn open(dir: &Path, id: WorkspaceId, peer: &str) -> Result<Self> {
        Self::open_at(dir, id, peer)
    }

    pub fn load(&mut self, id: WorkspaceId, name: &str) -> Result<(WorkspaceDoc, Opened)> {
        self.load_with(|| WorkspaceDoc::create(id, name))
    }
}

impl ProfileStore {
    /// Open (or create) the profile document's directory inside a profile folder.
    pub fn open_at(profile_dir: impl AsRef<Path>, peer: &str) -> Result<Self> {
        Self::open_dir(profile_dir.as_ref().join("profile"), peer)
    }

    pub fn load(
        &mut self,
        id: ProfileId,
        name: &str,
        email: Option<&str>,
    ) -> Result<(ProfileDoc, Opened)> {
        self.load_with(|| ProfileDoc::create(id, name, email))
    }
}
