// SPDX-License-Identifier: MPL-2.0

//! **Turns the sherpa-onnx runtime into a [`koru_voice::SpeechEngine`].**
//!
//! Two adapters over one native library, because the gate needs two shapes of
//! recogniser out of it:
//!
//! | | |
//! | --- | --- |
//! | [`nemotron`] | Nemotron 3.5 ASR Streaming 0.6B through sherpa's `OnlineRecognizer` — a genuinely streaming transducer that revises as audio arrives |
//! | [`parakeet`] | Parakeet TDT 0.6B v3 through sherpa's `OfflineRecognizer`, made to emit partials by **our** chunk-and-redecode policy |
//!
//! It knows no microphone, no socket, no UI and no model *store* — a model is a
//! directory somebody hands it, which is what lets the benchmark point it at a
//! provisioned tree and the worker point it at a leased one.
//!
//! ## The feature gate is about a build script, not about size
//!
//! **`runtime` is off by default and nothing in this workspace turns it on.**
//! `sherpa-onnx-sys`' build script downloads a matching prebuilt native archive
//! from GitHub releases whenever `SHERPA_ONNX_LIB_DIR` is unset, and this crate
//! is a workspace member — so with the feature on by default, an ordinary
//! `cargo test --workspace` on a fresh clone would reach the network *during the
//! build*. Nothing inside this crate can prevent that: a runtime check, a
//! *model missing* message and a `cfg(test)` guard all run after the build
//! script has already finished.
//!
//! So the offline property is stated operationally and is checked that way:
//! `cargo fetch --locked`, then with the network down, `cargo build --workspace
//! && cargo test --workspace`. See `docs/features/voice/RUNTIME_ARTIFACTS.md` for
//! **who provisions the native archive** for the builds that do turn the feature
//! on — the artifact per target, where it is stored, the hash it is verified
//! against and the script that places it, which is the discipline
//! `assets/fonts/*.ttf` already follows through `koru.sh fonts`.
//!
//! ## What is outside the gate, and why that matters
//!
//! Everything that does not need the native library:
//!
//! - [`layout`] — which files a sherpa model directory has to contain, resolved
//!   and refused without loading one.
//! - [`language`] — the per-stream `language` option string, including the fact
//!   that `Requested::Auto` is spelled `auto` and that an empty string means the
//!   same thing to this model family.
//! - [`redecode`] — the counter behind the chunk-and-redecode policy. The
//!   *value* of `H` is not here: the policy is the benchmark's, so the benchmark
//!   declares the interval and hands it in.
//! - [`artifacts`] — the per-target native archive names, mirroring
//!   `sherpa-onnx-sys`' own table so a version bump that renames one is a test
//!   failure here rather than a download failure on somebody's machine.
//!
//! An adapter whose only tested part is the twenty lines that call into C is an
//! adapter nobody can review, and those twenty lines are the part a compiler
//! checks anyway.
//!
//! ## Two things about these models that are not guessable
//!
//! **The chunk size is baked into the ONNX export.** Nemotron 3.5 ASR Streaming
//! 0.6B is published as *five* archives — 80, 160, 320, 560 and 1120 ms — all
//! 475.3 MB, produced by one exporter that sets `att_context_size = [56, ms/80
//! − 1]` before writing the graph. "Nemotron at 160 ms and 560 ms" is therefore
//! two downloads and two model directories, never one model and a flag; see
//! [`artifacts::CHUNK_IS_AN_EXPORT`].
//!
//! **A forced language is a per-stream option, not a config field.** The
//! Nemotron package carries a prompt dictionary in its encoder metadata, and
//! sherpa exposes overrides as `stream.set_option("language", "ja")`. An engine
//! asks `has_option("language")` before setting one rather than assuming. The
//! model still auto-detects when no override is supplied; option availability
//! and detection capability are therefore deliberately separate facts.

pub mod artifacts;
pub mod language;
pub mod layout;
pub mod redecode;

#[cfg(feature = "runtime")]
pub mod nemotron;
#[cfg(feature = "runtime")]
pub mod parakeet;

pub use artifacts::{NativeArtifact, SHERPA_ONNX_VERSION};
pub use language::{AUTO, option_value};
pub use layout::{OfflineLayout, OnlineLayout};
pub use redecode::Redecode;

/// The engine id both adapters claim, and therefore the one a signed
/// [`koru_voice_models::ModelDescriptor`]'s `engine` field has to name before
/// `LoadedModel::open` will hand these bytes over.
///
/// [`koru_voice_models::ModelDescriptor`]: https://docs.rs/koru-voice-models
///
/// One id for both, because `engine` answers *which native parser may read
/// these files* and the answer is sherpa-onnx in both cases. Which sherpa API
/// reads them is decided by the files that are present, which is
/// [`layout`]'s job.
pub fn engine_id() -> koru_voice::EngineId {
    koru_voice::EngineId::parse("sherpa").expect("`sherpa` is a legal engine id")
}

#[cfg(test)]
mod tests;
