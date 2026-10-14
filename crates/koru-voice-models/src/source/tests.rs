// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::fixtures;

fn nowhere() -> Url {
    Url::parse("https://models.koru.do/v1/parakeet-1.2.0.tar.gz").unwrap()
}

/// *This build cannot fetch anything* is a state a caller can report to a
/// person. A panic is not, which is why the placeholder is an implementation
/// that refuses rather than an `unimplemented!()`.
#[test]
fn a_build_with_no_downloader_refuses_with_a_code() {
    let dir = tempfile::tempdir().unwrap();
    let error = NoNetworkSource
        .fetch(
            &nowhere(),
            &fixtures::descriptor(),
            &dir.path().join("out.bin"),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(error.code, ModelErrorCode::Offline);
}

/// Installing from bytes somebody already has — the same reason
/// `koru_update::controller` keeps `copy_verified` beside `download_verified`,
/// and what Plan 1.3's end-to-end tests use instead of a server.
#[test]
fn a_directory_source_copies_the_file_the_url_names() {
    let from = tempfile::tempdir().unwrap();
    let into = tempfile::tempdir().unwrap();
    std::fs::write(
        from.path().join("parakeet-1.2.0.tar.gz"),
        fixtures::archive_bytes(),
    )
    .unwrap();

    let mut stages = Vec::new();
    let destination = into.path().join("staging").join("archive");
    DirectorySource::new(from.path())
        .fetch(
            &nowhere(),
            &fixtures::descriptor(),
            &destination,
            &mut |p| stages.push(p),
            &AtomicBool::new(false),
        )
        .unwrap();

    assert_eq!(
        std::fs::read(&destination).unwrap(),
        fixtures::archive_bytes()
    );
    assert!(stages.iter().all(|p| p.stage == InstallStage::Downloading));
    assert_eq!(stages.last().unwrap().received, 3);
    // The source produced bytes; the *store* is what says they are the right
    // ones. Asserting that here would be the split this crate exists to keep.
    fixtures::descriptor().verify_archive(&destination).unwrap();
}

#[test]
fn a_missing_archive_and_a_url_with_no_file_name_both_say_download_failed() {
    let from = tempfile::tempdir().unwrap();
    let into = tempfile::tempdir().unwrap();
    let source = DirectorySource::new(from.path());

    let absent = source
        .fetch(
            &nowhere(),
            &fixtures::descriptor(),
            &into.path().join("out"),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(absent.code, ModelErrorCode::DownloadFailed);

    let bare = Url::parse("https://models.koru.do/v1/").unwrap();
    assert_eq!(source.local_path(&bare), None);
    let nameless = source
        .fetch(
            &bare,
            &fixtures::descriptor(),
            &into.path().join("out"),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(nameless.code, ModelErrorCode::DownloadFailed);
}

/// A cancelled fetch leaves nothing a later read would mistake for a complete
/// download — here, nothing at all.
#[test]
fn cancelling_before_the_copy_writes_nothing() {
    let from = tempfile::tempdir().unwrap();
    let into = tempfile::tempdir().unwrap();
    std::fs::write(
        from.path().join("parakeet-1.2.0.tar.gz"),
        fixtures::archive_bytes(),
    )
    .unwrap();
    let destination = into.path().join("out");
    let error = DirectorySource::new(from.path())
        .fetch(
            &nowhere(),
            &fixtures::descriptor(),
            &destination,
            &mut |_| {},
            &AtomicBool::new(true),
        )
        .unwrap_err();
    assert_eq!(error.code, ModelErrorCode::Cancelled);
    assert!(!destination.exists());
}
