// SPDX-License-Identifier: MPL-2.0

//! The id bridge, and that the store's manifest behaves the way this crate is
//! about to rely on.
//!
//! The manifest's own suite lives in `koru-store-core` and is not repeated here.
//! What these pin is the seam: that an id survives the crossing, and that the
//! two operations Explore is built on — nesting and ordering — mean here what
//! they mean there.

use super::*;
use koru_store_core::manifest::Manifest;
use koru_store_core::model::Timestamp;

fn at() -> Timestamp {
    Timestamp::from_millis(1_700_000_000_000)
}

fn manifest() -> Manifest {
    Manifest::create(store_workspace(WorkspaceId::new()), "Acme", at()).unwrap()
}

/// Both directions, both types. A UUID is a UUID; the newtypes differ only in
/// which crate is allowed to name them.
#[test]
fn an_id_survives_the_crossing_in_both_directions() {
    let workspace = WorkspaceId::new();
    assert_eq!(store_workspace(workspace).as_uuid(), workspace.as_uuid());

    let document = DocumentId::new();
    assert_eq!(koru_document(store_document(document)), document);
}

/// The manifest keeps the id we gave it, rather than minting one of its own —
/// which is what lets a document's id be the same fact on both sides of the
/// seam.
#[test]
fn a_document_keeps_the_id_it_was_added_with() {
    let mut m = manifest();
    let id = DocumentId::new();
    m.add_document(store_document(id), "Roadmap", "page", None)
        .unwrap();

    let entries = m.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(koru_document(entries[0].id), id);
    assert_eq!(entries[0].title, "Roadmap");
    assert_eq!(entries[0].kind, "page");
    assert_eq!(entries[0].parent, None);
}

/// Nesting and ordering, which is the whole of what Explore asks of the list.
/// Pre-order, with each entry naming its parent — the same contract
/// `WorkspaceDoc::explore` answers from the tree today.
#[test]
fn the_manifest_answers_the_same_shape_explore_does() {
    let mut m = manifest();
    let (a, a1, b) = (DocumentId::new(), DocumentId::new(), DocumentId::new());
    m.add_document(store_document(a), "A", "page", None)
        .unwrap();
    m.add_document(store_document(a1), "A1", "page", Some(store_document(a)))
        .unwrap();
    m.add_document(store_document(b), "B", "page", None)
        .unwrap();

    let e = m.entries();
    assert_eq!(
        e.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(),
        ["A", "A1", "B"],
        "pre-order: a document, then its children, then its next sibling"
    );
    assert_eq!(koru_document(e[1].parent.unwrap()), a);
    assert_eq!((e[0].ordinal, e[1].ordinal, e[2].ordinal), (0, 0, 1));
}

/// A move reorders among siblings, addressed by index — the operation the
/// explore drag performs.
#[test]
fn a_move_reorders_among_siblings() {
    let mut m = manifest();
    let ids: Vec<DocumentId> = (0..3).map(|_| DocumentId::new()).collect();
    for (id, title) in ids.iter().zip(["A", "B", "C"]) {
        m.add_document(store_document(*id), title, "page", None)
            .unwrap();
    }

    m.move_document(store_document(ids[2]), None, 0).unwrap();
    assert_eq!(
        m.entries()
            .iter()
            .map(|x| x.title.as_str())
            .collect::<Vec<_>>(),
        ["C", "A", "B"]
    );
}

/// **A rename is last-writer-wins, not a text merge.** The reason the title is
/// a map value rather than a `LoroText` (ADR-80): two people renaming at once
/// should leave one of the two names, never an interleaving of both.
///
/// Asserted here rather than taken on trust, because it is the one place the
/// manifest deliberately behaves *unlike* the rest of the model — where
/// concurrent typing merging character by character is exactly what is wanted.
#[test]
fn a_rename_replaces_the_title_rather_than_merging_it() {
    let mut m = manifest();
    let id = DocumentId::new();
    m.add_document(store_document(id), "Roadmap", "page", None)
        .unwrap();
    m.rename_document(store_document(id), "Plan").unwrap();
    assert_eq!(m.entries()[0].title, "Plan");
}

/// Removing a document reports its whole subtree, so a caller knows every
/// document whose bytes are now unreferenced — the question a trash or a delete
/// has to answer.
#[test]
fn removing_a_document_reports_its_whole_subtree() {
    let mut m = manifest();
    let (parent, child) = (DocumentId::new(), DocumentId::new());
    m.add_document(store_document(parent), "Parent", "page", None)
        .unwrap();
    m.add_document(
        store_document(child),
        "Child",
        "page",
        Some(store_document(parent)),
    )
    .unwrap();

    let gone = m.remove_document(store_document(parent)).unwrap();
    let gone: Vec<DocumentId> = gone.into_iter().map(koru_document).collect();
    assert!(gone.contains(&parent) && gone.contains(&child));
    assert!(m.entries().is_empty());
}

/// A manifest round-trips through a snapshot, which is how it will be filed
/// beside the documents it lists.
#[test]
fn a_manifest_round_trips_through_a_snapshot() {
    let mut m = manifest();
    let id = DocumentId::new();
    m.add_document(store_document(id), "ロードマップ", "page", None)
        .unwrap();

    let bytes = m.export_snapshot().unwrap();
    let back = Manifest::from_snapshot(&bytes).unwrap();
    assert_eq!(back.entries()[0].title, "ロードマップ");
    assert_eq!(koru_document(back.entries()[0].id), id);
}
