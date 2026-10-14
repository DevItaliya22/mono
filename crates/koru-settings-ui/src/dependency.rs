// SPDX-License-Identifier: MPL-2.0

//! **The fence around this surface's dependencies, in both directions.**
//!
//! Down: a Settings surface that linked `koru-core`, `koru-workspace`,
//! `koru-editor` or `koru-workspace-host` could not be hosted by Koru
//! Terminal or Koru AI, which have none of them — and being hostable by all
//! three is the entire reason this crate exists rather than a fifth copy of
//! the rows. `koru-search-ui`'s manifest comment says the same thing in prose;
//! this is the version that fails the build.
//!
//! Up: nothing may reach *into* the surface for a preference. The authority is
//! `koru-settings` behind the Gateway, and a caller that got a value out of a
//! UI crate would be reading a screen instead of asking the owner.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

#[test]
fn the_manifest_names_only_what_a_neutral_surface_may_link() {
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
            "koru-i18n",
            "koru-settings",
            "koru-draw",
            "koru-keys",
            "koru-ui",
            "vello",
            "winit",
        ]),
        "the Settings surface draws rows and talks to the settings client; a \
         dependency beyond this list is a dependency one of the three hosts \
         does not have"
    );
}

#[test]
fn the_normal_graph_holds_no_document_workspace_or_session() {
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
        // The model, and everything that projects it. A settings row names no
        // document, no workspace and no session — the charter's own sentence.
        "koru-core",
        "koru-workspace",
        "koru-editor",
        "koru-document",
        "koru-blocks",
        // Koru's window host. Reaching this for a voice preference is what kept
        // these rows inside `apps/koru`; the host now performs them.
        "koru-workspace-host",
        // The store. This surface reads and writes no files and opens no
        // database; it asks the Gateway, like every other caller since SP-6b.
        "koru-store-server",
        "rusqlite",
        // The authority's own hosts. Consuming the *client* is the contract;
        // linking the authority would put a second writer in the process.
        "koru-api-gateway-core",
    ] {
        assert!(
            !packages.contains(forbidden),
            "forbidden dependency {forbidden}: the Settings surface must be \
             hostable by Koru, Koru Terminal and Koru AI alike"
        );
    }
}

/// **Nothing reads a preference out of this crate.**
///
/// The surface holds a mirror of two sections so it can draw them; that mirror
/// is not an authority and must not become one by being convenient. A caller
/// that wants a setting asks `koru_api_client::settings()`, which is the whole
/// point of ADR-266 — and the failure mode being guarded against is a second
/// answer, not a second `use`.
#[test]
fn no_caller_reaches_into_the_surface_for_a_value() {
    let root = workspace_root();
    let offenders: Vec<String> = rust_sources(&root)
        .into_iter()
        .filter(|file| !file.starts_with(root.join("crates").join("koru-settings-ui")))
        .filter(|file| {
            std::fs::read_to_string(file).is_ok_and(|text| {
                text.contains("koru_settings_ui::")
                    && text.contains("SettingsSurface::authoritative")
            })
        })
        .map(|file| relative(&root, &file))
        .collect();
    assert!(
        offenders.is_empty(),
        "a caller is reading a setting out of the Settings surface: {offenders:?}"
    );
}

fn rust_sources(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for area in ["crates", "apps"] {
        walk(&root.join(area), &mut out);
    }
    out
}

fn walk(directory: &Path, into: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            walk(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/koru-settings-ui sits two levels under the workspace root")
        .to_path_buf()
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}
