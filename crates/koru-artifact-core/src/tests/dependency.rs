// SPDX-License-Identifier: MPL-2.0

//! The fence around a leaf that the store, the routes, the MCP tools and the
//! surface all link. It may name the run vocabulary it reuses and nothing
//! that opens a database, carries a wire or draws.

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
        .map(|name| name.split('.').next().unwrap())
        .collect();
    assert_eq!(
        actual,
        BTreeSet::from(["koru-delegate-core", "schemars", "serde", "uuid"]),
        "koru-artifact-core is a leaf: the run vocabulary it reuses, serde, an \
         optional schema derive, and uuid"
    );
}

#[test]
fn normal_transitive_graph_has_no_runtime_store_wire_or_surface_package() {
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
        "rusqlite",
        "axum",
        "tokio",
        "reqwest",
        "koru-store-sqlite",
        "koru-store-api",
        "koru-store-server",
        "koru-api-gateway-core",
        "koru-api-client",
        "koru-artifact-sqlite",
        "koru-artifact-api",
        "koru-artifact-wire",
        "koru-core",
        "koru-ui",
    ] {
        assert!(
            !packages.contains(forbidden),
            "koru-artifact-core links {forbidden}"
        );
    }
}
