// SPDX-License-Identifier: MPL-2.0

use super::*;

fn argv(args: &[&str]) -> Vec<String> {
    std::iter::once("koru.exe")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect()
}

#[test]
fn only_the_explicit_absolute_root_is_authority() {
    assert!(matches_root(r"D:\test root", &argv(&["--root", r"D:\test root"])).is_ok());
    assert!(matches_root(r"D:\test root", &argv(&[r"--root=D:\test root"])).is_ok());
    for args in [
        vec![],
        vec!["--profile", "test root"],
        vec!["--root", r"D:\other"],
        vec!["--root", "test root"],
        vec!["--root", r"D:test root"],
        vec!["--root"],
    ] {
        assert!(matches_root(r"D:\test root", &argv(&args)).is_err());
    }
}

#[test]
fn duplicate_roots_are_not_resolved_by_guessing_precedence() {
    for args in [
        vec!["--root", r"D:\root", "--root", r"D:\root"],
        vec![r"--root=D:\root", r"--root=D:\other"],
    ] {
        assert!(matches_root(r"D:\root", &argv(&args)).is_err());
    }
}

#[test]
fn root_spelling_matches_ipc_without_resolving_disk_or_foreign_working_directory() {
    for spelling in [r"d:\ROOT", "D:/root", r"\\?\D:\root"] {
        assert!(matches_root(r"D:\root", &argv(&["--root", spelling])).is_ok());
    }
    assert!(
        matches_root(
            r"\\server\share\root",
            &argv(&["--root", r"\\?\UNC\server\share\root"])
        )
        .is_ok()
    );
    for spelling in [
        r"D:\a\..\root",
        r"D:\root\",
        r"\root",
        r"\\.\root",
        "D:\\root\0",
    ] {
        assert!(matches_root(r"D:\root", &argv(&["--root", spelling])).is_err());
    }
}

#[test]
fn content_in_other_arguments_does_not_establish_a_root() {
    assert!(matches_root(r"D:\root", &argv(&[r"--profile=--root=D:\root"])).is_err());
    assert!(matches_root(r"D:\root", &argv(&["--peer", "a --root D:\\root"])).is_err());
}

#[cfg(windows)]
#[test]
fn windows_argv_keeps_spaces_unicode_and_trailing_backslash_inside_the_root() {
    let args = platform::parse_for_test(
        r#""D:\program files\koru.exe" --root "D:\test 漢字\\" --peer proof"#,
    );
    assert_eq!(
        args,
        argv(&["--root", "D:\\test 漢字\\", "--peer", "proof"])
            .into_iter()
            .enumerate()
            .map(|(i, s)| if i == 0 {
                r"D:\program files\koru.exe".into()
            } else {
                s
            })
            .collect::<Vec<_>>()
    );
    assert!(matches_root("D:\\test 漢字\\", &args).is_ok());
}

#[test]
fn a_late_success_cannot_override_timeout_or_cancellation() {
    for status in [None, Some(false), Some(true)] {
        assert!(query_progress(true, false, status).is_err());
        assert!(query_progress(false, true, status).is_err());
    }
}

#[test]
fn a_failed_query_cannot_promote_output_into_ownership_evidence() {
    assert!(query_progress(false, false, Some(false)).is_err());
    assert_eq!(query_progress(false, false, None), Ok(false));
    assert_eq!(query_progress(false, false, Some(true)), Ok(true));
}
