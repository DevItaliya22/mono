// SPDX-License-Identifier: MPL-2.0

//! **The Compact product [`koru_voice::SpeechEngine`] over whisper.cpp.**
//!
//! Whisper is in the gate as the mature fallback: the model everyone has heard
//! of, with a runtime that has been run on more machines than anything else
//! here. It is candidate 4, `large-v3-turbo` at `q5_0`, driven by the same
//! chunk-and-redecode policy as Parakeet.
//!
//! The product worker enables this runtime unconditionally and dispatches a
//! signed `engine=whisper` model descriptor here. The client and window still
//! name only the Compact tier, not this implementation.
//!
//! ## Its build is off by default for a *different* reason than sherpa's
//!
//! Worth knowing, because the two look like one rule and are not.
//!
//! `sherpa-onnx-sys` downloads a prebuilt native archive from GitHub at build
//! time unless it is told where one is — a build script that reaches the
//! network, which is the trap Plan 0.3 exists to close.
//!
//! `whisper-rs-sys` does not. It ships the whisper.cpp sources **inside its own
//! `.crate`** and builds them with cmake and bindgen, so once `cargo fetch` has
//! run there is nothing left for it to fetch. What it needs instead is a C++
//! toolchain, CMake and libclang *on the build machine*, and making those a
//! requirement of `cargo test --workspace` means the repository launcher must
//! check them before invoking Cargo and report what to install.
//!
//! The feature remains off by default in this adapter crate so its pure suites
//! can run alone; `apps/koru-voice` always turns it on. The difference still
//! matters: sherpa needs an artifact placed and verified, whisper needs tools
//! present.
//!
//! ## The licence position, which decides whether this crate survives
//!
//! [`licence`] holds it, sourced rather than assumed, because
//! `docs/features/voice/archive/QUESTIONS.md` Q-V5 records it as the thing that would make
//! Whisper unusable as a shipped fallback even if it won the benchmark. Short
//! version: the runtime is MIT, the bindings are Unlicense, the original weights
//! are MIT, and the quantised GGML conversions we would actually ship are
//! published under MIT as well. Nothing here blocks the gate and nothing here
//! blocks packaging.
//!
//! ## What is outside the runtime gate
//!
//! [`model`] — which file in a directory is the weights, and what a whisper
//! language code is — [`licence`], and [`redecode`]. All three are compiled and
//! tested by every ordinary build, because each is a thing that fails on a real
//! machine and none of them needs a native library to be checked.

pub mod licence;
pub mod model;
pub mod redecode;

#[cfg(feature = "runtime")]
pub mod engine;

pub use model::{GgmlModel, Quantisation, whisper_code};
pub use redecode::Redecode;

/// The engine id this adapter claims.
///
/// Distinct from `sherpa`, because `engine` on a signed model descriptor
/// answers *which native parser may read these files* — and a GGML `.bin` handed
/// to ONNX Runtime, or an ONNX graph handed to whisper.cpp, is exactly the
/// routing mistake that field exists to prevent.
pub fn engine_id() -> koru_voice::EngineId {
    koru_voice::EngineId::parse("whisper").expect("`whisper` is a legal engine id")
}

#[cfg(test)]
mod tests;
