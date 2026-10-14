// SPDX-License-Identifier: MPL-2.0

//! Writing an edit back, and refusing to when there is nothing to write to.
//!
//! The disk exception, like `resolve`: every scenario works under a
//! directory named for this process and removed when it ends.

use super::*;

#[test]
fn a_resolved_file_names_the_path_it_came_from() {
    let install = Installation::new("path");
    install.edited(b"# Mine\n\nInstructions.\n");
    let resolved = resolve(install.path());
    assert_eq!(resolved.source, Source::Edited);
    assert_eq!(
        resolved.path.as_deref(),
        Some(install.preamble_path().as_path())
    );
}

/// **The whole of *what may be written back*.** The built-in copy has no
/// file behind it, so a surface has nothing to offer and offers nothing.
#[test]
fn the_bundled_copy_names_no_path() {
    let install = Installation::new("bundled-path");
    install.edited(b"   \n");
    let resolved = resolve_within(install.path(), None);
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.path, None);
}

/// The path is a path, so it stays out of `Debug` for the reason the text
/// does (§13.7).
#[test]
fn the_path_is_not_printed() {
    let install = Installation::new("path-debug");
    install.edited(b"# Mine\n\nInstructions.\n");
    let resolved = resolve(install.path());
    assert!(resolved.path.is_some());
    assert!(!format!("{resolved:?}").contains(PATH_SENTINEL));
}

/// A save writes the file the read came from, and the next `resolve` hands
/// back exactly what was written.
#[test]
fn saving_writes_the_file_the_instructions_were_read_from() {
    let install = Installation::new("save");
    install.edited(b"# Mine\n\nBefore.\n");
    let resolved = resolve(install.path());
    let path = resolved.path.expect("a file is in force");

    write(&path, "# Mine\n\nAfter.\n").expect("written");

    let again = resolve(install.path());
    assert_eq!(again.source, Source::Edited);
    assert_eq!(again.text.as_ref(), "# Mine\n\nAfter.\n");
    assert!(again.notice.is_none());
}

/// A save that would produce a file the *next* run refuses is refused now,
/// while the text is still on screen — and it changes nothing.
#[test]
fn a_save_refuses_what_resolve_would_refuse_and_changes_nothing() {
    let install = Installation::new("refuse");
    install.edited(b"# Mine\n\nBefore.\n");
    let path = resolve(install.path()).path.expect("a file");

    assert_eq!(write(&path, "   \n"), Err(WriteFault::Empty));
    let oversized = "x".repeat(SIZE_LIMIT_BYTES + 1);
    assert!(matches!(
        write(&path, &oversized),
        Err(WriteFault::Oversized { .. })
    ));

    assert_eq!(
        resolve(install.path()).text.as_ref(),
        "# Mine\n\nBefore.\n",
        "a refused save left the file alone"
    );
}

/// A write into a directory that is not there is a refusal, not a panic —
/// and the one it names is the one a person can act on.
#[test]
fn an_unwritable_path_refuses() {
    let install = Installation::new("unwritable");
    // A directory where the file should be: the create fails, the rename
    // never happens, and nothing is left behind.
    let path = install.preamble_path();
    std::fs::create_dir_all(&path).unwrap();
    assert_eq!(write(&path, "# Mine\n"), Err(WriteFault::Unwritable));
}

/// Every refusal has a sentence in every language, and none of them carries
/// a path.
#[test]
fn every_refusal_has_a_sentence_in_every_language() {
    for fault in [
        WriteFault::Empty,
        WriteFault::Oversized {
            bytes: 70_000,
            limit: SIZE_LIMIT_BYTES as u64,
        },
        WriteFault::Unwritable,
    ] {
        for lang in koru_i18n::Lang::ALL {
            let message = fault.message(lang);
            assert!(!message.trim().is_empty(), "{fault:?} in {}", lang.tag());
            assert!(!message.contains(PATH_SENTINEL));
            assert!(!message.contains(RELATIVE_PATH));
            assert!(!message.contains("{}"), "{fault:?} in {}", lang.tag());
        }
    }
}
