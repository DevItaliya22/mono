// SPDX-License-Identifier: MPL-2.0

//! **What a sherpa model directory has to contain, resolved without loading
//! one.**
//!
//! Both published packages this crate targets are transducers and carry the
//! same four names — three ONNX graphs and a token table — so the layout is one
//! shape with two callers rather than a shape per model:
//!
//! ```text
//! sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11/
//!   encoder.int8.onnx   encoder.onnx   encoder.data
//!   decoder.int8.onnx   decoder.onnx
//!   joiner.int8.onnx    joiner.onnx
//!   tokens.txt
//! ```
//!
//! Resolution is here, outside the `runtime` feature, for two reasons. It is the
//! part that fails on a real person's machine — a half-extracted archive, a
//! nested directory, an `fp32` package where an `int8` one was expected — and it
//! is the part a native call cannot report usefully: `OnlineRecognizer::create`
//! answers `Option`, so a missing file and a corrupt graph arrive as the same
//! `None`. Checking first is what turns that into
//! [`ErrorCode::ModelLoadFailed`] with the name of the file that is absent.
//!
//! ## Precision is chosen by what is on disk, and it is reported
//!
//! An `int8` package and an `fp32` one differ only in file names, and the gate
//! measures `int8`. So [`OnlineLayout::resolve`] prefers `*.int8.onnx`, falls
//! back to the plain names, and records which it took in
//! [`Precision`] — because a candidate row that says `int8` while the directory
//! held `fp32` is a wrong number rather than a missing one.

use std::path::{Path, PathBuf};

use koru_voice::{EngineError, ErrorCode};

/// Which set of graphs was found.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Precision {
    /// `encoder.int8.onnx` and friends — what the gate measures.
    Int8,
    /// `encoder.onnx` and friends. Correct, larger and slower; reported so a
    /// table cannot silently claim the other one.
    Full,
}

impl Precision {
    pub fn as_str(self) -> &'static str {
        match self {
            Precision::Int8 => "int8",
            Precision::Full => "fp32",
        }
    }
}

/// A resolved streaming transducer package: encoder, decoder, joiner, tokens.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OnlineLayout {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
    pub precision: Precision,
}

/// A resolved offline transducer package. The same four files; a separate type
/// because the two are handed to different sherpa APIs and a mix-up would
/// compile.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OfflineLayout {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
    pub precision: Precision,
}

impl OnlineLayout {
    /// Resolve inside `root`, preferring the `int8` graphs.
    pub fn resolve(root: &Path) -> Result<OnlineLayout, EngineError> {
        let (encoder, decoder, joiner, tokens, precision) = resolve_transducer(root)?;
        Ok(OnlineLayout {
            encoder,
            decoder,
            joiner,
            tokens,
            precision,
        })
    }
}

impl OfflineLayout {
    /// Resolve inside `root`, preferring the `int8` graphs.
    pub fn resolve(root: &Path) -> Result<OfflineLayout, EngineError> {
        let (encoder, decoder, joiner, tokens, precision) = resolve_transducer(root)?;
        Ok(OfflineLayout {
            encoder,
            decoder,
            joiner,
            tokens,
            precision,
        })
    }
}

/// The shared body. Answers `ModelLoadFailed` naming the first file that is
/// missing, because "the model would not load" is not a thing anybody can act
/// on.
fn resolve_transducer(
    root: &Path,
) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf, Precision), EngineError> {
    if !root.is_dir() {
        return Err(ErrorCode::ModelLoadFailed.with(format!(
            "{} is not a directory, so it holds no sherpa model",
            root.display()
        )));
    }

    // The precision is decided by the *encoder*, and then the other two are
    // required to match. A directory carrying `encoder.int8.onnx` beside
    // `decoder.onnx` is a half-extracted archive, and quietly pairing them
    // would produce a recogniser that runs and is wrong.
    let precision = if root.join("encoder.int8.onnx").is_file() {
        Precision::Int8
    } else if root.join("encoder.onnx").is_file() {
        Precision::Full
    } else {
        return Err(ErrorCode::ModelLoadFailed.with(format!(
            "{} has neither encoder.int8.onnx nor encoder.onnx",
            root.display()
        )));
    };

    let graph = |stem: &str| -> PathBuf {
        match precision {
            Precision::Int8 => root.join(format!("{stem}.int8.onnx")),
            Precision::Full => root.join(format!("{stem}.onnx")),
        }
    };

    let encoder = graph("encoder");
    let decoder = graph("decoder");
    let joiner = graph("joiner");
    let tokens = root.join("tokens.txt");

    for path in [&encoder, &decoder, &joiner, &tokens] {
        if !path.is_file() {
            return Err(ErrorCode::ModelLoadFailed.with(format!(
                "{} is missing from the {} model in {}",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                precision.as_str(),
                root.display()
            )));
        }
    }

    Ok((encoder, decoder, joiner, tokens, precision))
}

/// A path as sherpa wants it: a `String`, not a `Path`.
///
/// The C API takes `const char *`, so a path that is not valid UTF-8 cannot be
/// handed over at all. Refusing here with a code somebody can read beats
/// `to_string_lossy`, which would pass a *different* path to a native library
/// and get back `None`.
pub fn as_c_string(path: &Path) -> Result<String, EngineError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        ErrorCode::ModelLoadFailed.with(format!(
            "{} is not valid UTF-8, and the sherpa C API takes no other kind of path",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests;
