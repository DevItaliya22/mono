// SPDX-License-Identifier: MPL-2.0

//! **Whisper through whisper.cpp, driven by the same chunk-and-redecode policy
//! as Parakeet.** The product's Compact row currently supplies base q5_1.
//!
//! Compiled only with the `runtime` feature.
//!
//! ## Why the policy and not whisper.cpp's own streaming example
//!
//! whisper.cpp ships a `stream` example that samples the microphone every half
//! second and re-transcribes a rolling buffer. `H = 500 ms` is taken from it —
//! and then applied identically to Parakeet, because two offline candidates
//! measured under two different policies could not be compared to each other.
//! What the example adds beyond that, a sliding window with keep-context, is a
//! *second* policy with its own accuracy behaviour, and inventing one for a
//! candidate is how a gate ends up measuring the harness.
//!
//! ## Five things whisper.cpp does that this adapter has to work around
//!
//! **A decode is not incremental.** [`WhisperState::full`] takes the samples, so
//! every redecode is over the whole buffer — the quadratic cost the protocol
//! says to read RTF against utterance duration for. There is no partial-state
//! API to use instead.
//!
//! **The transcript is segments, not a string.** `full_n_segments` then
//! `get_segment(i)`, concatenated. The segments carry their own leading spaces,
//! so the join is a plain concatenation with one trim at the end; inserting
//! separators would put double spaces into a transcript that the benchmark's
//! normaliser would then collapse, hiding the mistake.
//!
//! **A short buffer is not decodable.** whisper's front end works on a 30 s mel
//! window and the first redecode fires at 500 ms of audio; rather than discover
//! per-build what the true minimum is, the buffer is padded with silence to
//! [`MIN_DECODE_SAMPLES`] before every call. Padding is cheaper than a decode
//! that fails, and a failure here would be counted as *no useful partial* and
//! land in the censored fraction — failing the candidate for a reason that was
//! ours.
//!
//! **`no_context` is on, deliberately.** Each redecode is the whole utterance
//! from the beginning, so carrying decoder state from the previous one would
//! prompt the model with a transcript of the audio it is about to be given
//! again. It is the setting that makes a redecode a redecode rather than a
//! continuation.
//!
//! **Language detection is not a separate pass.** In whisper.cpp, a language
//! value of `"auto"` detects the language and then transcribes. Setting
//! `detect_language` as well instead makes `whisper_full` return immediately
//! after detection, with no transcript. Keep the language set to `"auto"`, but
//! leave that detection-only flag off.
//!
//! One library behaviour worth knowing before reading a peak-RSS row:
//! `FullParams::set_language` does `CString::new(..).into_raw()` and never
//! reclaims it, so every redecode leaks the language string. It is three bytes
//! against tens of thousands of decodes — noise beside a 1.5 GiB model — but it
//! is a leak in a column the gate measures, and finding it in a profiler later
//! would cost an afternoon.

use std::collections::VecDeque;
use std::time::Duration;

use koru_voice::{
    EngineError, EngineEvent, EngineInfo, ErrorCode, ModelId, Requested, SAMPLE_RATE,
    SessionRequest, SpeechEngine, Transcript, VoiceLanguage,
};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::model::{GgmlModel, whisper_code};
use crate::redecode::Redecode;

/// The buffer is padded with silence to at least this many samples — one
/// second — before any decode. See the module note.
pub const MIN_DECODE_SAMPLES: usize = SAMPLE_RATE as usize;

/// How the recogniser is built.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WhisperConfig {
    pub model_dir: std::path::PathBuf,
    pub model: ModelId,
    /// A declared column in every table. No default.
    pub num_threads: i32,
    pub languages: Vec<VoiceLanguage>,
    /// The chunk-and-redecode interval, in audio time. Declared by the
    /// benchmark, never defaulted here — the same rule `ParakeetConfig`
    /// follows, so the two offline candidates take one number from one place.
    pub partial_every: Option<Duration>,
}

/// A live Whisper session.
///
/// The context and the state are both owned. `WhisperState` holds an `Arc` on
/// the inner context rather than borrowing it, so there is no self-referential
/// struct here and no lifetime to fight — worth stating, because the shape of
/// most C-library bindings would need one.
pub struct WhisperEngine {
    #[allow(dead_code)]
    context: WhisperContext,
    state: WhisperState,
    config: WhisperConfig,
    model_name: String,
    quantisation: &'static str,
    /// The code the current session asked for. `&'static str` because
    /// [`FullParams::set_language`] borrows it for the whole decode and every
    /// value it can take is a literal.
    language: &'static str,
    audio: Vec<f32>,
    redecode: Redecode,
    last: String,
    queued: VecDeque<EngineEvent>,
    open: bool,
    finished: bool,
}

impl WhisperEngine {
    pub fn open(config: WhisperConfig) -> Result<WhisperEngine, EngineError> {
        let weights = GgmlModel::resolve(&config.model_dir)?;

        let context =
            WhisperContext::new_with_params(&weights.path, WhisperContextParameters::default())
                .map_err(|e| {
                    ErrorCode::ModelLoadFailed.with(format!(
                        "whisper.cpp refused {}: {e}",
                        weights.path.display()
                    ))
                })?;

        let state = context.create_state().map_err(|e| {
            ErrorCode::ModelLoadFailed.with(format!("whisper.cpp would not open a state: {e}"))
        })?;

        let redecode = match config.partial_every {
            Some(interval) => Redecode::every(interval),
            None => Redecode::never(),
        };

        Ok(WhisperEngine {
            context,
            state,
            model_name: weights.name,
            quantisation: weights.quantisation.as_str(),
            config,
            language: "auto",
            audio: Vec::new(),
            redecode,
            last: String::new(),
            queued: VecDeque::new(),
            open: false,
            finished: false,
        })
    }

    /// The weight file's name — `large-v3-turbo-q5_0`. Printed into the run
    /// header so a row cannot claim a model the directory did not hold.
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub fn quantisation(&self) -> &'static str {
        self.quantisation
    }

    fn params(&self) -> FullParams<'static, 'static> {
        // Greedy with `best_of = 1`: the gate is measuring the model, and a
        // beam width is a knob that would need a column of its own to mean
        // anything. The same reason the sherpa adapters ask for greedy search.
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.config.num_threads);
        params.set_translate(false);
        params.set_language(Some(self.language));
        // `"auto"` already asks whisper.cpp to detect before transcribing.
        // `detect_language = true` is detection-only: whisper_full returns
        // before decoding any segments, yielding an empty final transcript.
        params.set_no_context(true);
        // Nothing may reach stdout: the harness prints its tables there, and a
        // progress line inside one is a report nobody can paste.
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params
    }

    /// Decode everything buffered so far, padded to the minimum.
    fn decode_all(&mut self) -> Result<String, EngineError> {
        let mut samples = self.audio.clone();
        if samples.len() < MIN_DECODE_SAMPLES {
            samples.resize(MIN_DECODE_SAMPLES, 0.0);
        }
        let params = self.params();
        self.state.full(params, &samples).map_err(|e| {
            ErrorCode::EngineFailed.with(format!("whisper.cpp failed to decode: {e}"))
        })?;

        let mut text = String::new();
        for index in 0..self.state.full_n_segments() {
            if let Some(segment) = self.state.get_segment(index)
                // Lossy rather than strict: a decoder that emitted one invalid
                // byte should cost a character in the error rate, not the whole
                // utterance. Refusing would put the row in the censored count
                // and fail the candidate for the wrong reason.
                && let Ok(part) = segment.to_str_lossy()
            {
                text.push_str(&part);
            }
        }
        Ok(text.trim().to_owned())
    }
}

impl SpeechEngine for WhisperEngine {
    fn describe(&self) -> EngineInfo {
        EngineInfo {
            engine: crate::engine_id(),
            version: format!("whisper.cpp / {} ({})", self.model_name, self.quantisation),
            model: Some(self.config.model.clone()),
            languages: self.config.languages.clone(),
            // Only because of the policy, and only when it is on.
            streaming: self.redecode.is_enabled(),
            // Whisper genuinely detects a language, which makes it the one
            // candidate in this gate that can answer `Requested::Auto`.
            detects_language: true,
        }
    }

    fn begin(&mut self, request: &SessionRequest) -> Result<(), EngineError> {
        if let Requested::Fixed(language) = request.language
            && !self.config.languages.contains(&language)
        {
            return Err(ErrorCode::LanguageUnsupported
                .with(format!("this model does not declare {}", language.tag())));
        }
        self.language = whisper_code(request.language);
        self.audio.clear();
        self.redecode.restart();
        self.last.clear();
        self.queued.clear();
        self.open = true;
        self.finished = false;
        Ok(())
    }

    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), EngineError> {
        if self.finished {
            return Err(ErrorCode::WrongState.with("audio arrived after finish"));
        }
        if !self.open {
            return Err(ErrorCode::WrongState.with("audio arrived before begin"));
        }
        self.audio.extend_from_slice(samples);
        if self.redecode.push(samples.len()) {
            let text = self.decode_all()?;
            if text != self.last {
                self.last = text.clone();
                self.queued
                    .push_back(EngineEvent::Partial(Transcript::new(text)));
            }
        }
        Ok(())
    }

    fn poll(&mut self) -> Option<EngineEvent> {
        self.queued.pop_front()
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        if self.finished {
            return Err(ErrorCode::WrongState.with("finish was called twice"));
        }
        if !self.open {
            return Err(ErrorCode::WrongState.with("finish arrived before begin"));
        }
        self.finished = true;
        let text = self.decode_all()?;
        self.last = text.clone();
        self.queued
            .push_back(EngineEvent::Final(Transcript::new(text)));
        Ok(())
    }

    fn cancel(&mut self) {
        self.audio.clear();
        self.redecode.restart();
        self.queued.clear();
        self.last.clear();
        self.open = false;
        self.finished = false;
    }
}
