// SPDX-License-Identifier: MPL-2.0

//! What is pinned here is the *format*, because the writer is in another
//! language and another process. Every test builds its own inbox in a
//! temporary directory; none of them can reach the developer's real one.

use super::*;

fn inbox() -> (tempfile::TempDir, Inbox) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let inbox = Inbox::at(dir.path().join(DIR_NAME));
    inbox.ensure().expect("the inbox directory");
    (dir, inbox)
}

/// Stage one text payload the way the extension does — payload first, manifest
/// last — and answer the entry directory.
fn stage(inbox: &Inbox, payload: &str, name: &str, workspace: Option<&str>) -> PathBuf {
    let entry = inbox.begin().expect("an entry directory");
    let file = uuid::Uuid::new_v4().to_string();
    fs::write(entry.join(&file), payload).expect("the payload");
    inbox
        .commit(
            &entry,
            &Manifest {
                files: vec![File {
                    file,
                    name: name.into(),
                    mime_type: "text/plain".into(),
                    declared_size: Some(payload.len() as i64),
                }],
                workspace_id: workspace.map(str::to_owned),
            },
        )
        .expect("the commit marker");
    entry
}

#[test]
fn a_committed_entry_round_trips() {
    let (_dir, inbox) = inbox();
    stage(&inbox, "hello", "Note.txt", None);

    let drained = inbox.drain();
    assert_eq!(drained.len(), 1);
    let entry = &drained[0];
    assert_eq!(entry.manifest.files.len(), 1);
    assert_eq!(entry.manifest.files[0].name, "Note.txt");
    assert_eq!(entry.manifest.files[0].mime_type, "text/plain");
    assert_eq!(entry.manifest.files[0].declared_size, Some(5));
    assert_eq!(entry.manifest.workspace_id, None);

    let payloads = entry.payloads().expect("the payload resolves");
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0].1, "Note.txt");
    assert_eq!(fs::read_to_string(&payloads[0].0).unwrap(), "hello");
}

/// The commit marker is the whole crash-safety story: payloads without one are
/// an extension that died mid-copy, and they are not a share.
#[test]
fn an_uncommitted_entry_is_invisible() {
    let (_dir, inbox) = inbox();
    let entry = inbox.begin().expect("an entry directory");
    fs::write(entry.join("payload"), "half a share").expect("the payload");

    assert!(inbox.drain().is_empty());
    assert!(entry.is_dir(), "a young uncommitted entry is left alone");
}

/// ...but only until it is old enough that no extension could still be copying
/// into it. A young one may be an extension mid-copy right now, so sweeping on
/// sight would delete a share while it was being written.
#[test]
fn an_old_uncommitted_entry_is_swept_and_a_young_one_is_not() {
    let (_dir, inbox) = inbox();
    let entry = inbox.begin().expect("an entry directory");
    fs::write(entry.join("payload"), "debris").expect("the payload");

    inbox.drain_at(SystemTime::now());
    assert!(entry.is_dir(), "not yet");

    inbox.drain_at(SystemTime::now() + SWEEP_AFTER + Duration::from_secs(1));
    assert!(!entry.exists(), "an hour-old crash is debris");
}

/// A committed entry is never swept, however old. Nothing purges on a timer —
/// the same rule the document bin follows.
#[test]
fn a_committed_entry_is_never_swept() {
    let (_dir, inbox) = inbox();
    stage(&inbox, "hello", "Note.txt", None);
    let drained = inbox.drain_at(SystemTime::now() + SWEEP_AFTER * 100);
    assert_eq!(drained.len(), 1);
}

#[test]
fn entries_drain_oldest_first() {
    let (_dir, inbox) = inbox();
    for index in 0..4 {
        stage(&inbox, "x", &format!("{index}.txt"), None);
        // Filesystem timestamps are coarse; the id tie-break is what makes the
        // order defined at all, and this only has to prove it is *stable*.
    }
    let first: Vec<String> = inbox
        .drain()
        .into_iter()
        .map(|entry| entry.manifest.files[0].name.clone())
        .collect();
    let again: Vec<String> = inbox
        .drain()
        .into_iter()
        .map(|entry| entry.manifest.files[0].name.clone())
        .collect();
    assert_eq!(first, again, "the drain order does not depend on read_dir");
    assert_eq!(first.len(), 4);
}

#[test]
fn a_chosen_workspace_round_trips_through_the_manifest() {
    let (_dir, inbox) = inbox();
    stage(&inbox, "x", "Note.txt", Some("ws-1"));
    let drained = inbox.drain();
    assert_eq!(drained[0].manifest.workspace_id.as_deref(), Some("ws-1"));
}

/// **The literal fixture from
/// `apps/ios/KoruMobileTests/ShareInboxTests.swift`.** It is the one
/// byte-level agreement between the Swift writer and this reader, so it is
/// copied rather than described: camelCase keys, and an absent `workspaceId`
/// (Swift's synthesized encoder omits a nil optional, which is the format's
/// whole versioning story).
#[test]
fn an_ios_manifest_decodes() {
    let ios =
        br#"{"files":[{"file":"p","name":"Note.txt","mimeType":"text/plain","declaredSize":5}]}"#;
    let manifest: Manifest = serde_json::from_slice(ios).expect("the iOS fixture decodes");
    assert_eq!(manifest.workspace_id, None);
    assert_eq!(manifest.files[0].file, "p");
    assert_eq!(manifest.files[0].name, "Note.txt");
    assert_eq!(manifest.files[0].mime_type, "text/plain");
    assert_eq!(manifest.files[0].declared_size, Some(5));
}

/// **A whole entry as `ShareInbox.swift` actually wrote it**, drained the way
/// the app drains it.
///
/// The bytes below were captured from a real run of the Swift writer against
/// `apps/ios/KoruShared/ShareInbox.swift` — the same file the macOS
/// appex compiles — so this is the end-to-end agreement rather than a Rust
/// test's idea of what Swift emits. Three things in it are Swift's and would
/// not have been guessed: the two-space `.prettyPrinted` layout with a space
/// before every colon, `.sortedKeys` ordering `declaredSize` ahead of `file`,
/// and `/` escaped as `\/`, which is legal JSON and which a hand-written
/// fixture would have spelled plainly.
///
/// The first entry is a shared Safari link — `SharedText.normalize` turned
/// `https://koru.do/a?b=c` into a `.url` file of `text/uri-list`, which is why
/// `koru_import_files::mime_for_name` had to learn that extension. The second
/// carries a `workspaceId` and omits `declaredSize`, which is the present/absent
/// optional pair in one entry.
#[test]
fn an_entry_written_by_the_swift_extension_drains() {
    let (_dir, inbox) = inbox();

    let link_payload = "4D10148D-0DB8-4CB8-B9E4-FE67943EDB89";
    let link = inbox.begin().expect("an entry directory");
    fs::write(link.join(link_payload), "https://koru.do/a?b=c\n").expect("the payload");
    fs::write(
        link.join(MANIFEST_FILE),
        br#"{
  "files" : [
    {
      "declaredSize" : 22,
      "file" : "4D10148D-0DB8-4CB8-B9E4-FE67943EDB89",
      "mimeType" : "text\/uri-list",
      "name" : "Shared link.url"
    }
  ]
}"#,
    )
    .expect("the commit marker");

    let text_payload = "BDE2765F-D7B1-432B-B176-214725C1C942";
    let chosen = inbox.begin().expect("an entry directory");
    fs::write(chosen.join(text_payload), "hello").expect("the payload");
    fs::write(
        chosen.join(MANIFEST_FILE),
        br#"{
  "files" : [
    {
      "file" : "BDE2765F-D7B1-432B-B176-214725C1C942",
      "mimeType" : "text\/plain",
      "name" : "evil na me"
    }
  ],
  "workspaceId" : "ws-1"
}"#,
    )
    .expect("the commit marker");

    let drained = inbox.drain();
    assert_eq!(drained.len(), 2);

    let by_name = |name: &str| {
        drained
            .iter()
            .find(|entry| entry.manifest.files[0].name == name)
            .unwrap_or_else(|| panic!("{name} was not drained"))
    };

    let link = by_name("Shared link.url");
    assert_eq!(link.manifest.files[0].mime_type, "text/uri-list");
    assert_eq!(link.manifest.files[0].declared_size, Some(22));
    assert_eq!(link.manifest.workspace_id, None);
    let payloads = link.payloads().expect("the payload resolves");
    assert_eq!(payloads[0].0.file_name().unwrap(), link_payload);
    assert_eq!(payloads[0].1, "Shared link.url");
    assert_eq!(
        fs::read_to_string(&payloads[0].0).unwrap(),
        "https://koru.do/a?b=c\n"
    );

    // `safeDisplayName` already took the control character and the separator
    // out on the Swift side; the display name is never a path on either.
    let text = by_name("evil na me");
    assert_eq!(text.manifest.files[0].declared_size, None);
    assert_eq!(text.manifest.workspace_id.as_deref(), Some("ws-1"));
    assert!(text.payloads().is_ok());
}

/// The other direction: what this crate writes has to be what Swift's
/// `JSONDecoder` reads back, which means camelCase keys and no `null`s where
/// Swift expects an absent key.
#[test]
fn what_rust_writes_is_shaped_like_what_swift_writes() {
    let manifest = Manifest {
        files: vec![File {
            file: "p".into(),
            name: "Note.txt".into(),
            mime_type: "text/plain".into(),
            declared_size: None,
        }],
        workspace_id: None,
    };
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(json.contains("\"mimeType\""), "{json}");
    assert!(
        !json.contains("declaredSize"),
        "a nil optional is absent: {json}"
    );
    assert!(
        !json.contains("workspaceId"),
        "a nil optional is absent: {json}"
    );
    assert!(
        !json.contains("null"),
        "Swift's decoder wants absence, not null: {json}"
    );
}

// ------------------------------------------------------- the path guard

/// The writer is a different process. A manifest that names a path rather than
/// a file would turn a buggy — or hostile — extension into a file-read
/// primitive against whatever the app can reach, so the reader is where the
/// "never a path" comment stops being a comment.
#[test]
fn a_payload_name_that_is_a_path_is_refused() {
    let (_dir, inbox) = inbox();
    let entry = inbox.begin().expect("an entry directory");
    for hostile in ["../escape", "..", ".", "", "a/b", "a\\b", "/etc/passwd"] {
        let manifest = Manifest {
            files: vec![File {
                file: hostile.into(),
                name: "Note.txt".into(),
                mime_type: "text/plain".into(),
                declared_size: None,
            }],
            workspace_id: None,
        };
        inbox.commit(&entry, &manifest).expect("the commit marker");
        let drained = inbox.drain();
        assert_eq!(drained.len(), 1);
        assert!(
            drained[0].payloads().is_err(),
            "{hostile:?} was accepted as a payload name"
        );
    }
}

/// A manifest listing a payload that never landed is a torn entry that reached
/// `commit` anyway — refused whole rather than imported short, because half a
/// share silently becoming one document is the failure nobody would notice.
#[test]
fn a_manifest_naming_a_missing_payload_is_refused() {
    let (_dir, inbox) = inbox();
    let entry = inbox.begin().expect("an entry directory");
    inbox
        .commit(
            &entry,
            &Manifest {
                files: vec![File {
                    file: "never-copied".into(),
                    name: "Note.txt".into(),
                    mime_type: "text/plain".into(),
                    declared_size: None,
                }],
                workspace_id: None,
            },
        )
        .expect("the commit marker");
    assert!(inbox.drain()[0].payloads().is_err());
}

/// A drain over a directory that does not exist is "nothing was shared", not an
/// error — the app calls this on every activation, including on a machine where
/// no share extension has ever run.
#[test]
fn an_absent_inbox_drains_to_nothing() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let inbox = Inbox::at(dir.path().join("never-created"));
    assert!(inbox.drain().is_empty());
}

// ------------------------------------------------------- the extension's names

/// `pluginkit` keys a plug-in by its bundle identifier, so five channels
/// installed side by side must not share one — four of them would lose the
/// registration. Apple also requires the extension's identifier to be prefixed
/// by its containing app's.
#[test]
fn every_channel_gets_its_own_extension_identifier() {
    let mut seen = std::collections::HashSet::new();
    for segment in koru_channel::SEGMENTS {
        let id = share_extension_bundle_id(segment);
        assert!(
            id.starts_with(&format!("{DESKTOP_BUNDLE_ID}.")),
            "{id} is not prefixed by the app's identifier"
        );
        assert!(seen.insert(id.clone()), "{id} is claimed by two channels");
    }
    assert_eq!(seen.len(), koru_channel::SEGMENTS.len());
}

/// Five apps all called "Koru" would be five identical rows in the sheet with
/// nothing to choose between them. `stable` is the product and reads as
/// itself; every other build says which one it is.
#[test]
fn the_sheet_row_names_the_channel_unless_it_is_stable() {
    assert_eq!(share_extension_display_name("stable"), "Koru");
    let mut seen = std::collections::HashSet::new();
    for segment in koru_channel::SEGMENTS {
        assert!(
            seen.insert(share_extension_display_name(segment)),
            "{segment} shares a row label with another channel"
        );
    }
}

/// The sandbox answer is not known, so both places are looked in. On macOS
/// that is two roots; anywhere else the question does not arise.
#[test]
fn a_sandboxed_extensions_container_is_also_drained() {
    let roots = every_inbox();
    if std::env::var_os("HOME").is_none() {
        return;
    }
    if cfg!(target_os = "macos") {
        assert_eq!(roots.len(), 2, "the channel root and the appex container");
        let container = roots[1].root().to_string_lossy().into_owned();
        assert!(container.contains("Library/Containers"), "{container}");
        assert!(
            container.contains(&share_extension_bundle_id(koru_channel::segment())),
            "the container is named by the extension's identifier: {container}"
        );
        assert!(container.ends_with(DIR_NAME), "{container}");
    } else {
        assert_eq!(roots.len(), 1);
    }
}

/// Where the inbox lives, in one place, because the Swift half spells it too.
#[test]
fn the_inbox_sits_under_the_channel_root() {
    assert_eq!(DIR_NAME, "share-inbox");
    let Some(inbox) = for_this_channel() else {
        return; // no HOME; nothing to assert about
    };
    let root = koru_channel::channel_root().expect("a channel root");
    assert_eq!(inbox.root(), root.join("share-inbox"));
    // Beside `store/` and `agent/`, never inside either — a share is not the
    // Store's, and the extension must never be pointed at a database.
    assert_ne!(inbox.root(), koru_channel::store_root_under(&root));
}
