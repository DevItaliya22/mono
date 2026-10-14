// SPDX-License-Identifier: MPL-2.0

//! The fence around a leaf the orchestrator, `runs.db`'s republisher and
//! the events worker's own tests all link: the vocabularies whose ids and
//! closed enums become attributes, and nothing that publishes, stores,
//! transports or draws.

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
        BTreeSet::from([
            "koru-artifact-core",
            "koru-delegate-core",
            "koru-events",
            "koru-session-protocol",
            "uuid",
        ]),
        "koru-delegate-events is a leaf: the run vocabulary, the artifact ids, \
         the event model, the session protocol's closed enums, and uuid"
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
        "koru-events-server",
        "koru-delegate-runtime",
        "koru-delegate-sqlite",
        "koru-delegate-document",
        "koru-delegate-scope",
        "koru-store",
        "koru-store-server",
        "koru-api-client",
        "koru-api-gateway-core",
        "koru-session-runtime",
        "koru-session-host",
        "koru-core",
        "koru-ui",
        "rusqlite",
        "axum",
        "reqwest",
        "tokio",
        // `loro` is *not* on this list: `koru-store-core` links it for the
        // store's ids, and the run vocabulary links that. This crate adds
        // nothing on top of what `koru-delegate-core` already carries.
    ] {
        assert!(
            !packages.contains(forbidden),
            "koru-delegate-events must not link `{forbidden}`; the graph:\n{tree}"
        );
    }
}
