// SPDX-License-Identifier: MPL-2.0

//! The charter, checked — and the tab double every other test module borrows.
//!
//! The dependency tests read the manifest and the transitive graph rather than
//! trusting the lib doc's sentence (the `koru-session-protocol` pattern,
//! required by ADR-262). **The compile graph is what keeps a workspace switch
//! from ever being the shell's business**, and a sentence is not what enforces
//! it.

use std::{collections::BTreeSet, path::Path, process::Command};

use uuid::Uuid;

use crate::element::TabElement;

/// A tab with nothing behind it, so the generic machinery can be tested without
/// a host.
///
/// **A double inside the crate, not a `cfg(test)` reach into `apps/koru`.** A
/// dependency is never compiled with `cfg(test)`, so a Koru `Tab` could not
/// reach here even if the dependency direction allowed it — which it does not.
/// Koru's own rules are tested against the real `Tab` in
/// `apps/koru/src/mainview/tab_rules.rs`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct TestTab {
    pub(crate) label: String,
    pub(crate) pinnable: bool,
    pub(crate) placeholder: bool,
    pub(crate) reusable: bool,
    pub(crate) pinned: Option<Uuid>,
    pub(crate) home: Option<Uuid>,
    pub(crate) show_title: bool,
}

impl TestTab {
    /// The floor: a placeholder, reusable, and never pinnable — which is
    /// exactly what a welcome tab is.
    pub(crate) fn floor() -> Self {
        Self {
            label: "floor".to_owned(),
            placeholder: true,
            reusable: true,
            ..Self::default()
        }
    }

    /// Real content: pinnable, not a placeholder.
    pub(crate) fn content(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            pinnable: true,
            reusable: true,
            ..Self::default()
        }
    }

    pub(crate) fn named(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            ..Self::floor()
        }
    }
}

impl TabElement for TestTab {
    fn pinned_scope(&self) -> Option<Uuid> {
        self.pinned
    }

    fn set_pinned_scope(&mut self, scope: Option<Uuid>) {
        self.pinned = scope;
    }

    fn home_scope(&self) -> Option<Uuid> {
        self.home
    }

    fn set_home_scope(&mut self, scope: Option<Uuid>) {
        self.home = scope;
    }

    fn pinnable(&self) -> bool {
        self.pinnable
    }

    fn show_title(&self) -> bool {
        self.show_title
    }

    fn set_show_title(&mut self, show: bool) {
        self.show_title = show;
    }

    fn is_reusable(&self) -> bool {
        self.reusable && self.pinned.is_none()
    }

    fn is_placeholder(&self) -> bool {
        self.placeholder
    }
}

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
        "koru-shell-chrome",
        "koru-draw",
        "koru-ui",
        "koru-window",
        "vello",
        "winit",
        "uuid",
    ]);
    assert_eq!(
        actual, expected,
        "the allowlist is the compile graph, not a review comment"
    );
}

/// No domain, no store, no host, no runtime. `koru-window` **is** here — the
/// shell sits on its multi-window runner — and that edge is the line: a window
/// is not a workspace, and nothing below it names one either.
#[test]
fn normal_transitive_graph_names_no_domain_and_no_host() {
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
        "koru-core",
        "koru-editor",
        "koru-workspace",
        "koru-workspace-host",
        "koru-session-protocol",
        "koru-session-client",
        "koru-session-host",
        "koru-session-view",
        "koru-store-core",
        "koru-agent-wire",
        "tokio",
        "axum",
    ] {
        assert!(
            !packages.contains(forbidden),
            "forbidden dependency {forbidden}: {packages:?}"
        );
    }
}

/// **The shell never depends on an app.** A conformance adapter lives in the
/// consumer crate, which is what keeps the arrow pointing one way — and the
/// manifest is where that is checkable.
#[test]
fn the_shell_never_depends_on_a_host_application() {
    let manifest = include_str!("../Cargo.toml");
    for app in ["apps/", "koru = ", "koru-workspace-host"] {
        assert!(
            !manifest.contains(app),
            "{app} must not be a dependency of the shell: the adapters live host-side"
        );
    }
}
