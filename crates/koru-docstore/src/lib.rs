// SPDX-License-Identifier: MPL-2.0

//! Where a profile and its workspaces live on disk, and how two replicas find
//! each other.
//!
//! ```text
//! <profile_dir>/                  one profile is one folder
//!   profile.conf                  the folder says whose it is, without a CRDT
//!   profile/                      the profile document, shaped like any other
//!     snapshot.loro
//!     updates/ backups/ quarantine/ blobs/
//!   workspaces/<workspace_id>/
//!     workspace.conf              the folder says what it is called, and looks like
//!     snapshot.loro               the whole document — sufficient on its own
//!     snapshot.<peer>.part        transient, and named per instance
//!     updates/
//!       peer-<id>.loro            append-only, one file per peer
//!     backups/
//!       snapshot-<stamp>.loro     the snapshot as earlier builds left it
//!     quarantine/
//!       <stamp>/                  copies of anything that would not decode
//!     blobs/
//!       <hash>                    content-addressed; never inside the CRDT
//! ```
//!
//! The two documents get the **same** machinery, not two copies of it — see
//! [`docstore`]. `profile/` sits *beside* `workspaces/` rather than above it so
//! that a build predating profiles, which looks only in `workspaces/`, opens the
//! same folder and walks straight past.
//!
//! Which `<profile_dir>` is decided at startup, never guessed.
//!
//! **Dev sync is deliberately the dumbest thing that exercises the real path.**
//! Each peer appends its own changes to its own file and reads everyone else's.
//! Append-only and one file per peer means no clobbering and no locking — two
//! processes never write the same bytes. It is the same
//! export → transport → import cycle the cloud will use, with the filesystem
//! standing in for the network, so the code that survives is the code that
//! matters. See `docs/deferred/`.
//!
//! **Nothing here ever opens empty by accident.** Loading says which of the
//! several things happened — first run, normal open, or recovery — and the
//! caller is expected to tell the user about the last one. Silently starting on
//! a blank workspace looks exactly like a fresh install, and a day of typing
//! into one is not recoverable.

use std::path::PathBuf;

pub mod backup;
pub mod blobs;
pub mod docstore;
pub mod profilefolder;
pub mod readonly;
pub mod registry;
pub mod trash;

pub use blobs::BlobStore;
pub use docstore::{Doc, DocStore, ProfileStore, SnapshotCoverage, WorkspaceStore};
pub use profilefolder::FolderKind;
pub use registry::ProfileRecord;

#[derive(Debug)]
pub enum StoreIoError {
    Io(std::io::Error),
    Core(koru_core::StoreError),
    /// No writable data directory could be determined.
    NoDataDir,
}

impl From<std::io::Error> for StoreIoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<koru_core::StoreError> for StoreIoError {
    fn from(e: koru_core::StoreError) -> Self {
        Self::Core(e)
    }
}

impl std::fmt::Display for StoreIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Core(e) => write!(f, "{e}"),
            Self::NoDataDir => write!(f, "no writable data directory"),
        }
    }
}

impl std::error::Error for StoreIoError {}

pub type Result<T> = std::result::Result<T, StoreIoError>;

/// How the document that was opened came to be — and, when it was not the
/// ordinary way, everything the user needs to be told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// Nothing was on disk at all. A genuine first run, and the only case in
    /// which an empty workspace is the right answer.
    Fresh,
    /// Read from `snapshot.loro`, plus whatever the peer logs added.
    Normal,
    /// The snapshot would not decode, or was missing while backups existed.
    Recovered(Recovery),
}

/// What happened on the recovery path. Every field is here to be shown: the
/// point of the exercise is that a recovery is never quiet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    /// Why the live snapshot could not be used.
    pub problem: String,
    /// Where the unreadable file was **copied** to. Copied rather than moved,
    /// so a failure inside recovery cannot destroy the thing being recovered.
    /// `None` when there was no file to keep.
    pub quarantined: Option<PathBuf>,
    /// The backup this document came from. `None` means there was no usable
    /// backup and the document was rebuilt from the peer update logs alone.
    pub from: Option<PathBuf>,
}

/// The safety net, and the reason [`docstore`] was generalised rather than
/// copied.
///
/// Every test below predates the profile document and is here **unchanged**.
/// They are the only exercise the recovery, quarantine and backup-rotation paths
/// ever get, and a refactor that quietly rewrote them would have removed the
/// evidence along with the risk.
#[cfg(test)]
mod tests {
    use super::*;
    use koru_core::WorkspaceId;
    use koru_core::node::Node;
    use koru_core::schema::{Parent, WorkspaceDoc};
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("koru-docstore-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Most tests here care about the document, not how it was obtained; the
    /// ones that do care assert on `load` directly.
    fn doc_of(store: &mut WorkspaceStore, id: WorkspaceId) -> WorkspaceDoc {
        store.load(id, "acme").unwrap().0
    }

    #[test]
    fn a_snapshot_round_trips_through_disk() {
        let root = temp_root("snapshot");
        let id = WorkspaceId::new();
        let mut store = WorkspaceStore::open_at(&root, id, "a").unwrap();

        let (mut doc, opened) = store.load(id, "acme").unwrap();
        assert_eq!(opened, Opened::Fresh, "nothing was on disk");
        let task = doc
            .insert(Parent::Root, Node::task("Reward flow").unwrap())
            .unwrap();
        store.save_snapshot(&doc).unwrap();

        let mut reopened = WorkspaceStore::open_at(&root, id, "a").unwrap();
        let (loaded, opened) = reopened.load(id, "acme").unwrap();
        assert_eq!(opened, Opened::Normal, "and now it is not a first run");
        assert_eq!(
            loaded.read(task).unwrap().content.as_text(),
            Some("Reward flow")
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The scenario the two-window walkthrough exercises.
    #[test]
    fn two_peers_converge_through_the_updates_directory() {
        let root = temp_root("twopeers");
        let id = WorkspaceId::new();

        let mut store_a = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut a = doc_of(&mut store_a, id);
        let shared = a
            .insert(Parent::Root, Node::task("payout flow").unwrap())
            .unwrap();
        store_a.publish(&a.export_all_updates().unwrap()).unwrap();

        // B starts fresh and catches up purely from the update log.
        let mut store_b = WorkspaceStore::open_at(&root, id, "B").unwrap();
        let (b, opened) = store_b.load(id, "acme").unwrap();
        let mut b = b;
        assert_eq!(
            opened,
            Opened::Normal,
            "a peer log on disk is not a first run"
        );
        assert_eq!(
            b.read(shared).unwrap().content.as_text(),
            Some("payout flow"),
            "B rebuilt from A's updates with no snapshot"
        );

        // Both edit offline.
        a.set_text(shared, "reward payout flow").unwrap();
        b.set_done(shared, true).unwrap();
        store_a.publish(&a.export_all_updates().unwrap()).unwrap();
        store_b.publish(&b.export_all_updates().unwrap()).unwrap();

        // And converge.
        store_a.sync_in(&mut a).unwrap();
        store_b.sync_in(&mut b).unwrap();

        let na = a.read(shared).unwrap();
        let nb = b.read(shared).unwrap();
        assert_eq!(na.content.as_text(), Some("reward payout flow"));
        assert_eq!(na.content.as_text(), nb.content.as_text(), "replicas agree");
        assert!(na.meta.done && nb.meta.done, "both edits survived");

        let _ = fs::remove_dir_all(&root);
    }

    /// The live path: each peer publishes only what it has learned since last
    /// time, so an hour of editing does not rewrite an hour of history.
    #[test]
    fn publish_new_sends_only_the_delta() {
        let root = temp_root("delta");
        let id = WorkspaceId::new();
        let mut store = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut doc = doc_of(&mut store, id);

        let task = doc
            .insert(Parent::Root, Node::task("one").unwrap())
            .unwrap();
        assert!(store.publish_new(&doc).unwrap());
        let after_first = fs::metadata(
            root.join("workspaces")
                .join(id.to_storage())
                .join("updates")
                .join("peer-A.loro"),
        )
        .unwrap()
        .len();

        // Nothing changed, so nothing is written.
        assert!(!store.publish_new(&doc).unwrap(), "no change, no bytes");

        doc.set_text(task, "one more").unwrap();
        assert!(store.publish_new(&doc).unwrap());
        let after_second = fs::metadata(
            root.join("workspaces")
                .join(id.to_storage())
                .join("updates")
                .join("peer-A.loro"),
        )
        .unwrap()
        .len();
        assert!(after_second > after_first);
        assert!(
            after_second < after_first * 2,
            "the second frame is a delta, not the whole history again"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The offline-merge demo, end to end through the incremental path.
    #[test]
    fn two_peers_converge_through_incremental_publishes() {
        let root = temp_root("incremental");
        let id = WorkspaceId::new();

        let mut store_a = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut a = doc_of(&mut store_a, id);
        let shared = a
            .insert(Parent::Root, Node::task("payout").unwrap())
            .unwrap();
        store_a.publish_new(&a).unwrap();

        let mut store_b = WorkspaceStore::open_at(&root, id, "B").unwrap();
        let mut b = doc_of(&mut store_b, id);
        assert_eq!(b.read(shared).unwrap().content.as_text(), Some("payout"));

        // Both edit the same line while "offline".
        a.insert_text(shared, 0, "reward ").unwrap();
        b.insert_text(shared, b.text_len(shared).unwrap(), " flow")
            .unwrap();
        store_a.publish_new(&a).unwrap();
        store_b.publish_new(&b).unwrap();

        // ...and back online.
        store_a.sync_in(&mut a).unwrap();
        store_b.sync_in(&mut b).unwrap();

        let text = a
            .read(shared)
            .unwrap()
            .content
            .as_text()
            .unwrap()
            .to_string();
        assert_eq!(text, b.read(shared).unwrap().content.as_text().unwrap());
        assert!(text.contains("reward") && text.contains("flow"), "{text}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn polling_again_with_no_new_bytes_is_a_no_op() {
        let root = temp_root("noop");
        let id = WorkspaceId::new();
        let mut store_a = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut a = doc_of(&mut store_a, id);
        a.insert(Parent::Root, Node::task("one").unwrap()).unwrap();
        store_a.publish(&a.export_all_updates().unwrap()).unwrap();

        let mut store_b = WorkspaceStore::open_at(&root, id, "B").unwrap();
        let mut b = doc_of(&mut store_b, id);
        assert!(!store_b.sync_in(&mut b).unwrap(), "nothing new to read");
        assert_eq!(b.walk().unwrap().len(), 1, "and nothing duplicated");

        let _ = fs::remove_dir_all(&root);
    }

    /// A background compactor freezes before a newer peer append by design.
    /// The checkpoint must not become "the truth": reopening combines it with
    /// the update tail and reaches the newer state.
    #[test]
    fn a_covered_snapshot_preserves_updates_appended_after_its_fork() {
        let root = temp_root("covered-snapshot");
        let id = WorkspaceId::new();
        let mut writer = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut current = doc_of(&mut writer, id);
        let task = current
            .insert(Parent::Root, Node::task("before").unwrap())
            .unwrap();
        writer.publish_new(&current).unwrap();

        let mut reader = WorkspaceStore::open_at(&root, id, "B").unwrap();
        let frozen = doc_of(&mut reader, id).fork_for_read();
        let coverage = reader.snapshot_coverage();

        current.set_text(task, "after").unwrap();
        writer.publish_new(&current).unwrap();
        reader
            .snapshot_writer()
            .save_covered_snapshot(&frozen, coverage)
            .unwrap();

        let mut reopened = WorkspaceStore::open_at(&root, id, "C").unwrap();
        let loaded = doc_of(&mut reopened, id);
        assert_eq!(
            loaded.read(task).unwrap().content.as_text(),
            Some("after"),
            "the newer update tail is applied above the older checkpoint"
        );
        let _ = fs::remove_dir_all(&root);
    }

    // ------------------------------------------------------- backup / recovery

    /// The safety net, end to end: a build opens the workspace, the snapshot is
    /// then destroyed, and the next open comes back with the content — saying
    /// so, and leaving the wreckage where it can be looked at.
    #[test]
    fn an_unreadable_snapshot_recovers_from_the_backup() {
        let root = temp_root("recover");
        let id = WorkspaceId::new();

        // Session one writes something and saves twice: the first save takes
        // the backup (of nothing), the second backs up the real content.
        let mut store = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut doc = doc_of(&mut store, id);
        let task = doc
            .insert(Parent::Root, Node::task("the only copy").unwrap())
            .unwrap();
        store.save_snapshot(&doc).unwrap();
        // A second session, so the once-per-session rule fires again.
        let mut store = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let (doc, _) = store.load(id, "acme").unwrap();
        store.save_snapshot(&doc).unwrap();
        assert_eq!(store.backups().len(), 1, "one copy of the live snapshot");

        // Now ruin it, the way a bad build or a bad sector would.
        let ws = root.join("workspaces").join(id.to_storage());
        fs::write(ws.join("snapshot.loro"), b"not a loro snapshot at all").unwrap();

        let mut reopened = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let (loaded, opened) = reopened.load(id, "acme").unwrap();
        let Opened::Recovered(r) = opened else {
            panic!("expected a recovery, got {opened:?}");
        };
        assert!(r.from.is_some(), "came from a backup");
        assert!(
            loaded.read(task).unwrap().content.as_text() == Some("the only copy"),
            "and the content came back"
        );

        // The unreadable file is kept, and kept *as well as* — not instead of.
        let kept = r.quarantined.expect("the bad snapshot was quarantined");
        assert_eq!(fs::read(&kept).unwrap(), b"not a loro snapshot at all");
        assert!(
            ws.join("snapshot.loro").exists(),
            "copied, not moved — recovery must not be able to destroy anything"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A missing snapshot with backups beside it is somebody having deleted
    /// something, not a first run — and answering `Fresh` there is how a real
    /// workspace gets replaced by an empty one.
    #[test]
    fn a_missing_snapshot_with_a_backup_is_not_a_first_run() {
        let root = temp_root("missing");
        let id = WorkspaceId::new();
        let mut store = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut doc = doc_of(&mut store, id);
        let task = doc
            .insert(Parent::Root, Node::task("kept").unwrap())
            .unwrap();
        store.save_snapshot(&doc).unwrap();
        let mut store = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let (doc, _) = store.load(id, "acme").unwrap();
        store.save_snapshot(&doc).unwrap();

        let ws = root.join("workspaces").join(id.to_storage());
        fs::remove_file(ws.join("snapshot.loro")).unwrap();

        let mut reopened = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let (loaded, opened) = reopened.load(id, "acme").unwrap();
        let Opened::Recovered(r) = opened else {
            panic!("expected a recovery, got {opened:?}");
        };
        assert_eq!(r.quarantined, None, "there was no file left to keep");
        assert_eq!(
            loaded.read(task).unwrap().content.as_text(),
            Some("kept"),
            "restored from the backup"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The first save of a session takes a copy; the ones that follow it a few
    /// milliseconds later do not. Otherwise an idle save every 800 ms would fill
    /// the disk with near-identical files.
    #[test]
    fn a_backup_is_taken_once_a_session_not_once_a_save() {
        let root = temp_root("rotate");
        let id = WorkspaceId::new();

        let mut store = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut doc = doc_of(&mut store, id);
        doc.insert(Parent::Root, Node::task("one").unwrap())
            .unwrap();
        // Nothing on disk yet, so this one has nothing to preserve.
        store.save_snapshot(&doc).unwrap();
        assert!(store.backups().is_empty(), "no snapshot existed to copy");

        for _ in 0..5 {
            store.save_snapshot(&doc).unwrap();
        }
        assert!(
            store.backups().is_empty(),
            "still the same session — the hour has not passed"
        );

        // A new process is a new session, and takes one.
        let mut next = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let (doc, _) = next.load(id, "acme").unwrap();
        next.save_snapshot(&doc).unwrap();
        next.save_snapshot(&doc).unwrap();
        assert_eq!(next.backups().len(), 1, "one per session, not one per save");

        let _ = fs::remove_dir_all(&root);
    }

    /// One peer's damaged log must not stop you opening a document that is
    /// sitting intact in the snapshot. This is the difference between "sync is
    /// degraded" and "the app will not start".
    #[test]
    fn a_damaged_peer_log_is_skipped_rather_than_fatal() {
        let root = temp_root("damaged");
        let id = WorkspaceId::new();

        let mut store = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut doc = doc_of(&mut store, id);
        let task = doc
            .insert(Parent::Root, Node::task("still here").unwrap())
            .unwrap();
        store.save_snapshot(&doc).unwrap();

        // A frame with a valid length prefix and nonsense inside it.
        let ws = root.join("workspaces").join(id.to_storage());
        let mut bad = (8u32).to_le_bytes().to_vec();
        bad.extend_from_slice(b"garbage!");
        fs::write(ws.join("updates").join("peer-B.loro"), bad).unwrap();

        let mut reopened = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let (loaded, opened) = reopened.load(id, "acme").unwrap();
        assert_eq!(opened, Opened::Normal, "the snapshot itself was fine");
        assert_eq!(
            loaded.read(task).unwrap().content.as_text(),
            Some("still here")
        );
        assert_eq!(reopened.damaged().len(), 1, "and it said which file");
        assert!(reopened.damaged()[0].contains("peer-B.loro"));

        let _ = fs::remove_dir_all(&root);
    }

    /// A frame still being written must not be half-imported.
    #[test]
    fn a_truncated_frame_is_left_for_the_next_poll() {
        let root = temp_root("truncated");
        let id = WorkspaceId::new();
        let mut store_a = WorkspaceStore::open_at(&root, id, "A").unwrap();
        let mut a = doc_of(&mut store_a, id);
        a.insert(Parent::Root, Node::task("one").unwrap()).unwrap();
        let update = a.export_all_updates().unwrap();
        store_a.publish(&update).unwrap();

        // Simulate a writer caught mid-frame.
        let path = root
            .join("workspaces")
            .join(id.to_storage())
            .join("updates")
            .join("peer-A.loro");
        let full = fs::read(&path).unwrap();
        fs::write(&path, &full[..full.len() - 5]).unwrap();

        let mut store_b = WorkspaceStore::open_at(&root, id, "B").unwrap();
        let mut b = doc_of(&mut store_b, id);
        assert_eq!(b.walk().unwrap().len(), 0, "partial frame ignored");

        // Once the rest lands, it imports cleanly.
        fs::write(&path, &full).unwrap();
        store_b.sync_in(&mut b).unwrap();
        assert_eq!(b.walk().unwrap().len(), 1, "picked up on the next poll");

        let _ = fs::remove_dir_all(&root);
    }
}

/// What the profile document adds — and what the two documents now share.
///
/// Separate module from the suite above so the line is visible: everything up
/// there is the pre-existing safety net, unchanged; everything down here is new.
#[cfg(test)]
mod profile_tests {
    use super::*;
    use koru_core::{ProfileId, WorkspaceId};
    use std::fs;
    use std::path::Path;

    const T0: i64 = 1_700_000_000_000;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("koru-profile-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn open(root: &Path, peer: &str) -> ProfileStore {
        ProfileStore::open_at(root, peer).unwrap()
    }

    #[test]
    fn a_profile_snapshot_round_trips_through_disk() {
        let root = temp_root("psnapshot");
        let id = ProfileId::new();

        let mut store = open(&root, "A");
        let (mut doc, opened) = store.load(id, "Work", Some("sam@acme.com")).unwrap();
        assert_eq!(opened, Opened::Fresh, "nothing was on disk");
        let ws = doc.add_workspace("Example", T0).unwrap();
        store.save_snapshot(&doc).unwrap();

        let mut reopened = open(&root, "A");
        let (loaded, opened) = reopened.load(id, "Work", None).unwrap();
        assert_eq!(opened, Opened::Normal);
        assert_eq!(loaded.id(), Some(id));
        assert_eq!(loaded.email().as_deref(), Some("sam@acme.com"));
        assert_eq!(loaded.workspaces(), vec![ws]);

        let _ = fs::remove_dir_all(&root);
    }

    /// The whole reason the store was generalised rather than copied: the list
    /// of your workspaces gets the same recovery path your tasks do.
    #[test]
    fn an_unreadable_profile_snapshot_recovers_from_the_backup() {
        let root = temp_root("precover");
        let id = ProfileId::new();

        let mut store = open(&root, "A");
        let (mut doc, _) = store.load(id, "Work", None).unwrap();
        let ws = doc.add_workspace("Example", T0).unwrap();
        store.save_snapshot(&doc).unwrap();
        // A second session, so the once-per-session backup rule fires.
        let mut store = open(&root, "A");
        let (doc, _) = store.load(id, "Work", None).unwrap();
        store.save_snapshot(&doc).unwrap();
        assert_eq!(store.backups().len(), 1);

        fs::write(root.join("profile").join("snapshot.loro"), b"not loro").unwrap();

        let mut reopened = open(&root, "A");
        let (loaded, opened) = reopened.load(id, "Work", None).unwrap();
        let Opened::Recovered(r) = opened else {
            panic!("expected a recovery, got {opened:?}");
        };
        assert!(r.from.is_some(), "came from a backup");
        assert_eq!(loaded.workspaces(), vec![ws], "the list came back");
        // Copied, not moved — recovery must not destroy what it was called to save.
        let kept = r.quarantined.expect("the bad snapshot was quarantined");
        assert_eq!(fs::read(&kept).unwrap(), b"not loro");

        let _ = fs::remove_dir_all(&root);
    }

    /// Two windows on one profile folder are now ordinary rather than exotic, so
    /// they must not both write `snapshot.loro.part` and rename the interleaving
    /// over the only good copy.
    #[test]
    fn two_instances_do_not_share_a_temp_snapshot_name() {
        let root = temp_root("parts");
        let id = ProfileId::new();

        let mut one = open(&root, "i3f9c");
        let (doc, _) = one.load(id, "Work", None).unwrap();
        one.save_snapshot(&doc).unwrap();

        let mut two = open(&root, "i9c11");
        let (doc, _) = two.load(id, "Work", None).unwrap();
        two.save_snapshot(&doc).unwrap();

        // Both saves landed on the one real file, and neither left a temp.
        let dir = root.join("profile");
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(names.contains(&"snapshot.loro".to_string()), "{names:?}");
        assert!(
            !names.iter().any(|n| n.ends_with(".part")),
            "no temp left behind: {names:?}"
        );
        // And the names they *would* have used differ, which is the guarantee.
        assert_ne!(
            dir.join("snapshot.loro").with_extension("i3f9c.part"),
            dir.join("snapshot.loro").with_extension("i9c11.part"),
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A profile folder is a workspace folder's sibling, not its parent — so a
    /// build that predates profiles opens the same folder and ignores it.
    #[test]
    fn the_profile_document_sits_beside_the_workspaces_not_above_them() {
        let root = temp_root("layout");
        let ws_id = WorkspaceId::new();

        let mut profile = open(&root, "A");
        let (doc, _) = profile.load(ProfileId::new(), "Work", None).unwrap();
        profile.save_snapshot(&doc).unwrap();
        let mut workspace = WorkspaceStore::open_at(&root, ws_id, "A").unwrap();
        let (doc, _) = workspace.load(ws_id, "Example").unwrap();
        workspace.save_snapshot(&doc).unwrap();

        assert!(root.join("profile").join("snapshot.loro").exists());
        assert!(
            root.join("workspaces")
                .join(ws_id.to_storage())
                .join("snapshot.loro")
                .exists()
        );

        let _ = fs::remove_dir_all(&root);
    }
}
