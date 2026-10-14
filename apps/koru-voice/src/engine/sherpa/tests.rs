// SPDX-License-Identifier: MPL-2.0

//! **What the sherpa host does before any weights exist**, which is the half of
//! it a machine with no model provisioned can still check.
//!
//! Compiled only with the `sherpa` feature, so `cargo test -p
//! koru-voice-worker` — the default — never sees them. The transcript proof is
//! next door in `crate::worker::tests::dictating`, and it needs half a gigabyte
//! of weights; everything here needs none, which is the point: the parts that
//! fail on a real person's machine are the refusals, and a refusal is testable
//! without the thing it refuses.

use std::sync::Arc;

use koru_voice::{ErrorCode, ModelId, VoiceLanguage};
use koru_voice_models::{
    InstalledModel, LicenceRef, LoadedModel, ModelDescriptor, ModelTier, ModelVersion, Sha256Digest,
};

use super::*;
use crate::models::MemoryModelStore;

const MODEL: &str = "nemotron-3.5-streaming-0.6b-160ms";
const VERSION: &str = "int8-2026-06-11";

/// A sherpa-engine descriptor over a directory of the caller's choosing.
///
/// Not `crate::fixtures::installed`, which names the *scripted* engine —
/// `LoadedModel::open` checks that field against the host's, so a fixture for
/// one host is not a fixture for the other. That check is the point of the
/// field and this is what it costs.
fn installed(id: &str, root: std::path::PathBuf) -> InstalledModel {
    InstalledModel {
        root,
        descriptor: ModelDescriptor {
            schema_version: koru_voice_models::DESCRIPTOR_SCHEMA_VERSION,
            id: ModelId::parse(id).unwrap(),
            version: ModelVersion::parse(VERSION).unwrap(),
            engine: koru_voice_sherpa::engine_id(),
            tier: ModelTier::HighAccuracy,
            languages: vec![VoiceLanguage::En, VoiceLanguage::Ja],
            size: 475_273_363,
            digest: Sha256Digest::parse(crate::fixtures::DIGEST).unwrap(),
            working_set_bytes: 2 * 1024 * 1024 * 1024,
            licence: LicenceRef::parse("OpenMDW-1.1").unwrap(),
            attribution: "a fixture".to_string(),
        },
    }
}

fn leased(id: &str, root: std::path::PathBuf) -> LoadedModel {
    let store: Arc<MemoryModelStore> = Arc::new(MemoryModelStore::new(vec![installed(id, root)]));
    LoadedModel::open(
        &*store,
        &ModelId::parse(id).unwrap(),
        &ModelVersion::parse(VERSION).unwrap(),
        &koru_voice_sherpa::engine_id(),
    )
    .expect("a sherpa descriptor is for the sherpa host")
}

#[test]
fn the_host_claims_sherpa_and_promises_no_language_it_has_not_loaded() {
    let host = SherpaEngineHost::new();
    let described = host.describe();
    assert_eq!(described.engine, koru_voice_sherpa::engine_id());
    // The two `None`-shaped answers, and they are the same rule: a host has
    // loaded nothing, so it knows neither which model nor which languages.
    assert!(described.model.is_none());
    assert!(
        described.languages.is_empty(),
        "an unloaded host must not advertise a language: {described:?}"
    );
    assert!(described.version.contains("1.13.6"));
}

#[test]
fn an_offline_package_is_refused_by_name_rather_than_loaded_without_a_partial_policy() {
    // See the module note on `super`: the files cannot say which sherpa API
    // reads them, so the id does, and the answer for the offline family is a
    // sentence rather than a recogniser that shows nothing until the key is
    // released.
    let error = SherpaEngineHost::accepts(&ModelId::parse("parakeet-tdt-0.6b-v3").unwrap())
        .expect_err("the offline family is not what this host runs");
    assert_eq!(error.code, ErrorCode::ModelLoadFailed);
    assert!(
        error.message.contains("parakeet-tdt-0.6b-v3"),
        "the refusal must name the model: {}",
        error.message
    );
    SherpaEngineHost::accepts(&ModelId::parse(MODEL).unwrap())
        .expect("the streaming family is what this host runs");
}

#[test]
fn a_directory_with_no_model_in_it_names_the_directory() {
    // **The failure a person actually meets**: a leased model whose bytes are
    // not where the descriptor says. It has to arrive as a code and a sentence,
    // never as a panic and never as a silent refusal.
    let empty = tempfile::tempdir().expect("a temporary directory");
    let model = leased(MODEL, empty.path().join("not-here"));
    // `expect_err` is unavailable: `Box<dyn SpeechEngine>` is not `Debug`, and
    // giving the trait one would put a recogniser's internals in a formatter.
    let Err(error) = SherpaEngineHost::new().open(&model) else {
        panic!("a directory that does not exist holds no weights");
    };
    assert_eq!(error.code, ErrorCode::ModelLoadFailed);
    assert!(
        error.message.contains("not-here"),
        "the refusal must name the directory: {}",
        error.message
    );
}

#[test]
fn a_half_extracted_package_names_the_file_that_is_missing() {
    // An `encoder.int8.onnx` with no `tokens.txt` beside it: the archive that
    // was interrupted, which `layout::resolve` exists to turn into a name.
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(dir.path().join("encoder.int8.onnx"), b"not really a graph").unwrap();
    let model = leased(MODEL, dir.path().to_path_buf());
    let Err(error) = SherpaEngineHost::new().open(&model) else {
        panic!("three of the four files are missing");
    };
    assert_eq!(error.code, ErrorCode::ModelLoadFailed);
    assert!(
        error.message.contains("decoder.int8.onnx"),
        "the refusal must name the first missing file: {}",
        error.message
    );
}

#[test]
fn the_thread_count_is_never_zero() {
    // It reaches a C API, where a pool of no threads is not a configuration.
    assert_eq!(SherpaEngineHost::with_threads(0).num_threads, 1);
    assert_eq!(SherpaEngineHost::with_threads(-4).num_threads, 1);
    assert_eq!(SherpaEngineHost::new().num_threads, DEFAULT_THREADS);
}
