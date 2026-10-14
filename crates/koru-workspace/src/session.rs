// SPDX-License-Identifier: MPL-2.0

//! Persistence and peer sync, from the editor's side.
//!
//! Two things happen here and nothing else: local changes are exported and
//! appended to this peer's update file, and other peers' files are polled and
//! imported. That is the whole of it — `export` → transport → `import` — which
//! is deliberately the same shape the cloud will use, with the filesystem
//! standing in for the network. See `docs/deferred/`.
//!
//! Going *offline* stops both directions. That is what makes the interesting
//! demo possible: edit the same line in two windows with sync off, turn it back
//! on, and watch both edits survive. See `docs/contributing/operations/RUNBOOK.md`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use koru_core::WorkspaceId;
use koru_core::schema::{Compatibility, WorkspaceDoc};
use koru_docstore::{Opened, SnapshotCoverage, WorkspaceStore};

/// How often to look for other peers' updates **when nobody else is there**.
///
/// Which is almost always: one window on one folder has nothing to poll for,
/// and this is the interval that decides what an idle Koru costs. It only has
/// to be short enough to notice a second window opening.
const POLL_ALONE: Duration = Duration::from_millis(300);

/// …and how often when another peer is live.
///
/// A poll that finds nothing is an opendir and a length per file — 607 µs
/// across a dozen of them, pinned by `a_poll_with_nothing_new_is_cheap`. At
/// this interval that is well under 2% of a core, and it is only paid while
/// somebody else is actually editing.
///
/// It used to be [`POLL_ALONE`] for both, from when a poll meant reading every
/// peer file whole — tens of megabytes — and so had to be rare. That cost is
/// gone ([ADR-45](../../../docs/decisions/README.md)) and the interval that was sized
/// against it should not outlive it.
///
/// Which of the two applies is `WorkspaceStore::peer_live`. *How recently* a
/// peer must have written to count as one is the store's business — it is the
/// thing looking at the directory.
const POLL_LIVE: Duration = Duration::from_millis(50);

/// Snapshots are a compaction, not the source of truth, so they can lag — but
/// only so far. This is the ceiling for somebody typing without pause.
const SNAPSHOT_EVERY: Duration = Duration::from_secs(5);

/// How long after the last edit to write, when there has been a pause.
///
/// The ceiling above is what a crash or a force-quit costs, and five seconds of
/// somebody's work is too much to lose to a hard kill. A pause is the cheap
/// moment to spend the write in: long enough that a burst of typing is one
/// save, short enough that stopping to think is enough to make the work safe.
const IDLE_SAVE: Duration = Duration::from_millis(800);

/// A fixed id so two instances launched with `--peer A` and `--peer B` land in
/// the same workspace directory with no setup. A real build gets this from the
/// profile document; the dev walkthrough should not need one.
const DEV_WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";

pub fn dev_workspace_id() -> WorkspaceId {
    WorkspaceId::parse(DEV_WORKSPACE).expect("the dev workspace id is a literal")
}

/// Everything the caller has to tell the user about how the workspace opened.
///
/// Returned rather than printed, because `eprintln!` on a build with no console
/// attached is indistinguishable from silence — which is how "opened blank over
/// a perfectly good backup" would go unnoticed for a day.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Set when the snapshot could not be used and something else was.
    pub recovery: Option<koru_docstore::Recovery>,
    /// Why this session will not write, if it will not.
    pub read_only: Option<String>,
    /// Peer update files that were skipped because they would not decode.
    pub damaged: Vec<String>,
    /// A genuine first run: nothing was on disk. Not an error — but the caller
    /// needs to know, because every *other* way of arriving at an empty
    /// workspace is one.
    pub fresh: bool,
}

impl Report {
    /// Whether there is anything here worth interrupting the user for.
    pub fn is_quiet(&self) -> bool {
        self.recovery.is_none() && self.read_only.is_none() && self.damaged.is_empty()
    }
}

/// Opening failed outright, and there is nothing sensible to show.
///
/// Deliberately not "carry on in memory": a workspace that cannot be saved
/// looks exactly like one that can, right up until it is closed.
#[derive(Debug)]
pub enum OpenError {
    /// The directory could not be created or read.
    Storage(String),
    /// Nothing on disk, in the backups, or in the peer logs would decode.
    Unreadable(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(e) => write!(f, "the workspace directory could not be opened: {e}"),
            Self::Unreadable(e) => write!(f, "the workspace could not be read: {e}"),
        }
    }
}

pub struct Session {
    store: WorkspaceStore,
    snapshot_worker: SnapshotWorker,
    /// Accepted jobs whose result has not yet been observed by this session.
    snapshot_pending: usize,
    pub peer: String,
    /// When false, nothing is sent and nothing is read — the offline half of the
    /// convergence demo.
    pub online: bool,
    /// Set when the document was written by a build newer than this one. Every
    /// write path checks it; see [`Compatibility`].
    read_only: Option<String>,
    /// Consecutive failed snapshot writes, and the last thing the OS said.
    /// Reset by any save that lands. See [`Session::write_failed`].
    write_failures: u32,
    last_write_error: Option<String>,
    last_poll: Instant,
    last_snapshot: Instant,
    /// True when the checkpoint lags the in-memory document. Update frames are
    /// already durable; this only decides whether compaction has work to do.
    snapshot_dirty: bool,
    /// When the last local edit landed, for the idle save.
    last_edit: Option<Instant>,
    /// Set by a local edit, cleared once the change reaches the update file.
    unsent: bool,
}

impl Session {
    /// Open the workspace `id` for `peer` under `dir`, loading whatever is
    /// already on disk.
    ///
    /// `dir` is passed in rather than looked up: which folder holds your
    /// content is a decision with a user in it (see `koru_workspace_host::startup`),
    /// and this function is not where that decision belongs. `id` and `name`
    /// come from the profile's workspace list for the same reason — which
    /// workspace to open is a decision, and it used to be a constant.
    ///
    /// `name` is only consumed when there is nothing on disk and a fresh
    /// document is created, so passing the profile entry's name makes the two
    /// agree from the first moment rather than after the first rename.
    pub fn open(
        dir: &Path,
        id: WorkspaceId,
        name: &str,
        peer: &str,
    ) -> Result<(Self, WorkspaceDoc, Report), OpenError> {
        let mut store =
            WorkspaceStore::open(dir, id, peer).map_err(|e| OpenError::Storage(e.to_string()))?;
        let (doc, opened) = store
            .load(id, name)
            .map_err(|e| OpenError::Unreadable(e.to_string()))?;

        let mut report = Report {
            damaged: store.damaged().to_vec(),
            ..Default::default()
        };
        match opened {
            Opened::Fresh => report.fresh = true,
            Opened::Normal => {}
            Opened::Recovered(r) => report.recovery = Some(r),
        }
        // A build that does not understand the document must not rewrite it.
        // Checked here rather than at each write, so there is one place the
        // answer comes from and one place to read to know what it is.
        if let Compatibility::TooNew { .. } = doc.compatibility() {
            report.read_only = Some(doc.compatibility().to_string());
        }

        let now = Instant::now();
        let snapshot_worker = SnapshotWorker::new(store.snapshot_writer());
        let session = Self {
            store,
            snapshot_worker,
            snapshot_pending: 0,
            peer: peer.to_string(),
            online: true,
            read_only: report.read_only.clone(),
            write_failures: 0,
            last_write_error: None,
            last_poll: now,
            last_snapshot: now,
            // Loading may have folded update tails above the checkpoint. One
            // initial compaction is useful; after that, quiet means quiet.
            snapshot_dirty: true,
            last_edit: None,
            unsent: false,
        };
        Ok((session, doc, report))
    }

    /// Why this session will not write, if it will not.
    pub fn read_only(&self) -> Option<&str> {
        self.read_only.as_deref()
    }

    /// The directory this session's content is in — for a status line, and for
    /// telling the user where to look when something has gone wrong.
    pub fn root(&self) -> &Path {
        self.store.root()
    }

    /// Every backup of this workspace, newest first.
    pub fn backups(&self) -> Vec<PathBuf> {
        self.store.backups()
    }

    /// Note that the document changed locally. Cheap — the export happens on
    /// the next tick, so a burst of keystrokes becomes one update frame.
    pub fn touch(&mut self) {
        self.unsent = true;
        self.snapshot_dirty = true;
        self.last_edit = Some(Instant::now());
    }

    pub fn toggle_online(&mut self, doc: &WorkspaceDoc) {
        self.online = !self.online;
        if self.online {
            // Everything made while offline goes out in one frame.
            self.publish(doc);
        }
    }

    /// A label for the status bar: what this peer is and whether it is sharing.
    ///
    /// Read-only is announced here as well as in the dialog on open, because a
    /// dialog is dismissed and then forgotten — and the whole point is that you
    /// find out before an afternoon's typing, not after.
    pub fn label(&self) -> String {
        let sharing = if self.online { "synced" } else { "offline" };
        match self.read_only {
            Some(_) => format!("read-only   F5 {sharing}   peer {}", self.peer),
            None => format!("F5 {sharing}   peer {}", self.peer),
        }
    }

    /// The content-addressed blob store. Images and other large bytes live here,
    /// never in the CRDT — see ADR-12.
    pub fn blobs(&self) -> koru_docstore::BlobStore {
        self.store.blobs()
    }

    /// Send anything local that has not been sent yet. The store tracks what it
    /// has already written, so this is an incremental export, not a rewrite.
    fn publish(&mut self, doc: &WorkspaceDoc) {
        if self.read_only.is_some() {
            return;
        }
        match self.store.publish_new(doc) {
            Ok(_) => self.unsent = false,
            Err(e) => eprintln!("warning: could not publish updates ({e})"),
        }
    }

    /// How often to look for other peers' work, given whether there are any.
    fn poll_every(&self) -> Duration {
        if self.store.peer_live() {
            POLL_LIVE
        } else {
            POLL_ALONE
        }
    }

    /// When [`tick`] next has something to do.
    ///
    /// **The event loop has to be told this or the intervals here are
    /// fiction.** The loop woke on a fixed timer of its own and called `tick`
    /// whenever it happened to be up, so a 300 ms poll checked at 177 ms
    /// intervals actually ran every 353 ms — and an edit waiting to be
    /// published sat for up to a wake before it was written at all. Both halves
    /// of that were latency a peer could feel.
    ///
    /// `None` when there is nothing pending and nothing to wait for, which
    /// leaves the caller free to sleep as long as it likes.
    ///
    /// [`tick`]: Self::tick
    pub fn next_due(&self) -> Option<Instant> {
        let mut due: Option<Instant> = None;
        let mut soonest = |t: Instant| {
            due = Some(due.map_or(t, |d: Instant| d.min(t)));
        };
        if self.online {
            // Unsent work is due *now*: the export and the append are what a
            // peer is waiting for, and they are not on the keystroke path
            // precisely so the loop can decide when to spend them.
            if self.unsent {
                soonest(Instant::now());
            }
            soonest(self.last_poll + self.poll_every());
        }
        if self.snapshot_dirty {
            if let Some(edit) = self.last_edit {
                soonest(edit + IDLE_SAVE);
            }
            soonest(self.last_snapshot + SNAPSHOT_EVERY);
        }
        due
    }

    /// One turn of the loop: push local changes, pull remote ones, and compact
    /// to a snapshot now and then. Returns true if `doc` changed underneath the
    /// caller, which means the rows have to be rebuilt.
    pub fn tick(&mut self, doc: &mut WorkspaceDoc) -> bool {
        let now = Instant::now();
        let mut changed = false;
        self.drain_snapshot_results();

        // Sharing decides what is *sent and received*, and nothing else. An
        // early return here meant an offline session never snapshotted at all,
        // so pressing F5 quietly turned off saving until the window closed.
        if self.online {
            if self.unsent {
                let _s = koru_core::timing::Watch::start("session.publish");
                self.publish(doc);
            }
            if now.duration_since(self.last_poll) >= self.poll_every() {
                self.last_poll = now;
                let _s = koru_core::timing::Watch::start("session.sync_in");
                match self.store.sync_in(doc) {
                    Ok(c) => {
                        changed = c;
                        self.snapshot_dirty |= c;
                    }
                    Err(e) => eprintln!("warning: could not read peer updates ({e})"),
                }
            }
        }

        // Whichever comes first: a pause in the typing, or the ceiling.
        let idle = self
            .last_edit
            .is_some_and(|t| now.duration_since(t) >= IDLE_SAVE);
        if self.snapshot_dirty && (idle || now.duration_since(self.last_snapshot) >= SNAPSHOT_EVERY)
        {
            self.last_snapshot = now;
            self.last_edit = None;
            let _s = koru_core::timing::Watch::start("session.save");
            if self.queue_snapshot(doc) {
                self.snapshot_dirty = false;
            }
        }
        changed
    }

    /// Compact everything seen so far into the snapshot. Explicit callers use
    /// this on close and workspace switches, so it waits until every accepted
    /// background compaction has landed before returning.
    pub fn save(&mut self, doc: &WorkspaceDoc) {
        if self.read_only.is_some() {
            return;
        }
        if self.online && self.unsent {
            self.publish(doc);
        }
        self.snapshot_dirty = false;
        self.drain_snapshot_results();
        let job = self.snapshot_job(doc);
        if self.snapshot_worker.send(job).is_err() {
            self.save_sync(doc);
            return;
        }
        self.snapshot_pending += 1;
        while self.snapshot_pending > 0 {
            let Some(result) = self.snapshot_worker.recv() else {
                self.snapshot_pending = 0;
                self.record_snapshot_result(Err("the snapshot worker stopped unexpectedly".into()));
                break;
            };
            self.snapshot_pending -= 1;
            self.record_snapshot_result(result);
        }
        if self.write_failures == 0 {
            // The final FIFO result belongs to the current explicit save, so
            // an older failed background job must not leave it looking dirty.
            self.snapshot_dirty = false;
        }
    }

    /// The narrow fallback when the OS could not create a worker thread.
    /// Normal periodic and explicit saves both use the serial worker.
    fn save_sync(&mut self, doc: &WorkspaceDoc) {
        // A build that does not understand this document must not re-export it.
        // Loro would not lose the newer build's data, but *this* build has no
        // idea it is there, and a full re-export is exactly the write that
        // makes that matter. See `Compatibility`.
        if self.read_only.is_some() {
            return;
        }
        // Publish first: a snapshot that is ahead of what peers have been told
        // would leave them permanently behind.
        if self.online && self.unsent {
            self.publish(doc);
        }
        match self.store.save_snapshot(doc) {
            Ok(_) => {
                self.write_failures = 0;
                self.last_write_error = None;
                self.snapshot_dirty = false;
            }
            Err(e) => {
                // **`eprintln!` is not a report.** Under `windows_subsystem`
                // there is no console, so on its own this is silence — and a
                // workspace that has quietly stopped saving looks exactly like
                // one that is saving until it is closed. The counter is what
                // turns that into something askable.
                eprintln!("warning: could not write the snapshot ({e})");
                self.write_failures = self.write_failures.saturating_add(1);
                self.last_write_error = Some(e.to_string());
                self.snapshot_dirty = true;
            }
        }
    }

    /// Freeze the state cheaply and let the serial worker do Loro export, file
    /// replacement, backup rotation and pruning away from the event loop.
    fn queue_snapshot(&mut self, doc: &WorkspaceDoc) -> bool {
        let job = self.snapshot_job(doc);
        match self.snapshot_worker.try_send(job) {
            Ok(()) => {
                self.snapshot_pending += 1;
                true
            }
            Err(TrySendError::Full(_)) => {
                // One frozen state is already waiting behind the active write.
                // Update frames remain canonical, so another redundant
                // compaction is unnecessary; the next cadence catches up.
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.save_sync(doc);
                !self.snapshot_dirty
            }
        }
    }

    fn snapshot_job(&self, doc: &WorkspaceDoc) -> SnapshotJob {
        SnapshotJob {
            doc: doc.fork_for_read(),
            coverage: self.store.snapshot_coverage(),
        }
    }

    fn drain_snapshot_results(&mut self) {
        while let Ok(result) = self.snapshot_worker.try_recv() {
            self.snapshot_pending = self.snapshot_pending.saturating_sub(1);
            self.record_snapshot_result(result);
        }
    }

    fn record_snapshot_result(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.write_failures = 0;
                self.last_write_error = None;
            }
            Err(error) => {
                // `eprintln!` is not a report under `windows_subsystem`; the
                // counter turns repeated failures into visible app state.
                eprintln!("warning: could not write the snapshot ({error})");
                self.write_failures = self.write_failures.saturating_add(1);
                self.last_write_error = Some(error);
                self.snapshot_dirty = true;
            }
        }
    }

    /// Peer logs that would not decode. Their content is intact in the
    /// snapshot; the damaged files were quarantined.
    pub fn damaged(&self) -> &[String] {
        self.store.damaged()
    }

    /// Whether another instance has written to this workspace recently.
    pub fn peer_live(&self) -> bool {
        self.store.peer_live()
    }

    /// Why this workspace can no longer be written, once it has failed enough
    /// times to mean it.
    ///
    /// `None` while saves are landing. One failure is a fluke — a file locked by
    /// a virus scanner for a moment, a network drive blinking — and shouting
    /// about it would train people to dismiss the shout. Several in a row is a
    /// workspace whose folder has gone: unplugged, renamed, or moved out from
    /// under a second window by a delete in the first.
    ///
    /// Deliberately *not* cleared by `read_only`, which is a different state
    /// with a different cause: that one is "an older build must not rewrite
    /// this", and it never tried to save at all.
    pub fn write_failed(&self) -> Option<&str> {
        (self.write_failures >= WRITE_FAILURES_BEFORE_REPORTING)
            .then(|| self.last_write_error.as_deref())
            .flatten()
    }
}

struct SnapshotJob {
    doc: WorkspaceDoc,
    coverage: SnapshotCoverage,
}

/// One serial compaction lane per session. A bounded queue prevents a slow
/// filesystem from accumulating old snapshots while edits continue.
struct SnapshotWorker {
    jobs: Option<SyncSender<SnapshotJob>>,
    results: Receiver<Result<(), String>>,
    handle: Option<JoinHandle<()>>,
}

impl SnapshotWorker {
    fn new(mut store: WorkspaceStore) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::sync_channel::<SnapshotJob>(1);
        let (results_tx, results) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("koru-snapshot".into())
            .spawn(move || {
                while let Ok(job) = jobs_rx.recv() {
                    let result = store
                        .save_covered_snapshot(&job.doc, job.coverage)
                        .map_err(|error| error.to_string());
                    if results_tx.send(result).is_err() {
                        break;
                    }
                }
            });
        match handle {
            Ok(handle) => Self {
                jobs: Some(jobs_tx),
                results,
                handle: Some(handle),
            },
            Err(error) => {
                eprintln!("warning: could not start the snapshot worker ({error})");
                Self {
                    jobs: None,
                    results,
                    handle: None,
                }
            }
        }
    }

    fn try_send(&self, job: SnapshotJob) -> Result<(), TrySendError<SnapshotJob>> {
        let Some(jobs) = &self.jobs else {
            return Err(TrySendError::Disconnected(job));
        };
        jobs.try_send(job)
    }

    fn send(&self, job: SnapshotJob) -> Result<(), SnapshotJob> {
        let Some(jobs) = &self.jobs else {
            return Err(job);
        };
        jobs.send(job).map_err(|error| error.0)
    }

    fn try_recv(&self) -> Result<Result<(), String>, mpsc::TryRecvError> {
        self.results.try_recv()
    }

    fn recv(&self) -> Option<Result<(), String>> {
        self.results.recv().ok()
    }
}

impl Drop for SnapshotWorker {
    fn drop(&mut self) {
        // Closing the channel lets the worker finish accepted work and exit.
        drop(self.jobs.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// How many saves in a row have to fail before it is reported. Three at the
/// idle-save cadence is a couple of seconds of a workspace not being written,
/// which is long enough to be real and short enough to still be news.
const WRITE_FAILURES_BEFORE_REPORTING: u32 = 3;

/// What an instance launched without `--peer` calls its window slot.
pub const DEFAULT_PEER: &str = "default";

/// Who this process is, on disk.
///
/// **Two names, because `--peer` was doing two jobs.** A *peer* names a replica:
/// it decides which `updates/peer-<name>.loro` this process appends to, and
/// which temp files it writes. A *slot* names a window: it decides which
/// `settings-<name>.conf` holds the geometry, the tabs and the zoom.
///
/// They used to be one string, which was fine while a second window meant
/// `--peer B`. It stops being fine now that switching profiles opens windows
/// routinely: every window needs its **own** update file, or two of them append
/// to one and the framing has no lock to protect it — but they should share the
/// *geometry*, or opening a second profile would reset the first one's window.
///
/// `--peer A` still pins both, so the two-peer walkthrough is unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    /// Names `updates/peer-<peer>.loro`, `snapshot.<peer>.part` and the blob
    /// temp files. Unique per launch unless `--peer` pinned it.
    pub peer: String,
    /// Names `settings-<slot>.conf`. [`DEFAULT_PEER`] unless `--peer` pinned it.
    pub slot: String,
}

impl Instance {
    pub fn from_args() -> Self {
        Self::from(std::env::args())
    }

    /// The testable half of [`Instance::from_args`].
    pub fn from<I: IntoIterator<Item = String>>(args: I) -> Self {
        match peer_flag(args) {
            // Named: both, exactly as before. `--peer A` and `--peer B` on one
            // folder is still the convergence walkthrough, unchanged.
            Some(name) => Self {
                peer: name.clone(),
                slot: name,
            },
            None => Self {
                peer: auto_peer(),
                slot: DEFAULT_PEER.into(),
            },
        }
    }
}

/// The `--peer <name>` flag, if it was given.
fn peer_flag<I: IntoIterator<Item = String>>(args: I) -> Option<String> {
    let mut args = args.into_iter().skip(1);
    while let Some(arg) = args.next() {
        if let Some(name) = arg.strip_prefix("--peer=") {
            return Some(sanitise(name));
        }
        if arg == "--peer" {
            return Some(sanitise(&args.next()?));
        }
    }
    None
}

/// A replica name for a process that was not given one: `i<pid><clock>`.
///
/// **Not remembered, on purpose.** The peer names a transport file, not an
/// identity — remembering it would reinstate exactly the collision it removes,
/// since two windows launched from the same shortcut would remember the same
/// name. The clock is mixed in because the operating system reuses process ids,
/// and a reused id would silently resume somebody else's update file.
///
/// The `i` prefix marks it as auto-named. **Nothing keys on that any more.**
/// An early draft of `prune_stale_peers` did, on the reasoning that a
/// launch-named replica is finished once its work is in the snapshot — but a
/// second window open on this folder right now has an auto-named file too, and
/// a running peer learns only from logs. Pruning goes by the clock instead
/// ([ADR-45](../../../docs/decisions/README.md)), which answers the question the name
/// only seemed to.
fn auto_peer() -> String {
    let pid = std::process::id();
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // 16 bits of the clock is plenty against a pid that has just been reused,
    // and keeps the file name short enough to read in a directory listing.
    sanitise(&format!("i{pid:x}{:04x}", clock & 0xffff))
}

/// The `--peer <name>` flag: which update file this instance writes, and which
/// settings file it keeps. Defaults to [`DEFAULT_PEER`].
///
/// Kept for callers that only care about the window slot. New code should ask
/// for an [`Instance`], which distinguishes the two.
pub fn peer_from_args() -> String {
    peer_from(std::env::args())
}

/// The testable half of [`peer_from_args`].
pub fn peer_from<I: IntoIterator<Item = String>>(args: I) -> String {
    peer_flag(args).unwrap_or_else(|| DEFAULT_PEER.into())
}

/// The name becomes a file name, so keep it to something that always is one.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect();
    if cleaned.is_empty() {
        DEFAULT_PEER.into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_core::node::Node;
    use koru_core::schema::Parent;

    /// A session on a scratch directory.
    ///
    /// This touches disk, which view tests must not — but a `Session` *is* the
    /// thing that owns a directory, and testing when it next wants servicing
    /// without one would be testing a mock.
    fn scratch_session(tag: &str) -> (Session, WorkspaceDoc) {
        let dir = std::env::temp_dir().join(format!(
            "koru-session-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (session, doc, _) = Session::open(&dir, WorkspaceId::new(), "t", "me").unwrap();
        (session, doc)
    }

    /// **Unsent work is due immediately.**
    ///
    /// The export and the append are deliberately off the keystroke path, so
    /// the loop decides when to spend them — and it can only decide well if it
    /// is told. While it waited on a clock of its own, an edit sat for up to a
    /// wake before it was even written, which is latency the peer at the other
    /// window feels for nothing.
    #[test]
    fn an_unsent_edit_is_due_at_once() {
        let (mut session, _doc) = scratch_session("unsent");
        let idle = session
            .next_due()
            .expect("a session always has a next tick");
        assert!(
            idle > Instant::now(),
            "with nothing pending the next tick is in the future"
        );

        session.touch();
        let due = session.next_due().expect("still due");
        assert!(
            due <= Instant::now(),
            "an edit waiting to go out should not wait for a timer"
        );
    }

    /// With nothing pending, the next tick is the poll — and on a folder with
    /// no other peer that is the slow one. This is what an idle window costs.
    #[test]
    fn a_quiet_session_waits_for_the_slow_poll() {
        let (session, _doc) = scratch_session("quiet");
        assert_eq!(
            session.poll_every(),
            POLL_ALONE,
            "nobody else is here, so there is nothing to poll quickly for"
        );
        let due = session
            .next_due()
            .expect("a session always has a next tick");
        let waiting = due.saturating_duration_since(Instant::now());
        assert!(
            waiting <= POLL_ALONE,
            "waited {waiting:?}, which is longer than the poll it is waiting for"
        );
    }

    #[test]
    fn dropping_a_session_finishes_an_accepted_snapshot() {
        let (mut session, mut doc) = scratch_session("snapshot-worker");
        let node = doc
            .insert(Parent::Root, Node::task("landed").unwrap())
            .unwrap();
        let root = session.root().to_path_buf();
        let _ = session.queue_snapshot(&doc);
        drop(session);

        let loaded = WorkspaceDoc::from_snapshot(
            &std::fs::read(root.join("snapshot.loro")).expect("worker wrote the snapshot"),
        )
        .unwrap();
        assert_eq!(loaded.read(node).unwrap().content.as_text(), Some("landed"));
        let _ = std::fs::remove_dir_all(root.parent().and_then(Path::parent).unwrap());
    }

    #[test]
    fn a_clean_offline_session_has_no_compaction_timer() {
        let (mut session, doc) = scratch_session("clean-cadence");
        session.save(&doc);
        session.online = false;
        assert!(
            session.next_due().is_none(),
            "a five-second ceiling is only needed while the checkpoint is dirty"
        );
    }

    #[test]
    fn the_dev_workspace_id_parses() {
        // A bad literal would panic at startup rather than here, which is a
        // miserable way to find out.
        assert_eq!(dev_workspace_id(), dev_workspace_id());
    }

    #[test]
    fn a_peer_name_is_always_a_usable_file_name() {
        assert_eq!(sanitise("A"), "A");
        assert_eq!(sanitise("work-2"), "work-2");
        assert_eq!(sanitise("../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitise(""), DEFAULT_PEER);
        assert_eq!(sanitise("!!!"), DEFAULT_PEER);
        assert_eq!(sanitise(&"x".repeat(80)).len(), 24);
    }

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("koru.exe")
            .chain(list.iter().copied())
            .map(str::to_string)
            .collect()
    }

    /// The slot name is a file name on disk, so changing the default renames the
    /// file somebody's window geometry is in. Pinned so it is a deliberate act.
    ///
    /// **This test changed when `--peer` split in two.** It used to assert that
    /// an unnamed instance was `peer == "default"`; it now asserts the *slot* is,
    /// because an unnamed instance gets its own replica name so that two windows
    /// never append to one update file. The rename it exists to make deliberate
    /// is the settings file's, and that is unchanged.
    #[test]
    fn an_instance_without_the_flag_is_the_default_peer() {
        assert_eq!(DEFAULT_PEER, "default");
        assert_eq!(Instance::from(args(&[])).slot, "default");
        assert_eq!(Instance::from(args(&["--profile", "/w"])).slot, "default");
        assert_eq!(Instance::from(args(&["--peer", "A"])).slot, "A");
        assert_eq!(Instance::from(args(&["--peer=B"])).slot, "B");
    }

    /// Switching profiles opens windows routinely now, so two of them sharing an
    /// update file has to be impossible rather than unlikely — the framed append
    /// has no lock behind it.
    #[test]
    fn an_instance_without_the_flag_gets_its_own_peer_but_the_default_slot() {
        let i = Instance::from(args(&[]));
        assert_eq!(i.slot, DEFAULT_PEER, "windows share their geometry");
        assert_ne!(i.peer, DEFAULT_PEER, "but never their update file");
        assert!(i.peer.starts_with('i'), "auto-named: {}", i.peer);
    }

    /// `--peer A` / `--peer B` is the convergence walkthrough, and it has to keep
    /// meaning exactly what `docs/contributing/operations/RUNBOOK.md` says it means.
    #[test]
    fn the_peer_flag_still_pins_both() {
        let a = Instance::from(args(&["--peer", "A"]));
        assert_eq!(a.peer, "A");
        assert_eq!(a.slot, "A");
        let b = Instance::from(args(&["--peer=B"]));
        assert_eq!((b.peer.as_str(), b.slot.as_str()), ("B", "B"));
        // And the old accessor still answers about the slot.
        assert_eq!(peer_from(args(&["--peer", "A"])), "A");
    }

    /// The pid is the same within one process, so the clock is what has to carry
    /// the difference — otherwise two launches in one second would collide.
    #[test]
    fn two_auto_peers_in_a_row_differ() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            seen.insert(auto_peer());
            // The clock is nanoseconds; a spin is enough to move it.
            std::hint::spin_loop();
        }
        assert!(seen.len() > 1, "all identical: {seen:?}");
    }

    /// It becomes a file name, so it has to always be one — the same guarantee
    /// `sanitise` gives a name somebody typed.
    #[test]
    fn an_auto_peer_name_is_always_a_usable_file_name() {
        let name = auto_peer();
        assert!(!name.is_empty());
        assert!(name.len() <= 24, "{name}");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{name}"
        );
    }
}
