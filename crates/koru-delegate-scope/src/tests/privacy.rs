// SPDX-License-Identifier: MPL-2.0

//! Standing requirement 7, structurally: the `Debug` of an identity, a
//! claim, a warning, an admission, a report and a refusal never contains
//! the path — while two identities of one folder still print the same
//! marker, so a test failure can be read.

use super::*;

fn assert_redacted(rendered: &str, temp: &TempDir, what: &str) {
    let root = temp.path().to_string_lossy();
    for leak in [PATH_SENTINEL, root.as_ref()] {
        assert!(
            !rendered.contains(leak),
            "{what} leaked the path: {rendered}"
        );
    }
    // The temp root's own last name — the process-named folder — must not
    // surface either, spelled in any case.
    let last = temp.path().file_name().unwrap().to_string_lossy();
    assert!(
        !rendered.to_lowercase().contains(&last.to_lowercase()),
        "{what} leaked the folder name: {rendered}"
    );
}

#[test]
fn debug_of_everything_that_touched_a_path_prints_no_path() {
    let temp = TempDir::new("privacy");
    let root = temp.dir("project");
    let inside = temp.dir("project/src");
    let roots = Roots::default().project(project_id(1), &root);

    let scope = identity(&inside);
    let mut claims = ScopeClaims::new();
    claims.admit_resolved(
        run_id(1),
        attempt_id(11),
        identity(&root),
        StartOrigin::Manual,
    );
    let manual = claims.admit(
        run_id(2),
        attempt_id(22),
        &folder_scope(1, "src"),
        &roots,
        StartOrigin::Manual,
    );
    let skipped = claims.admit(
        run_id(3),
        attempt_id(33),
        &root_scope(1),
        &roots,
        scheduled(5),
    );
    let refused = claims.admit(
        run_id(4),
        attempt_id(44),
        &folder_scope(1, "absent"),
        &roots,
        StartOrigin::Manual,
    );
    let durable = FakeDurable {
        claims: Ok(vec![DurableClaim {
            run: run_id(1),
            attempt: attempt_id(11),
            scope: folder_scope(1, "src"),
        }]),
    };
    let (rebuilt, report) =
        ScopeClaims::reconcile(&durable, &roots, |_| Liveness::Unknown).unwrap();

    let renderings = [
        ("identity", format!("{scope:?}")),
        ("identity (pretty)", format!("{scope:#?}")),
        ("claims", format!("{claims:?}")),
        ("claims (pretty)", format!("{claims:#?}")),
        ("manual admission", format!("{manual:?}")),
        ("skipped admission", format!("{skipped:#?}")),
        ("refused admission", format!("{refused:?}")),
        ("refusal", format!("{}", ScopeRefusal::NotFound)),
        ("rebuilt registry", format!("{rebuilt:#?}")),
        ("report", format!("{report:#?}")),
        ("durable claim", format!("{:?}", durable.claims)),
        ("holders", format!("{:?}", claims.holders_of(&scope))),
    ];
    for (what, rendered) in &renderings {
        assert_redacted(rendered, &temp, what);
    }
    assert!(
        format!("{scope:?}").starts_with("ScopeIdentity(#"),
        "the marker is a hash, not a path"
    );
}

#[test]
fn one_folder_prints_one_marker_and_two_folders_print_two() {
    let temp = TempDir::new("marker");
    let folder = temp.dir("work");
    let other = temp.dir("other");
    let mut with_separator = folder.clone().into_os_string();
    with_separator.push(std::path::MAIN_SEPARATOR_STR);

    let a = format!("{:?}", identity(&folder));
    let b = format!("{:?}", identity(std::path::Path::new(&with_separator)));
    let c = format!("{:?}", identity(&other));
    assert_eq!(a, b, "the same folder, spelled twice");
    assert_ne!(a, c, "a different folder");
}
