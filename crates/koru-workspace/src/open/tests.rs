// SPDX-License-Identifier: MPL-2.0

//! Opening a folder, on disk, the way a launch does it.
//!
//! These use temp directories deliberately. The no-disk rule is about *views*;
//! this module's whole subject is what is in a folder, and a test that mocked
//! the filesystem would be asserting the mock.

use super::*;
use koru_core::node::Node;
use koru_core::schema::Parent;
use koru_docstore::WorkspaceStore;
use std::fs;
use std::path::PathBuf;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("koru-openprofile-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn instance(peer: &str) -> Instance {
    Instance {
        peer: peer.into(),
        slot: peer.into(),
    }
}

/// Write a workspace directory the way a build predating profiles would, with
/// one task in it so there is content to lose.
fn legacy_workspace(dir: &Path, name: &str, task: &str) -> WorkspaceId {
    let id = WorkspaceId::new();
    let mut store = WorkspaceStore::open_at(dir, id, "A").unwrap();
    let (mut doc, _) = store.load(id, name).unwrap();
    doc.insert(Parent::Root, Node::task(task).unwrap()).unwrap();
    store.save_snapshot(&doc).unwrap();
    id
}

#[test]
fn a_brand_new_folder_becomes_a_seeded_profile() {
    let dir = temp("new");
    let opened = open_profile(&dir, &instance("A"), None).unwrap();

    assert!(opened.seeded);
    assert_eq!(opened.adopted, 0);
    assert!(opened.profile_report.fresh);
    assert_eq!(opened.profile.workspaces().len(), 1);
    assert_eq!(opened.profile.workspaces()[0].name, "Example");
    assert_eq!(opened.current, opened.profile.workspaces()[0].id);

    // The document a person lands on.
    let library = opened.workspace.children(Parent::Root).unwrap();
    assert_eq!(library.len(), 1);
    assert!(seed::is_get_started(&opened.workspace, library[0]));
    // ...and it is all the seed wrote: the document plus its body, with no
    // second tree for anything to hide in.
    assert_eq!(opened.workspace.walk().unwrap().len(), 11);

    let _ = fs::remove_dir_all(&dir);
}

/// The seed has to be **bytes on disk** before this returns, not an intention.
/// ADR-21 records what happens when it is not.
#[test]
fn a_seeded_profile_is_on_disk_before_the_first_frame() {
    let dir = temp("flushed");
    let opened = open_profile(&dir, &instance("A"), None).unwrap();
    let ws_id = opened.current;
    // Nothing dropped, nothing saved by us — exactly as it is when the window
    // is about to open.
    assert!(dir.join("profile").join("snapshot.loro").exists());
    assert!(
        dir.join("workspaces")
            .join(ws_id.to_storage())
            .join("snapshot.loro")
            .exists()
    );
    assert!(
        dir.join(koru_docstore::profilefolder::PROFILE_FILE)
            .exists(),
        "and the folder can introduce itself"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reopening_a_seeded_profile_does_not_seed_again() {
    let dir = temp("reseed");
    let first = open_profile(&dir, &instance("A"), None).unwrap();
    let id = first.profile.id();
    let ws = first.current;
    drop(first);

    let second = open_profile(&dir, &instance("A"), None).unwrap();
    assert!(!second.seeded);
    assert!(!second.profile_report.fresh);
    assert_eq!(second.profile.id(), id, "the same profile, not a new one");
    assert_eq!(second.profile.workspaces().len(), 1);
    assert_eq!(second.current, ws);
    assert_eq!(second.workspace.children(Parent::Root).unwrap().len(), 1);

    let _ = fs::remove_dir_all(&dir);
}

/// The ADR-21 scenario, as close as one process can get to it: a second
/// instance opens the folder the moment the first one has returned.
#[test]
fn a_second_instance_on_a_fresh_folder_does_not_seed_a_second_example() {
    let dir = temp("twoinstances");
    let a = open_profile(&dir, &instance("A"), None).unwrap();
    let b = open_profile(&dir, &instance("B"), None).unwrap();

    assert!(a.seeded);
    assert!(!b.seeded, "B found the bytes A had already written");
    assert_eq!(b.profile.workspaces().len(), 1);
    assert_eq!(a.current, b.current, "and they are in the same workspace");

    let _ = fs::remove_dir_all(&dir);
}

/// The compatibility promise. Adoption may create `profile/` and `profile.conf`
/// and must not touch one byte inside `workspaces/`, so an older build can still
/// open the folder while this change is in flight.
#[test]
fn a_legacy_workspaces_folder_is_adopted_without_moving_anything() {
    let dir = temp("legacy");
    let ws_id = legacy_workspace(&dir, "Koru", "written before profiles");
    let ws_dir = dir.join("workspaces").join(ws_id.to_storage());
    let snapshot = ws_dir.join("snapshot.loro");
    let before = fs::read(&snapshot).unwrap();
    let files_before = listing(&ws_dir);

    let opened = open_profile(&dir, &instance("A"), None).unwrap();

    assert_eq!(opened.adopted, 1);
    assert!(!opened.seeded, "adoption is not a first run");
    assert_eq!(opened.current, ws_id, "and it opened what was there");
    assert_eq!(
        opened.workspace.walk().unwrap().len(),
        1,
        "with the content still in it"
    );
    assert_eq!(
        fs::read(&snapshot).unwrap(),
        before,
        "not one byte of the old workspace was rewritten"
    );
    // And nothing was *added* either — including `workspace.conf`, which every
    // non-adopting open writes. The promise is about the directory, not about
    // the document inside it, and a sidecar is exactly the kind of small
    // harmless-looking file that would erode it.
    assert_eq!(
        listing(&ws_dir),
        files_before,
        "adoption added no file to the workspace directory"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The names directly inside a directory, sorted. Enough to catch a file being
/// added; the byte comparison beside it covers a file being changed.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The conf is not written by the launch that adopts the folder — but it *is*
/// written by the next one, or a moved workspace would never gain the file that
/// lets it introduce itself somewhere else.
#[test]
fn an_adopted_workspace_gains_its_sidecar_on_the_second_open() {
    let dir = temp("legacy-second-open");
    let ws_id = legacy_workspace(&dir, "Koru", "written before profiles");
    let conf = koru_docstore::profilefolder::workspace_conf_path(&dir, ws_id);

    drop(open_profile(&dir, &instance("A"), None).unwrap());
    assert!(!conf.exists(), "the adopting launch left the folder alone");

    drop(open_profile(&dir, &instance("A"), None).unwrap());
    let facts = koru_docstore::profilefolder::peek_workspace(&dir, ws_id).unwrap();
    assert_eq!(facts.name, "Koru");
    assert_eq!(facts.docs, Some(0), "a task is not a document");

    let _ = fs::remove_dir_all(&dir);
}

/// **What the sidecar is for.** A workspace folder moved into a profile that has
/// never seen it keeps its name, icon and colour — because the folder carries
/// them, and adoption reads them when there is no entry to disagree with.
#[test]
fn a_workspace_folder_moved_to_another_profile_keeps_its_identity() {
    let from = temp("move-from");
    let opened = open_profile(&from, &instance("A"), None).unwrap();
    let ws_id = opened.current;
    let mut profile = opened.profile;
    let mut profile_session = opened.profile_session;
    profile.rename_workspace(ws_id, "Research").unwrap();
    profile.set_workspace_icon(ws_id, Some("🔬")).unwrap();
    profile.set_workspace_color(ws_id, Some("teal")).unwrap();
    profile.set_workspace_pinned(ws_id, true).unwrap();
    profile_session.flush(&profile).unwrap();
    let entry = profile.workspace(ws_id).unwrap();
    write_workspace_sidecar(&from, &entry, Some(3));
    drop((profile, profile_session, opened.session));

    // Move the folder — and only the folder — into a profile that knows nothing
    // about it. This is somebody dragging a workspace onto a stick and back.
    let to = temp("move-to");
    let dest = to.join("workspaces").join(ws_id.to_storage());
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    copy_dir(&from.join("workspaces").join(ws_id.to_storage()), &dest);

    let landed = open_profile(&to, &instance("B"), Some(ws_id)).unwrap();
    let w = landed.profile.workspace(ws_id).unwrap();

    assert_eq!(w.name, "Research", "not an untitled folder");
    assert_eq!(w.icon.as_deref(), Some("🔬"));
    assert_eq!(w.color.as_deref(), Some("teal"));
    assert!(w.pinned);

    let _ = fs::remove_dir_all(&from);
    let _ = fs::remove_dir_all(&to);
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// The switcher shows what each workspace calls itself, not a uuid.
#[test]
fn adoption_names_every_workspace_it_finds() {
    let dir = temp("names");
    legacy_workspace(&dir, "Koru", "one");
    legacy_workspace(&dir, "Client work", "two");
    legacy_workspace(&dir, "Reading", "three");

    let opened = open_profile(&dir, &instance("A"), None).unwrap();
    assert_eq!(opened.adopted, 3);
    let mut names: Vec<String> = opened
        .profile
        .workspaces()
        .into_iter()
        .map(|w| w.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["Client work", "Koru", "Reading"]);

    let _ = fs::remove_dir_all(&dir);
}

/// Adoption runs on every open of a folder it has already adopted — the folder
/// is `Profile` by then — but the guard has to hold anyway, because two windows
/// can reach it at once.
#[test]
fn adopting_twice_produces_one_entry() {
    let dir = temp("twice");
    let ws_id = legacy_workspace(&dir, "Koru", "content");

    let first = open_profile(&dir, &instance("A"), None).unwrap();
    assert_eq!(first.adopted, 1);
    drop(first);

    let second = open_profile(&dir, &instance("A"), None).unwrap();
    assert_eq!(second.adopted, 0, "there was nothing left to adopt");
    assert_eq!(second.profile.workspaces().len(), 1);
    assert_eq!(second.profile.workspaces()[0].id, ws_id);

    let _ = fs::remove_dir_all(&dir);
}

/// One damaged snapshot must not cost somebody the other nine workspaces. It is
/// still adopted — under a fallback name — and recovers when it is opened.
#[test]
fn a_damaged_workspace_snapshot_does_not_fail_adoption() {
    let dir = temp("damaged");
    let good = legacy_workspace(&dir, "Koru", "fine");
    let bad = legacy_workspace(&dir, "Broken", "gone");
    fs::write(
        dir.join("workspaces")
            .join(bad.to_storage())
            .join("snapshot.loro"),
        b"not a loro snapshot",
    )
    .unwrap();

    let opened = open_profile(&dir, &instance("A"), None).unwrap();
    assert_eq!(opened.adopted, 2, "both, including the damaged one");
    assert_eq!(
        opened.profile.workspace(bad).unwrap().name,
        "Workspace",
        "named by fallback rather than left out"
    );
    assert_eq!(opened.profile.workspace(good).unwrap().name, "Koru");

    let _ = fs::remove_dir_all(&dir);
}

/// `--workspace <id>`, and the window reopening what it had open last.
#[test]
fn a_wanted_workspace_is_the_one_that_opens() {
    let dir = temp("wanted");
    legacy_workspace(&dir, "One", "a");
    let second = legacy_workspace(&dir, "Two", "b");

    let opened = open_profile(&dir, &instance("A"), Some(second)).unwrap();
    assert_eq!(opened.current, second);
    assert_eq!(opened.workspace.name().as_deref(), Some("Two"));

    let _ = fs::remove_dir_all(&dir);
}

/// A workspace id from another profile names content in a folder that is not
/// this one. Obeying it would create an empty directory here under somebody
/// else's id, which then appears in the switcher as a workspace that lost
/// everything.
#[test]
fn a_wanted_workspace_from_another_profile_is_ignored() {
    let dir = temp("stranger");
    let mine = legacy_workspace(&dir, "Mine", "content");
    let stranger = WorkspaceId::new();

    let opened = open_profile(&dir, &instance("A"), Some(stranger)).unwrap();
    assert_eq!(opened.current, mine);
    assert!(
        !dir.join("workspaces").join(stranger.to_storage()).exists(),
        "and nothing was created for it"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The sidecar is a cache of the document and is written from it, so a folder
/// whose `profile.conf` was deleted comes back with the same id — which is what
/// makes a lost registry recoverable by re-picking the folder rather than a way
/// to accidentally mint a second profile over the same content.
#[test]
fn a_folder_alone_reconstructs_its_registry_entry() {
    let dir = temp("sidecar");
    let first = open_profile(&dir, &instance("A"), None).unwrap();
    let id = first.profile.id().unwrap();
    drop(first);

    fs::remove_file(dir.join(koru_docstore::profilefolder::PROFILE_FILE)).unwrap();
    assert_eq!(
        describe(&dir).map(|(id, ..)| id),
        Some(id),
        "the document still says whose folder this is"
    );

    let second = open_profile(&dir, &instance("A"), None).unwrap();
    assert_eq!(second.profile.id(), Some(id), "not a second profile");
    assert!(!second.seeded);
    assert!(
        dir.join(koru_docstore::profilefolder::PROFILE_FILE)
            .exists(),
        "and the sidecar was rewritten from the document"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// A new workspace has to be on disk before the switcher shows a row for it,
/// or the row lies until it is clicked.
#[test]
fn a_created_workspace_is_listed_and_on_disk() {
    let dir = temp("create");
    let mut opened = open_profile(&dir, &instance("A"), None).unwrap();

    let id = create_workspace(
        &dir,
        &mut opened.profile,
        &mut opened.profile_session,
        &instance("A"),
        "Client work",
    )
    .unwrap();

    assert_eq!(opened.profile.workspace(id).unwrap().name, "Client work");
    assert!(
        dir.join("workspaces")
            .join(id.to_storage())
            .join("snapshot.loro")
            .exists()
    );
    // And a fresh open finds it in the list rather than re-seeding.
    drop(opened);
    let again = open_profile(&dir, &instance("A"), Some(id)).unwrap();
    assert_eq!(again.current, id);
    assert_eq!(again.profile.workspaces().len(), 2);

    let _ = fs::remove_dir_all(&dir);
}

/// `--where` and the profile picker read these without opening anything.
#[test]
fn a_profile_can_be_described_without_opening_a_session() {
    let dir = temp("describe");
    let opened = open_profile(&dir, &instance("A"), None).unwrap();
    let id = opened.profile.id().unwrap();
    drop(opened);

    let (got, name, _email) = describe(&dir).unwrap();
    assert_eq!(got, id);
    assert!(!name.is_empty());
    assert_eq!(peek_workspaces(&dir).len(), 1);

    assert_eq!(describe(&temp("describe-empty")), None);
}

// ------------------------------------------------------------------ archive

/// Archiving is "stop showing me this", not "move me somewhere else". A launch
/// with no preference skips archived workspaces when it picks one.
#[test]
fn a_launch_with_no_preference_skips_an_archived_workspace() {
    let dir = temp("archived-fallback");
    let opened = open_profile(&dir, &instance("A"), None).unwrap();
    let first = opened.current;
    let mut profile = opened.profile;
    let mut session = opened.profile_session;
    let second =
        create_workspace(&dir, &mut profile, &mut session, &instance("A"), "Second").unwrap();
    profile.set_workspace_archived(first, true).unwrap();
    session.flush(&profile).unwrap();
    drop((profile, session, opened.session));

    let again = open_profile(&dir, &instance("A"), None).unwrap();
    assert_eq!(again.current, second, "it walked past the archived one");

    let _ = fs::remove_dir_all(&dir);
}

/// **But `--workspace` still wins.** Archiving the workspace this window is
/// showing must not teleport it somewhere else on the next launch.
#[test]
fn asking_for_an_archived_workspace_by_id_still_opens_it() {
    let dir = temp("archived-wanted");
    let opened = open_profile(&dir, &instance("A"), None).unwrap();
    let first = opened.current;
    let mut profile = opened.profile;
    let mut session = opened.profile_session;
    create_workspace(&dir, &mut profile, &mut session, &instance("A"), "Second").unwrap();
    profile.set_workspace_archived(first, true).unwrap();
    session.flush(&profile).unwrap();
    drop((profile, session, opened.session));

    let again = open_profile(&dir, &instance("A"), Some(first)).unwrap();
    assert_eq!(again.current, first);

    let _ = fs::remove_dir_all(&dir);
}

/// **The one that would lose work.** With every workspace archived, the old code
/// fell through to "no workspaces at all" and minted a fresh empty one named
/// Example — which reads exactly like the app having forgotten everything. The
/// UI refuses to reach this state twice over, but a peer can sync it in.
#[test]
fn a_profile_whose_workspaces_are_all_archived_does_not_mint_a_new_one() {
    let dir = temp("all-archived");
    let opened = open_profile(&dir, &instance("A"), None).unwrap();
    let only = opened.current;
    let mut profile = opened.profile;
    let mut session = opened.profile_session;
    profile.set_workspace_archived(only, true).unwrap();
    session.flush(&profile).unwrap();
    drop((profile, session, opened.session));

    let again = open_profile(&dir, &instance("A"), None).unwrap();

    assert_eq!(again.current, only, "it opened the archived one");
    assert_eq!(
        again.profile.workspaces().len(),
        1,
        "and did not invent a second"
    );
    assert!(!again.seeded);

    let _ = fs::remove_dir_all(&dir);
}

/// **The rename authority rule.** The profile entry is the truth; the workspace
/// document's own `meta.name` catches up when it is opened. Without this a
/// rename made while the workspace was closed would silently revert for whoever
/// opened it next — the document would still be saying the old name, and the
/// entry would be refreshed from it.
#[test]
fn a_workspace_renamed_while_closed_catches_up_when_it_is_opened() {
    let dir = temp("rename-reconcile");
    let opened = open_profile(&dir, &instance("A"), None).unwrap();
    let id = opened.current;
    let mut profile = opened.profile;
    let mut session = opened.profile_session;
    profile.rename_workspace(id, "Renamed while shut").unwrap();
    session.flush(&profile).unwrap();
    drop((profile, session, opened.session));

    let again = open_profile(&dir, &instance("A"), None).unwrap();

    assert_eq!(
        again.profile.workspace(id).unwrap().name,
        "Renamed while shut",
        "the entry kept the new name"
    );
    assert_eq!(
        again.workspace.name().as_deref(),
        Some("Renamed while shut"),
        "and the document caught up rather than reverting it"
    );

    let _ = fs::remove_dir_all(&dir);
}
