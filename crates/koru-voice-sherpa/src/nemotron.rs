// SPDX-License-Identifier: MPL-2.0

//! **Nemotron 3.5 ASR Streaming through sherpa's `OnlineRecognizer`.**
//!
//! The one genuinely streaming candidate in the gate: audio goes in, the
//! hypothesis is revised as it goes, and partials are the recogniser's own
//! rather than a policy of ours.
//!
//! Compiled only with the `runtime` feature. See the crate note for why that is
//! off by default.
//!
//! ## Four things this adapter has to do that the API does not do for it
//!
//! **Decode is pulled, not pushed.** `accept_waveform` only buffers.
//! `is_ready` says whether a chunk's worth has accumulated and `decode` consumes
//! exactly one chunk, so the adapter loops until `is_ready` is false — otherwise
//! audio piles up and the transcript falls further behind for as long as
//! somebody talks, which is an RTF failure that looks like a latency failure.
//!
//! **A `Partial` is only emitted when the text changed.** `get_result` answers
//! the current hypothesis every time it is asked, and re-emitting an unchanged
//! string would make `partials` count polls rather than revisions and would put
//! a `Partial` in front of every `TTFUP` measurement for free.
//!
//! **Endpointing is off.** `enable_endpoint` defaults to `false` and stays
//! there. In this product finalisation is *requested* — a person holding a key
//! has not finished speaking because a recogniser found a pause — and
//! `koru_voice::EngineEvent::Final` is documented as the result of `finish` and
//! only of that. An endpoint arriving as a segment boundary would be a `Final`
//! nobody asked for.
//!
//! **`finish` does not block.** `input_finished` tells the stream no more audio
//! is coming; the remaining chunks still have to be decoded, and the contract
//! says `Ok` means finalisation has *started*. So the last decodes happen here
//! and the `Final` is queued for the next `poll`, which is what a caller that
//! is timing `t_final − t_last_push` is measuring.
//!
//! ## `input_finished` alone loses the last word, and that is not a bug here
//!
//! **A cache-aware encoder decodes whole chunks, and `input_finished` pads the
//! *feature extractor* rather than the waveform.** Whatever audio is left over
//! after the last whole chunk never becomes a chunk, so it is never decoded and
//! the words in it are simply absent. Measured against the publisher's own
//! `de.wav` through this adapter: `Alles hat ein Ende, nur die Wurst hat` — the
//! final `zwei` gone, with no error anywhere and a `Final` that looks like a
//! complete sentence. It is the worst shape of defect this file could have: the
//! transcript is plausible.
//!
//! So [`TAIL_PADDING`] of silence is accepted **before** `input_finished`,
//! which is what sherpa-onnx's own `online-decode-files` example does and for
//! this reason. The same probe recovered `zwei` at 200 ms of padding on the
//! 160 ms export; the number here is upstream's, because it also has to cover
//! the 560 ms export, and a value tuned to the one directory somebody happened
//! to have provisioned is a number that is right once.

use std::collections::VecDeque;
use std::path::Path;

use koru_voice::{
    EngineError, EngineEvent, EngineInfo, ErrorCode, ModelId, SAMPLE_RATE, SessionRequest,
    SpeechEngine, Transcript, VoiceLanguage,
};
use sherpa_onnx::{
    OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
    OnlineTransducerModelConfig,
};

use crate::language::{self, OPTION_KEY};
use crate::layout::{OnlineLayout, as_c_string};

/// **Silence pushed into the stream before `input_finished`.**
///
/// 0.66 s, which is `sherpa-onnx`'s own figure in
/// `cxx-api-examples`/`online-decode-files` — taken rather than tuned because
/// it has to cover every export this adapter reads, and the largest published
/// Nemotron chunk (560 ms) is most of it. See the module note: without it the
/// last partial chunk is never decoded and the final word is silently missing.
///
/// It costs a fixed amount of decode at finalisation, which is a real entry in
/// `t_final − t_last_push` and belongs in the benchmark's notes rather than in
/// a surprise.
pub const TAIL_PADDING: std::time::Duration = std::time::Duration::from_millis(660);

/// How the recogniser is built. Everything a run of the gate declares in a
/// column of its own table, so a row and an engine cannot disagree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NemotronConfig {
    /// Which model directory. One per chunk size: see
    /// [`crate::artifacts::CHUNK_IS_AN_EXPORT`].
    pub model_dir: std::path::PathBuf,
    /// The id this engine reports having loaded.
    pub model: ModelId,
    /// `num_threads`, which is a declared column in every table the protocol
    /// prints. **No default**, because "RTF without a thread count is not a
    /// number".
    pub num_threads: i32,
    /// Which languages this package claims. Reported through
    /// [`EngineInfo::languages`] and checked in [`SpeechEngine::begin`]; the
    /// prompt dictionary is inside the ONNX metadata and sherpa exposes no way
    /// to enumerate it, so this is declared by whoever provisioned the
    /// directory rather than read off it.
    pub languages: Vec<VoiceLanguage>,
}

/// A live Nemotron session.
pub struct NemotronEngine {
    recognizer: OnlineRecognizer,
    stream: Option<OnlineStream>,
    config: NemotronConfig,
    precision: &'static str,
    /// Whether this runtime can force a per-stream `language`. Probed once, on
    /// a throwaway stream, rather than assumed — see [`crate::language`].
    ///
    /// This is deliberately **not** whether the model detects a language. The
    /// multilingual Nemotron export uses its auto-detect prompt when no option
    /// is supplied; the option only overrides that default. Conflating those
    /// two facts made the product's default `Requested::Auto` refuse a model
    /// that was already capable of answering it.
    can_force_language: bool,
    /// The last text handed out, so an unchanged hypothesis is not re-emitted.
    last: String,
    queued: VecDeque<EngineEvent>,
    finished: bool,
}

impl NemotronEngine {
    /// Build a recogniser over a model directory.
    ///
    /// Resolution happens first and separately, so a missing file is named
    /// rather than arriving as `OnlineRecognizer::create`'s undifferentiated
    /// `None`.
    pub fn open(config: NemotronConfig) -> Result<NemotronEngine, EngineError> {
        let layout = OnlineLayout::resolve(&config.model_dir)?;
        let precision = layout.precision.as_str();

        let mut sherpa = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: Some(as_c_string(&layout.encoder)?),
                    decoder: Some(as_c_string(&layout.decoder)?),
                    joiner: Some(as_c_string(&layout.joiner)?),
                },
                tokens: Some(as_c_string(&layout.tokens)?),
                num_threads: config.num_threads,
                provider: Some("cpu".into()),
                ..Default::default()
            },
            // Greedy, because the gate is measuring the model rather than a
            // beam width we chose, and a beam is a knob that would need its own
            // column to mean anything.
            decoding_method: Some("greedy_search".into()),
            // See the module note: finalisation is requested, never offered.
            enable_endpoint: false,
            ..Default::default()
        };
        sherpa.feat_config.sample_rate = SAMPLE_RATE as i32;

        let recognizer = OnlineRecognizer::create(&sherpa).ok_or_else(|| {
            ErrorCode::ModelLoadFailed.with(format!(
                "sherpa-onnx refused the {precision} streaming model in {}",
                config.model_dir.display()
            ))
        })?;

        // The probe. A stream is cheap and this is the only way to ask whether
        // this runtime exposes the optional *override*. Auto-detection itself
        // is the model's default when no override is supplied.
        let can_force_language = recognizer.create_stream().has_option(OPTION_KEY);

        Ok(NemotronEngine {
            recognizer,
            stream: None,
            config,
            precision,
            can_force_language,
            last: String::new(),
            queued: VecDeque::new(),
            finished: false,
        })
    }

    /// The precision the directory turned out to hold — `int8` or `fp32`.
    /// Printed into the run header, because a row labelled `int8` over an
    /// `fp32` directory is a wrong number rather than a missing one.
    pub fn precision(&self) -> &'static str {
        self.precision
    }

    /// Where the model was loaded from.
    pub fn model_dir(&self) -> &Path {
        &self.config.model_dir
    }

    /// Decode every chunk that has accumulated, and queue a `Partial` if the
    /// hypothesis moved.
    fn drain(&mut self) {
        let Some(stream) = self.stream.as_ref() else {
            return;
        };
        while self.recognizer.is_ready(stream) {
            self.recognizer.decode(stream);
        }
        let Some(result) = self.recognizer.get_result(stream) else {
            return;
        };
        if result.text != self.last {
            self.last = result.text.clone();
            self.queued
                .push_back(EngineEvent::Partial(Transcript::new(result.text)));
        }
    }
}

impl SpeechEngine for NemotronEngine {
    fn describe(&self) -> EngineInfo {
        EngineInfo {
            engine: crate::engine_id(),
            version: format!(
                "sherpa-onnx {} ({})",
                crate::SHERPA_ONNX_VERSION,
                self.precision
            ),
            model: Some(self.config.model.clone()),
            languages: self.config.languages.clone(),
            streaming: true,
            // The multilingual export's default prompt is auto-detection.
            // `can_force_language` only says whether a caller can replace it.
            detects_language: true,
        }
    }

    fn begin(&mut self, request: &SessionRequest) -> Result<(), EngineError> {
        if let koru_voice::Requested::Fixed(language) = request.language
            && !self.config.languages.contains(&language)
        {
            return Err(ErrorCode::LanguageUnsupported
                .with(format!("this model does not declare {}", language.tag())));
        }

        let stream = self.recognizer.create_stream();
        if self.can_force_language {
            stream.set_option(OPTION_KEY, language::option_value(request.language));
        }
        self.stream = Some(stream);
        self.last.clear();
        self.queued.clear();
        self.finished = false;
        Ok(())
    }

    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), EngineError> {
        if self.finished {
            return Err(ErrorCode::WrongState.with("audio arrived after finish"));
        }
        let Some(stream) = self.stream.as_ref() else {
            return Err(ErrorCode::WrongState.with("audio arrived before begin"));
        };
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        self.drain();
        Ok(())
    }

    fn poll(&mut self) -> Option<EngineEvent> {
        self.queued.pop_front()
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        if self.finished {
            return Err(ErrorCode::WrongState.with("finish was called twice"));
        }
        let Some(stream) = self.stream.as_ref() else {
            return Err(ErrorCode::WrongState.with("finish arrived before begin"));
        };
        // See the module note and [`TAIL_PADDING`]: without this the audio
        // after the last whole chunk is never decoded, and the transcript is
        // missing its final word while looking entirely complete.
        let padding =
            vec![0.0f32; (SAMPLE_RATE as u128 * TAIL_PADDING.as_millis() / 1000) as usize];
        stream.accept_waveform(SAMPLE_RATE as i32, &padding);
        stream.input_finished();
        self.finished = true;
        // Drain what is left, then answer with whatever the last hypothesis is
        // — including the empty string, which is a person who said nothing and
        // is the caller's to interpret.
        self.drain();
        // `drain` may have queued the same text as a `Partial`; the `Final` is
        // still emitted, because a caller measuring `t_final` is timing the
        // event rather than the string.
        let text = self.last.clone();
        self.queued
            .push_back(EngineEvent::Final(Transcript::new(text)));
        Ok(())
    }

    fn cancel(&mut self) {
        self.stream = None;
        self.queued.clear();
        self.last.clear();
        self.finished = false;
    }
}
