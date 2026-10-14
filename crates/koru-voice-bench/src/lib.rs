// SPDX-License-Identifier: MPL-2.0

//! **The speech-engine benchmark, built to
//! `docs/features/voice/plans/ENGINE_BENCHMARK.md` and to nothing else.**
//!
//! Plan 0.4 runs this and writes `ENGINE_DECISION.md` and ADR-253 from its
//! output. Nothing after Plan 0.4 names a speech model, so the numbers this
//! produces are the whole basis of that choice — which is why the protocol was
//! written first and why this crate invents no metric of its own. **If a number
//! is not in that document, it is not printed here.**
//!
//! ```text
//! koru-voice-bench --stage screen --corpus <dir> --models <dir> --class W-desk
//! koru-voice-bench --smoke                        # the checked-in fixture, no corpus
//! ```
//!
//! | | |
//! | --- | --- |
//! | [`wav`] | 16 kHz mono 16-bit PCM, and no decoder |
//! | [`trim`] | −45 dBFS silence trimming, with 200 ms of the original lead put back |
//! | [`normalise`] | the four shared rules, plus Arabic, Simplified Chinese and Japanese |
//! | [`metrics`] | WER, CER, `TTFUP` with its censoring rule, revisions, RTF |
//! | [`stats`] | distribution-free quantile intervals and a bootstrap over utterances |
//! | [`sample`] | the seed, the shuffle and the eligibility filters |
//! | [`corpus`] | `MANIFEST.json`, `transcripts.tsv`, `speakers.tsv`, and the smoke fixture |
//! | [`pace`] | the two passes — one paced to a wall clock, one as fast as the engine accepts |
//! | [`candidate`] | the four candidates, and what each is when its runtime is not compiled in |
//! | [`host`] | the machine facts a run header has to carry, peak RSS and CPU |
//! | [`report`] | the run header, the four tables, the probe, and `bench-<run-id>.json` |
//!
//! ## The one thing to understand before reading any of it
//!
//! **Latency is measured paced; real-time factor is measured unpaced; they are
//! two passes and neither substitutes for the other.** Under pacing the harness
//! pushes one 20 ms frame at each wall-clock deadline, exactly as a microphone
//! would — the only condition in which a latency means what a person would
//! think it means. That makes wall time equal audio time by construction, so a
//! paced pass cannot measure RTF: every utterance would score 1.0. Unpaced, the
//! whole utterance goes in as fast as the engine takes it, which gives RTF and
//! peak memory and contains no clock a person waits for.
//!
//! The two passes' final transcripts are compared per utterance and any
//! disagreement is reported as `drift`. A streaming engine with a fixed chunk
//! schedule and a chunk-and-redecode policy are both supposed to be
//! timing-independent; a nonzero `drift` says one of them is not.
//!
//! ## What it will not do
//!
//! No microphone — it reads files. No IPC, no UI, no model downloading: a model
//! directory is an argument. And **no default corpus and no search path**. A
//! benchmark that silently found *some* corpus is the same failure as a
//! mistyped `--profile` silently opening the last-selected workspace, so
//! `--corpus` is required and `KORU_VOICE_BENCH_CORPUS` is the only fallback.

pub mod candidate;
pub mod cli;
pub mod corpus;
pub mod host;
pub mod metrics;
pub mod normalise;
pub mod pace;
pub mod report;
pub mod run;
pub mod sample;
pub mod stats;
pub mod trim;
pub mod wav;

pub use cli::{Options, Stage};
pub use corpus::{Corpus, SmokeFixture, Utterance};
pub use normalise::normalise;
pub use trim::Trimmed;
pub use wav::read_wav;

/// **The revision of the protocol this harness implements.**
///
/// A corpus tree records the revision it was prepared for, and the harness
/// refuses to run against a tree that names a different one. A corpus that moves
/// under a benchmark makes two runs incomparable, and a *protocol* that moves
/// under a corpus does the same thing more quietly.
pub const PROTOCOL_REV: u32 = 1;

/// The harness's own version, printed in the run header beside the protocol
/// revision. The workspace version, because there is only one in this product.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
