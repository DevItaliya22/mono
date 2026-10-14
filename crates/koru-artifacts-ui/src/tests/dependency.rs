// SPDX-License-Identifier: MPL-2.0

//! **The dependency list is the point.**
//!
//! This surface is a projection of the Artifact Store, not of a document,
//! so it must not learn what a node, a workspace document or an editor is:
//! a host that has none of those can still draw it. `Cargo.toml` says so in
//! prose and this says so in a test — the same check
//! `koru-artifact-core`'s own dependency test makes, one layer up.

use std::collections::BTreeSet;

/// Crates this surface must never link, and why each would be a mistake.
///
/// **`koru-core` left this list, deliberately** (ADR-364). A Markdown
/// version becomes a `WorkspaceDoc` on the way to being drawn — in memory,
/// out of bytes the column was handed, discarded when the reader closes —
/// and that happens inside `koru-markdown-view`, which is the *only* crate
/// here that may do it. What stays forbidden is everything that could hand
/// this column a document somebody else owns: an artifact is still not a
/// document (D1), and the column is still a projection of the Artifact
/// Store.
const FORBIDDEN: &[&str] = &[
    // The workspace document and the shell that owns one.
    "koru-workspace",
    "koru-editor",
    "koru-docstore",
    // The shell: this is a column inside a window, not a window.
    "koru-workspace-host",
    "koru-window",
    // The delegate worker and its database: the column reads the Artifact
    // Store through the gateway, and knows nothing about how a run is run.
    "koru-delegate-runtime",
    "koru-delegate-sqlite",
    "koru-artifact-sqlite",
];

fn manifest() -> String {
    include_str!("../../Cargo.toml").to_owned()
}

#[test]
fn the_surface_names_no_document_model_no_shell_and_no_worker() {
    let manifest = manifest();
    let named: BTreeSet<&str> = manifest
        .lines()
        .filter_map(|line| line.split_once(" = {").map(|(name, _)| name.trim()))
        .collect();
    for forbidden in FORBIDDEN {
        assert!(
            !named.contains(forbidden),
            "{forbidden} would make this surface un-hostable"
        );
    }
    // And the two that make it a *surface* are there.
    assert!(named.contains("koru-draw"));
    assert!(named.contains("koru-i18n"));
    // **One route to the node model, and it is named here.** A second one
    // would be a change to this test as well as to the manifest.
    assert!(named.contains("koru-markdown-view"));
}

/// The `no-reader` feature exists, because a host's own tests cannot be
/// covered by this crate's `cfg(test)`.
#[test]
fn a_host_can_turn_the_reader_off_for_its_own_tests() {
    assert!(manifest().contains("no-reader = []"));
}
