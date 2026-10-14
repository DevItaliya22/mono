// SPDX-License-Identifier: MPL-2.0

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use koru_session_attachments::{AttachmentLimits, AttachmentStore, IngestRequest};
use koru_session_paths::{SessionPaths, VerifiedAttachmentExtension};
use koru_session_protocol::{
    AttachmentId, AttachmentSource, AttachmentStatus, ErrorCode, SessionId,
};

fn limits() -> AttachmentLimits {
    AttachmentLimits {
        max_bytes: 1024 * 1024,
        max_dimension: 256,
        max_decoded_bytes: 4 * 256 * 256,
        max_per_session: 2,
        preview_edge: 32,
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = RgbaImage::from_pixel(width, height, Rgba([40, 80, 120, 255]));
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

fn make_store(root: &std::path::Path, limits: AttachmentLimits) -> AttachmentStore {
    let sessions = root.join("agent/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    AttachmentStore::new(SessionPaths::from_resolved_directory(sessions), limits).unwrap()
}

fn ingest(
    store: &AttachmentStore,
    session_id: SessionId,
    attachment_id: AttachmentId,
    bytes: Vec<u8>,
) -> Result<koru_session_protocol::Attachment, koru_session_attachments::AttachmentStoreError> {
    store.ingest(IngestRequest {
        session_id,
        attachment_id,
        source: AttachmentSource::Clipboard,
        bytes,
        created_at_ms: 17,
    })
}

#[test]
fn verified_images_publish_atomically_and_previews_are_bounded_and_cached() {
    let root = tempfile::tempdir().unwrap();
    let store = make_store(root.path(), limits());
    let session = SessionId::mint();
    let attachment = AttachmentId::mint();
    store.session_started(session);

    let metadata = ingest(&store, session, attachment, png(80, 40)).unwrap();
    assert_eq!(metadata.media_type, "image/png");
    assert_eq!(
        metadata.byte_count,
        std::fs::metadata(store.path(session, attachment).unwrap())
            .unwrap()
            .len()
    );
    assert_eq!(metadata.status, AttachmentStatus::Pending);
    assert_eq!(metadata.dimensions.unwrap().width, 80);
    assert!(
        store
            .path(session, attachment)
            .unwrap()
            .ends_with(format!("{attachment}.png"))
    );

    let first = store.preview(session, attachment, 16).unwrap();
    let second = store.preview(session, attachment, 16).unwrap();
    assert_eq!(first, second);
    assert!(first.dimensions.width <= 16 && first.dimensions.height <= 16);
    assert_eq!(image::guess_format(&first.bytes).unwrap(), ImageFormat::Png);

    let removed = store.remove(session, attachment).unwrap();
    assert_eq!(removed.status, AttachmentStatus::Removed);
    assert!(!removed.preview_available);
    assert!(store.list(session).unwrap().is_empty());
}

#[test]
fn byte_dimension_decoded_and_count_limits_fail_before_pending_state() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = limits();
    policy.max_bytes = 8;
    let store = make_store(root.path(), policy);
    let session = SessionId::mint();
    store.session_started(session);
    let error = ingest(&store, session, AttachmentId::mint(), png(2, 2)).unwrap_err();
    assert_eq!(error.code, ErrorCode::AttachmentLimit);
    assert!(store.list(session).unwrap().is_empty());

    let root = tempfile::tempdir().unwrap();
    let mut policy = limits();
    policy.max_dimension = 8;
    policy.preview_edge = 8;
    let store = make_store(root.path(), policy);
    let session = SessionId::mint();
    store.session_started(session);
    let error = ingest(&store, session, AttachmentId::mint(), png(9, 1)).unwrap_err();
    assert_eq!(error.code, ErrorCode::AttachmentLimit);
    assert!(store.list(session).unwrap().is_empty());

    let root = tempfile::tempdir().unwrap();
    let mut policy = limits();
    policy.max_decoded_bytes = 63;
    let store = make_store(root.path(), policy);
    let session = SessionId::mint();
    store.session_started(session);
    let error = ingest(&store, session, AttachmentId::mint(), png(4, 4)).unwrap_err();
    assert_eq!(error.code, ErrorCode::AttachmentLimit);
    assert!(store.list(session).unwrap().is_empty());

    let root = tempfile::tempdir().unwrap();
    let mut policy = limits();
    policy.max_per_session = 1;
    let store = make_store(root.path(), policy);
    let session = SessionId::mint();
    store.session_started(session);
    ingest(&store, session, AttachmentId::mint(), png(2, 2)).unwrap();
    let error = ingest(&store, session, AttachmentId::mint(), png(2, 2)).unwrap_err();
    assert_eq!(error.code, ErrorCode::AttachmentLimit);
    assert_eq!(store.list(session).unwrap().len(), 1);
}

#[test]
fn corrupt_and_unsupported_bytes_never_create_a_pending_attachment() {
    let root = tempfile::tempdir().unwrap();
    let store = make_store(root.path(), limits());
    let session = SessionId::mint();
    store.session_started(session);
    let corrupt = ingest(
        &store,
        session,
        AttachmentId::mint(),
        b"not an image".to_vec(),
    )
    .unwrap_err();
    assert_eq!(corrupt.code, ErrorCode::InvalidImage);

    // A valid BMP header is recognized by type sniffing but is not admitted by
    // the session image policy.
    let bmp = [b'B', b'M', 0, 0, 0, 0, 0, 0, 0, 0, 54, 0, 0, 0].to_vec();
    let unsupported = ingest(&store, session, AttachmentId::mint(), bmp).unwrap_err();
    assert_eq!(unsupported.code, ErrorCode::UnsupportedMedia);
    assert!(store.list(session).unwrap().is_empty());
}

#[test]
fn termination_removes_only_the_exact_session_directory() {
    let root = tempfile::tempdir().unwrap();
    let store = make_store(root.path(), limits());
    let ended = SessionId::mint();
    let live = SessionId::mint();
    store.session_started(ended);
    store.session_started(live);
    ingest(&store, ended, AttachmentId::mint(), png(2, 2)).unwrap();
    let live_attachment = AttachmentId::mint();
    ingest(&store, live, live_attachment, png(2, 2)).unwrap();
    let sentinel = root.path().join("sentinel");
    std::fs::write(&sentinel, b"outside").unwrap();

    assert!(store.session_ended(ended).unwrap());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside");
    assert!(store.path(live, live_attachment).unwrap().is_file());
    assert_eq!(
        store.list(ended).unwrap_err().code,
        ErrorCode::SessionExited
    );
}

#[test]
fn identical_ids_in_two_roots_never_share_bytes_or_cleanup() {
    let first_root = tempfile::tempdir().unwrap();
    let second_root = tempfile::tempdir().unwrap();
    let first = make_store(first_root.path(), limits());
    let second = make_store(second_root.path(), limits());
    let session: SessionId = "11111111-1111-4111-8111-111111111111".parse().unwrap();
    let attachment: AttachmentId = "22222222-2222-4222-8222-222222222222".parse().unwrap();
    first.session_started(session);
    second.session_started(session);
    ingest(&first, session, attachment, png(2, 2)).unwrap();
    ingest(&second, session, attachment, png(3, 3)).unwrap();
    assert_ne!(
        std::fs::read(first.path(session, attachment).unwrap()).unwrap(),
        std::fs::read(second.path(session, attachment).unwrap()).unwrap()
    );
    first.session_ended(session).unwrap();
    assert!(second.path(session, attachment).unwrap().is_file());
}

#[cfg(unix)]
#[test]
fn a_live_attachment_root_symlink_is_refused_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"outside").unwrap();
    let sessions = root.path().join("agent/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    symlink(outside.path(), sessions.join("attachments")).unwrap();
    let store =
        AttachmentStore::new(SessionPaths::from_resolved_directory(sessions), limits()).unwrap();
    let session = SessionId::mint();
    store.session_started(session);
    assert_eq!(
        ingest(&store, session, AttachmentId::mint(), png(2, 2))
            .unwrap_err()
            .code,
        ErrorCode::Internal
    );
    assert_eq!(std::fs::read(sentinel).unwrap(), b"outside");
}

#[cfg(windows)]
#[test]
fn a_live_attachment_root_reparse_point_is_refused_when_supported() {
    use std::os::windows::fs::symlink_dir;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"outside").unwrap();
    let sessions = root.path().join("agent/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    if let Err(error) = symlink_dir(outside.path(), sessions.join("attachments")) {
        eprintln!("skipped reparse-point assertion: {error}");
        return;
    }
    let store =
        AttachmentStore::new(SessionPaths::from_resolved_directory(sessions), limits()).unwrap();
    let session = SessionId::mint();
    store.session_started(session);
    assert_eq!(
        ingest(&store, session, AttachmentId::mint(), png(2, 2))
            .unwrap_err()
            .code,
        ErrorCode::Internal
    );
    assert_eq!(std::fs::read(sentinel).unwrap(), b"outside");
}

#[test]
fn normalized_extension_is_derived_from_bytes_not_a_name() {
    let root = tempfile::tempdir().unwrap();
    let store = make_store(root.path(), limits());
    let session = SessionId::mint();
    let attachment = AttachmentId::mint();
    store.session_started(session);
    ingest(&store, session, attachment, png(1, 1)).unwrap();
    let expected = SessionPaths::from_resolved_directory(root.path().join("agent/sessions"))
        .attachment_file(session, attachment, VerifiedAttachmentExtension::Png);
    assert_eq!(store.path(session, attachment).unwrap(), expected);
}
