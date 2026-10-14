// SPDX-License-Identifier: MPL-2.0

//! The resolution order and the five objective faults, against real
//! directories named for this process.
//!
//! The fault cases go through `resolve_within(dir, None)` — **a released
//! build's own behaviour**, where the file beside the executables is the
//! only place there is — so that what a person installs is what these
//! assertions are about. The order tests below stand a second temporary
//! directory in for the repository and drive both.

use super::*;

/// A released build: the installation directory and nothing else.
fn installed(install: &Installation) -> Resolved {
    resolve_within(install.path(), None)
}

#[test]
fn an_edited_copy_wins_and_is_used_as_written() {
    let install = Installation::new("edited");
    let text = "# Mine\n\nDo the thing my way. No tool names here at all.\n";
    install.edited(text.as_bytes());

    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Edited);
    assert_eq!(resolved.notice, None);
    assert_eq!(
        resolved.text.as_ref(),
        text,
        "readable text is used as written (D35)"
    );
}

#[test]
fn a_missing_copy_falls_back_to_the_bundled_text_with_no_notice() {
    let install = Installation::new("missing");
    assert!(!install.preamble_path().exists());

    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.notice, None, "a fresh install is not news");
    assert_eq!(resolved.text.as_ref(), BUNDLED);
}

#[test]
fn a_missing_installation_directory_is_also_just_missing() {
    let install = Installation::new("no-such-dir");
    let nowhere = install.path().join("never-created");

    let resolved = resolve_within(&nowhere, None);
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.notice, None);
}

#[test]
fn an_empty_copy_falls_back_and_says_so() {
    let install = Installation::new("empty");
    install.edited(b"");
    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.notice, Some(Notice::Empty));
    assert_eq!(resolved.text.as_ref(), BUNDLED);
}

#[test]
fn a_whitespace_only_copy_is_empty() {
    let install = Installation::new("whitespace");
    install.edited(b" \n\r\n\t  \n");
    assert_eq!(installed(&install).notice, Some(Notice::Empty));
}

#[test]
fn a_copy_that_is_only_a_byte_order_mark_is_empty() {
    let install = Installation::new("bom-only");
    install.edited(b"\xEF\xBB\xBF\r\n");
    assert_eq!(installed(&install).notice, Some(Notice::Empty));
}

#[test]
fn a_leading_byte_order_mark_is_dropped_and_the_rest_used() {
    let install = Installation::new("bom");
    install.edited(b"\xEF\xBB\xBFInstructions.\n");
    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Edited);
    assert_eq!(resolved.text.as_ref(), "Instructions.\n");
}

#[test]
fn a_utf16_copy_is_bad_encoding() {
    let install = Installation::new("utf16");
    // What Notepad writes for "Hi" when a person picks UTF-16 LE.
    install.edited(b"\xFF\xFEH\x00i\x00\r\x00\n\x00");
    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.notice, Some(Notice::BadEncoding));
}

#[test]
fn a_copy_with_an_invalid_sequence_is_bad_encoding() {
    let install = Installation::new("latin1");
    install.edited(b"Caf\xE9 instructions\n");
    assert_eq!(installed(&install).notice, Some(Notice::BadEncoding));
}

#[test]
fn a_copy_over_the_limit_falls_back_and_reports_both_numbers() {
    let install = Installation::new("oversized");
    let bytes = vec![b'x'; SIZE_LIMIT_BYTES + 1];
    install.edited(&bytes);
    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(
        resolved.notice,
        Some(Notice::Oversized {
            bytes: (SIZE_LIMIT_BYTES + 1) as u64,
            limit: SIZE_LIMIT_BYTES as u64,
        })
    );
}

#[test]
fn a_copy_exactly_at_the_limit_is_used() {
    let install = Installation::new("at-limit");
    let bytes = vec![b'x'; SIZE_LIMIT_BYTES];
    install.edited(&bytes);
    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Edited);
    assert_eq!(resolved.text.len(), SIZE_LIMIT_BYTES);
}

#[test]
fn a_directory_at_the_path_is_unreadable() {
    let install = Installation::new("directory");
    std::fs::create_dir_all(install.preamble_path()).unwrap();
    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.notice, Some(Notice::Unreadable));
}

#[test]
fn a_file_where_the_docs_folder_should_be_falls_back() {
    let install = Installation::new("docs-is-a-file");
    std::fs::write(install.path().join("docs"), b"not a directory").unwrap();
    let resolved = installed(&install);
    assert_eq!(resolved.source, Source::Bundled);
    // `open` answers NotFound on some platforms and NotADirectory on others
    // for a path through a file; the first is a missing copy and the second
    // an unreadable one, and both fall back. Which one is the platform's
    // to say, so the assertion is on the fallback.
    assert!(
        matches!(resolved.notice, Some(Notice::Unreadable) | None),
        "{:?}",
        resolved.notice
    );
}

#[test]
fn the_bundled_copy_is_the_repository_file() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join(SOURCE_PATH);
    let on_disk = std::fs::read_to_string(&repository).expect("the preamble is in the tree");
    assert_eq!(on_disk, BUNDLED);
}

// ------------------------------------------------------- the three places

/// **The order, all three places at once.** Every one of them holds a
/// different text, so which was used is unambiguous.
#[test]
fn the_copy_beside_the_executables_beats_the_repository_and_both_beat_the_bundled_text() {
    let install = Installation::new("order-install");
    let repository = Installation::new("order-repository");
    install.edited(
        b"beside the executables
",
    );
    repository.edited(
        b"in the repository
",
    );

    let resolved = resolve_within(install.path(), Some(&repository.preamble_path()));
    assert_eq!(resolved.source, Source::Edited);
    assert_eq!(
        resolved.text.as_ref(),
        "beside the executables
"
    );
    assert_eq!(resolved.notice, None);
}

/// What `cargo run` is: `target/debug/` has no `docs/` beside it, and the
/// repository copy is what takes effect.
#[test]
fn a_development_build_reads_the_repository_when_nothing_is_beside_the_executables() {
    let install = Installation::new("dev-install");
    let repository = Installation::new("dev-repository");
    repository.edited(
        b"in the repository
",
    );
    assert!(!install.preamble_path().exists());

    let resolved = resolve_within(install.path(), Some(&repository.preamble_path()));
    assert_eq!(resolved.source, Source::Edited);
    assert_eq!(
        resolved.text.as_ref(),
        "in the repository
"
    );
    assert_eq!(resolved.notice, None);
}

/// **A released build is unchanged**: the repository directory is not
/// searched, because in a release there is no repository directory to
/// search. The same two directories, and the answer is the bundled text.
#[test]
fn a_released_build_looks_beside_the_executables_and_nowhere_else() {
    let install = Installation::new("release-install");
    let repository = Installation::new("release-repository");
    repository.edited(
        b"in the repository
",
    );

    let resolved = resolve_within(install.path(), None);
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.text.as_ref(), BUNDLED);
    assert_eq!(resolved.notice, None, "a fresh install is not news");
}

/// A broken edit beside the executables is not papered over by the next
/// place along: the person who made it hears about it.
#[test]
fn a_rejected_copy_beside_the_executables_stops_the_search() {
    let install = Installation::new("rejected-first");
    let repository = Installation::new("rejected-second");
    install.edited(b"");
    repository.edited(
        b"in the repository
",
    );

    let resolved = resolve_within(install.path(), Some(&repository.preamble_path()));
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.notice, Some(Notice::Empty));
}

/// Neither place has one: the bundled text, with nothing to report.
#[test]
fn nothing_anywhere_is_the_bundled_text_with_no_notice() {
    let install = Installation::new("nothing-install");
    let repository = Installation::new("nothing-repository");
    let resolved = resolve_within(install.path(), Some(&repository.preamble_path()));
    assert_eq!(resolved.source, Source::Bundled);
    assert_eq!(resolved.notice, None);
}

/// **Which build this is decides whether there is a second place at all.**
/// The behaviour under test is the compile-time one, so the assertion is
/// too: a debug build names the workspace root, a release names nothing.
#[test]
fn only_a_development_build_has_a_repository_to_look_in() {
    if cfg!(debug_assertions) {
        let file = repository_file().expect("a development build knows its repository copy");
        assert!(
            file.is_file(),
            "the repository copy is where the crate says it is: {}",
            file.display()
        );
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        assert_eq!(
            std::fs::read(&file).unwrap(),
            std::fs::read(root.join(SOURCE_PATH)).unwrap(),
            "SOURCE_PATH names the file a development build reads"
        );
    } else {
        assert_eq!(
            repository_file(),
            None,
            "a release must carry no path into a source tree"
        );
    }
}

/// **The maintainers' question, answered by the public entry point.** In a
/// development build, `resolve` from a directory that has no `docs/` beside
/// it — which is exactly `target/debug/` — finds the repository's file. So
/// editing the repository copy takes effect, and because the orchestrator
/// calls `resolve` at the start of every run, the next run is enough: no
/// rebuild, no restart.
#[cfg(debug_assertions)]
#[test]
fn a_development_build_resolves_the_repository_file_from_a_directory_with_no_docs() {
    let install = Installation::new("cargo-run");
    assert!(!install.preamble_path().exists());

    let resolved = resolve(install.path());
    assert_eq!(resolved.source, Source::Edited);
    assert_eq!(resolved.notice, None);
    assert_eq!(
        resolved.text.as_ref(),
        BUNDLED,
        "an untouched tree reads the same bytes it compiled in"
    );
}
