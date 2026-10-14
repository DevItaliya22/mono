// SPDX-License-Identifier: MPL-2.0

//! **A backup is not valid because it was written.** Every test here restores
//! the artefact and opens it, and asserts on what comes back out.

use super::*;
use koru_store_core::fixtures::SAMPLES;
use koru_store_core::ids::{DocumentId, WorkspaceId};
use koru_store_core::lifecycle::LiveDocument;
use koru_store_core::manifest::Manifest;
use koru_store_core::model::DocumentKind;
use koru_store_core::ports::{UpdateBatch, WorkspaceRepo};
use koru_store_core::recovery::open_document;

use crate::workspace::SqliteWorkspace;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000 + ms)
}

struct Fixture {
    _dir: tempfile::TempDir,
    source: PathBuf,
    backups: SqliteBackups,
    document: DocumentId,
}

/// A real workspace database with a real Loro history in it.
fn fixture(titles: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("workspace.db");
    let backups = SqliteBackups::new(&source, dir.path().join("backups"));
    let document = DocumentId::new();

    let mut manifest = Manifest::create(WorkspaceId::new(), "W", at(0)).unwrap();
    let mut repo = SqliteWorkspace::open(&source).unwrap();
    repo.create_document(document, DocumentKind::Manifest, at(0))
        .unwrap();

    // Deltas from an empty version vector, so the journal is replayable on its
    // own — see `a_delta_whose_ancestors_are_missing_applies_nothing`.
    let mut prev = loro::VersionVector::new();
    for title in titles {
        manifest
            .add_document(DocumentId::new(), title, "page", None)
            .unwrap();
        let update = manifest.export_updates_since(&prev).unwrap();
        prev = manifest.version();
        repo.commit_updates(&UpdateBatch {
            document,
            updates: vec![update],
            version_vector: manifest.version().encode(),
            frontiers: manifest.frontiers().encode(),
            received_at: at(0),
            origins: vec![koru_store_core::ports::UpdateOrigin::Store],
        })
        .unwrap();
    }
    drop(repo);

    Fixture {
        _dir: dir,
        source,
        backups,
        document,
    }
}

/// Open the live database and report what the document actually contains.
/// This is the assertion that matters: not "a file exists" but "the work is
/// there".
fn titles_in(f: &Fixture) -> Vec<String> {
    let repo = SqliteWorkspace::open(&f.source).unwrap();
    let opened = open_document(&repo, f.document).unwrap();
    let live: &LiveDocument = opened.value();
    Manifest::from_snapshot(&live.export_snapshot().unwrap())
        .unwrap()
        .entries()
        .into_iter()
        .map(|e| e.title)
        .collect()
}

// ------------------------------------------------------------------ take

#[test]
fn a_backup_restores_and_opens_with_its_work_intact() {
    let f = fixture(&["Quarterly plan", "四半期計画"]);
    let id = f.backups.take(at(100)).unwrap();

    // Lose everything since.
    std::fs::remove_file(&f.source).unwrap();
    f.backups.restore(&id).unwrap();

    assert_eq!(titles_in(&f), ["Quarterly plan", "四半期計画"]);
}

#[test]
fn a_backup_captures_work_still_in_the_write_ahead_log() {
    // The reason `VACUUM INTO` is used rather than copying the file: the newest
    // committed data lives in the WAL, and a plain copy of `x.db` would miss it
    // while looking perfectly valid.
    let f = fixture(&["Before"]);

    // Hold the database open, so its WAL is not checkpointed away, and write
    // more through the live connection.
    let mut repo = SqliteWorkspace::open(&f.source).unwrap();
    let mut manifest = Manifest::create(WorkspaceId::new(), "W", at(0)).unwrap();
    let mut prev = loro::VersionVector::new();
    for title in ["Before", "After the WAL"] {
        manifest
            .add_document(DocumentId::new(), title, "page", None)
            .unwrap();
        let update = manifest.export_updates_since(&prev).unwrap();
        prev = manifest.version();
        repo.commit_updates(&UpdateBatch {
            document: f.document,
            updates: vec![update],
            version_vector: manifest.version().encode(),
            frontiers: manifest.frontiers().encode(),
            received_at: at(0),
            origins: vec![koru_store_core::ports::UpdateOrigin::Store],
        })
        .unwrap();
    }

    let id = f.backups.take(at(200)).unwrap();
    drop(repo);

    std::fs::remove_file(&f.source).unwrap();
    f.backups.restore(&id).unwrap();
    assert!(
        titles_in(&f).contains(&"After the WAL".to_string()),
        "the backup missed data that was only in the WAL"
    );
}

#[test]
fn every_supported_language_survives_a_backup_and_restore() {
    let titles: Vec<&str> = SAMPLES.iter().map(|s| s.title).collect();
    let f = fixture(&titles);
    let id = f.backups.take(at(100)).unwrap();
    std::fs::remove_file(&f.source).unwrap();
    f.backups.restore(&id).unwrap();

    let got = titles_in(&f);
    assert_eq!(got, titles, "a title changed across a backup and restore");
}

#[test]
fn taking_a_backup_twice_at_one_instant_is_idempotent() {
    // `VACUUM INTO` refuses to overwrite, so without care a repeat is an error
    // nobody can act on.
    let f = fixture(&["A"]);
    let first = f.backups.take(at(100)).unwrap();
    let second = f.backups.take(at(100)).unwrap();
    assert_eq!(first, second);
    assert_eq!(f.backups.list().unwrap().len(), 1);
}

// ------------------------------------------------------------------ the restore trap

#[test]
fn a_restore_clears_a_stale_write_ahead_log() {
    // The trap the module is arranged around: a `-wal` left from the old
    // generation is replayed over the restored file, or refuses it. Either is
    // worse than not restoring.
    let f = fixture(&["Original"]);
    let id = f.backups.take(at(100)).unwrap();

    let mut wal = f.source.as_os_str().to_owned();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    std::fs::write(&wal, b"a stale write-ahead log from another generation").unwrap();

    f.backups.restore(&id).unwrap();
    assert!(
        !wal.exists(),
        "the stale WAL was left beside the restored file"
    );
    assert_eq!(titles_in(&f), ["Original"]);
}

#[test]
fn a_corrupt_backup_is_refused_before_the_live_database_is_touched() {
    // Finding out that a backup will not open *after* overwriting the live
    // database is the worst possible moment.
    let f = fixture(&["Precious"]);
    let id = f.backups.take(at(100)).unwrap();
    let path = f.backups.path_of(&id).unwrap();
    std::fs::write(&path, b"this is not a database").unwrap();

    assert_eq!(
        f.backups.restore(&id).unwrap_err().code(),
        Code::CorruptState
    );
    // The live database is exactly as it was.
    assert_eq!(titles_in(&f), ["Precious"]);
}

#[test]
fn restoring_something_that_is_not_a_backup_is_refused() {
    let f = fixture(&["A"]);
    for bad in [
        "../../etc/passwd",
        r"..\..\windows\system32",
        "C:\\somewhere\\else.db",
        "workspace-1.db/nested",
        "",
        "notes.txt",
    ] {
        let id = BackupId(bad.to_string());
        assert!(
            f.backups.restore(&id).is_err(),
            "{bad:?} was accepted as a backup name"
        );
    }
    assert_eq!(titles_in(&f), ["A"]);
}

#[test]
fn restoring_a_backup_that_does_not_exist_says_so() {
    let f = fixture(&["A"]);
    let id = BackupId(format!("workspace-{}.db", at(999).for_filename()));
    assert_eq!(
        f.backups.restore(&id).unwrap_err().code(),
        Code::BackupNotFound
    );
}

// ------------------------------------------------------------------ list and prune

#[test]
fn backups_are_listed_newest_first() {
    let f = fixture(&["A"]);
    for ms in [100, 300, 200] {
        f.backups.take(at(ms)).unwrap();
    }
    let listed = f.backups.list().unwrap();
    assert_eq!(
        listed.iter().map(|b| b.taken_at).collect::<Vec<_>>(),
        [at(300), at(200), at(100)]
    );
    assert!(listed.iter().all(|b| b.size > 0));
}

#[test]
fn listing_with_no_backup_directory_is_empty_not_an_error() {
    let f = fixture(&["A"]);
    assert!(f.backups.list().unwrap().is_empty());
}

#[test]
fn unrelated_files_in_the_directory_are_ignored() {
    let f = fixture(&["A"]);
    f.backups.take(at(100)).unwrap();
    std::fs::write(f.backups.dir.join("notes.txt"), b"hello").unwrap();
    std::fs::write(f.backups.dir.join("workspace-notanumber.db"), b"x").unwrap();
    assert_eq!(f.backups.list().unwrap().len(), 1);
}

#[test]
fn pruning_keeps_the_newest_and_they_still_restore() {
    let f = fixture(&["A"]);
    for ms in [100, 200, 300, 400, 500] {
        f.backups.take(at(ms)).unwrap();
    }
    assert_eq!(f.backups.prune(2).unwrap(), 3);

    let left = f.backups.list().unwrap();
    assert_eq!(
        left.iter().map(|b| b.taken_at).collect::<Vec<_>>(),
        [at(500), at(400)]
    );
    // Kept is not the same as usable, so prove the survivor still restores.
    std::fs::remove_file(&f.source).unwrap();
    f.backups.restore(&left[0].id).unwrap();
    assert_eq!(titles_in(&f), ["A"]);
}

#[test]
fn pruning_more_than_there_are_removes_nothing() {
    let f = fixture(&["A"]);
    f.backups.take(at(100)).unwrap();
    assert_eq!(f.backups.prune(BACKUPS_KEPT).unwrap(), 0);
    assert_eq!(f.backups.list().unwrap().len(), 1);
}
