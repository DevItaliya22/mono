// SPDX-License-Identifier: MPL-2.0

//! The fence around the role's composition: a delegate worker reaches the
//! ledger, the Store through the gateway and the session host — and never
//! a window. `winit`, `vello`, `parley` and the `koru-*` surface crates
//! are forbidden in the normal graph.
//!
//! **`koru-core` is not one of them any more.** It was forbidden while
//! capture was a stub: an orchestrator that writes no context has no
//! business acquiring the node model. It writes context now — §7.2 renders
//! a pinned version to Markdown through the one exporter Koru has, and
//! that exporter walks the model. `koru-core` is the model with no UI and
//! no rendering of its own, so what it brings is a document, not a
//! surface; the surface crates that draw one are still refused below.

use std::{collections::BTreeSet, path::Path, process::Command};

#[test]
fn normal_transitive_graph_has_no_window_or_surface_package() {
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
        "wgpu",
        "koru-ui",
        "koru-window",
        "koru-editor",
        "koru-workspace",
        "koru-draw",
        "koru-keys",
        "koru-search-ui",
        "koru-voice-ui",
        "koru-agents-ui",
        "tray-icon",
        "rfd",
        // The other roles' compositions: a delegate worker is not a
        // gateway and not a session host.
        "koru-api-gateway",
        "koru-api-gateway-core",
        "koru-session-host",
        "koru-session-runtime",
        "koru-store-server",
        "axum",
    ] {
        assert!(
            !packages.contains(forbidden),
            "forbidden dependency {forbidden}: {packages:?}"
        );
    }
    for required in [
        "koru-delegate-runtime",
        // The two ports the parallel plans filled, now composed rather
        // than stubbed.
        "koru-delegate-context",
        "koru-delegate-events",
        "koru-delegate-api",
        "koru-delegate-sqlite",
        "koru-api-client",
        "koru-session-client",
        "koru-agent-worker",
    ] {
        assert!(
            packages.contains(required),
            "{required} missing: {packages:?}"
        );
    }
}
