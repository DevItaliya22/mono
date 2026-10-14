// SPDX-License-Identifier: MPL-2.0

//! Canonical path identity against the real filesystem — the explicit disk
//! exception — plus the pure fold and overlap rules over spelled paths.

use std::path::Path;

use super::*;

const NESTED_INSIDE_CANDIDATE: Overlap = Overlap::Nested {
    inner: Party::Candidate,
    outer: Party::Holder,
};
const NESTED_INSIDE_HOLDER: Overlap = Overlap::Nested {
    inner: Party::Holder,
    outer: Party::Candidate,
};

/// An identity from a spelled path, without touching the disk — the fold
/// and prefix rules alone.
fn spelled(path: &str) -> ScopeIdentity {
    ScopeIdentity::from_resolved(Path::new(path))
}

#[test]
fn a_trailing_separator_does_not_make_a_second_folder() {
    let temp = TempDir::new("trailing");
    let folder = temp.dir("work");
    let mut with_separator = folder.clone().into_os_string();
    with_separator.push(std::path::MAIN_SEPARATOR_STR);
    assert_eq!(identity(&folder), identity(Path::new(&with_separator)));
    assert_eq!(
        identity(&folder).overlap(&identity(Path::new(&with_separator))),
        Overlap::Same
    );
}

#[test]
fn a_dot_segment_does_not_make_a_second_folder() {
    let temp = TempDir::new("dot");
    let folder = temp.dir("work");
    let through_dot = temp.path().join(".").join("work").join(".");
    assert_eq!(identity(&folder), identity(&through_dot));
}

#[test]
fn a_parent_segment_resolves_to_the_folder_it_names() {
    let temp = TempDir::new("parent");
    let folder = temp.dir("work");
    let sibling = temp.dir("other");
    let through_parent = sibling.join("..").join("work");
    assert_eq!(identity(&folder), identity(&through_parent));
}

#[cfg(windows)]
#[test]
fn windows_case_and_verbatim_prefix_and_slash_direction_are_one_folder() {
    let temp = TempDir::new("spelling");
    let folder = temp.dir("Work");
    let canonical = identity(&folder);

    let lower = folder.to_string_lossy().to_lowercase();
    assert_eq!(identity(Path::new(&lower)), canonical, "case");

    let upper = folder.to_string_lossy().to_uppercase();
    assert_eq!(identity(Path::new(&upper)), canonical, "upper case");

    let verbatim = format!(r"\\?\{}", folder.display());
    assert_eq!(identity(Path::new(&verbatim)), canonical, "verbatim prefix");

    let forward = folder.to_string_lossy().replace('\\', "/");
    assert_eq!(identity(Path::new(&forward)), canonical, "forward slashes");
}

#[cfg(windows)]
#[test]
fn windows_junction_is_the_folder_it_points_at() {
    let temp = TempDir::new("junction");
    let target = temp.dir("target");
    let Some(link) = temp.junction("alias", &target) else {
        return;
    };
    assert_eq!(
        identity(&link),
        identity(&target),
        "the alias is the target"
    );

    let inside_target = temp.dir(r"target\src");
    let inside_link = link.join("src");
    assert_eq!(identity(&inside_link), identity(&inside_target));
    assert_eq!(
        identity(&inside_link).overlap(&identity(&target)),
        NESTED_INSIDE_CANDIDATE,
        "a folder reached through the alias nests inside the target"
    );
}

#[test]
fn a_folder_and_its_parent_overlap_in_both_directions_and_siblings_do_not() {
    let temp = TempDir::new("nesting");
    let outer = identity(&temp.dir("repo"));
    let inner = identity(&temp.dir("repo/crates/one"));
    let sibling = identity(&temp.dir("repo/crates/two"));
    let unrelated = identity(&temp.dir("elsewhere"));

    assert_eq!(inner.overlap(&outer), NESTED_INSIDE_CANDIDATE);
    assert_eq!(outer.overlap(&inner), NESTED_INSIDE_HOLDER);
    assert_eq!(inner.overlap(&sibling), Overlap::Disjoint);
    assert_eq!(sibling.overlap(&inner), Overlap::Disjoint);
    assert_eq!(outer.overlap(&unrelated), Overlap::Disjoint);
    assert!(inner.overlap(&outer).is_overlap());
    assert!(!inner.overlap(&sibling).is_overlap());
    assert_eq!(inner.depth(), outer.depth() + 2);
}

#[test]
fn a_name_that_merely_starts_with_a_siblings_name_is_disjoint() {
    // `repo` and `repo2` share a string prefix and no directory.
    let temp = TempDir::new("prefix");
    let repo = identity(&temp.dir("repo"));
    let repo2 = identity(&temp.dir("repo2"));
    assert_eq!(repo.overlap(&repo2), Overlap::Disjoint);
}

#[test]
fn a_missing_folder_is_refused_as_not_found() {
    let temp = TempDir::new("missing");
    assert_eq!(
        ScopeIdentity::of_directory(&temp.path().join("absent")),
        Err(ScopeRefusal::NotFound)
    );
}

#[test]
fn a_file_is_refused_as_not_a_directory() {
    let temp = TempDir::new("file");
    let file = temp.file("notes.txt");
    assert_eq!(
        ScopeIdentity::of_directory(&file),
        Err(ScopeRefusal::NotADirectory)
    );
}

#[test]
fn resolving_a_scope_follows_the_project_root_port() {
    let temp = TempDir::new("resolve");
    let root = temp.dir("project");
    let roots = Roots::default().project(project_id(1), &root);

    assert_eq!(
        ScopeIdentity::resolve(&root_scope(1), run_id(9), &roots),
        Ok(identity(&root))
    );
    assert_eq!(
        ScopeIdentity::resolve(&root_scope(2), run_id(9), &roots),
        Err(ScopeRefusal::UnknownProject)
    );
    let roots = roots.project_without_root(project_id(3));
    assert_eq!(
        ScopeIdentity::resolve(&root_scope(3), run_id(9), &roots),
        Err(ScopeRefusal::ProjectHasNoRoot)
    );
}

#[test]
fn a_project_folder_resolves_inside_its_root_and_nowhere_else() {
    let temp = TempDir::new("folder");
    let root = temp.dir("project");
    let inside = temp.dir("project/src/private");
    temp.dir("outside");
    let roots = Roots::default().project(project_id(1), &root);

    assert_eq!(
        ScopeIdentity::resolve(&folder_scope(1, "src/private"), run_id(9), &roots),
        Ok(identity(&inside))
    );
    assert_eq!(
        ScopeIdentity::resolve(&folder_scope(1, "."), run_id(9), &roots),
        Ok(identity(&root)),
        "the root itself is inside the root"
    );
    assert_eq!(
        ScopeIdentity::resolve(&folder_scope(1, "../outside"), run_id(9), &roots),
        Err(ScopeRefusal::FolderOutsideProject)
    );
    let absolute = temp.path().join("outside");
    assert_eq!(
        ScopeIdentity::resolve(
            &folder_scope(1, &absolute.to_string_lossy()),
            run_id(9),
            &roots
        ),
        Err(ScopeRefusal::FolderOutsideProject),
        "an absolute spelling replaces the root on join and is caught"
    );
    assert_eq!(
        ScopeIdentity::resolve(&folder_scope(1, "src/absent"), run_id(9), &roots),
        Err(ScopeRefusal::NotFound)
    );
}

#[test]
fn a_temporary_scope_is_the_runs_own_directory() {
    let temp = TempDir::new("temporary");
    let owned = temp.dir("run-9");
    let roots = Roots::default().temporary(run_id(9), &owned);

    assert_eq!(
        ScopeIdentity::resolve(&WorkspaceScope::Temporary, run_id(9), &roots),
        Ok(identity(&owned))
    );
    assert_eq!(
        ScopeIdentity::resolve(&WorkspaceScope::Temporary, run_id(8), &roots),
        Err(ScopeRefusal::NoTemporaryDirectory)
    );
}

#[test]
fn no_scope_is_refused_before_the_filesystem_is_asked() {
    let roots = Roots::default();
    assert_eq!(
        ScopeIdentity::resolve(&WorkspaceScope::None, run_id(9), &roots),
        Err(ScopeRefusal::NotAdmissible)
    );
}

#[test]
fn refusals_are_codes_with_prose_that_names_no_path() {
    for refusal in [
        ScopeRefusal::NotAdmissible,
        ScopeRefusal::UnknownProject,
        ScopeRefusal::ProjectHasNoRoot,
        ScopeRefusal::NoTemporaryDirectory,
        ScopeRefusal::FolderOutsideProject,
        ScopeRefusal::NotFound,
        ScopeRefusal::NotADirectory,
        ScopeRefusal::Unreadable(std::io::ErrorKind::PermissionDenied),
    ] {
        let shown = refusal.to_string();
        assert!(!shown.is_empty());
        assert!(!shown.contains(std::path::MAIN_SEPARATOR), "{shown}");
    }
}

// --- the fold and prefix rules, over spelled paths and no disk ---

#[cfg(windows)]
#[test]
fn windows_fold_makes_case_prefix_and_slashes_one_identity() {
    let canonical = spelled(r"\\?\C:\Users\Someone\Work");
    assert_eq!(spelled(r"C:\Users\Someone\Work"), canonical);
    assert_eq!(spelled(r"c:\users\someone\work"), canonical);
    assert_eq!(spelled(r"C:/Users/Someone/Work/"), canonical);
    assert_eq!(spelled(r"C:\USERS\SOMEONE\WORK"), canonical);
    assert_eq!(spelled(r"\\?\C:\Users\Someone\Work\"), canonical);
    assert_ne!(
        spelled(r"D:\Users\Someone\Work"),
        canonical,
        "another volume"
    );
}

#[cfg(windows)]
#[test]
fn windows_fold_treats_unc_and_verbatim_unc_as_one_share() {
    let canonical = spelled(r"\\?\UNC\Server\Share\Work");
    assert_eq!(spelled(r"\\server\share\work"), canonical);
    assert_ne!(spelled(r"\\server\other\work"), canonical);
}

#[cfg(windows)]
#[test]
fn windows_fold_is_the_simple_mapping_so_sharp_s_stays_itself() {
    // NTFS keeps `straße` and `strasse` apart; the full mapping would not.
    assert_ne!(spelled(r"C:\straße"), spelled(r"C:\strasse"));
    assert_eq!(spelled(r"C:\straße"), spelled(r"C:\STRAßE"));
    assert_eq!(spelled(r"C:\Café"), spelled(r"C:\CAFÉ"));
}

/// **macOS folds case, as a default APFS volume compares.** The fold alone,
/// over spelled paths; the simple mapping, so `straße` stays itself.
#[cfg(target_os = "macos")]
#[test]
fn macos_fold_makes_case_one_identity() {
    let canonical = spelled("/Users/someone/Work/Repo");
    assert_eq!(spelled("/Users/someone/work/repo"), canonical);
    assert_eq!(spelled("/USERS/SOMEONE/WORK/REPO/"), canonical);
    assert_ne!(spelled("/Users/someone/Work/Other"), canonical);
    assert_ne!(spelled("/straße"), spelled("/strasse"));
    assert_eq!(spelled("/Café"), spelled("/CAFÉ"));
}

/// The case M22 was written for: a Koru-owned child planned under a folder
/// is not resolved, so two spellings of it must still be one identity — or a
/// scheduled run would start in a folder another run holds.
#[cfg(target_os = "macos")]
#[test]
fn macos_two_spellings_of_a_planned_child_collide() {
    let temp = TempDir::new("planned-case");
    let parent = temp.dir("Work");
    let one = ResolvedScope::planned_under(&parent, Path::new("Runs/Alpha")).unwrap();
    let other = ResolvedScope::planned_under(&parent, Path::new("runs/alpha")).unwrap();
    assert_eq!(one.identity.overlap(&other.identity), Overlap::Same);
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn other_platforms_compare_names_exactly() {
    assert_eq!(
        spelled("/home/someone/work"),
        spelled("/home/someone/work/")
    );
    assert_ne!(spelled("/home/someone/work"), spelled("/home/someone/Work"));
}

#[test]
fn spelled_nesting_follows_segments_not_characters() {
    #[cfg(windows)]
    let (outer, inner, lookalike) = (r"C:\repo", r"C:\repo\crates", r"C:\repo2");
    #[cfg(not(windows))]
    let (outer, inner, lookalike) = ("/repo", "/repo/crates", "/repo2");
    assert_eq!(
        spelled(inner).overlap(&spelled(outer)),
        NESTED_INSIDE_CANDIDATE
    );
    assert_eq!(
        spelled(outer).overlap(&spelled(inner)),
        NESTED_INSIDE_HOLDER
    );
    assert_eq!(
        spelled(lookalike).overlap(&spelled(outer)),
        Overlap::Disjoint
    );
}
