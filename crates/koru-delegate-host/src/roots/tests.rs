// SPDX-License-Identifier: MPL-2.0

//! Where a temporary run works, and who decides it (ADR-365).

use super::*;

fn a_run() -> RunId {
    RunId::mint()
}

fn an_endpoint() -> koru_ipc::Endpoint {
    koru_ipc::Endpoint::for_service_root(
        koru_ipc::AGENT,
        "internal-debug",
        std::path::Path::new("."),
    )
    .expect("an endpoint name")
}

/// **Nothing chosen is this machine's temporary directory** — `%TMP%` on
/// Windows, `/tmp` on Unix, which is what was asked for. Asserted against
/// `std::env::temp_dir` rather than a spelled-out path so the test says the
/// rule rather than today's machine.
#[test]
fn with_no_settings_the_base_is_the_platforms_temporary_directory() {
    let roots = HostRoots::new(an_endpoint(), None);
    assert_eq!(roots.temporary_base(), std::env::temp_dir());
}

/// The shape, without making anything: one named folder inside the base, the
/// run's own directory inside that, and `workspace` inside that.
#[test]
fn a_runs_folder_is_named_inside_one_folder_of_korus_own() {
    let run = a_run();
    let made = run_directory(std::path::Path::new("D:/base"), run);
    assert_eq!(
        made,
        std::path::Path::new("D:/base")
            .join("koru-runs")
            .join(run.to_storage())
            .join("workspace")
    );
    assert!(
        made.starts_with(std::path::Path::new("D:/base").join(koru_settings::RUNS_FOLDER)),
        "one named folder, so a person can find and delete every run at once"
    );
    // Two runs never share a directory, which is what lets a base be a folder
    // Koru does not own.
    assert_ne!(
        made,
        run_directory(std::path::Path::new("D:/base"), a_run())
    );
}

/// **The chosen folder is honoured, and read at the moment it is needed** —
/// so changing it while runs are going moves the next one rather than waiting
/// for a restart.
///
/// A disk test under a temporary directory of its own: the settings file it
/// reads and the folder it makes are both inside it.
#[test]
fn a_chosen_folder_is_where_the_run_works_and_is_re_read_each_time() {
    let root = tempfile::tempdir().expect("a temp root");
    let paths = koru_settings::SettingsPaths::under_root(root.path());
    let roots = HostRoots::new(an_endpoint(), Some(paths.clone()));

    // Nothing written yet: the platform's answer.
    assert_eq!(roots.temporary_base(), std::env::temp_dir());

    let chosen = root.path().join("chosen");
    write_base(&paths, &chosen);
    assert_eq!(roots.temporary_base(), chosen);

    let run = a_run();
    let made = roots.temporary_directory(run).expect("the folder is made");
    assert_eq!(made, run_directory(&chosen, run));
    assert!(made.is_dir(), "resolving it is what makes it");

    // Change it again with the same `HostRoots` standing: the next run goes
    // to the new folder, because the value is read and never cached.
    let moved = root.path().join("moved");
    write_base(&paths, &moved);
    let later = a_run();
    assert_eq!(
        roots.temporary_directory(later),
        Some(run_directory(&moved, later))
    );
}

/// Scheduled admission plans the exact Koru-owned path without creating a
/// candidate directory before the durable claim owns it. Once the claim has
/// committed, creating and resolving that path must produce the identity that
/// admission compared.
#[test]
fn scheduled_temporary_planning_creates_only_the_owned_parent() {
    let root = tempfile::tempdir().expect("a temp root");
    let paths = koru_settings::SettingsPaths::under_root(root.path());
    let chosen = root.path().join("chosen");
    write_base(&paths, &chosen);
    let roots = HostRoots::new(an_endpoint(), Some(paths));
    let run = a_run();

    let planned = roots.planned_temporary(run).expect("plan the run path");
    let runs = chosen.join(koru_settings::RUNS_FOLDER);
    let candidate = run_directory(&chosen, run);
    assert!(runs.is_dir(), "the stable Koru-owned parent is made");
    assert!(
        !candidate.exists(),
        "pre-claim planning must not create the candidate run folder"
    );

    std::fs::create_dir_all(&planned.directory).expect("the durable claim may create it");
    let resolved = koru_delegate_scope::ResolvedScope::resolve(
        &koru_delegate_core::WorkspaceScope::Temporary,
        run,
        &roots,
    )
    .expect("resolve the claimed directory");
    assert_eq!(resolved.identity, planned.identity);
    assert_eq!(resolved.directory, planned.directory);
}

/// A base that cannot be made is not a folder: resolving answers nothing
/// rather than a path no run could work in. (The choosing end refuses one
/// before it is ever stored; this is the backstop for a folder that went away
/// afterwards.)
#[test]
fn a_base_that_cannot_be_made_resolves_to_nothing() {
    let root = tempfile::tempdir().expect("a temp root");
    let paths = koru_settings::SettingsPaths::under_root(root.path());
    // A *file* where the base should be, so `create_dir_all` cannot succeed.
    let blocked = root.path().join("not-a-folder");
    std::fs::write(&blocked, b"").expect("a file in the way");
    write_base(&paths, &blocked);

    let roots = HostRoots::new(an_endpoint(), Some(paths));
    assert_eq!(roots.temporary_directory(a_run()), None);
}

fn write_base(paths: &koru_settings::SettingsPaths, base: &std::path::Path) {
    let file = paths.desktop_file();
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("the settings directory");
    std::fs::write(
        file,
        serde_json::to_vec_pretty(&serde_json::json!({
            "delegate_temporary_root": base.to_string_lossy(),
        }))
        .expect("a settings file"),
    )
    .expect("write the settings file");
}
