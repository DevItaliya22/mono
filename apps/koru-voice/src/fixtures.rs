// SPDX-License-Identifier: MPL-2.0

//! **What every suite in this binary builds its worker out of.**
//!
//! One descriptor, one store, one scripted engine — shared, so that *"a
//! dictation ran"* means the same thing in five files. That is the lesson
//! `koru_store_core::fixtures` already carries for the eight languages, and the
//! reason `koru-voice-models`' own fixtures are not reachable here is that they
//! are `#[cfg(test)]` inside that crate: a dependency is never compiled with
//! `cfg(test)`.
//!
//! **Nothing here touches a disk, a network or a device.** The model's `root`
//! is a path that does not exist and is never opened, because a scripted engine
//! reads no files — which is exactly why the seam between a `ModelStore` and an
//! `EngineHost` is two traits rather than one function.

use std::sync::Arc;

use koru_voice::{EngineId, ModelId, VoiceLanguage};
use koru_voice_models::{
    InstalledModel, LicenceRef, ModelDescriptor, ModelTier, ModelVersion, Sha256Digest,
};

use crate::engine::{Script, ScriptedEngineHost};
use crate::idle::IdlePolicy;
use crate::models::MemoryModelStore;
use crate::worker::{CaptureSource, WorkerConfig};

/// The digest of the three bytes `abc`. A real SHA-256, so the newtype's own
/// rule is satisfied by something a reader can verify rather than by 64
/// arbitrary hex characters.
pub const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/// A descriptor for a model the scripted engine can run.
pub fn descriptor(id: &str, version: &str) -> ModelDescriptor {
    ModelDescriptor {
        schema_version: koru_voice_models::DESCRIPTOR_SCHEMA_VERSION,
        id: ModelId::parse(id).unwrap(),
        version: ModelVersion::parse(version).unwrap(),
        engine: ScriptedEngineHost::engine(),
        tier: ModelTier::Compact,
        languages: vec![VoiceLanguage::En, VoiceLanguage::ZhHans],
        size: 3,
        digest: Sha256Digest::parse(DIGEST).unwrap(),
        working_set_bytes: 1_073_741_824,
        licence: LicenceRef::parse("Apache-2.0").unwrap(),
        attribution: "a fixture".to_string(),
    }
}

pub fn installed(id: &str, version: &str) -> InstalledModel {
    let descriptor = descriptor(id, version);
    InstalledModel {
        // Under the memory store's own root, so `ModelStore::root`'s contract —
        // every path this store answers with lives under it — is true of the
        // fake as well as of the real one.
        root: std::path::PathBuf::from("memory:/models")
            .join(id)
            .join(version)
            .join(DIGEST),
        descriptor,
    }
}

/// The ordinary fixture model.
pub const MODEL: &str = "fixture-model";
pub const VERSION: &str = "1.0.0";

pub fn store() -> Arc<MemoryModelStore> {
    Arc::new(MemoryModelStore::new(vec![installed(MODEL, VERSION)]))
}

/// Half a second of a recognisable waveform.
///
/// **A constant rather than noise**, so the redaction suite can hunt for its
/// printed form: if any log line ever holds a sample, it holds `0.1234567`.
pub const SAMPLE: f32 = 0.123_456_7;

pub fn recording(seconds: f32) -> Arc<Vec<f32>> {
    let count = (koru_voice::SAMPLE_RATE as f32 * seconds) as usize;
    Arc::new(vec![SAMPLE; count])
}

/// A worker configuration over a scripted engine and a recording.
///
/// The idle deadlines are switched off, so a suite is never racing a timer it
/// did not mean to test; `crate::idle`'s own suite is where the deadlines are
/// checked, against injected durations rather than against the clock.
pub fn config(script: &Script, store: Arc<MemoryModelStore>) -> WorkerConfig {
    WorkerConfig {
        store,
        engine: Box::new(script.host()),
        capture: CaptureSource::Recorded(recording(0.5)),
        ring_capacity: crate::ring::Ring::DEFAULT_CAPACITY,
        idle: IdlePolicy {
            unload_after: std::time::Duration::ZERO,
            exit_after: std::time::Duration::ZERO,
        },
        supervisor_launch_id: None,
    }
}

/// The engine id no fixture model is for, so `LoadedModel::open`'s check has
/// something to refuse.
pub fn other_engine() -> EngineId {
    EngineId::parse("not-scripted").unwrap()
}
