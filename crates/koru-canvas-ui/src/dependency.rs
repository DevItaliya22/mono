// SPDX-License-Identifier: MPL-2.0

//! **The fence around this surface's dependencies.**
//!
//! A canvas surface that linked the store, a workspace or the document model
//! would be one that could write a document itself — and every write here is
//! the container's, sent as a request. The manifest is read at compile time,
//! so the fence costs no disk at test time.

use std::collections::BTreeSet;

#[test]
fn the_manifest_names_only_what_a_drawing_surface_may_link() {
    let manifest = include_str!("../Cargo.toml");
    let section = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("a [dependencies] section")
        .split("\n[")
        .next()
        .expect("its body");
    let actual: BTreeSet<&str> = section
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
        .map(|name| name.split('.').next().unwrap_or(name))
        .collect();
    assert_eq!(
        actual,
        BTreeSet::from([
            "koru-i18n",
            "parley",
            "koru-draw",
            "koru-ui",
            "uuid",
            "vello",
            "winit",
        ]),
        "the canvas surface draws a plain scene and hands back requests; a \
         dependency beyond this list is a way to reach the document itself"
    );
}
