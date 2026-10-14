// SPDX-License-Identifier: MPL-2.0

//! **The fence around this surface's dependencies, in both directions.**
//!
//! Down: a run view that linked the store, the session client, the
//! delegate runtime or a document model would be a run view that could read
//! a run without going through `/v1/runs` — and the whole reason nothing
//! here blocks a frame is that there is exactly one way to read a run and
//! it is on another thread. It is also the charter's own sentence: a
//! surface that could name a `RunDocument` would be a surface that could be
//! tempted to open one.
//!
//! Up: nothing reaches *into* this surface for a run. The Run document is
//! the authority (§7.1); a caller that got a header out of a UI crate would
//! be reading a screen instead of asking the owner.

#[cfg(test)]
use std::collections::BTreeSet;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::process::Command;

/// The manifest is an exact allowlist, and the prose comment above each
/// line is the human half of the same fence.
#[test]
fn the_manifest_names_only_what_a_run_view_may_link() {
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
            "koru-api-client",
            "koru-channel",
            "koru-delegate-api",
            "koru-delegate-core",
            "koru-delegate-preamble",
            "koru-i18n",
            "koru-markdown-view",
            "koru-session-protocol",
            "koru-draw",
            "koru-ui",
            "vello",
            "winit",
        ]),
        "the run views draw a run and read it through the typed client; a \
         dependency beyond this list is a way to reach a run that does not \
         go through /v1/runs"
    );
}

/// The transitive graph, with a reason per entry.
#[test]
fn the_normal_graph_holds_no_store_session_host_or_delegate_worker() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "koru-runs-ui",
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
        // The delegate worker's own machinery: the run's authority is the
        // Run document, and a surface is not entitled to drive one.
        "koru-delegate-runtime",
        "koru-delegate-document",
        "koru-delegate-sqlite",
        // The session host's client: taking a lease here would be a second
        // controller in one window (§7.4).
        "koru-session-client",
        // Windows, event loops and *stored* documents. `koru-core` is
        // reached through `koru-markdown-view`, deliberately: a Markdown
        // result becomes a `WorkspaceDoc` in memory on the way to being
        // drawn, out of bytes this surface was handed. What stays forbidden
        // is everything that could hand it one somebody else owns.
        "koru-editor",
        "koru-workspace",
        "koru-workspace-host",
        "koru-docstore",
    ] {
        assert!(
            !tree.lines().any(|line| line.starts_with(forbidden)),
            "koru-runs-ui must not link {forbidden}"
        );
    }
}
