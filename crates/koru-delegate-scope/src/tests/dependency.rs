// SPDX-License-Identifier: MPL-2.0

//! The fence around a leaf the orchestrator calls before every start.
//!
//! It links the run vocabulary and `std`'s filesystem, and nothing that
//! runs a process, opens a database, carries a wire or draws — `runs.db`
//! implements the port declared here, not the other way round.

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
        BTreeSet::from(["koru-delegate-core", "uuid"]),
        "koru-delegate-scope is a leaf over the run vocabulary and std"
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
        // Surfaces.
        "winit",
        "vello",
        "parley",
        "koru-ui",
        "koru-window",
        "koru-editor",
        "koru-core",
        // Runtimes and transports.
        "tokio",
        "axum",
        "reqwest",
        "portable-pty",
        "alacritty_terminal",
        "koru-ipc",
        // Databases: the port `runs.db` implements is declared here.
        "rusqlite",
        "koru-store-sqlite",
        "koru-ai-sqlite",
        // File identity helpers the tree links only transitively.
        "same-file",
        // The crates above this one in the delegation stack.
        "koru-session-runtime",
        "koru-session-wire",
        "koru-session-client",
        "koru-session-paths",
        "koru-session-profiles",
        "koru-agent-wire",
        "koru-api-gateway-core",
        "koru-store-client",
        "koru-store-wire",
        "koru-ai-core",
    ] {
        assert!(
            !packages.contains(forbidden),
            "forbidden dependency {forbidden}: {packages:?}"
        );
    }
}
