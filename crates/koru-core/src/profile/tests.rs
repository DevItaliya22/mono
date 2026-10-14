// SPDX-License-Identifier: MPL-2.0

//! What the profile document has to guarantee.
//!
//! Two peers is the interesting case throughout: the workspace list is the one
//! container two windows on one folder both write to without coordinating, so
//! most of these are convergence tests wearing ordinary clothes.

use super::*;
use crate::node::{Content, Kind};

/// Stand-in timestamps. Real ones come from the caller; these are literals so
/// the resulting order is exactly assertable rather than clock-dependent.
const T0: i64 = 1_700_000_000_000;
const T1: i64 = T0 + 1000;
const T2: i64 = T0 + 2000;

fn profile() -> ProfileDoc {
    ProfileDoc::create(ProfileId::new(), "Personal", Some("me@example.com")).unwrap()
}

/// **First run has nobody to name, and the document says so** (S21, ADR-269).
///
/// The tempting alternative is a document with a fresh `ProfileId` and an empty
/// name, which reads as a profile everywhere downstream and would have to be
/// unpicked at every reader. An absent id is the answer `id()` was already
/// shaped to give.
#[test]
fn an_unseated_profile_has_no_identity_and_no_name() {
    let p = ProfileDoc::unseated().unwrap();

    assert_eq!(p.id(), None, "first run invents nobody");
    assert_eq!(p.name(), None);
    assert!(p.workspaces().is_empty());
    // Still this build's schema: an empty answer is not an unreadable one.
    assert_eq!(p.schema_version(), Some(PROFILE_SCHEMA_VERSION));
}

fn merged(a: &ProfileDoc, b: &mut ProfileDoc) {
    b.import(&a.export_all_updates().unwrap()).unwrap();
}

#[test]
fn a_profile_round_trips_through_a_snapshot() {
    let id = ProfileId::new();
    let mut p = ProfileDoc::create(id, "Work", Some("sam@acme.com")).unwrap();
    let ws = p.add_workspace("Example", T0).unwrap();

    let reopened = ProfileDoc::from_snapshot(&p.export_snapshot().unwrap()).unwrap();
    assert_eq!(reopened.id(), Some(id));
    assert_eq!(reopened.name().as_deref(), Some("Work"));
    assert_eq!(reopened.email().as_deref(), Some("sam@acme.com"));
    assert_eq!(reopened.schema_version(), Some(PROFILE_SCHEMA_VERSION));
    assert_eq!(reopened.workspaces(), vec![ws]);
}

/// Expansion is document *configuration*, not persisted data, so a document
/// loaded from a snapshot has none until it is set again. Miss this and pills in
/// the profile library start swallowing the text typed beside them — but only
/// after a restart, which is the worst way to find a bug.
#[test]
fn styles_are_configured_on_create_and_on_reopen() {
    let mut p = profile();
    let node = p
        .insert_library(Node::new(Kind::Contact, Content::text("Kim")).unwrap())
        .unwrap();
    let reopened = ProfileDoc::from_snapshot(&p.export_snapshot().unwrap()).unwrap();

    // The observable proof that `configure_styles` ran: a pill mark is
    // configured `ExpandType::None`, and an unconfigured document rejects the
    // mark outright rather than expanding it.
    let text = reopened
        .library()
        .get_meta(reopened.index[&node])
        .unwrap()
        .get("text")
        .expect("a text node keeps its container");
    let ValueOrContainer::Container(c) = text else {
        panic!("text is a container");
    };
    let t = c.into_text().unwrap();
    t.mark(0..1, crate::text::PILL_KEY, "x")
        .expect("styles are configured, so the mark is accepted");
}

/// The one that decides the container type. Adoption re-registers every
/// workspace folder it finds on **every** open, so two windows on one profile
/// folder both call `ensure_workspace` with the same id and must not produce
/// two entries.
#[test]
fn two_replicas_adopting_the_same_workspace_converge_on_one_entry() {
    let id = WorkspaceId::new();
    let mut a = profile();
    let mut b = ProfileDoc::from_snapshot(&a.export_snapshot().unwrap()).unwrap();

    a.ensure_workspace(id, "Example", T0).unwrap();
    b.ensure_workspace(id, "Example", T1).unwrap();

    let updates = b.export_all_updates().unwrap();
    a.import(&updates).unwrap();
    merged(&a, &mut b);

    assert_eq!(a.workspaces().len(), 1, "{:?}", a.workspaces());
    assert_eq!(a.workspaces(), b.workspaces(), "and they agree");
}

/// Two *different* workspaces made at once are two workspaces. The map must
/// merge them, not pick one.
#[test]
fn two_replicas_adding_a_workspace_concurrently_keep_both() {
    let mut a = profile();
    let mut b = ProfileDoc::from_snapshot(&a.export_snapshot().unwrap()).unwrap();

    a.add_workspace("Client work", T0).unwrap();
    b.add_workspace("Reading", T1).unwrap();

    let from_b = b.export_all_updates().unwrap();
    a.import(&from_b).unwrap();
    merged(&a, &mut b);

    let names: Vec<String> = a.workspaces().into_iter().map(|w| w.name).collect();
    assert_eq!(names, vec!["Client work", "Reading"], "oldest first");
    assert_eq!(a.workspaces(), b.workspaces());
}

#[test]
fn an_explicit_workspace_order_overrides_creation_order() {
    let mut p = profile();
    let a = p.add_workspace("A", T0).unwrap();
    let b = p.add_workspace("B", T1).unwrap();
    p.set_workspace_order(a.id, 1).unwrap();
    p.set_workspace_order(b.id, 0).unwrap();
    let names: Vec<_> = p.workspaces().into_iter().map(|w| w.name).collect();
    assert_eq!(names, vec!["B", "A"]);
}

/// `ensure_workspace` runs on every open. If it renamed, every launch would
/// silently undo the rename before the window appeared.
#[test]
fn ensure_workspace_is_idempotent_and_does_not_rename() {
    let mut p = profile();
    let id = WorkspaceId::new();
    p.ensure_workspace(id, "Example", T0).unwrap();
    p.rename_workspace(id, "Q3 planning").unwrap();

    // A second open re-registers it with the name it found on disk.
    let again = p.ensure_workspace(id, "Example", T2).unwrap();

    assert_eq!(again.name, "Q3 planning", "the rename survives adoption");
    assert_eq!(again.created, T0, "and so does when it was registered");
    assert_eq!(p.workspaces().len(), 1);
}

#[test]
fn renaming_a_workspace_survives_a_merge() {
    let id = WorkspaceId::new();
    let mut a = profile();
    a.ensure_workspace(id, "Example", T0).unwrap();
    let mut b = ProfileDoc::from_snapshot(&a.export_snapshot().unwrap()).unwrap();

    a.rename_workspace(id, "Q3 planning").unwrap();
    merged(&a, &mut b);

    assert_eq!(b.workspace(id).unwrap().name, "Q3 planning");
}

/// Icon, colour and the two flags are what a workspace looks like in the
/// switcher, and they have to survive the trip through disk like the name does.
#[test]
fn a_workspaces_identity_round_trips_through_a_snapshot() {
    let mut p = profile();
    let id = p.add_workspace("Research", T0).unwrap().id;
    p.set_workspace_icon(id, Some("🔬")).unwrap();
    p.set_workspace_color(id, Some("teal")).unwrap();
    p.set_workspace_pinned(id, true).unwrap();
    p.set_workspace_archived(id, true).unwrap();

    let reopened = ProfileDoc::from_snapshot(&p.export_snapshot().unwrap()).unwrap();
    let w = reopened.workspace(id).unwrap();

    assert_eq!(w.icon.as_deref(), Some("🔬"));
    assert_eq!(w.color.as_deref(), Some("teal"));
    assert!(w.pinned && w.archived);
    assert_eq!(w.name, "Research", "and the name is untouched by any of it");
}

/// Each setter clears with `None`/`false`, and clearing leaves the entry exactly
/// as it was before the field was ever set — which is what makes "was this ever
/// pinned?" an unaskable question, and therefore not one anything can rely on.
#[test]
fn clearing_a_workspaces_identity_is_indistinguishable_from_never_setting_it() {
    let mut p = profile();
    let id = p.add_workspace("Research", T0).unwrap().id;
    let untouched = p.workspace(id).unwrap();

    p.set_workspace_icon(id, Some("🔬")).unwrap();
    p.set_workspace_color(id, Some("teal")).unwrap();
    p.set_workspace_pinned(id, true).unwrap();
    p.set_workspace_archived(id, true).unwrap();

    p.set_workspace_icon(id, None).unwrap();
    p.set_workspace_color(id, None).unwrap();
    p.set_workspace_pinned(id, false).unwrap();
    p.set_workspace_archived(id, false).unwrap();

    assert_eq!(p.workspace(id).unwrap(), untouched);
}

/// **The migration-free claim, pinned.** An entry written by a build that
/// predates icon, colour, pinned and archived has none of those keys. It must
/// still read — as a workspace with none of them set — or every document
/// written before this change would need a migration to open.
///
/// `ensure_workspace` writes exactly `name` and `created`, which is what such a
/// build wrote, so this is that entry rather than an imitation of it.
#[test]
fn an_entry_written_before_these_keys_existed_still_reads() {
    let mut p = profile();
    let id = WorkspaceId::new();
    p.ensure_workspace(id, "Example", T0).unwrap();

    let reopened = ProfileDoc::from_snapshot(&p.export_snapshot().unwrap()).unwrap();
    let w = reopened.workspace(id).unwrap();

    assert_eq!(w.name, "Example");
    assert_eq!(w.created, T0);
    assert_eq!(w.icon, None);
    assert_eq!(w.color, None);
    assert!(!w.pinned);
    assert!(!w.archived);
}

/// Two windows on one folder both write this map. A colour set on one and an
/// icon set on the other are edits to different keys of the same entry, so both
/// have to survive — losing one would mean the switcher showed different things
/// in the two windows until somebody reopened.
#[test]
fn an_icon_and_a_colour_set_concurrently_both_survive() {
    let id = WorkspaceId::new();
    let mut a = profile();
    a.ensure_workspace(id, "Research", T0).unwrap();
    let mut b = ProfileDoc::from_snapshot(&a.export_snapshot().unwrap()).unwrap();

    a.set_workspace_icon(id, Some("🔬")).unwrap();
    b.set_workspace_color(id, Some("teal")).unwrap();
    merged(&a, &mut b);

    let w = b.workspace(id).unwrap();
    assert_eq!(w.icon.as_deref(), Some("🔬"));
    assert_eq!(w.color.as_deref(), Some("teal"));
}

/// Dropping a workspace from the list must be exactly that. Nothing here can
/// reach a folder, and the name says so — the safest delete is one the type
/// system cannot express.
#[test]
fn forgetting_a_workspace_removes_the_entry_only() {
    let mut p = profile();
    let keep = p.add_workspace("Keep", T0).unwrap();
    let drop = p.add_workspace("Drop", T1).unwrap();

    p.forget_workspace(drop.id).unwrap();

    assert_eq!(p.workspaces(), vec![keep]);
    assert_eq!(p.workspace(drop.id), None);
}

/// The switcher lists these, so the order has to be the same on every launch
/// and on every machine — including when two were made in the same millisecond.
#[test]
fn workspaces_come_back_in_a_stable_order() {
    let mut p = profile();
    let third = p.add_workspace("third", T2).unwrap();
    let first = p.add_workspace("first", T0).unwrap();
    let same_a = p.add_workspace("same a", T1).unwrap();
    let same_b = p.add_workspace("same b", T1).unwrap();

    let got: Vec<WorkspaceId> = p.workspaces().into_iter().map(|w| w.id).collect();
    assert_eq!(got[0], first.id);
    assert_eq!(got[3], third.id);

    // The two sharing a timestamp break on id, so the pair is ordered even
    // though nothing about them says which came first.
    let mut tie = [same_a.id, same_b.id];
    tie.sort_by_key(|i| i.to_storage());
    assert_eq!(&got[1..3], &tie);
}

/// The gate is the profile's own, not the workspace's. A build that reads
/// workspaces happily may still be too old for the list of them.
#[test]
fn a_newer_profile_document_is_not_writable() {
    let p = profile();
    assert!(p.compatibility().writable());

    // Forge a version from the future the only way there is: write the key.
    p.doc()
        .get_map(crate::schema::C_META)
        .insert(crate::schema::K_SCHEMA, PROFILE_SCHEMA_VERSION + 1)
        .unwrap();
    p.commit();

    assert!(!p.compatibility().writable());
    assert!(p.compatibility().describe("profile").contains("profile"));
}

/// `locations` says *where to look*, never *what is there*. It is a plain map so
/// that stays obvious.
#[test]
fn locations_map_a_node_to_a_workspace() {
    let mut p = profile();
    let node = NodeId::new();
    let ws = WorkspaceId::new();

    assert_eq!(p.locate(node), None, "unknown is the normal state");
    p.record_location(node, ws).unwrap();
    assert_eq!(p.locate(node), Some(ws));

    p.forget_location(node).unwrap();
    assert_eq!(p.locate(node), None);
}

/// The profile library is the third home a node can have. It uses the same
/// encoder as a workspace's, which is what lets a node keep its `NodeId` when it
/// is eventually promoted across the document boundary.
#[test]
fn the_library_holds_nodes_with_the_same_envelope() {
    let mut p = profile();
    let id = p
        .insert_library(Node::new(Kind::Contact, Content::text("Kim Alvarez")).unwrap())
        .unwrap();

    assert_eq!(p.library_children().unwrap(), vec![id]);
    let node = p.read(id).unwrap();
    assert_eq!(node.id, id);
    assert_eq!(node.kind, Kind::Contact);
    assert_eq!(node.content.as_text(), Some("Kim Alvarez"));

    // And it survives the trip through disk, reindexed against new tree ids.
    let reopened = ProfileDoc::from_snapshot(&p.export_snapshot().unwrap()).unwrap();
    assert_eq!(reopened.read(id).unwrap().id, id);
}

/// Identity is the uuid in the meta map, not Loro's `TreeID` — which is exactly
/// what makes a folder copied to another machine the *same* profile.
#[test]
fn a_copied_profile_is_the_same_profile() {
    let mut a = profile();
    a.add_workspace("Example", T0).unwrap();
    let copy = ProfileDoc::from_snapshot(&a.export_snapshot().unwrap()).unwrap();

    assert_eq!(copy.id(), a.id(), "the copy is not a new profile");
    assert_eq!(copy.workspaces(), a.workspaces());
}
