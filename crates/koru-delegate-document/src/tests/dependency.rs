// SPDX-License-Identifier: MPL-2.0

//! The fence around the charter: the shape, not the store, and never the
//! window. What this crate links is what every reader of a run links — the
//! orchestrator, the surfaces, the phone — so it names the run vocabulary,
//! the store's ids and gate, the CRDT and serde, and nothing that opens a
//! store, takes a lock or draws.

use std::{collections::BTreeSet, path::Path};

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
            "koru-delegate-core",
            "koru-delegate-schedule",
            "koru-store-core",
            "loro",
            "serde",
            "serde_json",
        ]),
        "koru-delegate-document knows the shape, not the store, and never the window"
    );
}
