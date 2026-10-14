// SPDX-License-Identifier: MPL-2.0

//! What deleting a workspace is allowed to do.
//!
//! Disk, deliberately, for the same reason `profilefolder`'s tests use it: the
//! subject *is* what is in a folder, and a mocked filesystem would be asserting
//! the mock. The one thing worth checking without disk is the naming, because
//! that is what makes two deletes of one workspace not collide.

use super::*;
use std::fs;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("koru-trash-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// A workspace folder with something in it, so a move that lost content would
/// show.
fn workspace(profile: &Path, id: WorkspaceId) -> PathBuf {
    let dir = profile.join("workspaces").join(id.to_storage());
    fs::create_dir_all(dir.join("updates")).unwrap();
    fs::write(dir.join("snapshot.loro"), b"the document").unwrap();
    fs::write(dir.join("updates").join("peer-A.loro"), b"an update").unwrap();
    dir
}

/// Delete, restore by hand, delete again — the two must live side by side. A
/// name without the stamp would put the second on top of the first, which is
/// the one way this module could lose the thing it exists to keep.
#[test]
fn two_deletes_of_one_workspace_do_not_collide() {
    let id = WorkspaceId::new();
    assert_ne!(
        destination_name(id, "20260101-120000"),
        destination_name(id, "20260101-120001")
    );
    assert!(destination_name(id, "20260101-120000").starts_with(&id.to_storage()));
}

#[test]
fn deleting_moves_the_folder_and_keeps_every_byte() {
    let profile = temp("move");
    let id = WorkspaceId::new();
    let from = workspace(&profile, id);

    let to = move_to_trash(&profile, id).unwrap();

    assert!(!from.exists(), "the workspace left `workspaces/`");
    assert!(to.starts_with(trash_dir(&profile)));
    assert_eq!(
        fs::read(to.join("snapshot.loro")).unwrap(),
        b"the document",
        "the document went with it"
    );
    assert_eq!(
        fs::read(to.join("updates").join("peer-A.loro")).unwrap(),
        b"an update",
        "and so did the peer log"
    );

    let _ = fs::remove_dir_all(&profile);
}

/// A workspace that is not there is an error rather than a silent success —
/// otherwise the caller forgets its entry believing it moved a folder that was
/// never found, and the workspace becomes unreachable rather than deleted.
#[test]
fn deleting_a_workspace_that_is_not_there_fails() {
    let profile = temp("missing");
    assert!(move_to_trash(&profile, WorkspaceId::new()).is_err());
    let _ = fs::remove_dir_all(&profile);
}

#[test]
fn the_trash_is_pruned_by_age_and_only_by_age() {
    let profile = temp("prune");
    let old = trash_dir(&profile).join("old");
    let new = trash_dir(&profile).join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();

    // "Now" is a month and a day after the folders were written, so both are
    // older than they look to a clock reading the real time.
    prune_trash(
        &profile,
        SystemTime::now() + KEEP_FOR + Duration::from_secs(60),
    );
    assert!(!old.exists() && !new.exists(), "both were past the window");

    fs::create_dir_all(&old).unwrap();
    prune_trash(&profile, SystemTime::now());
    assert!(old.exists(), "a folder written just now is not stale");

    let _ = fs::remove_dir_all(&profile);
}

/// The number beside a workspace has to mean "what I would lose", not "what is
/// in this directory". `backups/` holds up to twenty copies of the document, so
/// counting it into one total reports a small workspace as an enormous one.
#[test]
fn size_separates_the_document_from_its_backups() {
    let profile = temp("size");
    let id = WorkspaceId::new();
    let dir = workspace(&profile, id);
    fs::create_dir_all(dir.join("backups")).unwrap();
    fs::write(dir.join("backups").join("snapshot-1.loro"), vec![0u8; 5000]).unwrap();

    let sizes = size_of(&dir);

    assert_eq!(
        sizes.live,
        ("the document".len() + "an update".len()) as u64
    );
    assert_eq!(sizes.backups, 5000);
    assert!(
        sizes.backups > sizes.live * 100,
        "which is exactly why one number would mislead"
    );

    let _ = fs::remove_dir_all(&profile);
}

/// Nothing is a valid answer. A workspace directory that will not read must not
/// make a whole listing fail.
#[test]
fn an_unreadable_directory_measures_zero() {
    assert_eq!(size_of(Path::new("no/such/place")), Sizes::default());
}
