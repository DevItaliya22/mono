// SPDX-License-Identifier: MPL-2.0

//! **The four candidates, their chunking policies, and what each is when its
//! runtime is not compiled in.**
//!
//! The table is the protocol's, transcribed, and it is a table rather than four
//! `if`s so a fifth candidate is a row.
//!
//! | # | Candidate | Runtime | API | Chunk policy | Is chunk a runtime knob? |
//! | --- | --- | --- | --- | --- | --- |
//! | 1 | Nemotron 3.5 ASR Streaming 0.6B, int8, **160 ms** | sherpa-onnx | `OnlineRecognizer` | cache-aware | **No** |
//! | 2 | Nemotron 3.5 ASR Streaming 0.6B, int8, **560 ms** | sherpa-onnx | `OnlineRecognizer` | cache-aware | **No** |
//! | 3 | Parakeet TDT 0.6B v3, int8 | sherpa-onnx | `OfflineRecognizer` | ours, `H` = 500 ms | Yes |
//! | 4 | Whisper `large-v3-turbo`, q5_0 | whisper.cpp | `whisper-rs` | ours, `H` = 500 ms | Yes |
//!
//! ## `H` is declared here and nowhere else
//!
//! [`REDECODE_INTERVAL`] is the single declaration of the chunk-and-redecode
//! interval. The adapters take it as a parameter and default to nothing,
//! deliberately: **the policy is the benchmark's, not the candidate's**, and a
//! poor latency number for Parakeet or Whisper is evidence that *this policy* is
//! poor rather than that the model is. `ENGINE_DECISION.md` must say so where it
//! draws a conclusion from those rows.
//!
//! ## Candidates 1 and 2 are two downloads
//!
//! **The chunk size is baked into the ONNX export, not set at run time.** The
//! published packages are produced by one exporter that fixes `att_context_size`
//! before writing the graph, and sherpa publishes five archives — 80, 160, 320,
//! 560 and 1120 ms — all the same size. So candidates 1 and 2 are two
//! provisioned model directories, and [`Candidate::model_dir_name`] is what the
//! provisioning step places. **This is the single most likely thing to get
//! wrong, and it would show up as a silent 560 ms result in a row labelled
//! 160 ms** — which is why the directory name carries the chunk size and the run
//! header prints the directory it actually opened.
//!
//! ## Qwen3-ASR is deferred, not rejected
//!
//! Its streaming path through sherpa performs whole-utterance recognition with
//! end-of-utterance latency growing with length, which is the shape candidates 3
//! and 4 already probe; building an adapter to confirm it costs a third
//! inference runtime in the workspace. If the gate's answer on Simplified
//! Chinese is unsatisfactory, it is the first thing to add and this protocol
//! runs against it unchanged.

use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(any(feature = "sherpa", feature = "whisper"))]
use koru_voice::VoiceLanguage;
use koru_voice::{EngineError, ErrorCode, SpeechEngine};

/// **`H`** — the chunk-and-redecode interval for the offline candidates.
///
/// Taken from whisper.cpp's own streaming example, which samples the microphone
/// every half second and re-transcribes a rolling buffer. The same value for
/// both offline candidates is what keeps them comparable to each other.
pub const REDECODE_INTERVAL: Duration = Duration::from_millis(500);

/// Which runtime a candidate needs.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Runtime {
    Sherpa,
    Whisper,
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Sherpa => "sherpa-onnx",
            Runtime::Whisper => "whisper.cpp",
        }
    }

    /// The cargo feature that compiles it in.
    pub fn feature(self) -> &'static str {
        match self {
            Runtime::Sherpa => "sherpa",
            Runtime::Whisper => "whisper",
        }
    }

    /// Whether this build has it.
    pub fn is_compiled_in(self) -> bool {
        match self {
            Runtime::Sherpa => cfg!(feature = "sherpa"),
            Runtime::Whisper => cfg!(feature = "whisper"),
        }
    }
}

/// Which sherpa API, or whisper.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Api {
    Online,
    Offline,
    Whisper,
}

/// One row of the candidate table.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    /// 1..=4, and it is the `cand` column in every table.
    pub number: u8,
    pub name: &'static str,
    pub runtime: Runtime,
    pub api: Api,
    /// The directory the provisioning step places under `--models`.
    pub model_dir_name: &'static str,
    /// The `ModelId` this candidate reports.
    pub model_id: &'static str,
    /// `Some` when partials come from **our** chunk-and-redecode policy rather
    /// than from the recogniser.
    pub our_policy: bool,
}

/// The four, in the protocol's order.
pub const CANDIDATES: [Candidate; 4] = [
    Candidate {
        number: 1,
        name: "Nemotron 3.5 ASR Streaming 0.6B int8 160ms",
        runtime: Runtime::Sherpa,
        api: Api::Online,
        model_dir_name: "sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-160ms-int8-2026-06-11",
        model_id: "nemotron-3.5-streaming-160ms-int8",
        our_policy: false,
    },
    Candidate {
        number: 2,
        name: "Nemotron 3.5 ASR Streaming 0.6B int8 560ms",
        runtime: Runtime::Sherpa,
        api: Api::Online,
        model_dir_name: "sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11",
        model_id: "nemotron-3.5-streaming-560ms-int8",
        our_policy: false,
    },
    Candidate {
        number: 3,
        name: "Parakeet TDT 0.6B v3 int8",
        runtime: Runtime::Sherpa,
        api: Api::Offline,
        model_dir_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        model_id: "parakeet-tdt-0.6b-v3-int8",
        our_policy: true,
    },
    Candidate {
        number: 4,
        name: "Whisper large-v3-turbo q5_0",
        runtime: Runtime::Whisper,
        api: Api::Whisper,
        model_dir_name: "whisper-large-v3-turbo-q5_0",
        model_id: "whisper-large-v3-turbo-q5-0",
        our_policy: true,
    },
];

/// The contingency the protocol names: if Whisper `large-v3-turbo` q5_0 exceeds
/// RTF 1.0 on `W-base` in the screen, the decision run substitutes `medium`
/// q5_0 **and the substitution is recorded in the run header.** A candidate that
/// cannot keep up is not a fallback.
pub const WHISPER_CONTINGENCY_DIR: &str = "whisper-medium-q5_0";

impl Candidate {
    pub fn by_number(number: u8) -> Option<&'static Candidate> {
        CANDIDATES.iter().find(|c| c.number == number)
    }

    /// Where this candidate's model directory is under `--models`.
    pub fn model_dir(&self, models: &Path) -> PathBuf {
        models.join(self.model_dir_name)
    }

    /// The redecode interval this candidate runs with, if any.
    pub fn partial_every(&self) -> Option<Duration> {
        self.our_policy.then_some(REDECODE_INTERVAL)
    }
}

/// Why a candidate could not be built for this run.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Unavailable {
    /// The runtime is not compiled into this binary. Carries the model
    /// directory's state as well, so one line says everything that has to
    /// happen before this candidate can run.
    RuntimeNotBuilt {
        feature: &'static str,
        dir: PathBuf,
        model_present: bool,
    },
    /// The model directory is not there. **This is the state the smoke test
    /// runs in**, and it is reported and exits zero rather than failing: a
    /// harness that could only be run with 3 GB of models provisioned is a
    /// harness nobody runs.
    ModelMissing { dir: PathBuf },
    /// It is there and would not load.
    Failed(EngineError),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unavailable::RuntimeNotBuilt {
                feature,
                dir,
                model_present,
            } => {
                write!(f, "runtime not built (rebuild with --features {feature})")?;
                if !model_present {
                    write!(f, "; model missing ({})", dir.display())?;
                }
                Ok(())
            }
            Unavailable::ModelMissing { dir } => {
                write!(f, "model missing ({})", dir.display())
            }
            Unavailable::Failed(error) => write!(f, "{error}"),
        }
    }
}

/// Build one candidate's engine over a provisioned model directory.
///
/// **The languages a package claims are declared here rather than read off the
/// model**, because sherpa exposes no way to enumerate a prompt dictionary and
/// whisper's language list is the whole of its training set rather than the
/// eight this product ships. All four candidates are multilingual over the eight
/// and the gate measures all eight; a package that turns out not to answer one
/// fails that language's row, which is the outcome that should be visible.
pub fn open(
    candidate: &Candidate,
    models: &Path,
    num_threads: i32,
) -> Result<Box<dyn SpeechEngine>, Unavailable> {
    let dir = candidate.model_dir(models);
    // **Both facts, in that order.** The model directory is checked even when
    // the runtime is not compiled in, because "rebuild with a feature" and
    // "download three gigabytes" are two different things somebody has to do
    // and finding out about the second one only after paying for the first is
    // an hour wasted.
    if !candidate.runtime.is_compiled_in() {
        return Err(Unavailable::RuntimeNotBuilt {
            feature: candidate.runtime.feature(),
            model_present: dir.is_dir(),
            dir,
        });
    }
    if !dir.is_dir() {
        return Err(Unavailable::ModelMissing { dir });
    }
    open_built(candidate, dir, num_threads)
}

#[cfg(feature = "sherpa")]
fn open_sherpa(
    candidate: &Candidate,
    dir: PathBuf,
    num_threads: i32,
) -> Result<Box<dyn SpeechEngine>, Unavailable> {
    use koru_voice_sherpa::{
        nemotron::{NemotronConfig, NemotronEngine},
        parakeet::{ParakeetConfig, ParakeetEngine},
    };
    let model = model_id(candidate)?;
    match candidate.api {
        Api::Online => NemotronEngine::open(NemotronConfig {
            model_dir: dir,
            model,
            num_threads,
            languages: VoiceLanguage::ALL.to_vec(),
        })
        .map(|e| Box::new(e) as Box<dyn SpeechEngine>)
        .map_err(Unavailable::Failed),
        Api::Offline => ParakeetEngine::open(ParakeetConfig {
            model_dir: dir,
            model,
            num_threads,
            languages: VoiceLanguage::ALL.to_vec(),
            partial_every: candidate.partial_every(),
        })
        .map(|e| Box::new(e) as Box<dyn SpeechEngine>)
        .map_err(Unavailable::Failed),
        Api::Whisper => unreachable!("a sherpa candidate does not use the whisper API"),
    }
}

#[cfg(feature = "whisper")]
fn open_whisper(
    candidate: &Candidate,
    dir: PathBuf,
    num_threads: i32,
) -> Result<Box<dyn SpeechEngine>, Unavailable> {
    use koru_voice_whisper::engine::{WhisperConfig, WhisperEngine};
    let model = model_id(candidate)?;
    WhisperEngine::open(WhisperConfig {
        model_dir: dir,
        model,
        num_threads,
        languages: VoiceLanguage::ALL.to_vec(),
        partial_every: candidate.partial_every(),
    })
    .map(|e| Box::new(e) as Box<dyn SpeechEngine>)
    .map_err(Unavailable::Failed)
}

#[cfg(any(feature = "sherpa", feature = "whisper"))]
fn model_id(candidate: &Candidate) -> Result<koru_voice::ModelId, Unavailable> {
    koru_voice::ModelId::parse(candidate.model_id).map_err(Unavailable::Failed)
}

/// The dispatch, once the runtime is known to be present.
#[allow(unused_variables)]
fn open_built(
    candidate: &Candidate,
    dir: PathBuf,
    num_threads: i32,
) -> Result<Box<dyn SpeechEngine>, Unavailable> {
    match candidate.runtime {
        #[cfg(feature = "sherpa")]
        Runtime::Sherpa => open_sherpa(candidate, dir, num_threads),
        #[cfg(feature = "whisper")]
        Runtime::Whisper => open_whisper(candidate, dir, num_threads),
        // Unreachable in practice — `open` checked `is_compiled_in` first — but
        // it has to compile in every feature combination, and a `match` with no
        // arm for a variant does not.
        #[allow(unreachable_patterns)]
        other => Err(Unavailable::Failed(ErrorCode::EngineFailed.with(format!(
            "the {} runtime is not compiled into this binary",
            other.as_str()
        )))),
    }
}

#[cfg(test)]
mod tests;
