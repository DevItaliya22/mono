// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

use super::SessionPaths;

fn paths() -> SessionPaths {
    SessionPaths::from_resolved_directory(PathBuf::from("/root/agent/sessions"))
}

#[test]
fn the_project_registry_is_one_file_with_an_atomic_sibling() {
    let paths = paths();
    let write = paths.projects_atomic_path();

    assert_eq!(
        paths.projects_file(),
        paths.directory().join("projects.conf")
    );
    assert_eq!(write.destination(), paths.projects_file());
    // A staged write is a sibling in the same directory, so the rename that
    // publishes it stays on one filesystem and stays atomic.
    assert_eq!(
        write.staging(),
        paths.directory().join("projects.conf.part")
    );
    assert_eq!(write.staging().parent(), write.destination().parent());
}

#[test]
fn projects_and_profiles_are_separate_registries_with_separate_staging() {
    let paths = paths();
    assert_ne!(paths.projects_file(), paths.profiles_file());
    assert_ne!(
        paths.projects_atomic_path().staging(),
        paths.profiles_atomic_path().staging()
    );
    // Quarantine is a third name, so recovering one registry cannot overwrite
    // the live file or the staging area of either.
    let quarantine = paths.projects_quarantine_path();
    assert_ne!(quarantine, paths.projects_file());
    assert_ne!(quarantine.as_path(), paths.projects_atomic_path().staging());
}

#[test]
fn a_rooted_run_keeps_its_own_project_registry() {
    let ordinary = SessionPaths::from_resolved_directory(PathBuf::from("/channel/agent/sessions"));
    let rooted = SessionPaths::from_resolved_directory(PathBuf::from("/isolated/agent/sessions"));
    assert_ne!(ordinary.projects_file(), rooted.projects_file());
}
