// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::fixtures::{BIDI_LINE, COMPOSED, DECOMPOSED, SAMPLES};
use koru_store_core::ids::WorkspaceId;
use koru_store_core::manifest::Manifest;
use koru_store_core::model::Timestamp;

fn at() -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000)
}

fn repo() -> SqliteWorkspace {
    SqliteWorkspace::in_memory().expect("open")
}

fn entry(title: &str, parent: Option<DocumentId>, ordinal: u32) -> DocumentEntry {
    DocumentEntry {
        id: DocumentId::new(),
        parent,
        ordinal,
        title: title.into(),
        kind: "page".into(),
        mime: None,
        note: None,
        trashed_at: None,
        trashed_by: None,
    }
}

#[test]
fn an_empty_index_lists_nothing() {
    assert!(repo().entries().unwrap().is_empty());
}

#[test]
fn entries_come_back_in_the_order_they_were_indexed() {
    // Walk order, not id order: a listing must match what the person arranged.
    let mut w = repo();
    let rows = vec![
        entry("A", None, 0),
        entry("B", None, 1),
        entry("C", None, 2),
    ];
    w.reindex(&rows).unwrap();
    assert_eq!(
        w.entries()
            .unwrap()
            .iter()
            .map(|e| e.title.clone())
            .collect::<Vec<_>>(),
        ["A", "B", "C"]
    );
}

#[test]
fn hierarchy_and_ordinals_survive() {
    let mut w = repo();
    let parent = entry("Parent", None, 0);
    let child = entry("Child", Some(parent.id), 0);
    w.reindex(&[parent.clone(), child.clone()]).unwrap();

    let got = w.entries().unwrap();
    assert_eq!(got, vec![parent.clone(), child.clone()]);
    assert_eq!(w.entry(child.id).unwrap().unwrap().parent, Some(parent.id));
    assert_eq!(w.entry(parent.id).unwrap().unwrap().parent, None);
}

#[test]
fn reindexing_replaces_rather_than_accumulates() {
    // Wholesale replacement is what makes the projection impossible to drift
    // from the manifest. If this appended, a rename would leave both titles.
    let mut w = repo();
    let first = vec![entry("Old", None, 0), entry("Also old", None, 1)];
    w.reindex(&first).unwrap();
    let second = vec![entry("New", None, 0)];
    w.reindex(&second).unwrap();

    assert_eq!(w.entries().unwrap(), second);
    assert_eq!(w.entry(first[0].id).unwrap(), None);
}

#[test]
fn reindexing_to_nothing_empties_the_listing() {
    let mut w = repo();
    w.reindex(&[entry("Gone", None, 0)]).unwrap();
    w.reindex(&[]).unwrap();
    assert!(w.entries().unwrap().is_empty());
}

#[test]
fn an_unknown_document_is_none() {
    assert_eq!(repo().entry(DocumentId::new()).unwrap(), None);
}

#[test]
fn titles_in_every_supported_language_round_trip_byte_for_byte() {
    let mut w = repo();
    let rows: Vec<DocumentEntry> = SAMPLES
        .iter()
        .enumerate()
        .map(|(i, s)| entry(s.title, None, i as u32))
        .collect();
    w.reindex(&rows).unwrap();
    assert_eq!(w.entries().unwrap(), rows);
}

#[test]
fn bidi_and_both_unicode_spellings_stay_distinct_rows() {
    let mut w = repo();
    let rows = vec![
        entry(BIDI_LINE, None, 0),
        entry(COMPOSED, None, 1),
        entry(DECOMPOSED, None, 2),
    ];
    w.reindex(&rows).unwrap();
    let got = w.entries().unwrap();
    assert_eq!(got, rows);
    assert_ne!(got[1].title, got[2].title, "the two spellings were folded");
}

#[test]
fn the_projection_matches_the_manifest_it_came_from() {
    // The whole point of the module, end to end: what the manifest says and
    // what a listing query returns are the same thing in the same order.
    let mut manifest = Manifest::create(WorkspaceId::new(), "W", at()).unwrap();
    let parent = DocumentId::new();
    let child = DocumentId::new();
    let sibling = DocumentId::new();
    manifest
        .add_document(parent, "四半期計画", "page", None)
        .unwrap();
    manifest
        .add_document(child, "الخطة الفصلية", "page", Some(parent))
        .unwrap();
    manifest
        .add_document(sibling, "Plan trimestriel", "page", None)
        .unwrap();

    let mut w = repo();
    w.reindex(&manifest.entries()).unwrap();
    assert_eq!(w.entries().unwrap(), manifest.entries());

    // And after a change, a rebuild still matches — no diffing, no drift.
    manifest.move_document(sibling, None, 0).unwrap();
    w.reindex(&manifest.entries()).unwrap();
    assert_eq!(w.entries().unwrap(), manifest.entries());
    assert_eq!(w.entries().unwrap()[0].title, "Plan trimestriel");
}

#[test]
fn a_corrupt_row_is_reported_rather_than_panicking() {
    let mut w = repo();
    w.reindex(&[entry("A", None, 0)]).unwrap();
    w.connection()
        .execute("UPDATE documents SET doc_id = 'not-a-uuid'", [])
        .unwrap();
    assert_eq!(
        w.entries().unwrap_err().code(),
        koru_store_core::errors::Code::CorruptState
    );
}

/// **The trash flag survives the mirror**, which is what makes a listing
/// filtered on it correct after a restart.
///
/// The `documents` table is a projection, so this is really a test that the
/// column is written and read rather than dropped somewhere in the middle — the
/// kind of loss that shows up as a document quietly climbing back out of the bin.
#[test]
fn a_trashed_document_is_still_trashed_after_a_reindex() {
    use koru_store_core::model::Timestamp;

    let mut repo = repo();
    let mut binned = entry("Gone", None, 0);
    binned.trashed_at = Some(Timestamp::from_millis(1_700_000_000_000));
    let kept = entry("Here", None, 1);
    repo.reindex(&[binned.clone(), kept.clone()])
        .expect("index");

    let listed = repo.entries().expect("entries");
    assert_eq!(
        listed.len(),
        2,
        "the bin is listed, not hidden by the store"
    );
    assert_eq!(
        listed
            .iter()
            .find(|e| e.id == binned.id)
            .and_then(|e| e.trashed_at),
        binned.trashed_at,
        "the stamp did not survive"
    );
    assert_eq!(
        listed.iter().find(|e| e.id == kept.id).unwrap().trashed_at,
        None
    );

    // ...and one row at a time, which is the other read path.
    assert_eq!(
        repo.entry(binned.id).expect("entry").unwrap().trashed_at,
        binned.trashed_at
    );
}

/// **A workspace database stamped `2` before `trashed_by` existed still opens,
/// and can still be written to.**
///
/// Reported from a running window as *Couldn't create that document: table
/// documents has no column named trashed_by*, at a moment that has nothing to
/// do with the trash — `reindex` writes every column of the projection, and
/// creating a document reindexes.
///
/// The cause was `trashed_by` being appended to migration 2 *after* migration 2
/// had run: no database stamped `2` ever re-runs it, so the column reached
/// every workspace created afterwards and none created before. This builds the
/// older shape by hand, which is the only way to have one.
#[test]
fn a_workspace_from_before_trashed_by_gains_the_column_on_open() {
    let dir = std::env::temp_dir().join(format!("koru-migration-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("workspace.db");

    // The `documents` table as migration 2 left it before the edit, stamped with
    // the version that says it is finished.
    {
        let connection = rusqlite::Connection::open(&path).expect("create");
        connection
            .execute_batch(
                "CREATE TABLE documents (
                   doc_id     TEXT PRIMARY KEY,
                   parent_id  TEXT,
                   ordinal    INTEGER NOT NULL,
                   title      TEXT    NOT NULL,
                   kind       TEXT    NOT NULL,
                   position   INTEGER NOT NULL,
                   trashed_at INTEGER
                 );
                 CREATE INDEX documents_walk ON documents(position);
                 -- The rest of migration 1's durable journal existed in a
                 -- real schema-2 workspace. This hand-built fixture used to
                 -- omit it because the index test never queried it; later
                 -- journal migrations correctly require the table.
                 CREATE TABLE loro_documents (
                   id                    TEXT PRIMARY KEY,
                   kind                  TEXT NOT NULL,
                   latest_seq            INTEGER NOT NULL,
                   latest_checkpoint_seq INTEGER,
                   version_vector        BLOB NOT NULL,
                   frontiers             BLOB NOT NULL,
                   created_at            INTEGER NOT NULL,
                   updated_at            INTEGER NOT NULL
                 );
                 CREATE TABLE loro_updates (
                   document_id TEXT NOT NULL,
                   seq         INTEGER NOT NULL,
                   update_bytes BLOB NOT NULL,
                   received_at INTEGER NOT NULL,
                   from_client TEXT,
                   PRIMARY KEY(document_id, seq)
                 );
                 -- Present since migration 1; migration 9 alters it.
                 CREATE TABLE blobs (hash TEXT PRIMARY KEY, size INTEGER NOT NULL,
                   mime TEXT NOT NULL, filename TEXT, created_at INTEGER NOT NULL);",
            )
            .expect("older shape");
        connection
            .pragma_update(None, "user_version", 2i64)
            .expect("stamp");
    }

    let mut w = SqliteWorkspace::open(&path).expect("open an older workspace");
    // This is the call that failed: writing the projection names every column.
    w.reindex(&[entry("Notes", None, 0)])
        .expect("reindex an older workspace");

    let back = w.entries().expect("entries");
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].title, "Notes");

    let _ = std::fs::remove_dir_all(&dir);
}
