// SPDX-License-Identifier: MPL-2.0

//! **How this worker gets a [`SpeechEngine`] from either product runtime.**
//!
//! `koru_voice::SpeechEngine` is *a recogniser from the outside*; it says
//! nothing about how one is constructed, because construction needs a runtime,
//! a model directory and whatever else a native library wants. [`EngineHost`]
//! is that missing half. The shipped host contains sherpa and whisper.cpp and
//! dispatches by the signed model descriptor's engine id; the worker itself
//! remains unaware of native-runtime details.
//!
//! ## Why [`ScriptedEngine`] is not behind `cfg(test)`
//!
//! Because Plan 1.4's client suite and the M0 benchmark harness both need an
//! engine that answers predictably without a model, and neither is compiled
//! with this crate's `cfg(test)` — the `no-reader` lesson, one crate out. It is
//! forty lines, it links nothing, and it is what
//! `a_recorded_session_reaches_a_final_transcript` drives.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use koru_voice::{
    EngineError, EngineEvent, EngineId, EngineInfo, ErrorCode, ModelId, SessionRequest,
    SpeechEngine, Transcript, VoiceLanguage,
};
use koru_voice_models::LoadedModel;

pub mod sherpa;
pub mod whisper;

/// **The host every product worker runs on.**
///
/// `WorkerConfig::product` calls this and stores the box; nothing else in the
/// binary asks what is inside it.
pub fn product_host() -> Box<dyn EngineHost> {
    Box::new(ProductEngineHost::new())
}

/// **What builds a [`SpeechEngine`] over a leased model.**
///
/// Both methods run on the inference thread. [`EngineHost::open`] is allowed to
/// take a second: it is a native library loading several hundred megabytes, and
/// it is on that thread precisely so nothing answering a person waits for it.
pub trait EngineHost: Send {
    /// What this host would build, before anything is loaded.
    ///
    /// Answered with `model: None` — [`EngineInfo::model`] is what a *loaded*
    /// engine reports, and a host that guessed would put a model in
    /// `koru-voice-worker status` that no memory holds.
    fn describe(&self) -> EngineInfo;

    /// Every engine this host can dispatch to. A test host normally has one;
    /// the product host reports both compiled runtimes.
    fn descriptions(&self) -> Vec<EngineInfo> {
        vec![self.describe()]
    }

    fn supports(&self, engine: &EngineId) -> bool {
        self.descriptions()
            .iter()
            .any(|description| &description.engine == engine)
    }

    /// Build an engine over these bytes.
    ///
    /// The lease inside `model` is held by the caller for as long as the engine
    /// lives, so nothing on another channel can prune the files out from under
    /// a running recogniser.
    fn open(&self, model: &LoadedModel) -> Result<Box<dyn SpeechEngine>, EngineError>;
}

impl dyn EngineHost {
    /// The engine id this host builds for, which is also what
    /// `LoadedModel::open` checks a descriptor against — a signed `engine`
    /// field exists so verified bytes cannot reach the wrong native parser, and
    /// a check nobody performs is a field nobody signed.
    pub fn engine_id(&self) -> EngineId {
        self.describe().engine
    }
}

/// The shipped worker hosts both runtimes and dispatches only by the signed
/// model descriptor's engine id.
pub struct ProductEngineHost {
    sherpa: sherpa::SherpaEngineHost,
    whisper: whisper::WhisperEngineHost,
}

impl ProductEngineHost {
    pub fn new() -> ProductEngineHost {
        ProductEngineHost {
            sherpa: sherpa::SherpaEngineHost::new(),
            whisper: whisper::WhisperEngineHost::new(),
        }
    }
}

impl Default for ProductEngineHost {
    fn default() -> ProductEngineHost {
        ProductEngineHost::new()
    }
}

impl EngineHost for ProductEngineHost {
    fn describe(&self) -> EngineInfo {
        self.sherpa.describe()
    }

    fn descriptions(&self) -> Vec<EngineInfo> {
        vec![self.sherpa.describe(), self.whisper.describe()]
    }

    fn open(&self, model: &LoadedModel) -> Result<Box<dyn SpeechEngine>, EngineError> {
        let engine = &model.descriptor().engine;
        if self.sherpa.supports(engine) {
            return self.sherpa.open(model);
        }
        if self.whisper.supports(engine) {
            return self.whisper.open(model);
        }
        Err(ErrorCode::EngineFailed.with(format!(
            "no compiled speech runtime accepts engine {engine}"
        )))
    }
}

/// **A fake recogniser that answers from a script.**
///
/// It ignores the audio entirely and hands back whatever it was told to, which
/// is exactly what a test of the *worker* wants: the thing under test is which
/// effects are performed in what order, and a real recogniser would make that
/// depend on a model, a machine and a person's voice.
///
/// The script is shared through an `Arc<Mutex<..>>` so a test can push a
/// `Final` from outside — which is how "a `Final` that arrives after the
/// session was cancelled" is arranged, since that race cannot be scheduled from
/// inside the engine.
#[derive(Clone, Debug, Default)]
pub struct Script {
    queued: Arc<Mutex<VecDeque<EngineEvent>>>,
    /// What `begin`, `accept_audio` and `finish` answer. `None` is success.
    begin_fails: Arc<Mutex<Option<ErrorCode>>>,
    audio_fails: Arc<Mutex<Option<ErrorCode>>>,
    finish_fails: Arc<Mutex<Option<ErrorCode>>>,
    /// What `finish` queues when it does not fail.
    final_text: Arc<Mutex<Option<Transcript>>>,
    /// Samples the engine was handed. A count, and the samples themselves, so a
    /// test can assert that audio really reached it.
    accepted: Arc<Mutex<Vec<f32>>>,
    cancelled: Arc<Mutex<bool>>,
    /// How many engines this script has built. **The evidence that a model was
    /// loaded once**, which is what `Effect::Release` not meaning *unload* is
    /// for: a person who dictates twice in ten seconds must not pay a
    /// several-hundred-megabyte load twice.
    opens: Arc<std::sync::atomic::AtomicUsize>,
}

impl Script {
    pub fn new() -> Script {
        Script::default()
    }

    /// Answer this text when the session is finalised.
    pub fn finishing_with(self, text: &str) -> Script {
        *self.final_text.lock().unwrap() = Some(Transcript::new(text));
        self
    }

    /// Queue an event for the next `poll`.
    pub fn queue(&self, event: EngineEvent) {
        self.queued.lock().unwrap().push_back(event);
    }

    pub fn fail_begin(&self, code: ErrorCode) {
        *self.begin_fails.lock().unwrap() = Some(code);
    }

    pub fn fail_audio(&self, code: ErrorCode) {
        *self.audio_fails.lock().unwrap() = Some(code);
    }

    /// Stop failing, so a suite can show the worker recovering rather than
    /// merely surviving.
    pub fn allow_audio(&self) {
        *self.audio_fails.lock().unwrap() = None;
    }

    /// How many times an engine was built over a model.
    pub fn opens(&self) -> usize {
        self.opens.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn fail_finish(&self, code: ErrorCode) {
        *self.finish_fails.lock().unwrap() = Some(code);
    }

    /// Every sample the engine was fed.
    pub fn accepted(&self) -> Vec<f32> {
        self.accepted.lock().unwrap().clone()
    }

    pub fn was_cancelled(&self) -> bool {
        *self.cancelled.lock().unwrap()
    }

    /// A host that builds engines against this script.
    pub fn host(&self) -> ScriptedEngineHost {
        ScriptedEngineHost {
            script: self.clone(),
        }
    }
}

/// A [`Script`] as an [`EngineHost`].
#[derive(Clone, Debug)]
pub struct ScriptedEngineHost {
    script: Script,
}

impl ScriptedEngineHost {
    /// The engine id a scripted host claims, and therefore the one a fixture
    /// descriptor has to name for `LoadedModel::open` to accept it.
    pub fn engine() -> EngineId {
        EngineId::parse("scripted").expect("`scripted` is a legal engine id")
    }

    fn info(model: Option<ModelId>) -> EngineInfo {
        EngineInfo {
            engine: ScriptedEngineHost::engine(),
            version: "scripted".into(),
            model,
            languages: VoiceLanguage::ALL.to_vec(),
            streaming: true,
            detects_language: true,
        }
    }
}

impl EngineHost for ScriptedEngineHost {
    fn describe(&self) -> EngineInfo {
        ScriptedEngineHost::info(None)
    }

    fn open(&self, model: &LoadedModel) -> Result<Box<dyn SpeechEngine>, EngineError> {
        self.script
            .opens
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Box::new(ScriptedEngine {
            script: self.script.clone(),
            model: model.installed().id().clone(),
        }))
    }
}

/// The engine a [`Script`] builds. See [`Script`].
#[derive(Debug)]
pub struct ScriptedEngine {
    script: Script,
    model: ModelId,
}

impl SpeechEngine for ScriptedEngine {
    fn describe(&self) -> EngineInfo {
        ScriptedEngineHost::info(Some(self.model.clone()))
    }

    fn begin(&mut self, _request: &SessionRequest) -> Result<(), EngineError> {
        match *self.script.begin_fails.lock().unwrap() {
            Some(code) => Err(code.with("the script refused begin")),
            None => Ok(()),
        }
    }

    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), EngineError> {
        if let Some(code) = *self.script.audio_fails.lock().unwrap() {
            return Err(code.with("the script refused audio"));
        }
        self.script
            .accepted
            .lock()
            .unwrap()
            .extend_from_slice(samples);
        Ok(())
    }

    fn poll(&mut self) -> Option<EngineEvent> {
        self.script.queued.lock().unwrap().pop_front()
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        if let Some(code) = *self.script.finish_fails.lock().unwrap() {
            return Err(code.with("the script refused finish"));
        }
        // Queued rather than returned, because that is the contract:
        // `Ok` means finalisation has *started*, and the result arrives through
        // `poll` — possibly several polls later, since a whole-utterance
        // decoder does real work here.
        if let Some(text) = self.script.final_text.lock().unwrap().clone() {
            self.script.queue(EngineEvent::Final(text));
        }
        Ok(())
    }

    fn cancel(&mut self) {
        *self.script.cancelled.lock().unwrap() = true;
        self.script.queued.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests;
