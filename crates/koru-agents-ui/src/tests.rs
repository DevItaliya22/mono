// SPDX-License-Identifier: MPL-2.0

//! The dependency direction is the crate's contract, so it is tested rather
//! than promised (mirrors `koru-session-backend`'s and `koru-app-shell`'s
//! allowlist tests).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// The crate is the seam a second host reuses, so its compile graph is the
/// contract.
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
    let expected = BTreeSet::from([
        "koru-i18n",
        "koru-session-observability",
        "koru-session-protocol",
        "koru-session-view",
        "koru-draw",
        "koru-ui",
        "vello",
    ]);
    assert_eq!(
        actual, expected,
        "the allowlist is the compile graph, not a review comment"
    );
}

/// No host application, no process or IPC authority, no window runner. The
/// surface takes a [`SurfaceInteraction`](crate::SurfaceInteraction) and emits
/// typed intents; connection, discovery, root and launch identity belong to
/// the host, so none of their crates may enter this tree.
///
/// `winit` is deliberately absent from the forbidden list: it rides in through
/// `koru-ui` (which links it for its own text-field and cursor modules), and
/// forbidding the package would really be forbidding the tooltip. What this
/// crate owes is narrower and is pinned by the source check below: no module
/// here names a winit type, so it cannot own an event loop.
#[test]
fn normal_transitive_graph_names_no_host_no_process_and_no_window_runner() {
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
        "koru-app-shell",
        "koru-ipc",
        "koru-session-client",
        "koru-session-wire",
        "koru-session-backend",
        "koru-api-client",
        "koru-window",
    ] {
        assert!(
            !packages.contains(forbidden),
            "{forbidden} must not appear in koru-agents-ui's normal dependency tree"
        );
    }
}

/// The code half of the rule above: the production modules name no winit type
/// (so the crate cannot own an event loop even though `koru-ui` links the
/// package) and reach no shell or backend by path.
#[test]
fn production_modules_name_no_window_toolkit_and_no_backend() {
    for module in [
        include_str!("lib.rs"),
        include_str!("interaction.rs"),
        include_str!("surface.rs"),
    ] {
        let production = module
            .split("#[cfg(test)]")
            .next()
            .expect("the module's production half");
        for forbidden in [
            "winit",
            "koru_app_shell",
            "koru_session_backend",
            "koru_window",
        ] {
            assert!(
                !production.contains(forbidden),
                "a surface module must not name {forbidden}"
            );
        }
    }
}
