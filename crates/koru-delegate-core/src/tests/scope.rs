// SPDX-License-Identifier: MPL-2.0

//! **Which folders Koru may remove, and which are somebody's own.**
//!
//! The one question emptying the trash asks of a run before it touches a
//! disk. It is enumerated rather than matched on a wildcard so that a
//! variant added later — M4's git shapes — cannot inherit "yes" by
//! accident, and so the answer is stated in one place instead of at the
//! call site that happens to need it first.

use super::*;

fn project() -> ProjectId {
    v4(3).hyphenated().to_string().parse().unwrap()
}

/// **A temporary folder is Koru's.** It was made for this run alone (D37),
/// and the run ending is what ends Koru's claim on it.
#[test]
fn a_temporary_folder_is_korus_to_remove() {
    assert!(WorkspaceScope::Temporary.is_koru_owned());
}

/// **A project's folder is never Koru's, at either depth.**
///
/// This is the whole point of the question. A run scoped to a project root
/// runs *inside somebody's repository*; a run scoped to a folder inside one
/// runs inside part of it. Deleting "the directory the run was run in"
/// after either would delete work Koru never made and was never given.
#[test]
fn a_project_folder_is_the_persons_own_at_either_depth() {
    assert!(!WorkspaceScope::ProjectRoot { project: project() }.is_koru_owned());
    assert!(
        !WorkspaceScope::ProjectFolder {
            project: project(),
            folder: RelativeFolder::new("src/private").unwrap(),
        }
        .is_koru_owned()
    );
}

/// A scope that never started names no folder to have an opinion about.
#[test]
fn an_unadmissible_scope_owns_nothing() {
    assert!(!WorkspaceScope::None.is_koru_owned());
}

/// **Every variant answers deliberately.** The match inside `is_koru_owned`
/// is exhaustive, so a variant added later is a compile error there; this
/// says the same thing from the outside, so the list of shapes a scope can
/// have and the list this question has been asked of stay the same list.
#[test]
fn every_scope_shape_has_an_answer() {
    let every = [
        WorkspaceScope::None,
        WorkspaceScope::Temporary,
        WorkspaceScope::ProjectRoot { project: project() },
        WorkspaceScope::ProjectFolder {
            project: project(),
            folder: RelativeFolder::new("docs").unwrap(),
        },
    ];
    let owned: Vec<bool> = every.iter().map(WorkspaceScope::is_koru_owned).collect();
    assert_eq!(
        owned,
        [false, true, false, false],
        "exactly one shape is a folder Koru made"
    );
}
