// SPDX-License-Identifier: MPL-2.0

//! The fence: this crate links the store's *service* and the renderer, and
//! never a SQLite backend, a transport, a runtime or a surface. The tests
//! open SQLite; the crate does not.

use std::{collections::BTreeSet, path::Path, process::Command};

#[test]
fn normal_dependency_direction_is_an_allowlist() {
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
            "koru-export",
            "koru-export-markdown",
            "koru-session-protocol",
            "koru-store-core",
            "serde",
            "serde_json",
            "koru-core",
        ]),
        "koru-delegate-context links the run vocabulary, the store service, \
         the Markdown exporter and the model it walks, and serde"
    );
}

#[test]
fn normal_transitive_graph_has_no_database_transport_runtime_or_surface() {
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
        "parley",
        "koru-ui",
        "koru-window",
        "koru-editor",
        "tokio",
        "axum",
        "reqwest",
        "portable-pty",
        "koru-ipc",
        "rusqlite",
        "koru-store-sqlite",
        "koru-ai-sqlite",
        "koru-delegate-sqlite",
        "koru-session-runtime",
        "koru-session-wire",
        "koru-store-client",
        "koru-store-wire",
        "koru-api-client",
    ] {
        assert!(
            !packages.contains(forbidden),
            "forbidden dependency {forbidden}: {packages:?}"
        );
    }
}
