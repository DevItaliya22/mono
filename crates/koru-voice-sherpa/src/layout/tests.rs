// SPDX-License-Identifier: MPL-2.0

use super::*;

use std::fs;

/// Write a package with the given file names into a fresh directory.
fn package(names: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    for name in names {
        fs::write(dir.path().join(name), b"not really onnx").expect("write a fixture file");
    }
    dir
}

const INT8: [&str; 4] = [
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];
const FULL: [&str; 4] = ["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"];

#[test]
fn an_int8_package_resolves_and_says_so() {
    let dir = package(&INT8);
    let layout = OnlineLayout::resolve(dir.path()).expect("a complete int8 package resolves");
    assert_eq!(layout.precision, Precision::Int8);
    assert_eq!(layout.encoder, dir.path().join("encoder.int8.onnx"));
    assert_eq!(layout.tokens, dir.path().join("tokens.txt"));
}

#[test]
fn int8_wins_when_both_are_present() {
    // The published packages carry both, and the gate measures int8. Preferring
    // the smaller one is what makes a row labelled `int8` true without anybody
    // deleting files by hand.
    let mut both: Vec<&str> = INT8.to_vec();
    both.extend_from_slice(&FULL);
    let dir = package(&both);
    let layout = OfflineLayout::resolve(dir.path()).expect("both present resolves");
    assert_eq!(layout.precision, Precision::Int8);
}

#[test]
fn a_full_precision_package_resolves_as_fp32() {
    let dir = package(&FULL);
    let layout = OnlineLayout::resolve(dir.path()).expect("a complete fp32 package resolves");
    assert_eq!(layout.precision, Precision::Full);
    assert_eq!(layout.precision.as_str(), "fp32");
}

#[test]
fn a_half_extracted_package_names_the_file_that_is_missing() {
    // "the model would not load" is not something anybody can act on.
    // `OnlineRecognizer::create` answers `Option`, so this is the only place the
    // difference between a missing file and a corrupt graph still exists.
    let dir = package(&["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"]);
    let error = OnlineLayout::resolve(dir.path()).expect_err("a joiner-less package is refused");
    assert_eq!(error.code, koru_voice::ErrorCode::ModelLoadFailed);
    assert!(
        error.message.contains("joiner.int8.onnx"),
        "the message must name the file: {}",
        error.message
    );
}

#[test]
fn a_mixed_precision_package_is_refused_rather_than_paired() {
    // An `int8` encoder beside a full-precision decoder is a half-extracted
    // archive. Pairing them silently would produce a recogniser that runs and
    // is wrong, which is worse than one that does not start.
    let dir = package(&[
        "encoder.int8.onnx",
        "decoder.onnx",
        "joiner.onnx",
        "tokens.txt",
    ]);
    let error = OfflineLayout::resolve(dir.path()).expect_err("a mixed package is refused");
    assert!(
        error.message.contains("decoder.int8.onnx"),
        "the message must name the file the int8 package is missing: {}",
        error.message
    );
}

#[test]
fn a_package_with_no_encoder_at_all_says_which_two_names_it_looked_for() {
    let dir = package(&["tokens.txt"]);
    let error = OnlineLayout::resolve(dir.path()).expect_err("no encoder is refused");
    assert!(error.message.contains("encoder.int8.onnx"));
    assert!(error.message.contains("encoder.onnx"));
}

#[test]
fn a_path_that_is_not_a_directory_is_refused_before_anything_is_opened() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let missing = dir.path().join("no-such-model");
    let error = OnlineLayout::resolve(&missing).expect_err("a missing directory is refused");
    assert_eq!(error.code, koru_voice::ErrorCode::ModelLoadFailed);
    assert!(error.message.contains("is not a directory"));
}

#[test]
fn a_utf8_path_converts_and_a_non_utf8_one_would_not() {
    let dir = package(&INT8);
    let layout = OnlineLayout::resolve(dir.path()).expect("resolves");
    let converted = as_c_string(&layout.tokens).expect("a temp path is UTF-8");
    assert!(converted.ends_with("tokens.txt"));
    // The failing half cannot be constructed portably — Windows and Unix
    // disagree about what an invalid path even is — so what is pinned is that
    // the conversion is fallible at all, rather than a `to_string_lossy` that
    // would hand a *different* path to a native library and get back `None`.
    fn _is_fallible(p: &std::path::Path) -> Result<String, koru_voice::EngineError> {
        as_c_string(p)
    }
}
