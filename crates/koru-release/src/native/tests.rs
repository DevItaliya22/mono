// SPDX-License-Identifier: MPL-2.0

use super::*;
use sha2::{Digest, Sha256};

const CANONICAL: &[u8] = b"the canonical archive's bytes";
const LOCAL: &[u8] = b"a stale or local archive: the right name, a different hash";

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A row shaped like the Windows one in `artifacts::NAMES`, pinned to bytes a
/// test can write — the real pin names a 117 MB archive.
fn pinned_to(bytes: &[u8]) -> (NativeArtifact, &'static str) {
    let sha = Box::leak(hex(bytes).into_boxed_str());
    (
        NativeArtifact {
            target: "x86_64-pc-windows-msvc",
            target_os: "windows",
            target_arch: "x86_64",
            stem: "win-x64-static-MT-Release-no-tts-lib",
            sha256: sha,
            size: bytes.len() as u64,
        },
        sha,
    )
}

fn cache_with(artifact: &NativeArtifact, bytes: &[u8]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(artifact.file_name()), bytes).unwrap();
    dir
}

#[test]
fn the_canonical_archive_is_accepted() {
    let (artifact, _) = pinned_to(CANONICAL);
    let cache = cache_with(&artifact, CANONICAL);
    let found = linked(&artifact, cache.path(), None).unwrap();
    assert_eq!(found, Linked::Canonical);
    assert!(require_canonical(&artifact, &found).is_ok());
    assert_eq!(notice(&artifact, &found), None);
}

#[test]
fn an_official_release_refuses_a_non_canonical_archive_and_a_local_bundle_names_it() {
    let (artifact, pin) = pinned_to(CANONICAL);
    let cache = cache_with(&artifact, LOCAL);
    let found = linked(&artifact, cache.path(), None).unwrap();
    assert!(matches!(&found, Linked::NotCanonical { sha256, .. } if *sha256 == hex(LOCAL)));
    let refusal = require_canonical(&artifact, &found).unwrap_err();
    assert!(refusal.contains(&hex(LOCAL)), "{refusal}");
    assert!(
        refusal.contains(pin),
        "the refusal names the pin: {refusal}"
    );
    assert!(
        notice(&artifact, &found)
            .unwrap()
            .contains("dist-cloud would refuse")
    );
}

/// The size is part of the pin: same name, same hash prefix is not enough,
/// and a truncated copy must not pass on a hash comparison it never reached.
#[test]
fn a_size_mismatch_is_not_canonical() {
    let (mut artifact, _) = pinned_to(CANONICAL);
    artifact.size += 1;
    let cache = cache_with(&artifact, CANONICAL);
    let found = linked(&artifact, cache.path(), None).unwrap();
    assert!(matches!(found, Linked::NotCanonical { .. }));
}

/// An unpinned row pins nothing, so nothing can match it.
#[test]
fn an_unpinned_row_is_never_canonical() {
    let (mut artifact, _) = pinned_to(CANONICAL);
    artifact.sha256 = "";
    let cache = cache_with(&artifact, CANONICAL);
    let found = linked(&artifact, cache.path(), None).unwrap();
    assert!(require_canonical(&artifact, &found).is_err());
}

#[test]
fn an_extracted_lib_dir_cannot_be_verified() {
    let (artifact, _) = pinned_to(CANONICAL);
    // Even with the canonical archive cached: the override is what was linked.
    let cache = cache_with(&artifact, CANONICAL);
    let lib = cache.path().join("lib");
    let found = linked(&artifact, cache.path(), Some(&lib)).unwrap();
    assert_eq!(found, Linked::LibDirOverride(lib));
    let refusal = require_canonical(&artifact, &found).unwrap_err();
    assert!(refusal.contains("SHERPA_ONNX_LIB_DIR"), "{refusal}");
}

#[test]
fn a_cache_without_the_archive_cannot_be_verified() {
    let (artifact, _) = pinned_to(CANONICAL);
    let cache = tempfile::tempdir().unwrap();
    let found = linked(&artifact, cache.path(), None).unwrap();
    assert_eq!(
        found,
        Linked::Missing(cache.path().join(artifact.file_name()))
    );
    assert!(require_canonical(&artifact, &found).is_err());
}

/// The cache is where `sherpa-onnx-sys` puts it: `CARGO_TARGET_DIR` when set,
/// otherwise `target/` — the same rule as its `target_dir_from_out_dir`.
#[test]
fn the_cache_follows_the_build_scripts_rule() {
    let root = Path::new("repo");
    assert_eq!(
        cache_root(root, None),
        Path::new("repo")
            .join("target")
            .join("sherpa-onnx-prebuilt")
    );
    assert_eq!(
        cache_root(root, Some(Path::new("elsewhere"))),
        Path::new("elsewhere").join("sherpa-onnx-prebuilt")
    );
}

/// The release checks the very table the provisioning scripts and the build
/// script's naming test read, rather than a copy of it.
#[test]
fn every_shipped_host_has_a_pinned_archive_to_check() {
    for artifact in artifacts::NAMES {
        assert!(artifact.is_pinned(), "{} is unpinned", artifact.target);
        assert!(artifact.size > 0);
    }
    if cfg!(any(windows, target_os = "macos")) {
        assert!(artifacts::artifact_for_this_build().is_some());
    }
}
