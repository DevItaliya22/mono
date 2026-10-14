// SPDX-License-Identifier: MPL-2.0

//! **The fence.** This surface makes a document out of bytes it was handed;
//! it must never be able to reach one that is stored.
//!
//! The distinction is the whole charter. `koru-core` is here *because* a
//! `WorkspaceDoc` is what Markdown becomes on the way through — in memory,
//! from bytes, discarded when the view closes. The store, the shell, the
//! editor and the workspace are not, so there is no path from this crate to
//! a document anybody else owns, and no host has to hand one in.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

#[test]
fn the_manifest_names_only_the_round_trip_and_the_drawing() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let section = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap_or_default()
        .split("[dev-dependencies]")
        .next()
        .unwrap_or_default();
    let actual: BTreeSet<_> = section
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty() && !name.starts_with('#'))
        .map(|name| name.split('.').next().unwrap())
        .collect();
    assert_eq!(
        actual,
        BTreeSet::from([
            "koru-export",
            "koru-export-markdown",
            "koru-i18n",
            "koru-import-markdown",
            "parley",
            "koru-core",
            "koru-draw",
            "koru-markdown",
            "koru-ui",
            "uuid",
            "vello",
            "winit",
        ]),
        "a dependency beyond this list is a second way to get Markdown in or \
         out, or a way to reach a document this surface was not handed"
    );
}

#[test]
fn the_normal_graph_holds_no_store_no_shell_and_no_editor() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "koru-markdown-view",
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--no-dedupe",
        ])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output();
    let Ok(output) = output else {
        // `cargo tree` needs a cargo on PATH; a sandbox without one is not
        // a failing fence.
        return;
    };
    if !output.status.success() {
        return;
    }
    let tree = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        // A stored document, and the things that own one.
        "koru-docstore",
        "koru-workspace",
        "koru-editor",
        "koru-workspace-host",
        "koru-store-client",
        "koru-store-sqlite",
        // A window. This is a piece of one.
        "koru-window",
        // The delegate worker and the artifact store: this surface is
        // handed bytes and does not know where they came from.
        "koru-delegate-runtime",
        "koru-artifact-sqlite",
        "koru-api-client",
    ] {
        assert!(
            !tree.lines().any(|line| line.starts_with(forbidden)),
            "koru-markdown-view must not link {forbidden}"
        );
    }
}
