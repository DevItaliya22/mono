// SPDX-License-Identifier: MPL-2.0

//! The charter, checked: the manifest *is* the boundary, so these tests read it
//! rather than trusting the lib doc's sentence (the `koru-session-protocol`
//! pattern, required by ADR-262).

use std::{collections::BTreeSet, path::Path, process::Command};

#[test]
fn normal_dependency_direction_is_the_declared_allowlist() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let section = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap()
        .split("[dev-dependencies]")
        .next()
        .unwrap();
    let actual: BTreeSet<_> = section
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty() && !name.starts_with('#'))
        .map(|name| name.split('.').next().unwrap())
        .collect();
    let expected = BTreeSet::from(["koru-draw", "koru-ui", "vello", "peniko"]);
    assert_eq!(actual, expected);
}

/// No id, no loop, no host: the transitive graph proves the slots cannot learn
/// what they mean. (`winit` does arrive transitively — `koru-ui` links it for
/// key normalisation — but no *window* does: `koru-window` is the line.)
#[test]
fn normal_transitive_graph_names_no_domain_and_no_loop() {
    let output = Command::new(env!("CARGO"))
        .args(["tree", "--manifest-path"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .args(["-e", "normal", "--prefix", "none"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8(output.stdout).unwrap();
    let packages: BTreeSet<_> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    for forbidden in [
        "koru-core",
        "koru-window",
        "koru-editor",
        "koru-workspace",
        "koru-workspace-host",
        "koru-session-protocol",
        "koru-session-client",
        "koru-store-core",
        "koru-agent-wire",
        "tokio",
        "axum",
    ] {
        assert!(
            !packages.contains(forbidden),
            "forbidden dependency {forbidden}: {packages:?}"
        );
    }
}
