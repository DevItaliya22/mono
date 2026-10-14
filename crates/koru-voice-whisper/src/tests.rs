// SPDX-License-Identifier: MPL-2.0

use super::*;

/// This crate's own manifest, read at compile time. See the sibling test in
/// `koru-voice-sherpa` for why the guarantee is a manifest property rather than
/// something `cfg!` can see.
const MANIFEST: &str = include_str!("../Cargo.toml");

#[test]
fn the_engine_id_is_its_own_and_is_a_legal_path_component() {
    assert_eq!(engine_id().as_str(), "whisper");
    // Distinct from sherpa's, because `engine` on a signed descriptor answers
    // *which native parser may read these files*. A GGML `.bin` handed to ONNX
    // Runtime is exactly the routing mistake that field exists to prevent.
    assert_ne!(engine_id().as_str(), "sherpa");
}

#[test]
fn no_default_feature_compiles_the_runtime() {
    // whisper-rs-sys reaches no network — it vendors whisper.cpp — but it needs
    // CMake, a C++ toolchain and libclang, and making those a requirement of
    // `cargo test --workspace` would break a fresh clone on a machine that has
    // none of them, for a crate nothing in the product uses.
    assert!(
        MANIFEST.contains("default = []"),
        "koru-voice-whisper must declare an empty default feature set"
    );
    assert!(
        MANIFEST.contains("optional = true"),
        "the whisper-rs dependency must be optional"
    );
}

#[test]
fn no_gpu_feature_is_turned_on() {
    // No candidate in this gate is configured for a GPU: the desktop worker's
    // target is a CPU budget on a machine doing other work, and a GPU path
    // would be a different set of artifacts to provision. Turning one on here
    // would also make the build depend on a vendor SDK.
    for gpu in ["cuda", "metal", "vulkan", "coreml", "hipblas", "intel-sycl"] {
        assert!(
            !MANIFEST.contains(&format!("\"{gpu}\"")),
            "koru-voice-whisper must not enable whisper-rs's {gpu} feature"
        );
    }
}
