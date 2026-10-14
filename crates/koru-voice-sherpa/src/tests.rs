// SPDX-License-Identifier: MPL-2.0

use super::*;

/// This crate's own manifest, read at compile time.
///
/// The guarantee Plan 0.3 exists for is a *manifest* property — the runtime is
/// an optional dependency and no default feature turns it on — and no amount of
/// `cfg!` can see it, because a test compiled with `--features runtime` would
/// report the feature on and say nothing about whether the default set does. So
/// the manifest is what is asserted.
const MANIFEST: &str = include_str!("../Cargo.toml");

#[test]
fn the_engine_id_is_a_legal_path_component() {
    // `EngineId::parse` is the validation, and it is what stops a signed
    // descriptor's `engine` field from ever becoming `../..`. The spelling is
    // asserted too, because the descriptor a model is published with has to
    // name this exact string.
    assert_eq!(engine_id().as_str(), "sherpa");
}

#[test]
fn no_default_feature_compiles_the_runtime() {
    // The build-script trap, closed at the only place that can close it.
    // `sherpa-onnx-sys`' build script downloads a native archive when
    // `SHERPA_ONNX_LIB_DIR` is unset; a crate that is not compiled has no build
    // script that runs. So: `default` is empty, and `sherpa-onnx` is optional.
    assert!(
        MANIFEST.contains("default = []"),
        "koru-voice-sherpa must declare an empty default feature set, or an \
         ordinary `cargo test --workspace` reaches the network at build time"
    );
    assert!(
        MANIFEST.contains("optional = true"),
        "the sherpa-onnx dependency must be optional"
    );
}

#[test]
fn the_runtime_version_is_pinned_exactly() {
    // The native archive the build script fetches is named from
    // `CARGO_PKG_VERSION`, so a caret range would let a minor bump ask for a
    // differently named file — and the hashes in `artifacts::NAMES` would then
    // be pinning an artifact nothing downloads.
    let pin = format!("version = \"={SHERPA_ONNX_VERSION}\"");
    assert!(
        MANIFEST.contains(&pin),
        "Cargo.toml must pin sherpa-onnx as {pin}, matching artifacts::SHERPA_ONNX_VERSION"
    );
}
