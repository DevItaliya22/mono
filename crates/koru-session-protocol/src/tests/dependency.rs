// SPDX-License-Identifier: MPL-2.0

use std::{collections::BTreeSet, path::Path, process::Command};

#[test]
fn normal_dependency_direction_is_a_leaf_allowlist() {
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
        .collect();
    let expected = BTreeSet::from(["schemars.workspace", "serde.workspace", "uuid.workspace"]);
    // Strip the `.workspace` TOML key suffix produced by this compact parser.
    let actual: BTreeSet<_> = actual
        .into_iter()
        .map(|name| name.split('.').next().unwrap())
        .collect();
    let expected: BTreeSet<_> = expected
        .into_iter()
        .map(|name| name.split('.').next().unwrap())
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn normal_transitive_graph_has_no_runtime_or_surface_package() {
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
        "winit",
        "vello",
        "portable-pty",
        "alacritty_terminal",
        "tokio",
        "axum",
        "koru-agent-wire",
        "koru-store-core",
        "koru-api-gateway-core",
    ] {
        assert!(
            !packages.contains(forbidden),
            "forbidden dependency {forbidden}: {packages:?}"
        );
    }
}
