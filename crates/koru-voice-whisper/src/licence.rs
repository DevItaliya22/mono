// SPDX-License-Identifier: MPL-2.0

//! **What this crate's runtime and weights are licensed under, with sources.**
//!
//! It is a module rather than a paragraph in a document because Q-V5 makes it a
//! *decision input*: models are downloaded from our own infrastructure and
//! signed by our own key, so shipping Whisper as the fallback means
//! redistributing a weight file. If the quantised conversions could not be
//! hosted, Whisper would be usable in the benchmark — a local evaluation
//! redistributes nothing — and not usable as a product fallback, and ADR-253's
//! choice would be a second sherpa model or no fallback at all.
//!
//! **The answer, checked rather than assumed: nothing here blocks shipping.**
//! Every row below was read from the artifact or its registry entry, and the
//! `checked` column says how.
//!
//! | Artifact | Licence | How it was checked |
//! | --- | --- | --- |
//! | whisper.cpp (the runtime) | MIT | `whisper.cpp/LICENSE` inside the `whisper-rs-sys` `.crate` — "MIT License, Copyright (c) 2023-2024 The ggml authors" |
//! | `whisper-rs` (the bindings) | Unlicense | the crates.io registry entry for `whisper-rs 0.16.0` |
//! | `whisper-rs-sys` | Unlicense | the crates.io registry entry for `whisper-rs-sys 0.15.0` |
//! | `openai/whisper-large-v3-turbo` (original weights) | MIT | the model's own card metadata on Hugging Face |
//! | `ggerganov/whisper.cpp` (the GGML conversions, incl. `ggml-large-v3-turbo-q5_0.bin`) | MIT | that repository's card metadata; not a gated repo |
//!
//! Two things this does *not* settle, both recorded rather than glossed:
//!
//! - **Unlicense is a public-domain dedication, not a permissive licence**, and
//!   some organisations' policies treat the two differently. It applies to the
//!   Rust bindings only — the code we would link, not the code we would
//!   redistribute — and this crate is deleted if ADR-253 does not promote it, so
//!   the exposure is bounded by that decision rather than by a review.
//! - **A licence read today is not a licence attested forever.** The
//!   redistribution decision belongs to Plan 1.1's catalog, where every model
//!   carries a `LicenceRef` inside the *signed* descriptor; this module is what
//!   that entry should be filled in from, not a substitute for it.

/// One artifact's licence, as read.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct LicenceFact {
    /// What the licence covers.
    pub artifact: &'static str,
    /// The SPDX identifier.
    pub spdx: &'static str,
    /// Whether this is a thing we would **redistribute** (a weight file we
    /// host and sign) or merely **link** (a runtime compiled into our binary).
    ///
    /// The distinction is the whole of Q-V5: a benchmark redistributes nothing,
    /// so only the `redistributed` rows can block shipping.
    pub redistributed: bool,
    /// Where the answer came from, so a second person can repeat the check.
    pub source: &'static str,
}

/// Every licence fact this crate depends on.
pub const FACTS: [LicenceFact; 5] = [
    LicenceFact {
        artifact: "whisper.cpp",
        spdx: "MIT",
        redistributed: false,
        source: "whisper.cpp/LICENSE, vendored inside the whisper-rs-sys 0.15.0 .crate",
    },
    LicenceFact {
        artifact: "whisper-rs",
        spdx: "Unlicense",
        redistributed: false,
        source: "crates.io registry metadata for whisper-rs 0.16.0",
    },
    LicenceFact {
        artifact: "whisper-rs-sys",
        spdx: "Unlicense",
        redistributed: false,
        source: "crates.io registry metadata for whisper-rs-sys 0.15.0",
    },
    LicenceFact {
        artifact: "openai/whisper-large-v3-turbo",
        spdx: "MIT",
        redistributed: false,
        source: "Hugging Face model card metadata, license: mit",
    },
    LicenceFact {
        artifact: "ggerganov/whisper.cpp ggml-large-v3-turbo-q5_0.bin",
        spdx: "MIT",
        // **The one row that matters for packaging.** This is the file a Koru
        // catalog would host and sign.
        redistributed: true,
        source: "Hugging Face repository card metadata, license: mit, not gated",
    },
];

/// The SPDX identifiers that let us host a file.
///
/// A short allow-list rather than a predicate over licence text: the question
/// is not "is this permissive" but "has somebody decided we may redistribute
/// under it", and that decision belongs in a list somebody can read.
pub const REDISTRIBUTABLE: [&str; 3] = ["MIT", "Apache-2.0", "CC-BY-4.0"];

/// Whether every artifact we would redistribute carries a licence we may
/// redistribute under.
///
/// `false` is the answer that deletes this crate at the gate, which is why it
/// is a function rather than a sentence in a document.
pub fn may_be_shipped() -> bool {
    FACTS
        .iter()
        .filter(|fact| fact.redistributed)
        .all(|fact| REDISTRIBUTABLE.contains(&fact.spdx))
}

#[cfg(test)]
mod tests;
