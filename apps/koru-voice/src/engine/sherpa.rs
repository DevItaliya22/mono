// SPDX-License-Identifier: MPL-2.0

//! **The sherpa adapter as one product [`EngineHost`].**
//!
//! Every product worker compiles this and the whisper host. Everything above
//! this file still knows only [`EngineHost`]: it asks for an engine over a
//! leased model and is handed a `Box<dyn SpeechEngine>`.
//!
//! ## Which sherpa API reads a directory cannot be read off the directory
//!
//! `koru-voice-sherpa` has two adapters — a streaming `OnlineRecognizer` for
//! Nemotron and an offline `OfflineRecognizer` for Parakeet — and
//! `layout::resolve` finds **the same four files** for both, because both
//! published packages are transducers carrying `encoder`, `decoder`, `joiner`
//! and `tokens.txt`. So the files cannot say which API to hand them to, and the
//! signed descriptor has no `streaming` field to say it either: `engine` is
//! `sherpa` for all three published rows.
//!
//! What is left is the **model id**, which is inside the signature envelope and
//! is therefore not a guess about untrusted text. This host runs the
//! `nemotron-` family and **refuses everything else by name**, which is the
//! honest state of Plan 0.3: the offline adapter needs a chunk-and-redecode
//! interval, that interval is the benchmark's policy with a quadratic cost
//! attached, and Plan 0.4 has not chosen one. A host that silently loaded
//! Parakeet with no partial policy would be a dictation that shows nothing
//! until the key is released, which is a product decision taken by accident.
//! Recorded as Q-V88.
//!
//! ## Two things this file adds that the adapter deliberately does not
//!
//! **A thread count.** `NemotronConfig::num_threads` has no default on purpose
//! — "RTF without a thread count is not a number" — but a *product* host has to
//! pass something, so [`DEFAULT_THREADS`] is a named placeholder rather than a
//! measured choice. Plan 0.4's table is where it acquires a number.
//!
//! **A catch for a native panic.** The adapter answers `Result` for everything
//! it can see; what it cannot see is the runtime unwinding inside
//! `OnlineRecognizer::create` on bytes it dislikes. That unwind would kill the
//! *inference thread*, and a dead inference thread is a worker that stops
//! answering rather than one that says why — the silent refusal the brief for
//! this seam rules out. So the load is wrapped and a panic becomes
//! [`ErrorCode::ModelLoadFailed`] with the directory named, which is the same
//! shape of answer a missing `tokens.txt` gets.

use std::panic::AssertUnwindSafe;

use koru_voice::{EngineError, EngineInfo, ErrorCode, ModelId, SpeechEngine};
use koru_voice_models::LoadedModel;
use koru_voice_sherpa::nemotron::{NemotronConfig, NemotronEngine};

use crate::engine::EngineHost;

/// How many threads a product recogniser decodes on.
///
/// **A placeholder with a name, not a measurement.** Four is what sherpa's own
/// examples use for a 0.6B transducer on a desktop; the honest statement is
/// that nothing in this tree has measured it yet, and
/// `docs/features/voice/plans/ENGINE_BENCHMARK.md` prints a thread count in every row
/// precisely because a latency figure without one means nothing. Overridden
/// per host with [`SherpaEngineHost::with_threads`].
pub const DEFAULT_THREADS: i32 = 4;

/// The model ids this host will load, by prefix. See the module note: the
/// streaming/offline split is not readable from the files.
const STREAMING_PREFIX: &str = "nemotron-";

/// **Builds a streaming sherpa recogniser over a leased model.**
#[derive(Clone, Debug)]
pub struct SherpaEngineHost {
    num_threads: i32,
}

impl Default for SherpaEngineHost {
    fn default() -> SherpaEngineHost {
        SherpaEngineHost::new()
    }
}

impl SherpaEngineHost {
    pub fn new() -> SherpaEngineHost {
        SherpaEngineHost {
            num_threads: DEFAULT_THREADS,
        }
    }

    /// Decode on `num_threads`. Clamped to at least one, because a zero would
    /// reach a C API as a thread pool of no threads.
    pub fn with_threads(num_threads: i32) -> SherpaEngineHost {
        SherpaEngineHost {
            num_threads: num_threads.max(1),
        }
    }

    /// Whether this host knows which sherpa API reads a model, and refuses by
    /// name when it does not.
    ///
    /// Separate from [`SherpaEngineHost::open`] so it can be asserted without a
    /// gigabyte of weights on the machine running the assertion.
    pub fn accepts(model: &ModelId) -> Result<(), EngineError> {
        if model.as_str().starts_with(STREAMING_PREFIX) {
            return Ok(());
        }
        Err(ErrorCode::ModelLoadFailed.with(format!(
            "{model} is a sherpa model this build does not know how to run: this host runs the \
             streaming '{STREAMING_PREFIX}' family, and the offline packages need a \
             chunk-and-redecode interval that Plan 0.4 has not chosen"
        )))
    }
}

impl EngineHost for SherpaEngineHost {
    /// What this host would build, before anything is loaded.
    ///
    /// `model: None` and `languages: []` for the same reason, and it is the
    /// reason [`EngineInfo::model`] is documented as `None` here: languages are
    /// a property of the *package*, declared in its signed descriptor, and a
    /// host that answered `VoiceLanguage::ALL` would put a promise in
    /// `koru-voice-worker status` that no bytes on this machine back. A loaded
    /// `NemotronEngine` answers both truthfully.
    ///
    /// `detects_language` is `false` for the third version of the same rule:
    /// whether a package carries the prompt dictionary is probed on a real
    /// stream at load, and only the loaded engine has one.
    fn describe(&self) -> EngineInfo {
        EngineInfo {
            engine: koru_voice_sherpa::engine_id(),
            version: format!(
                "sherpa-onnx {} (no model loaded)",
                koru_voice_sherpa::SHERPA_ONNX_VERSION
            ),
            model: None,
            languages: Vec::new(),
            streaming: true,
            detects_language: false,
        }
    }

    fn open(&self, model: &LoadedModel) -> Result<Box<dyn SpeechEngine>, EngineError> {
        let id = model.installed().id().clone();
        SherpaEngineHost::accepts(&id)?;
        let model_dir = model.root().to_path_buf();
        let config = NemotronConfig {
            model_dir: model_dir.clone(),
            model: id,
            num_threads: self.num_threads,
            // **The signed list, not one this file invented.** `begin` refuses
            // a language the model does not declare, so the set that decides
            // that refusal has to be the one somebody signed.
            languages: model.descriptor().languages.clone(),
        };
        // See the module note: an unwind out of the native runtime would take
        // the inference thread with it.
        let built = std::panic::catch_unwind(AssertUnwindSafe(|| NemotronEngine::open(config)));
        match built {
            Ok(Ok(engine)) => Ok(Box::new(engine)),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(ErrorCode::ModelLoadFailed.with(format!(
                "the sherpa-onnx runtime panicked while loading the model in {}",
                model_dir.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests;
