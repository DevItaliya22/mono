// SPDX-License-Identifier: MPL-2.0

//! **Which file in a directory is the weights, and what a whisper language code
//! is.**
//!
//! Whisper's model is one file rather than a package: `ggml-<name>.bin`, where
//! `<name>` carries the size and — if it is quantised — the quantisation.
//! `ggml-large-v3-turbo-q5_0.bin` is the gate's candidate 4;
//! `ggml-medium-q5_0.bin` is the contingency the protocol names if turbo cannot
//! keep up on `W-base`.
//!
//! Resolution lives outside the runtime feature for the same reason it does in
//! `koru-voice-sherpa`: it is the part that fails on a real machine, and
//! `WhisperContext::new_with_params` cannot tell a missing file from a corrupt
//! one in a way anybody can act on.
//!
//! ## The quantisation is read off the file name, and that is a choice
//!
//! It could be read out of the GGML header instead. It is not, because the
//! header would have to be parsed by us — a format we do not own, ahead of the
//! library that does — to answer a question the file name already answers, and
//! because a directory containing `ggml-large-v3-turbo-q5_0.bin` renamed from
//! something else is a provisioning bug rather than a thing to be tolerant of.
//! What the name gives us is a *label for a table column*, and a label that
//! silently disagreed with the bytes would be the same failure as a row marked
//! `160 ms` over a 560 ms export.
//!
//! ## Language codes are ISO 639-1, and `auto` is spelled `auto`
//!
//! whisper.cpp takes a two-letter code or `auto`, which is a happy accident
//! rather than a shared convention: `koru_voice::Requested` splits `Auto` out of
//! the language enum precisely so this mapping can be total, and here the string
//! it needs for "work it out" happens to be the same one sherpa wants.

use std::path::{Path, PathBuf};

use koru_voice::{EngineError, ErrorCode, Requested, VoiceLanguage};

/// How a weight file was quantised, read from its name.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Quantisation {
    /// `q5_0` — the gate's candidate.
    Q5_0,
    /// `q8_0`.
    Q8_0,
    /// Some other `q*` suffix. Carried rather than refused: a quantisation this
    /// crate has not heard of is still a legal model, and the report should
    /// print what it found rather than an error.
    Other,
    /// No `q` suffix: full precision.
    Full,
}

impl Quantisation {
    pub fn as_str(self) -> &'static str {
        match self {
            Quantisation::Q5_0 => "q5_0",
            Quantisation::Q8_0 => "q8_0",
            Quantisation::Other => "quantised",
            Quantisation::Full => "f16",
        }
    }
}

/// A resolved GGML weight file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GgmlModel {
    pub path: PathBuf,
    /// Everything between `ggml-` and `.bin` — `large-v3-turbo-q5_0`. Printed
    /// into the run header, because a table that said `large-v3-turbo` over a
    /// directory holding `medium` would be a wrong number.
    pub name: String,
    pub quantisation: Quantisation,
}

impl GgmlModel {
    /// Find the one `ggml-*.bin` in `root`.
    ///
    /// **Exactly one.** Two weight files in a model directory is a provisioning
    /// mistake with two plausible resolutions — newest, or largest — and a
    /// benchmark that picked one would attribute a `medium` result to a `turbo`
    /// row. Refusing names both files.
    pub fn resolve(root: &Path) -> Result<GgmlModel, EngineError> {
        if !root.is_dir() {
            return Err(ErrorCode::ModelLoadFailed.with(format!(
                "{} is not a directory, so it holds no whisper model",
                root.display()
            )));
        }

        let mut found: Vec<PathBuf> = std::fs::read_dir(root)
            .map_err(|e| {
                ErrorCode::ModelLoadFailed.with(format!("could not read {}: {e}", root.display()))
            })?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.is_file() && is_ggml_name(path))
            .collect();
        found.sort();

        match found.len() {
            0 => Err(ErrorCode::ModelLoadFailed.with(format!(
                "{} holds no ggml-*.bin weight file",
                root.display()
            ))),
            1 => {
                let path = found.remove(0);
                let name = model_name(&path).expect("is_ggml_name accepted it");
                Ok(GgmlModel {
                    quantisation: quantisation_of(&name),
                    name,
                    path,
                })
            }
            _ => {
                let names: Vec<String> = found
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                Err(ErrorCode::ModelLoadFailed.with(format!(
                    "{} holds {} weight files ({}), and picking one would put the \
                     wrong model's numbers in a labelled row",
                    root.display(),
                    names.len(),
                    names.join(", ")
                )))
            }
        }
    }
}

/// `ggml-<something>.bin`, and nothing else.
///
/// The `.mlmodelc.zip` Core ML encoders sit in the same published repository and
/// are not weight files; excluding by extension rather than by a deny-list is
/// what keeps that true when a new companion artifact appears.
fn is_ggml_name(path: &Path) -> bool {
    model_name(path).is_some()
}

fn model_name(path: &Path) -> Option<String> {
    let file = path.file_name()?.to_str()?;
    let stem = file.strip_prefix("ggml-")?.strip_suffix(".bin")?;
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_owned())
}

fn quantisation_of(name: &str) -> Quantisation {
    match name.rsplit_once('-').map(|(_, tail)| tail) {
        Some("q5_0") => Quantisation::Q5_0,
        Some("q8_0") => Quantisation::Q8_0,
        Some(tail) if tail.starts_with('q') && tail.contains('_') => Quantisation::Other,
        _ => Quantisation::Full,
    }
}

/// The language code whisper.cpp takes: ISO 639-1, or `auto`.
///
/// A `match` with no wildcard over [`VoiceLanguage`], so a ninth language is a
/// compile error rather than a session that quietly transcribes as English.
pub fn whisper_code(requested: Requested) -> &'static str {
    match requested {
        Requested::Auto => "auto",
        Requested::Fixed(language) => match language {
            VoiceLanguage::En => "en",
            VoiceLanguage::Es => "es",
            // Whisper has no Brazilian Portuguese; `pt` is the whole of its
            // Portuguese. Reported here so a `pt-BR` row is read as what it is.
            VoiceLanguage::PtBr => "pt",
            VoiceLanguage::De => "de",
            VoiceLanguage::Fr => "fr",
            VoiceLanguage::Ar => "ar",
            // Likewise no script distinction: whisper's `zh` emits whichever
            // script its decoder prefers, which is why the protocol normalises
            // Traditional to Simplified before scoring rather than asking the
            // model to promise one.
            VoiceLanguage::ZhHans => "zh",
            VoiceLanguage::Ja => "ja",
        },
    }
}

#[cfg(test)]
mod tests;
