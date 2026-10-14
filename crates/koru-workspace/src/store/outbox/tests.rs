// SPDX-License-Identifier: MPL-2.0

use super::*;

fn ids() -> (StoreWorkspaceId, StoreDocumentId) {
    (StoreWorkspaceId::new(), StoreDocumentId::new())
}

/// A real version vector off a real document, so the round trip is not
/// comparing two empties and calling it agreement — and so `includes_vv` is
/// answering about history Loro actually built.
fn a_version(text: &str) -> loro::VersionVector {
    let doc = loro::LoroDoc::new();
    doc.get_text("t").insert(0, text).expect("insert");
    doc.commit();
    doc.oplog_vv()
}

/// A document's version before and after more was typed into it, so the second
/// genuinely includes the first.
fn before_and_after() -> (loro::VersionVector, loro::VersionVector) {
    let doc = loro::LoroDoc::new();
    let text = doc.get_text("t");
    text.insert(0, "first").expect("insert");
    doc.commit();
    let before = doc.oplog_vv();
    text.insert(5, " and second").expect("insert");
    doc.commit();
    (before, doc.oplog_vv())
}

#[test]
fn what_was_written_is_what_comes_back() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    let acked = a_version("what was acked");

    write(
        Some(root.path()),
        workspace,
        document,
        0xfeed,
        &acked,
        b"the updates",
    )
    .expect("write");

    let (found, refused) = read_all(Some(root.path()), workspace, document);
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].replica, 0xfeed);
    assert_eq!(found[0].acked, acked);
    assert_eq!(found[0].updates, b"the updates");
}

/// The whole reason there is no lock: two replicas of one document are two
/// files, and neither writer has to know the other exists.
#[test]
fn two_replicas_of_one_document_do_not_overwrite_each_other() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();

    write(
        Some(root.path()),
        workspace,
        document,
        1,
        &a_version("a"),
        b"one",
    )
    .expect("write");
    write(
        Some(root.path()),
        workspace,
        document,
        2,
        &a_version("b"),
        b"two",
    )
    .expect("write");

    let (found, _) = read_all(Some(root.path()), workspace, document);
    assert_eq!(found.len(), 2);
    // Sorted by replica, so this is an assertion and not a coin toss.
    assert_eq!(found[0].updates, b"one");
    assert_eq!(found[1].updates, b"two");
}

/// A replica replaces its own file rather than accumulating one per write —
/// each buffer is the whole of what that replica owes, not an increment.
#[test]
fn a_replica_replaces_its_own_buffer() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();

    let (before, after) = before_and_after();
    write(Some(root.path()), workspace, document, 9, &before, b"first").expect("write");
    write(Some(root.path()), workspace, document, 9, &after, b"second").expect("write");

    let (found, _) = read_all(Some(root.path()), workspace, document);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].updates, b"second");
    assert_eq!(found[0].acked, after);
}

/// The guard that keeps replay honest. Loro parks a delta whose ancestors are
/// missing and answers `Ok`, so importing one that cannot apply is silent —
/// the buffer would look delivered and the work would be gone.
#[test]
fn a_delta_the_document_cannot_apply_is_refused_rather_than_imported() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    let (before, after) = before_and_after();

    write(Some(root.path()), workspace, document, 1, &after, b"owed").expect("write");
    let (found, _) = read_all(Some(root.path()), workspace, document);
    let buffered = &found[0];

    // A document that has never heard of this replica at all.
    assert!(!buffered.applies_to(&loro::VersionVector::new()));
    // One that is behind the point the delta starts from — the case that would
    // import `Ok` and apply nothing.
    assert!(!buffered.applies_to(&before));
    // One that is exactly there.
    assert!(buffered.applies_to(&after));
}

/// A build that cannot interpret a buffer has no business destroying it, so an
/// unreadable file is *named* rather than skipped or removed.
#[test]
fn a_file_this_build_cannot_read_is_reported_and_left_alone() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    let dir = dir(Some(root.path()), workspace, document).expect("dir");
    std::fs::create_dir_all(&dir).expect("create");
    let path = dir.join("00000000000000ff.loro");
    std::fs::write(&path, b"not an outbox file at all").expect("write");

    let (found, refused) = read_all(Some(root.path()), workspace, document);
    assert!(found.is_empty());
    assert_eq!(refused.len(), 1);
    assert!(path.exists(), "the file must still be there");
}

/// Replaying a delta written by a build that understood more of the document
/// than this one would be a way around `Compatibility`, which exists to stop
/// exactly that.
#[test]
fn a_buffer_from_a_newer_document_schema_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    write(
        Some(root.path()),
        workspace,
        document,
        1,
        &a_version("x"),
        b"x",
    )
    .expect("write");

    let path = dir(Some(root.path()), workspace, document)
        .expect("dir")
        .join("0000000000000001.loro");
    let mut bytes = std::fs::read(&path).expect("read");
    bytes[8..16].copy_from_slice(&(koru_core::schema::SCHEMA_VERSION + 1).to_le_bytes());
    std::fs::write(&path, &bytes).expect("write");

    let (found, refused) = read_all(Some(root.path()), workspace, document);
    assert!(found.is_empty());
    assert_eq!(refused.len(), 1);
    assert!(path.exists());
}

/// An older one is our own earlier work, and replaying it is ordinary.
#[test]
fn a_buffer_from_an_older_document_schema_still_replays() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    write(
        Some(root.path()),
        workspace,
        document,
        1,
        &a_version("x"),
        b"x",
    )
    .expect("write");

    let path = dir(Some(root.path()), workspace, document)
        .expect("dir")
        .join("0000000000000001.loro");
    let mut bytes = std::fs::read(&path).expect("read");
    bytes[8..16].copy_from_slice(&(koru_core::schema::SCHEMA_VERSION - 1).to_le_bytes());
    std::fs::write(&path, &bytes).expect("write");

    let (found, refused) = read_all(Some(root.path()), workspace, document);
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(found.len(), 1);
}

/// A truncated file is a half-written buffer read back as a whole one. It has
/// to be refused, not trusted — which is why writing goes through a rename.
#[test]
fn a_truncated_file_is_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    write(
        Some(root.path()),
        workspace,
        document,
        1,
        &a_version("z"),
        b"the updates",
    )
    .expect("write");

    let path = dir(Some(root.path()), workspace, document)
        .expect("dir")
        .join("0000000000000001.loro");
    let bytes = std::fs::read(&path).expect("read");
    std::fs::write(&path, &bytes[..HEAD + 1]).expect("truncate");

    let (found, refused) = read_all(Some(root.path()), workspace, document);
    assert!(found.is_empty());
    assert_eq!(refused.len(), 1);
}

/// The `.part` a write goes through must never be read back as a buffer.
#[test]
fn a_half_written_part_file_is_not_read() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    let dir = dir(Some(root.path()), workspace, document).expect("dir");
    std::fs::create_dir_all(&dir).expect("create");
    std::fs::write(dir.join("0000000000000001.part"), b"half of something").expect("write");

    let (found, refused) = read_all(Some(root.path()), workspace, document);
    assert!(found.is_empty());
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn removing_the_last_buffer_takes_its_directories_with_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    let path = write(
        Some(root.path()),
        workspace,
        document,
        1,
        &a_version("x"),
        b"x",
    )
    .expect("write");

    remove(&path);

    assert!(!path.exists());
    assert!(
        !dir(Some(root.path()), workspace, document)
            .expect("dir")
            .exists()
    );
    // And nothing above it was touched.
    assert!(root.path().join(DIR).exists());
}

/// One replica finishing must not take a sibling's buffer with it.
#[test]
fn removing_one_buffer_leaves_another_replicas_alone() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    let mine = write(
        Some(root.path()),
        workspace,
        document,
        1,
        &a_version("x"),
        b"a",
    )
    .expect("write");
    write(
        Some(root.path()),
        workspace,
        document,
        2,
        &a_version("y"),
        b"b",
    )
    .expect("write");

    remove(&mine);

    let (found, _) = read_all(Some(root.path()), workspace, document);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].replica, 2);
}

/// Two documents are two directories, so one workspace's traffic never reads
/// or removes another's.
#[test]
fn documents_do_not_share_a_directory() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, one) = ids();
    let two = StoreDocumentId::new();

    write(
        Some(root.path()),
        workspace,
        one,
        1,
        &a_version("x"),
        b"one",
    )
    .expect("write");
    write(
        Some(root.path()),
        workspace,
        two,
        1,
        &a_version("x"),
        b"two",
    )
    .expect("write");

    let (found, _) = read_all(Some(root.path()), workspace, one);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].updates, b"one");
}

/// Nothing buffered is not an error and not a complaint — it is the ordinary
/// state of a window that is reaching its store.
#[test]
fn an_empty_outbox_is_silent() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = ids();
    let (found, refused) = read_all(Some(root.path()), workspace, document);
    assert!(found.is_empty());
    assert!(refused.is_empty());
}
