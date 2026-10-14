// SPDX-License-Identifier: MPL-2.0

//! No disk anywhere in here: two in-memory documents are the whole of what the
//! seed touches, which is the isolation being asserted rather than described.

use super::*;
use koru_core::{ProfileId, WorkspaceId};

const T0: i64 = 1_700_000_000_000;

fn fresh() -> (ProfileDoc, WorkspaceDoc) {
    (
        ProfileDoc::create(ProfileId::new(), "Personal", None).unwrap(),
        WorkspaceDoc::create(WorkspaceId::new(), "Koru").unwrap(),
    )
}

#[test]
fn a_seeded_profile_has_exactly_one_workspace_called_example() {
    let (mut p, mut ws) = fresh();
    assert!(seed_new_profile(&mut p, &mut ws, T0).unwrap());

    let listed = p.workspaces();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, WORKSPACE);
    assert_eq!(listed[0].id, ws.id().unwrap(), "the entry names this one");
    assert_eq!(listed[0].created, T0);
    // The workspace and its entry agree from the first moment, rather than
    // after the first rename.
    assert_eq!(ws.name().as_deref(), Some(WORKSPACE));
}

#[test]
fn the_seed_creates_one_get_started_document_in_the_library() {
    let (mut p, mut ws) = fresh();
    seed_new_profile(&mut p, &mut ws, T0).unwrap();

    let library = ws.children(Parent::Root).unwrap();
    assert_eq!(library.len(), 1, "one document, nothing else");
    let doc = ws.read(library[0]).unwrap();
    assert_eq!(doc.kind, Kind::Document);
    assert_eq!(doc.content.as_text(), Some(DOCUMENT));
    assert!(is_get_started(&ws, library[0]));

    let body = ws.children(Parent::Node(library[0])).unwrap();
    assert_eq!(body.len(), BODY.len());
    assert!(
        body.iter()
            .any(|&id| ws.read(id).unwrap().meta.heading.is_some()),
        "it teaches headings by having some"
    );
    assert!(
        body.iter().all(|&id| ws
            .read(id)
            .unwrap()
            .content
            .as_text()
            .is_some_and(|t| !t.is_empty())),
        "and no empty paragraphs"
    );
}

/// The seed writes one document and its body, and nothing else.
///
/// It used to assert the opposite — that the walk came back **empty** — because
/// the seed wrote into the library while `koru_editor::prepare` opened on the
/// outline, and anything in the outline would have broken the promise that a
/// fresh workspace opens on one empty line. One tree now, so the count is the
/// check: too few means the seed wrote nothing, too many means it wrote twice.
#[test]
fn the_seed_writes_one_document_and_its_body() {
    let (mut p, mut ws) = fresh();
    seed_new_profile(&mut p, &mut ws, T0).unwrap();
    assert_eq!(ws.walk().unwrap().len(), 11);
    assert_eq!(ws.children(Parent::Root).unwrap().len(), 1, "one document");
}

/// The structural guard, independent of the caller's `Opened::Fresh`. Either
/// alone is sufficient, which is why there are two.
#[test]
fn seeding_twice_writes_nothing_the_second_time() {
    let (mut p, mut ws) = fresh();
    assert!(seed_new_profile(&mut p, &mut ws, T0).unwrap());

    let before = ws.export_snapshot().unwrap();
    assert!(
        !seed_new_profile(&mut p, &mut ws, T0 + 5000).unwrap(),
        "it says it did nothing"
    );
    assert_eq!(ws.export_snapshot().unwrap(), before, "and it did nothing");
    assert_eq!(p.workspaces().len(), 1);
    assert_eq!(ws.children(Parent::Root).unwrap().len(), 1);
}

/// A profile that already lists a workspace has been used, however it came to
/// be — recovered from a backup, rebuilt from peer logs, or adopted from a
/// folder written by an older build. None of those wants a second Example.
#[test]
fn seeding_a_profile_that_already_has_a_workspace_writes_nothing() {
    let (mut p, mut ws) = fresh();
    p.ensure_workspace(WorkspaceId::new(), "Q3 planning", T0)
        .unwrap();

    assert!(!seed_new_profile(&mut p, &mut ws, T0).unwrap());
    assert_eq!(p.workspaces().len(), 1, "still just theirs");
    assert!(ws.children(Parent::Root).unwrap().is_empty());
}

/// A document with no id in its meta is not one this function made, and
/// registering it would put a row in the switcher that cannot be opened.
#[test]
fn a_workspace_without_an_id_is_not_registered() {
    let mut p = ProfileDoc::create(ProfileId::new(), "Personal", None).unwrap();
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "Koru").unwrap();
    ws.doc().get_map("meta").delete("id").unwrap();
    ws.commit();

    assert!(!seed_new_profile(&mut p, &mut ws, T0).unwrap());
    assert!(p.workspaces().is_empty());
}

/// Renaming the document is a person saying they do not want it treated as the
/// seed any more, and nothing should second-guess that.
#[test]
fn a_renamed_get_started_document_is_no_longer_the_seeded_one() {
    let (mut p, mut ws) = fresh();
    seed_new_profile(&mut p, &mut ws, T0).unwrap();
    let doc = ws.children(Parent::Root).unwrap()[0];

    assert!(is_get_started(&ws, doc));
    ws.set_text(doc, "My notes").unwrap();
    assert!(!is_get_started(&ws, doc));
}
