// SPDX-License-Identifier: MPL-2.0

//! **Parakeet TDT 0.6B v3 through sherpa's `OfflineRecognizer`, made to look
//! streaming by a policy that is ours.**
//!
//! Compiled only with the `runtime` feature.
//!
//! ## Read this before drawing a conclusion from a Parakeet latency row
//!
//! An offline recogniser has no partials. This adapter produces them by keeping
//! every sample of the session and re-decoding the whole buffer every `H` of
//! audio — the policy `docs/features/voice/plans/ENGINE_BENCHMARK.md` declares for
//! candidates 3 and 4, with `H` handed in rather than defaulted here so both
//! offline candidates take the same number from the same place.
//!
//! Two facts the report has to carry, and they are the reason this file is
//! documented rather than obvious:
//!
//! - **Cost grows quadratically with utterance length.** Each redecode is over
//!   everything so far, so an RTF for this candidate has to be read against
//!   utterance duration and never pooled across corpora whose mean lengths
//!   differ.
//! - **A poor number here is evidence about the policy**, not about the model.
//!   `ENGINE_DECISION.md` must say so wherever it draws a conclusion from one of
//!   these rows.
//!
//! ## The buffer is a `Vec<f32>` and that is a considered choice
//!
//! It grows to hold the whole utterance — 20 s at 16 kHz is 1.28 MB — and the
//! session contract is explicit that `accept_audio` runs on the inference
//! thread and may allocate. The real-time obligation belongs to the ring-buffer
//! write, which is somewhere else entirely.

use std::collections::VecDeque;
use std::time::Duration;

use koru_voice::{
    EngineError, EngineEvent, EngineInfo, ErrorCode, ModelId, SAMPLE_RATE, SessionRequest,
    SpeechEngine, Transcript, VoiceLanguage,
};
use sherpa_onnx::{
    OfflineModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig,
};

use crate::layout::{OfflineLayout, as_c_string};
use crate::redecode::Redecode;

/// How the recogniser is built.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParakeetConfig {
    pub model_dir: std::path::PathBuf,
    pub model: ModelId,
    /// A declared column in every table. No default.
    pub num_threads: i32,
    pub languages: Vec<VoiceLanguage>,
    /// The chunk-and-redecode interval, in **audio** time. `None` means emit no
    /// partials at all, which is the honest configuration for measuring this
    /// model without the policy.
    ///
    /// The gate's value is declared in the benchmark's candidate table, not
    /// here: the policy is the benchmark's, and a default in this file would be
    /// a second answer to the same question.
    pub partial_every: Option<Duration>,
}

/// A live Parakeet session.
pub struct ParakeetEngine {
    recognizer: OfflineRecognizer,
    config: ParakeetConfig,
    precision: &'static str,
    /// Every sample since `begin`. See the module note.
    audio: Vec<f32>,
    redecode: Redecode,
    last: String,
    queued: VecDeque<EngineEvent>,
    open: bool,
    finished: bool,
}

impl ParakeetEngine {
    pub fn open(config: ParakeetConfig) -> Result<ParakeetEngine, EngineError> {
        let layout = OfflineLayout::resolve(&config.model_dir)?;
        let precision = layout.precision.as_str();

        let mut sherpa = OfflineRecognizerConfig {
            model_config: OfflineModelConfig {
                transducer: OfflineTransducerModelConfig {
                    encoder: Some(as_c_string(&layout.encoder)?),
                    decoder: Some(as_c_string(&layout.decoder)?),
                    joiner: Some(as_c_string(&layout.joiner)?),
                },
                tokens: Some(as_c_string(&layout.tokens)?),
                num_threads: config.num_threads,
                provider: Some("cpu".into()),
                // **Required, and not inferable.** sherpa's own worked example
                // for this package sets `model_type = "nemo_transducer"`;
                // without it the loader guesses from the graph and a TDT model
                // is not what it guesses.
                model_type: Some("nemo_transducer".into()),
                ..Default::default()
            },
            decoding_method: Some("greedy_search".into()),
            ..Default::default()
        };
        sherpa.feat_config.sample_rate = SAMPLE_RATE as i32;

        let recognizer = OfflineRecognizer::create(&sherpa).ok_or_else(|| {
            ErrorCode::ModelLoadFailed.with(format!(
                "sherpa-onnx refused the {precision} offline model in {}",
                config.model_dir.display()
            ))
        })?;

        let redecode = match config.partial_every {
            Some(interval) => Redecode::every(interval),
            None => Redecode::never(),
        };

        Ok(ParakeetEngine {
            recognizer,
            config,
            precision,
            audio: Vec::new(),
            redecode,
            last: String::new(),
            queued: VecDeque::new(),
            open: false,
            finished: false,
        })
    }

    pub fn precision(&self) -> &'static str {
        self.precision
    }

    /// Decode everything buffered so far. The whole cost of the policy is this
    /// one call, and it is charged again on every interval.
    fn decode_all(&self) -> String {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, &self.audio);
        self.recognizer.decode(&stream);
        stream.get_result().map(|r| r.text).unwrap_or_default()
    }
}

impl SpeechEngine for ParakeetEngine {
    fn describe(&self) -> EngineInfo {
        EngineInfo {
            engine: crate::engine_id(),
            version: format!(
                "sherpa-onnx {} ({}, offline)",
                crate::SHERPA_ONNX_VERSION,
                self.precision
            ),
            model: Some(self.config.model.clone()),
            languages: self.config.languages.clone(),
            // **True only because of the policy**, and only when it is on. An
            // offline recogniser answers `false` unless a declared
            // chunk-and-redecode policy makes it true, which is exactly what
            // `EngineInfo::streaming`'s own doc says.
            streaming: self.redecode.is_enabled(),
            // Parakeet TDT v3 carries no prompt dictionary. It is multilingual
            // and it is not askable, which is a different thing from being
            // monolingual and is why this is `false` rather than absent.
            detects_language: false,
        }
    }

    fn begin(&mut self, request: &SessionRequest) -> Result<(), EngineError> {
        match request.language {
            koru_voice::Requested::Auto => {
                return Err(ErrorCode::LanguageUnsupported
                    .with("this model cannot be asked to detect a language"));
            }
            koru_voice::Requested::Fixed(language)
                if !self.config.languages.contains(&language) =>
            {
                return Err(ErrorCode::LanguageUnsupported
                    .with(format!("this model does not declare {}", language.tag())));
            }
            _ => {}
        }
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
            let text = self.decode_all();
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
        let text = self.decode_all();
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
