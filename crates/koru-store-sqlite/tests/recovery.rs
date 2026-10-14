// SPDX-License-Identifier: MPL-2.0

//! **The recovery suite: every case restores *and opens*.**
//!
//! A backup is not valid because a file appeared, and a fall-back is not
//! working because it returned `Ok`. Every test here drives a real SQLite
//! database with a real Loro history through real damage, and then asserts on
//! **the document titles that come back out**. If the work is not readable at
//! the end, the test fails, whatever the return values said.
//!
//! These live in `tests/` rather than beside a module because each one crosses
//! three of them — the workspace repository, the recovery policy and the backup
//! store — the same reason `crates/koru-core/tests/` exists.

use koru_store_core::errors::Code;
use koru_store_core::ids::{DocumentId, WorkspaceId};
use koru_store_core::manifest::Manifest;
use koru_store_core::model::{DocumentKind, Timestamp};
use koru_store_core::ports::{Backups, Opened, UpdateBatch, WorkspaceRepo};
use koru_store_core::recovery::open_document;
use koru_store_sqlite::{SqliteBackups, SqliteWorkspace};
use rusqlite::params;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000 + ms)
}

struct Workspace {
    dir: tempfile::TempDir,
    document: DocumentId,
    /// The manifest as the "client" holds it, for producing further updates.
    manifest: Manifest,
    /// The version already handed to the store.
    sent: loro::VersionVector,
}

impl Workspace {
    /// A workspace database with one manifest document and nothing in it.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let document = DocumentId::new();
        let mut repo = SqliteWorkspace::open(&Self::path_in(dir.path())).unwrap();
        repo.create_document(document, DocumentKind::Manifest, at(0))
            .unwrap();
        Self {
            dir,
            document,
            manifest: Manifest::create(WorkspaceId::new(), "W", at(0)).unwrap(),
            sent: loro::VersionVector::new(),
        }
    }

    fn path_in(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("workspace.db")
    }

    fn path(&self) -> std::path::PathBuf {
        Self::path_in(self.dir.path())
    }

    fn repo(&self) -> SqliteWorkspace {
        SqliteWorkspace::open(&self.path()).unwrap()
    }

    fn backups(&self) -> SqliteBackups {
        SqliteBackups::new(self.path(), self.dir.path().join("backups"))
    }

    /// Add a document and persist the delta — the shape a real edit takes:
    /// the client sends everything the store does not have.
    fn add(&mut self, title: &str) {
        self.manifest
            .add_document(DocumentId::new(), title, "page", None)
            .unwrap();
        let update = self.manifest.export_updates_since(&self.sent).unwrap();
        self.sent = self.manifest.version();
        let mut repo = self.repo();
        repo.commit_updates(&UpdateBatch {
            document: self.document,
            updates: vec![update],
            version_vector: self.manifest.version().encode(),
            frontiers: self.manifest.frontiers().encode(),
            received_at: at(0),
            origins: vec![koru_store_core::ports::UpdateOrigin::Store],
        })
        .unwrap();
    }

    /// Snapshot everything through the current head.
    fn checkpoint(&self, seq: u64) {
        let mut repo = self.repo();
        let opened = open_document(&repo, self.document).unwrap();
        let checkpoint = opened.value().checkpoint(seq, at(0)).unwrap();
        repo.commit_checkpoint(self.document, &checkpoint).unwrap();
    }

    /// Open the document and report what is actually readable. **This is the
    /// assertion that matters in every test below.**
    fn open(&self) -> (Vec<String>, Option<String>) {
        let repo = self.repo();
        let opened = open_document(&repo, self.document).unwrap();
        let note = opened.recovery().map(|r| r.problem.clone());
        let titles = Manifest::from_snapshot(&opened.value().export_snapshot().unwrap())
            .unwrap()
            .entries()
            .into_iter()
            .map(|e| e.title)
            .collect();
        (titles, note)
    }

    fn titles(&self) -> Vec<String> {
        self.open().0
    }

    fn raw(&self, sql: &str) {
        let connection = rusqlite::Connection::open(self.path()).unwrap();
        connection.execute(sql, []).unwrap();
    }
}

// ------------------------------------------------------------------ clean paths

#[test]
fn a_clean_reopen_returns_every_acknowledged_edit() {
    let mut w = Workspace::new();
    for title in ["Quarterly plan", "四半期計画", "الخطة الفصلية"] {
        w.add(title);
    }
    let (titles, note) = w.open();
    assert_eq!(titles, ["Quarterly plan", "四半期計画", "الخطة الفصلية"]);
    assert_eq!(note, None, "a clean open is not a recovery");
}

#[test]
fn a_checkpoint_and_the_updates_after_it_reconstruct_the_document() {
    let mut w = Workspace::new();
    w.add("One");
    w.checkpoint(1);
    w.add("Two");
    w.add("Three");
    w.checkpoint(3);
    w.add("Four");

    // Two checkpoints, so pruning has actually happened: update 1 is gone and
    // the document can only be whole if the snapshot is doing its job. This is
    // genuinely snapshot-plus-journal rather than journal alone.
    assert_eq!(
        w.repo().update_count(w.document).unwrap(),
        3,
        "1 pruned; 2, 3 and 4 retained"
    );
    assert_eq!(w.titles(), ["One", "Two", "Three", "Four"]);
}

// ------------------------------------------------------------------ interrupted work

#[test]
fn work_that_was_never_committed_is_absent_and_committed_work_is_intact() {
    // The durability boundary, seen from outside: an interrupted write leaves
    // no trace, and everything acknowledged before it survives. This is what a
    // process kill mid-transaction looks like to the next open.
    let mut w = Workspace::new();
    w.add("Acknowledged");

    {
        let connection = rusqlite::Connection::open(w.path()).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        connection
            .execute(
                "INSERT INTO loro_updates (document_id, seq, update_bytes, received_at)
                 VALUES (?1, 99, x'00', 0)",
                params![w.document.to_storage()],
            )
            .unwrap();
        // Dropped without COMMIT — the crash.
    }

    assert_eq!(w.repo().update_count(w.document).unwrap(), 1);
    assert_eq!(w.titles(), ["Acknowledged"]);
}

#[test]
fn a_torn_update_stops_the_replay_and_leaves_a_prefix_rather_than_a_blend() {
    // Loro's history is causal. Stepping over a broken update and applying the
    // next would build a document from a history that never happened — and no
    // amount of syncing repairs that. Stopping leaves the document *behind*,
    // which syncing does repair.
    let mut w = Workspace::new();
    for title in ["One", "Two", "Three"] {
        w.add(title);
    }
    w.raw("UPDATE loro_updates SET update_bytes = x'00' WHERE seq = 2");

    let (titles, note) = w.open();
    assert_eq!(titles, ["One"], "the replay stepped over the torn update");
    let note = note.expect("a stopped replay is a recovery");
    assert!(note.contains("update 2"), "{note}");
}

// ------------------------------------------------------------------ damaged checkpoints

#[test]
fn a_damaged_newest_checkpoint_falls_back_and_loses_nothing() {
    let mut w = Workspace::new();
    w.add("One");
    w.checkpoint(1);
    w.add("Two");
    w.add("Three");
    w.checkpoint(3);

    // The newest snapshot rots; its recorded checksum is untouched, which is
    // what a bad sector looks like.
    w.raw("UPDATE loro_checkpoints SET snapshot = x'00' WHERE seq = 3");

    let (titles, note) = w.open();
    let note = note.expect("a fall-back is a recovery");
    assert!(note.contains("checkpoint 3"), "{note}");
    // Lossless, because the journal above checkpoint 1 was still there.
    assert_eq!(titles, ["One", "Two", "Three"]);

    // And the damaged checkpoint was not deleted — it is evidence.
    assert!(w.repo().checkpoint(w.document, 3).unwrap().is_some());
}

#[test]
fn every_checkpoint_damaged_still_rebuilds_from_the_journal() {
    let mut w = Workspace::new();
    for title in ["One", "Two"] {
        w.add(title);
    }
    // A checkpoint that covers nothing, so the whole history stays in the
    // journal — the state a store is in before its first real checkpoint.
    w.checkpoint(0);
    w.raw("UPDATE loro_checkpoints SET snapshot = x'00'");

    let (titles, note) = w.open();
    assert!(
        note.expect("a rebuild is a recovery")
            .contains("did not verify")
    );
    assert_eq!(titles, ["One", "Two"]);
}

#[test]
fn a_checkpoint_is_refused_at_write_time_if_its_checksum_is_wrong() {
    let w = Workspace::new();
    let mut repo = w.repo();
    let opened = open_document(&repo, w.document).unwrap();
    let mut checkpoint = opened.value().checkpoint(0, at(0)).unwrap();
    checkpoint.checksum = "0".repeat(64);

    assert_eq!(
        repo.commit_checkpoint(w.document, &checkpoint)
            .unwrap_err()
            .code(),
        Code::CorruptState
    );
    assert!(repo.checkpoints(w.document).unwrap().is_empty());
}

// ------------------------------------------------------------------ backups

#[test]
fn a_backup_restored_into_an_emptied_directory_opens_with_its_work() {
    let mut w = Workspace::new();
    for title in ["Planejamento trimestral", "Quartalsplanung"] {
        w.add(title);
    }
    let backups = w.backups();
    let id = backups.take(at(100)).unwrap();

    // Everything since is lost, including the sidecars.
    for suffix in ["", "-wal", "-shm"] {
        let mut p = w.path().into_os_string();
        p.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(p));
    }
    assert!(!w.path().exists());

    backups.restore(&id).unwrap();
    assert_eq!(w.titles(), ["Planejamento trimestral", "Quartalsplanung"]);
}

#[test]
fn a_backup_taken_before_a_migration_can_be_put_back() {
    // The pre-migration copy exists so a migration that goes wrong is
    // survivable. Proving it means restoring and opening, not checking a file.
    let mut w = Workspace::new();
    w.add("Before the migration");
    let backups = w.backups();
    let id = backups.take(at(100)).unwrap();

    // A migration "fails" partway and leaves the database unusable.
    std::fs::write(w.path(), b"a half-written migration").unwrap();

    backups.restore(&id).unwrap();
    assert_eq!(w.titles(), ["Before the migration"]);
}

#[test]
fn a_damaged_live_database_is_detected_rather_than_silently_read() {
    let mut w = Workspace::new();
    w.add("One");
    assert!(w.repo().quick_check().unwrap());

    let backups = w.backups();
    let id = backups.take(at(100)).unwrap();
    std::fs::write(w.path(), b"not a database at all").unwrap();

    // Opening it does not quietly return an empty workspace.
    assert!(SqliteWorkspace::open(&w.path()).is_err());
    backups.restore(&id).unwrap();
    assert!(w.repo().quick_check().unwrap());
    assert_eq!(w.titles(), ["One"]);
}

// ------------------------------------------------------------------ absence

#[test]
fn a_document_that_was_never_created_opens_fresh_rather_than_failing() {
    let w = Workspace::new();
    let repo = w.repo();
    let opened = open_document(&repo, DocumentId::new()).unwrap();
    assert!(matches!(opened, Opened::Fresh(_)));
}
