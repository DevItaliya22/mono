// SPDX-License-Identifier: MPL-2.0

use super::*;

use crate::WorkspaceStore;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("koru-folder-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Write a workspace directory the way a build predating profiles would.
fn legacy_workspace(dir: &Path, name: &str) -> WorkspaceId {
    let id = WorkspaceId::new();
    let mut store = WorkspaceStore::open_at(dir, id, "A").unwrap();
    let (doc, _) = store.load(id, name).unwrap();
    store.save_snapshot(&doc).unwrap();
    id
}

#[test]
fn a_profile_conf_round_trips() {
    let id = ProfileId::new();
    let text = profile_conf_body(id, "Work", Some("sam@acme.com"));
    assert_eq!(
        parse_profile_conf(&text),
        Some((id, "Work".into(), Some("sam@acme.com".into())))
    );

    // An empty email is absent, not a blank string.
    let text = profile_conf_body(id, "Personal", None);
    assert_eq!(
        parse_profile_conf(&text),
        Some((id, "Personal".into(), None))
    );
}

/// Every Windows editor writes a byte-order mark, and a Koru conf was caught
/// by exactly this once already.
#[test]
fn a_hand_edited_profile_conf_still_parses() {
    let id = ProfileId::new();
    let text = format!(
        "\u{feff}# a comment\n\n  id={}  \n name  =  Work \nnonsense\nemail=\n",
        id.to_storage()
    );
    assert_eq!(parse_profile_conf(&text), Some((id, "Work".into(), None)));
}

/// A file that cannot say *which* profile this is says nothing useful.
#[test]
fn a_profile_conf_without_an_id_is_not_a_profile() {
    assert_eq!(parse_profile_conf("name = Work\nemail = a@b.c\n"), None);
    assert_eq!(parse_profile_conf(""), None);
    assert_eq!(parse_profile_conf("id = not-a-uuid\n"), None);
}

// ------------------------------------------------------------ workspace.conf

fn facts(id: WorkspaceId) -> WorkspaceFacts {
    WorkspaceFacts {
        id,
        name: "Research".into(),
        icon: Some("🔬".into()),
        color: Some("teal".into()),
        pinned: true,
        archived: false,
        docs: Some(76),
    }
}

#[test]
fn a_workspace_conf_round_trips() {
    let id = WorkspaceId::new();
    let text = workspace_conf_body(&facts(id));
    assert_eq!(parse_workspace_conf(&text), Some(facts(id)));
}

/// Everything optional is *absent* rather than empty, so a workspace that never
/// had an icon and one whose icon was cleared produce the same bytes — which is
/// what stops a cleared field reading back as a set one.
#[test]
fn an_unset_field_is_absent_from_the_conf_rather_than_blank() {
    let id = WorkspaceId::new();
    let bare = WorkspaceFacts::new(id, "Sandbox");
    let text = workspace_conf_body(&bare);

    assert!(!text.contains("icon"), "{text}");
    assert!(!text.contains("color"), "{text}");
    assert!(!text.contains("pinned"), "{text}");
    assert!(!text.contains("archived"), "{text}");
    assert!(!text.contains("docs"), "{text}");
    assert_eq!(parse_workspace_conf(&text), Some(bare));
}

/// Same tolerance `profile.conf` has, and for the same reasons: a BOM from a
/// Windows editor, a key from a newer build, and a line that is not a pair at
/// all must none of them cost the file its meaning.
#[test]
fn a_hand_edited_workspace_conf_still_parses() {
    let id = WorkspaceId::new();
    let text = format!(
        "\u{feff}# a comment\n\n  id={}  \n name  =  Research \nnonsense\n\
         future-key = whatever\ncolor=teal\n",
        id.to_storage()
    );
    let parsed = parse_workspace_conf(&text).unwrap();
    assert_eq!(parsed.name, "Research");
    assert_eq!(parsed.color.as_deref(), Some("teal"));
}

/// A file that cannot say *which* workspace it describes describes nothing.
#[test]
fn a_workspace_conf_without_an_id_is_not_a_workspace() {
    assert_eq!(parse_workspace_conf("name = Research\n"), None);
    assert_eq!(parse_workspace_conf(""), None);
    assert_eq!(parse_workspace_conf("id = not-a-uuid\n"), None);
}

/// A folder copied whole — conf and all — into another workspace's directory
/// describes *itself*, not the directory it landed in. The id is the one thing
/// about it that cannot be reconciled, so the file is ignored rather than
/// believed: adopting it would give two workspaces one identity.
#[test]
fn a_conf_naming_a_different_workspace_is_ignored() {
    let root = temp("conf-mismatch");
    let here = WorkspaceId::new();
    let elsewhere = WorkspaceId::new();
    fs::create_dir_all(workspace_dir(&root, here)).unwrap();
    fs::write(
        workspace_conf_path(&root, here),
        workspace_conf_body(&facts(elsewhere)),
    )
    .unwrap();

    assert_eq!(peek_workspace(&root, here), None);
}

/// The read path in full, and the reason the sidecar exists: the name comes back
/// **without decoding the snapshot**. A folder that has no conf yet still
/// answers, from the snapshot, which is what keeps every workspace written
/// before this change readable.
#[test]
fn a_name_comes_from_the_conf_when_there_is_one_and_the_snapshot_when_there_is_not() {
    let root = temp("conf-name");
    let id = legacy_workspace(&root, "From the snapshot");
    assert_eq!(
        peek_workspace_name(&root, id).as_deref(),
        Some("From the snapshot"),
        "no conf yet, so the snapshot answers"
    );

    write_workspace_conf(&root, &WorkspaceFacts::new(id, "From the conf")).unwrap();
    assert_eq!(
        peek_workspace_name(&root, id).as_deref(),
        Some("From the conf")
    );
}

/// The three cases the first-run dialogs branch on. Getting `Legacy` wrong in
/// either direction is the bad one: as `Empty` it offers to create a new profile
/// over somebody's notes, and as `Profile` it opens a profile that is not there.
#[test]
fn an_empty_a_legacy_and_a_profile_folder_are_told_apart() {
    let root = temp("kinds");

    let empty = root.join("empty");
    fs::create_dir_all(&empty).unwrap();
    assert_eq!(inspect(&empty), FolderKind::Empty);
    assert!(!inspect(&empty).holds_content());

    let legacy = root.join("legacy");
    fs::create_dir_all(&legacy).unwrap();
    legacy_workspace(&legacy, "Koru");
    assert_eq!(inspect(&legacy), FolderKind::Legacy);
    assert!(inspect(&legacy).holds_content());

    let id = ProfileId::new();
    let profile = root.join("profile-folder");
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        profile_conf_path(&profile),
        profile_conf_body(id, "Work", Some("sam@acme.com")),
    )
    .unwrap();
    assert_eq!(
        inspect(&profile),
        FolderKind::Profile {
            id: Some(id),
            name: Some("Work".into()),
            email: Some("sam@acme.com".into()),
        }
    );

    // A folder that does not exist at all is not a crash.
    assert_eq!(inspect(&root.join("nope")), FolderKind::Empty);

    let _ = fs::remove_dir_all(&root);
}

/// Deleting `profile.conf` does not stop you owning the profile — the document
/// is what makes the folder one, and the sidecar is rewritten from it.
#[test]
fn a_profile_is_recognised_from_its_document_alone() {
    let root = temp("nosidecar");
    fs::create_dir_all(root.join("profile")).unwrap();
    fs::write(root.join("profile").join("snapshot.loro"), b"bytes").unwrap();

    assert_eq!(
        inspect(&root),
        FolderKind::Profile {
            id: None,
            name: None,
            email: None
        },
        "recognised, but it cannot say whose without the sidecar"
    );

    let _ = fs::remove_dir_all(&root);
}

/// Adoption walks this. A stray folder must not be able to stop a launch.
#[test]
fn every_workspace_directory_is_found_and_anything_else_is_skipped() {
    let root = temp("dirs");
    let a = legacy_workspace(&root, "One");
    let b = legacy_workspace(&root, "Two");
    fs::create_dir_all(root.join("workspaces").join("not-a-uuid")).unwrap();
    fs::write(root.join("workspaces").join("stray.txt"), "x").unwrap();

    let mut found = workspace_dirs(&root);
    let mut want = vec![a, b];
    found.sort_by_key(|i| i.to_storage());
    want.sort_by_key(|i| i.to_storage());
    assert_eq!(found, want);

    // No `workspaces/` at all is an empty list, not an error.
    assert!(workspace_dirs(&temp("dirs-empty")).is_empty());

    let _ = fs::remove_dir_all(&root);
}

/// Adoption names each workspace from its own document, so the switcher shows
/// what the workspace calls itself rather than a uuid.
#[test]
fn a_workspace_name_can_be_read_without_opening_a_session() {
    let root = temp("peek");
    let id = legacy_workspace(&root, "Koru");

    assert_eq!(peek_workspace_name(&root, id).as_deref(), Some("Koru"));
    assert_eq!(
        peek_workspace_name(&root, WorkspaceId::new()),
        None,
        "a workspace that is not there has no name"
    );

    let _ = fs::remove_dir_all(&root);
}

/// One damaged snapshot must not stop the other nine being adopted. The
/// workspace is still registered — under a fallback name — and its own recovery
/// happens when it is opened.
#[test]
fn a_damaged_workspace_snapshot_has_no_name_rather_than_failing() {
    let root = temp("damaged");
    let id = legacy_workspace(&root, "Koru");
    fs::write(
        root.join("workspaces")
            .join(id.to_storage())
            .join("snapshot.loro"),
        b"not a loro snapshot",
    )
    .unwrap();

    assert_eq!(peek_workspace_name(&root, id), None);
    assert_eq!(workspace_dirs(&root), vec![id], "and it is still adopted");

    let _ = fs::remove_dir_all(&root);
}
