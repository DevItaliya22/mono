// SPDX-License-Identifier: MPL-2.0

//! **The artifact table: one row per *export*, not one per model.**
//!
//! The discipline is `docs/features/voice/RUNTIME_ARTIFACTS.md`'s, copied
//! deliberately rather than reinvented: upstream publishes no checksum file, so
//! **this table is the checksum file**, and it pins what we measured against
//! rather than what anybody attests to. A re-tagged asset under the same name
//! becomes a hash mismatch here, which is the failure worth having.
//!
//! **An empty `upstream_sha256` means unpinned, and an unpinned artifact is
//! never fetched.** That is the same rule `NativeArtifact::is_pinned` already
//! carries; `voice-models pin` is how a row is filled in the first time, by
//! downloading the asset once and hashing it.
//!
//! ## Why a row is an export
//!
//! Nemotron's chunk size is baked into the ONNX graph — `export_onnx.py`
//! computes `chunk = ms // 80 - 1` and calls `set_default_att_context_size`
//! *before* writing it — so "160 ms and 560 ms" is two downloads, two model
//! directories and two rows. Q-V52 confirmed this against the published assets
//! rather than assuming it, and the failure it prevents is a 560 ms model
//! serving a person who chose 160 ms, which nothing else would notice.
//!
//! ## The second digest, and what it buys
//!
//! `repacked_sha256` pins the **zip we build**, not the archive we downloaded.
//! It is what makes a re-run cheap: the published key is
//! `models/<id>/<version>/<digest>.zip`, so with this pinned the publisher can
//! compute every key from the table alone, ask the origin what is already
//! there, and download nothing at all when the answer is *everything*. Without
//! it, learning the key costs a 475 MB download and a repack per model.
//!
//! Which in turn is why `repack` is deterministic: fixed entry timestamps,
//! fixed permissions, and the tar's own order. Two runs over the same pinned
//! upstream bytes must produce the same zip, or this field is a trap rather
//! than a shortcut.

use koru_voice::VoiceLanguage;
use koru_voice_models::ModelTier;

/// What an upstream artifact *is*, which decides how it is repackaged.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Shape {
    /// A `.tar.bz2` with a single top-level directory, which is **stripped**.
    ///
    /// Stripping is not cosmetic. `DiskModelStore` extracts into
    /// `<id>/<version>/<digest>/` and the engine adapter resolves
    /// `encoder.int8.onnx` and friends directly beneath the directory it is
    /// handed, so a zip that kept the tar's wrapper would install a model whose
    /// graphs are one level too deep and whose only symptom is
    /// "has neither encoder.int8.onnx nor encoder.onnx".
    TarBz2,
    /// One file, which becomes the zip's only entry beside the licence.
    /// Whisper's weights are a bare `.bin` and are not an archive at all.
    BareFile,
}

/// One published export.
#[derive(Copy, Clone, Debug)]
pub struct ModelArtifact {
    /// The `ModelId`. Lowercase, and a path component — `ModelId::parse`'s
    /// rules, checked at build time by `every_row_is_a_legal_descriptor`.
    pub id: &'static str,
    /// The `ModelVersion`. An opaque label rather than semver, so it names the
    /// export: `int8-2026-06-11`, `q5_0`.
    pub version: &'static str,
    /// `sherpa` or `whisper`. **Signed**, because verified bytes handed to the
    /// wrong native parser is a crash at best.
    pub engine: &'static str,
    /// **Provisional.** The gate (Plan 0.4) has not run, so nothing has decided
    /// which of these is the product's compact model and which is its accurate
    /// one. The assignment below is the defensible reading of the exports
    /// themselves — a shorter chunk is lower latency, a longer one is more
    /// accurate — and it is recorded in `docs/features/voice/archive/QUESTIONS.md` Q-V79 so
    /// `ENGINE_DECISION.md` revisits it rather than inheriting it.
    pub tier: ModelTier,
    /// BCP 47 tags, resolved through `VoiceLanguage::matching` rather than
    /// spelled as variants — an unknown tag then fails loudly here instead of
    /// silently narrowing what a model claims.
    pub languages: &'static [&'static str],
    pub shape: Shape,
    /// The upstream URL, verbatim.
    pub url: &'static str,
    /// The name the bytes are saved under in the work directory, and — for a
    /// [`Shape::BareFile`] — the name they carry inside the zip.
    pub file_name: &'static str,
    /// The published length. A cheap pre-check that costs no hashing and
    /// catches the common failure — an HTML error page saved under the
    /// archive's name — before a half-gigabyte read.
    pub upstream_size: u64,
    /// **Ours, not upstream's**, and empty means unpinned. See the module note.
    pub upstream_sha256: &'static str,
    /// The digest of the zip *we* build, empty until measured. See the module
    /// note for what it buys.
    pub repacked_sha256: &'static str,
    /// **Declared, not measured**, and the descriptor signs it because a phone
    /// decides whether to download *before* it can measure anything. The
    /// figures below are conservative readings of the one public data point —
    /// roughly 1.23 GB resident on iOS for a comparable int8 0.6B model — and
    /// Q-V79 records that the benchmark owes real numbers.
    pub working_set_bytes: u64,
    /// The SPDX identifier, which goes into the signed descriptor.
    pub licence: &'static str,
    /// The file under `crates/koru-release/licences/` whose text ships inside
    /// the zip. Named per artifact rather than per SPDX id because the MIT
    /// template carries a `<year> <copyright holders>` placeholder and what
    /// has to travel is OpenAI's actual notice.
    pub licence_file: &'static str,
    /// Who to credit. Signed, because an attribution a catalog can change is a
    /// licence notice about somebody else's work that nobody agreed to.
    pub attribution: &'static str,
    /// The copyright and origin notices that go into `NOTICE.txt`, one per
    /// line. OpenMDW-1.1's only substantive condition is that these and the
    /// agreement itself are retained in any distribution, so this field is a
    /// legal obligation rather than documentation.
    pub origin_notice: &'static [&'static str],
}

impl ModelArtifact {
    /// Whether the upstream hash above is a hash.
    pub fn is_pinned(&self) -> bool {
        self.upstream_sha256.len() == 64
    }

    /// Whether the zip this row builds has a digest recorded, which is what
    /// lets the publisher name its key without downloading anything.
    pub fn repack_is_pinned(&self) -> bool {
        self.repacked_sha256.len() == 64
    }

    /// The eight launch languages resolved to the model's own type.
    pub fn languages(&self) -> Result<Vec<VoiceLanguage>, String> {
        self.languages
            .iter()
            .map(|tag| {
                VoiceLanguage::matching(tag)
                    .ok_or_else(|| format!("{}: {tag} is not one of the eight", self.id))
            })
            .collect()
    }
}

/// Every language the two multilingual 0.6B models are published for, and the
/// eight Koru ships. `zh-Hans` is in Nemotron's *broad-coverage* tier rather
/// than its transcription-ready one, which the benchmark protocol treats as the
/// stated accuracy risk; it is still a language the model claims, so it is
/// still declared.
const EIGHT: &[&str] = &["en", "es", "pt-BR", "de", "fr", "ar", "zh-Hans", "ja"];

/// Attribution shared by the two Nemotron exports: the weights are NVIDIA's and
/// the ONNX export is the sherpa-onnx project's, and both belong in a notice
/// that claims to say where these bytes came from.
const NEMOTRON_ATTRIBUTION: &str = "NVIDIA Corporation, Nemotron 3.5 ASR Streaming 0.6B (OpenMDW-1.1); \
     ONNX export by the k2-fsa sherpa-onnx project (Apache-2.0)";

const NEMOTRON_NOTICE: &[&str] = &[
    "Nemotron 3.5 ASR Streaming 0.6B",
    "Copyright (c) NVIDIA Corporation. Licensed under OpenMDW-1.1.",
    "Model card: https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b",
    "",
    "INT8 ONNX export produced by the sherpa-onnx project (k2-fsa), Apache-2.0,",
    "using scripts/nemo/nemotron-3.5-asr-streaming-0.6b/export_onnx.py.",
    "Upstream package: https://github.com/k2-fsa/sherpa-onnx (tag asr-models)",
];

/// **Every export this publisher knows how to publish.**
///
/// Four rows for four candidates, and the first two are one model exported
/// twice. Sizes were read from the published assets; the hashes are not filled
/// in, because filling one in means downloading half a gigabyte and hashing it,
/// which is what `voice-models pin` is for and what an unpinned row refuses to
/// let anybody skip.
pub const ARTIFACTS: &[ModelArtifact] = &[
    ModelArtifact {
        id: "nemotron-3.5-streaming-0.6b-160ms",
        version: "int8-2026-06-11",
        engine: "sherpa",
        tier: ModelTier::HighAccuracy,
        languages: EIGHT,
        shape: Shape::TarBz2,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-160ms-int8-2026-06-11.tar.bz2",
        file_name: "sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-160ms-int8-2026-06-11.tar.bz2",
        upstream_size: 475_273_363,
        upstream_sha256: "a81909a1780d84cff16d73c15e13e67d9d81d8839faf14870d507d8499f7a61a",
        repacked_sha256: "8e3ca0895ed78a04a3a86cd16530d41dbf7003ec2eec198bd3b25ed6ca33f229",
        working_set_bytes: 2 * 1024 * 1024 * 1024,
        licence: "OpenMDW-1.1",
        licence_file: "OpenMDW-1.1.txt",
        attribution: NEMOTRON_ATTRIBUTION,
        origin_notice: NEMOTRON_NOTICE,
    },
    ModelArtifact {
        id: "nemotron-3.5-streaming-0.6b-560ms",
        version: "int8-2026-06-11",
        engine: "sherpa",
        tier: ModelTier::HighAccuracy,
        languages: EIGHT,
        shape: Shape::TarBz2,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11.tar.bz2",
        file_name: "sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11.tar.bz2",
        upstream_size: 475_271_763,
        upstream_sha256: "",
        repacked_sha256: "",
        working_set_bytes: 2 * 1024 * 1024 * 1024,
        licence: "OpenMDW-1.1",
        licence_file: "OpenMDW-1.1.txt",
        attribution: NEMOTRON_ATTRIBUTION,
        origin_notice: NEMOTRON_NOTICE,
    },
    ModelArtifact {
        id: "parakeet-tdt-0.6b-v3",
        // No date in the upstream name to borrow, so the version names only the
        // precision. If the project re-exports under the same file name, the
        // pinned `upstream_sha256` fails first and the version is bumped by
        // hand — which is the right order: the mismatch is the evidence.
        version: "int8",
        engine: "sherpa",
        tier: ModelTier::HighAccuracy,
        languages: EIGHT,
        shape: Shape::TarBz2,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
        file_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
        upstream_size: 487_170_055,
        upstream_sha256: "",
        repacked_sha256: "",
        working_set_bytes: 2 * 1024 * 1024 * 1024,
        licence: "CC-BY-4.0",
        licence_file: "CC-BY-4.0.txt",
        attribution: "NVIDIA Corporation, Parakeet TDT 0.6B v3 (CC-BY-4.0); \
                      ONNX export by the k2-fsa sherpa-onnx project (Apache-2.0)",
        origin_notice: &[
            "Parakeet TDT 0.6B v3",
            "Copyright (c) NVIDIA Corporation.",
            "Licensed under the Creative Commons Attribution 4.0 International",
            "licence (CC-BY-4.0): https://creativecommons.org/licenses/by/4.0/",
            "Model card: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3",
            "",
            "INT8 ONNX export produced by the sherpa-onnx project (k2-fsa),",
            "Apache-2.0. Upstream package:",
            "https://github.com/k2-fsa/sherpa-onnx (tag asr-models)",
            "",
            "Changes: the upstream .tar.bz2 was repackaged as a .zip by Koru.",
            "No model weights were altered.",
        ],
    },
    // **The Compact tier, and the only row that is genuinely small.** The two
    // Nemotron exports differ by chunk size, not by size on disk - both are
    // ~475 MB - so labelling one of them Compact promised a saving the download
    // did not deliver. Whisper base is 74M parameters against Nemotron's 600M,
    // and q5_1 puts it under 60 MB: an eighth of the download and far less
    // resident memory, which is also what makes it the candidate for a phone.
    //
    // The cost is real and belongs in the settings copy rather than here:
    // Whisper is **not** a streaming recogniser, so Compact produces its text
    // shortly after the key is released instead of revising it live. Plan 0.4
    // should also weigh ggml-small-q5_1 (190 MB) for this slot - better across
    // eight languages, still under half of Nemotron - before this is final.
    ModelArtifact {
        id: "whisper-base",
        version: "q5_1",
        engine: "whisper",
        tier: ModelTier::Compact,
        languages: EIGHT,
        shape: Shape::BareFile,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q5_1.bin",
        file_name: "ggml-base-q5_1.bin",
        upstream_size: 59_707_625,
        upstream_sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
        repacked_sha256: "c48fb5a2a1499e8d90cefab6e5f36ba6bc71e291354c1f7a781e2930ec4f4493",
        working_set_bytes: 512 * 1024 * 1024,
        licence: "MIT",
        licence_file: "MIT-openai-whisper.txt",
        attribution: "OpenAI, Whisper base (MIT); GGML quantisation published by the whisper.cpp project",
        origin_notice: &[
            "Whisper base, q5_1 GGML quantisation",
            "Copyright (c) 2022 OpenAI. Licensed under the MIT licence,",
            "reproduced in LICENCE.txt as required by that licence.",
            "Model card: https://huggingface.co/openai/whisper-base",
            "",
            "GGML conversion and quantisation published by the whisper.cpp",
            "project at https://huggingface.co/ggerganov/whisper.cpp",
        ],
    },
    ModelArtifact {
        id: "whisper-large-v3-turbo",
        version: "q5_0",
        engine: "whisper",
        tier: ModelTier::HighAccuracy,
        languages: EIGHT,
        shape: Shape::BareFile,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        file_name: "ggml-large-v3-turbo-q5_0.bin",
        upstream_size: 574_041_195,
        upstream_sha256: "",
        repacked_sha256: "",
        working_set_bytes: 2 * 1024 * 1024 * 1024,
        licence: "MIT",
        licence_file: "MIT-openai-whisper.txt",
        attribution: "OpenAI, Whisper large-v3-turbo (MIT); \
                      GGML quantisation published by the whisper.cpp project",
        origin_notice: &[
            "Whisper large-v3-turbo, q5_0 GGML quantisation",
            "Copyright (c) 2022 OpenAI. Licensed under the MIT licence,",
            "reproduced in LICENCE.txt as required by that licence.",
            "Model card: https://huggingface.co/openai/whisper-large-v3-turbo",
            "",
            "GGML conversion and quantisation published by the whisper.cpp",
            "project at https://huggingface.co/ggerganov/whisper.cpp",
        ],
    },
];

/// The two artifacts offered by the product. Other rows remain available to
/// benchmark and pinning tools, but cannot block or enter a public catalog.
pub const PRODUCT_ARTIFACT_IDS: &[&str] = &["whisper-base", "nemotron-3.5-streaming-0.6b-160ms"];

pub fn is_product(artifact: &ModelArtifact) -> bool {
    PRODUCT_ARTIFACT_IDS.contains(&artifact.id)
}

pub fn product_artifacts() -> impl Iterator<Item = &'static ModelArtifact> {
    ARTIFACTS.iter().filter(|artifact| is_product(artifact))
}

/// The row an id names, for `--only`.
pub fn find(id: &str) -> Option<&'static ModelArtifact> {
    ARTIFACTS.iter().find(|artifact| artifact.id == id)
}
