// SPDX-License-Identifier: MPL-2.0

//! **Two corpora, kept apart, and one of them is checked in.**
//!
//! ## The smoke fixture
//!
//! `fixtures/smoke/` — eight WAVs, one per `VoiceLanguage`, and a
//! `transcripts.tsv`. **Its only job is to prove the harness runs.** It is not
//! evidence about any engine and no number derived from it appears in a decision
//! table. It is compiled into the binary's path knowledge rather than found,
//! because a smoke test that needed an argument is a smoke test nobody runs.
//!
//! **It may be a placeholder**, and if it is, `fixtures/smoke/PLACEHOLDER` says
//! so and the run header repeats it. [`SmokeFixture::is_placeholder`] is that
//! bit. Q-V2 is who records the real ones.
//!
//! ## The decision corpus
//!
//! Downloaded, never committed, and named by `--corpus` or
//! `KORU_VOICE_BENCH_CORPUS` — **there is no default and no search path.** A
//! benchmark that silently found *some* corpus is the same failure as a mistyped
//! `--profile` silently opening the last-selected workspace.
//!
//! ```text
//! <corpus>/
//!   MANIFEST.json
//!   opencc-t2s.tsv                  (see crate::normalise::opencc)
//!   fleurs/<config>/                *.wav, transcripts.tsv
//!   commonvoice/<locale>/           *.wav, transcripts.tsv, speakers.tsv
//!   aishell1/test/                  *.wav, transcripts.tsv, speakers.tsv
//! ```
//!
//! ## Pinning, and the two hashes
//!
//! A corpus that moves under a benchmark makes two runs incomparable, so the pin
//! is enforced rather than described. Two runs are **comparable** when
//! `source_sha256` matches — same audio, same references — and **identical**
//! when `converted_sha256` matches too. Hashing only the converted tree would
//! make an ffmpeg upgrade look like a different corpus; hashing only the source
//! would hide a resampler change that genuinely moved the numbers.
//!
//! **The harness refuses to run when `protocol_rev` does not match its own**,
//! and prints both hashes into the run header.
//!
//! ## The language codes do not line up, and that is data rather than code
//!
//! `es` is FLEURS `es_419` (Latin American) and Common Voice `es` (majority
//! European); `pt-BR` has no Common Voice locale of its own; FLEURS Arabic is
//! Egyptian-accented reading of MSA text. [`fleurs_config`] and
//! [`commonvoice_locale`] are the mapping, with the caveats beside them, because
//! a harness written from language names would get all three wrong.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use koru_voice::VoiceLanguage;
use serde::{Deserialize, Serialize};

use crate::normalise::T2sTable;
use crate::trim::{self, Trimmed};
use crate::wav;

/// What went wrong reading a corpus.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CorpusError(pub String);

impl std::fmt::Display for CorpusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CorpusError {}

fn bad(message: impl Into<String>) -> CorpusError {
    CorpusError(message.into())
}

/// Which source an utterance came from. Never pooled in a report.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceId {
    Fleurs,
    #[serde(rename = "commonvoice")]
    CommonVoice,
    #[serde(rename = "aishell1")]
    Aishell1,
}

impl SourceId {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::Fleurs => "fleurs",
            SourceId::CommonVoice => "commonvoice",
            SourceId::Aishell1 => "aishell1",
        }
    }

    /// What the table's `corpus` column prints. Short, because the column is.
    pub fn column(self) -> &'static str {
        match self {
            SourceId::Fleurs => "fleurs",
            SourceId::CommonVoice => "cv",
            SourceId::Aishell1 => "aishel",
        }
    }
}

/// One source's pin, from `MANIFEST.json`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SourcePin {
    pub source_id: SourceId,
    /// FLEURS: the Hugging Face commit sha. Common Voice: the release version
    /// **and** the `cv-dataset` metadata commit. AISHELL-1: the OpenSLR archive
    /// filename.
    pub source_revision: String,
    /// The hash of what was downloaded, before conversion.
    pub source_sha256: String,
    /// The hash of the converted WAV tree that was actually measured.
    pub converted_sha256: String,
}

/// `MANIFEST.json`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Manifest {
    /// The revision of the protocol the tree was prepared for. The harness
    /// refuses a mismatch.
    pub protocol_rev: u32,
    /// The exact converter version string, e.g. `ffmpeg 7.1.1`.
    pub converter: String,
    pub sources: Vec<SourcePin>,
}

impl Manifest {
    pub fn read(root: &Path) -> Result<Manifest, CorpusError> {
        let path = root.join("MANIFEST.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| bad(format!("could not read {}: {e}", path.display())))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| bad(format!("{} is not a corpus manifest: {e}", path.display())))?;
        if manifest.protocol_rev != crate::PROTOCOL_REV {
            return Err(bad(format!(
                "{} was prepared for protocol rev {}, and this harness implements rev {}; \
                 two runs under different revisions are not comparable, so this is a refusal \
                 rather than a warning",
                path.display(),
                manifest.protocol_rev,
                crate::PROTOCOL_REV
            )));
        }
        Ok(manifest)
    }

    pub fn pin(&self, source: SourceId) -> Option<&SourcePin> {
        self.sources.iter().find(|p| p.source_id == source)
    }
}

/// One utterance, ready to be measured.
#[derive(Clone, PartialEq, Debug)]
pub struct Utterance {
    pub id: String,
    pub language: VoiceLanguage,
    pub source: SourceId,
    pub reference: String,
    pub speaker: Option<String>,
    pub path: PathBuf,
}

impl Utterance {
    /// Read and trim. Deliberately not cached: the decision run reads each file
    /// twice — once paced, once not — and holding 900 utterances of `f32` would
    /// be 1.2 GB for no benefit, since the disk read is a rounding error beside
    /// a decode.
    pub fn audio(&self) -> Result<Trimmed, CorpusError> {
        let samples = wav::read_wav(&self.path).map_err(|e| bad(e.to_string()))?;
        Ok(trim::trim(&samples))
    }
}

/// The checked-in smoke fixture.
#[derive(Clone, PartialEq, Debug)]
pub struct SmokeFixture {
    pub root: PathBuf,
    pub utterances: Vec<Utterance>,
    placeholder: bool,
}

/// The seven names the probe looks for.
///
/// **Not scored, not aggregated, and not a gate.** It is a list to look at when
/// deciding whether a hotword or biasing facility is worth having later; seven
/// names is not a distribution and this is labelled as a probe everywhere it
/// appears.
pub const PROPER_NOUNS: [&str; 7] = ["Koru", "Loro", "vello", "parley", "iroh", "Claude", "Codex"];

impl SmokeFixture {
    /// Where the fixture lives relative to this crate. Compiled in, because a
    /// smoke test that needed an argument is a smoke test nobody runs.
    pub fn default_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("smoke")
    }

    /// Load it, in `VoiceLanguage::ALL` order.
    pub fn load(root: &Path) -> Result<SmokeFixture, CorpusError> {
        let references = read_transcripts(&root.join("transcripts.tsv"))?;
        let mut utterances = Vec::with_capacity(VoiceLanguage::ALL.len());
        for language in VoiceLanguage::ALL {
            let tag = language.tag();
            let path = root.join(format!("{tag}.wav"));
            if !path.is_file() {
                return Err(bad(format!(
                    "the smoke fixture has no {tag}.wav; all eight languages are \
                     required, because running seven of them silently is how a \
                     language stops being tested"
                )));
            }
            let reference = references
                .get(tag)
                .cloned()
                .ok_or_else(|| bad(format!("transcripts.tsv has no reference for {tag}")))?;
            utterances.push(Utterance {
                id: tag.to_owned(),
                language,
                source: SourceId::Fleurs,
                reference,
                speaker: None,
                path,
            });
        }
        Ok(SmokeFixture {
            placeholder: root.join("PLACEHOLDER").is_file(),
            root: root.to_path_buf(),
            utterances,
        })
    }

    /// Whether the audio is synthetic. Printed in the run header for as long as
    /// it is true.
    pub fn is_placeholder(&self) -> bool {
        self.placeholder
    }
}

/// The decision corpus on disk.
#[derive(Clone, PartialEq, Debug)]
pub struct Corpus {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub t2s: T2sTable,
}

impl Corpus {
    /// Open a corpus tree: read and check the manifest, load the t2s table if
    /// there is one.
    pub fn open(root: &Path) -> Result<Corpus, CorpusError> {
        let manifest = Manifest::read(root)?;
        let t2s_path = root.join(crate::normalise::T2S_FILE);
        let t2s = if t2s_path.is_file() {
            let text = std::fs::read_to_string(&t2s_path)
                .map_err(|e| bad(format!("could not read {}: {e}", t2s_path.display())))?;
            T2sTable::parse(&text).map_err(|e| bad(format!("{}: {e}", t2s_path.display())))?
        } else {
            T2sTable::empty()
        };
        Ok(Corpus {
            root: root.to_path_buf(),
            manifest,
            t2s,
        })
    }

    /// The directory one `(source, language)` lives in.
    pub fn directory(&self, source: SourceId, language: VoiceLanguage) -> Option<PathBuf> {
        match source {
            SourceId::Fleurs => Some(self.root.join("fleurs").join(fleurs_config(language))),
            SourceId::CommonVoice => Some(
                self.root
                    .join("commonvoice")
                    .join(commonvoice_locale(language)),
            ),
            SourceId::Aishell1 => {
                (language == VoiceLanguage::ZhHans).then(|| self.root.join("aishell1").join("test"))
            }
        }
    }

    /// Every utterance one `(source, language)` holds.
    pub fn utterances(
        &self,
        source: SourceId,
        language: VoiceLanguage,
    ) -> Result<Vec<Utterance>, CorpusError> {
        let Some(dir) = self.directory(source, language) else {
            return Ok(Vec::new());
        };
        if !dir.is_dir() {
            return Err(bad(format!(
                "{} is missing, so {} has no {} utterances",
                dir.display(),
                language.tag(),
                source.as_str()
            )));
        }
        let references = read_transcripts(&dir.join("transcripts.tsv"))?;
        let speakers = read_speakers(&dir.join("speakers.tsv"))?;
        let mut utterances: Vec<Utterance> = references
            .into_iter()
            .map(|(id, reference)| {
                let path = dir.join(format!("{id}.wav"));
                Utterance {
                    speaker: speakers.get(&id).cloned(),
                    id,
                    language,
                    source,
                    reference,
                    path,
                }
            })
            .collect();
        // Sorted here as well as in `sample::draw`, so a caller that walks this
        // list without sampling — a `--only` debugging run — is deterministic
        // too.
        utterances.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(utterances)
    }
}

/// `<key>\t<value>` into a map.
///
/// A duplicate key is an error rather than a last-wins: two references for one
/// utterance id means the tree was assembled wrong, and picking one silently is
/// how a corpus with a build mistake produces numbers nobody can reproduce.
fn read_transcripts(path: &Path) -> Result<HashMap<String, String>, CorpusError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| bad(format!("could not read {}: {e}", path.display())))?;
    let mut map = HashMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (id, reference) = line
            .split_once('\t')
            .ok_or_else(|| bad(format!("{}:{} has no tab", path.display(), number + 1)))?;
        if map.insert(id.to_owned(), reference.to_owned()).is_some() {
            return Err(bad(format!(
                "{}:{} repeats the utterance id {id}",
                path.display(),
                number + 1
            )));
        }
    }
    Ok(map)
}

/// `<utterance_id>\t<speaker_id>\t<gender>\t<age_band>\t<accent>`, of which only
/// the speaker id is needed here. Absent is fine: FLEURS carries `gender` but no
/// speaker id at all, so a per-speaker analysis is reportable only for Common
/// Voice and AISHELL-1.
fn read_speakers(path: &Path) -> Result<HashMap<String, String>, CorpusError> {
    if !path.is_file() {
        return Ok(HashMap::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| bad(format!("could not read {}: {e}", path.display())))?;
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(id), Some(speaker)) = (fields.next(), fields.next()) else {
            continue;
        };
        if !speaker.is_empty() {
            map.insert(id.to_owned(), speaker.to_owned());
        }
    }
    Ok(map)
}

/// FLEURS' config name.
///
/// **`es` is `es_419`, and FLEURS has no `es_es`** — its Spanish is Latin
/// American, while Common Voice `es` is majority European. Together they are a
/// mix; neither alone is "Spanish". **`ar_eg` is Egyptian-accented reading of
/// MSA text**: the text is Modern Standard Arabic, the accent is not.
pub fn fleurs_config(language: VoiceLanguage) -> &'static str {
    match language {
        VoiceLanguage::En => "en_us",
        VoiceLanguage::Es => "es_419",
        VoiceLanguage::PtBr => "pt_br",
        VoiceLanguage::De => "de_de",
        VoiceLanguage::Fr => "fr_fr",
        VoiceLanguage::Ar => "ar_eg",
        VoiceLanguage::ZhHans => "cmn_hans_cn",
        VoiceLanguage::Ja => "ja_jp",
    }
}

/// Common Voice's locale.
///
/// **`pt-BR` is `pt`, because Common Voice does not split Portuguese by
/// variety.** Only utterances whose `accent` metadata declares a Brazilian
/// variant are drawn; if fewer than the target qualify, the shortfall is
/// reported and European Portuguese is **not** substituted.
pub fn commonvoice_locale(language: VoiceLanguage) -> &'static str {
    match language {
        VoiceLanguage::En => "en",
        VoiceLanguage::Es => "es",
        VoiceLanguage::PtBr => "pt",
        VoiceLanguage::De => "de",
        VoiceLanguage::Fr => "fr",
        VoiceLanguage::Ar => "ar",
        VoiceLanguage::ZhHans => "zh-CN",
        VoiceLanguage::Ja => "ja",
    }
}

#[cfg(test)]
mod tests;
