// SPDX-License-Identifier: MPL-2.0

use std::{collections::BTreeSet, path::Path, process::Command};

#[test]
fn direct_dependencies_are_the_frozen_renderer_boundary() {
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
    let expected = BTreeSet::from([
        "koru-session-observability",
        "koru-session-protocol",
        "parley",
        "peniko",
        "serde",
        "koru-draw",
        "vello",
    ]);
    assert_eq!(actual, expected);
}

#[test]
fn transitive_graph_has_no_host_transport_runtime_or_emulator() {
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
        "alacritty_terminal",
        "portable-pty",
        "winit",
        "tokio",
        "axum",
        "koru-session-client",
        "koru-session-wire",
        "koru-terminal-emulator",
        "koru-agent-wire",
        "koru-session-runtime",
        "koru-window",
    ] {
        assert!(
            !packages.contains(forbidden),
            "forbidden dependency {forbidden}: {packages:?}"
        );
    }
}
