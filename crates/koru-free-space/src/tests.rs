// SPDX-License-Identifier: MPL-2.0

use super::*;

/// A real volume answers with a real number.
#[test]
#[ignore = "uses a real filesystem; run explicitly with --ignored"]
fn a_temporary_directory_has_room() {
    let root = tempfile::tempdir().expect("a temporary directory");
    assert!(available(root.path()).expect("a readable volume") > 0);
}

/// **Never less than `statvfs`.** Foundation's number adds what the system
/// would purge for an important write to what is already free; if it ever came
/// back smaller, the Apple branch would be refusing downloads the old code
/// allowed. The margin absorbs other writers on the machine between the calls.
#[test]
#[ignore = "uses a real filesystem; run explicitly with --ignored"]
fn the_answer_is_at_least_what_statvfs_reports() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let ours = available(root.path()).unwrap();
    let statvfs = fs2::available_space(root.path()).unwrap();
    let margin = 256 * 1024 * 1024;
    assert!(
        ours + margin >= statvfs,
        "available() said {ours}, statvfs {statvfs}"
    );
}

/// Missing-path behavior belongs to the platform. Windows resolves the
/// containing volume even when the final component does not exist; Unix
/// refuses it. A free-space query is not an existence check.
#[test]
#[ignore = "uses a real filesystem; run explicitly with --ignored"]
fn a_missing_path_uses_platform_volume_resolution() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let result = available(&root.path().join("not-here"));
    if cfg!(windows) {
        assert!(
            result.is_ok(),
            "an existing volume still resolves: {result:?}"
        );
    } else {
        assert!(result.is_err(), "a missing path was accepted: {result:?}");
    }
}
