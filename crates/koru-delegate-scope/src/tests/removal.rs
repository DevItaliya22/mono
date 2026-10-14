// SPDX-License-Identifier: MPL-2.0

//! **The test that stops somebody's repository being deleted in a year's
//! time.**
//!
//! Emptying the trash on a deleted run removes the folder it worked in —
//! but only when Koru made that folder. A run scoped to a project root ran
//! *inside the person's own repository*, and the whole of this file is the
//! difference between those two cases, asserted against a real filesystem
//! rather than against a fake, because the failure it prevents is a real
//! `remove_dir_all`.
//!
//! Disk, deliberately: this is the scenario the standing requirement's disk
//! exception exists for. Every case works under a directory named for this
//! process and removed when the fixture drops.

use super::*;

/// **The trap.** A run scoped to a project root is a run inside somebody's
/// repository. Emptying the trash forgets the run and leaves every byte of
/// that directory where it was — the file inside it included, because a
/// directory that survives empty would be just as much of a loss.
#[test]
fn emptying_the_trash_on_a_project_scoped_run_leaves_its_folder_on_disk() {
    let temp = TempDir::new("project-root-survives");
    let project = temp.dir("their-repository");
    let theirs = project.join("their-work.md");
    std::fs::write(&theirs, b"a year of somebody's work").unwrap();

    let outcome = remove_working_folder(&root_scope(1), Some(&project));

    assert_eq!(outcome, Removal::Kept, "it is not Koru's to remove");
    assert!(project.is_dir(), "their repository is still there");
    assert_eq!(
        std::fs::read(&theirs).unwrap(),
        b"a year of somebody's work",
        "and so is what was in it"
    );
}

/// The same for a **folder inside** a project. It is part of their
/// repository, one level down; nothing about being nested makes it Koru's.
/// The project around it is untouched too — a removal that walked upward
/// would be the same disaster with an extra step.
#[test]
fn a_folder_inside_a_project_is_left_alone_and_so_is_the_project() {
    let temp = TempDir::new("project-folder-survives");
    let project = temp.dir("their-repository");
    let inside = temp.dir("their-repository/src");
    std::fs::write(inside.join("main.rs"), b"fn main() {}").unwrap();

    let outcome = remove_working_folder(&folder_scope(1, "src"), Some(&inside));

    assert_eq!(outcome, Removal::Kept);
    assert!(
        inside.is_dir(),
        "the folder the run worked in is still there"
    );
    assert!(project.is_dir(), "and so is the project around it");
}

/// **A folder Koru made is Koru's**, and emptying the trash is what ends
/// its life (D37). The run's own directory above it goes too — it is empty
/// once the working folder is gone, and a bare identifier with nothing in it
/// is litter in a directory Koru does not own.
#[test]
fn a_temporary_folder_is_removed_with_the_run() {
    let temp = TempDir::new("temporary-goes");
    let run_directory = temp.dir("koru-runs/a-run");
    let working = temp.dir("koru-runs/a-run/workspace");
    std::fs::write(working.join("notes.md"), b"what the agent wrote").unwrap();

    let outcome = remove_working_folder(&WorkspaceScope::Temporary, Some(&working));

    assert_eq!(outcome, Removal::Removed);
    assert!(!working.exists(), "the working folder is gone");
    assert!(
        !run_directory.exists(),
        "and the empty run directory with it"
    );
    assert!(
        temp.path().join("koru-runs").is_dir(),
        "but not the folder Koru puts its runs in, which other runs share"
    );
}

/// **The prune stops at one empty level.** A sibling in the run's own
/// directory keeps it, because `remove_dir` refuses a directory that is not
/// empty — the guard is the filesystem's, not a check that could be raced.
#[test]
fn the_run_directory_stays_when_something_else_is_in_it() {
    let temp = TempDir::new("temporary-sibling");
    let run_directory = temp.dir("koru-runs/a-run");
    let working = temp.dir("koru-runs/a-run/workspace");
    std::fs::write(run_directory.join("kept.txt"), b"not the agent's").unwrap();

    assert_eq!(
        remove_working_folder(&WorkspaceScope::Temporary, Some(&working)),
        Removal::Removed
    );
    assert!(!working.exists());
    assert!(run_directory.is_dir(), "the sibling kept its directory");
}

/// **Gone already is an answer, not a failure.** Reported so the trash can
/// say so, and never retried: there is nothing left to do.
#[test]
fn a_folder_that_is_already_gone_is_reported_as_absent() {
    let temp = TempDir::new("temporary-absent");
    let missing = temp.path().join("koru-runs/never-made/workspace");

    assert_eq!(
        remove_working_folder(&WorkspaceScope::Temporary, Some(&missing)),
        Removal::Absent
    );
    // …and so is a run this device never resolved a folder for at all.
    assert_eq!(
        remove_working_folder(&WorkspaceScope::Temporary, None),
        Removal::Absent
    );
}

/// **A path with no scope behind it removes nothing.** `None` never started,
/// so there is no folder it is entitled to; the path is beside the point and
/// the decision is made before it is looked at.
#[test]
fn a_scope_that_never_started_removes_nothing_whatever_path_it_is_given() {
    let temp = TempDir::new("no-scope");
    let directory = temp.dir("somebody-elses");

    assert_eq!(
        remove_working_folder(&WorkspaceScope::None, Some(&directory)),
        Removal::Kept
    );
    assert!(directory.is_dir());
}

/// **A project-scoped run is left alone even when the folder is gone.** The
/// answer is *Kept* rather than *Absent*: the rule is about whose folder it
/// is, and it is asked before anything reads a disk. A rule that looked
/// first would have a window in it — and would report the wrong thing to a
/// person about a directory it never had the right to touch.
#[test]
fn the_decision_is_made_before_the_filesystem_is_read() {
    let temp = TempDir::new("decision-first");
    let missing = temp.path().join("moved-away");

    assert_eq!(
        remove_working_folder(&root_scope(1), Some(&missing)),
        Removal::Kept
    );
}

/// A refusal is a kind from `std`'s closed vocabulary and nothing else —
/// no path, no message (standing requirement 7). Asserted on the shape,
/// because a filesystem that refuses on demand is not something a test may
/// arrange portably.
#[test]
fn a_refusal_carries_a_kind_and_nothing_else() {
    let refused = Removal::Refused(std::io::ErrorKind::PermissionDenied);
    assert!(refused.is_refusal());
    assert_eq!(refused.code(), "refused");
    assert_eq!(format!("{refused:?}"), "Refused(PermissionDenied)");
    for outcome in [Removal::Removed, Removal::Kept, Removal::Absent] {
        assert!(!outcome.is_refusal(), "{outcome:?}");
    }
}
