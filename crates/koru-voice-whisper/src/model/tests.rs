// SPDX-License-Identifier: MPL-2.0

use super::*;

use std::fs;

fn dir_with(names: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    for name in names {
        fs::write(dir.path().join(name), b"not really ggml").expect("write a fixture file");
    }
    dir
}

#[test]
fn the_gates_candidate_resolves_and_reports_its_quantisation() {
    let dir = dir_with(&["ggml-large-v3-turbo-q5_0.bin"]);
    let model = GgmlModel::resolve(dir.path()).expect("one weight file resolves");
    assert_eq!(model.name, "large-v3-turbo-q5_0");
    assert_eq!(model.quantisation, Quantisation::Q5_0);
    assert_eq!(model.quantisation.as_str(), "q5_0");
}

#[test]
fn the_contingency_model_resolves_too() {
    // The protocol substitutes `medium` q5_0 if turbo cannot keep up on
    // `W-base`, and records the substitution in the run header. It has to be a
    // directory swap and nothing else.
    let dir = dir_with(&["ggml-medium-q5_0.bin"]);
    let model = GgmlModel::resolve(dir.path()).expect("medium resolves");
    assert_eq!(model.name, "medium-q5_0");
    assert_eq!(model.quantisation, Quantisation::Q5_0);
}

#[test]
fn an_unquantised_file_is_reported_as_full_precision() {
    let dir = dir_with(&["ggml-large-v3-turbo.bin"]);
    let model = GgmlModel::resolve(dir.path()).expect("resolves");
    assert_eq!(model.quantisation, Quantisation::Full);
    assert_eq!(model.quantisation.as_str(), "f16");
}

#[test]
fn a_quantisation_we_have_not_heard_of_is_carried_rather_than_refused() {
    // It is still a legal model. The report should print what it found; only a
    // *label* is at stake, and refusing would be the harness deciding which
    // models exist.
    let dir = dir_with(&["ggml-large-v3-turbo-q4_k.bin"]);
    let model = GgmlModel::resolve(dir.path()).expect("resolves");
    assert_eq!(model.quantisation, Quantisation::Other);
}

#[test]
fn the_core_ml_encoder_beside_the_weights_is_not_mistaken_for_one() {
    // `ggerganov/whisper.cpp` publishes `ggml-large-v3-turbo-encoder.mlmodelc.zip`
    // in the same repository. Filtering by extension rather than by a deny-list
    // is what keeps this true when a new companion artifact appears.
    let dir = dir_with(&[
        "ggml-large-v3-turbo-q5_0.bin",
        "ggml-large-v3-turbo-encoder.mlmodelc.zip",
    ]);
    let model = GgmlModel::resolve(dir.path()).expect("only the .bin counts");
    assert_eq!(model.name, "large-v3-turbo-q5_0");
}

#[test]
fn two_weight_files_are_refused_and_both_are_named() {
    // Picking one has two plausible rules and no right answer, and either would
    // attribute a `medium` result to a `turbo` row.
    let dir = dir_with(&["ggml-medium-q5_0.bin", "ggml-large-v3-turbo-q5_0.bin"]);
    let error = GgmlModel::resolve(dir.path()).expect_err("two weight files are refused");
    assert_eq!(error.code, koru_voice::ErrorCode::ModelLoadFailed);
    assert!(error.message.contains("ggml-medium-q5_0.bin"));
    assert!(error.message.contains("ggml-large-v3-turbo-q5_0.bin"));
}

#[test]
fn an_empty_directory_says_what_it_looked_for() {
    let dir = dir_with(&[]);
    let error = GgmlModel::resolve(dir.path()).expect_err("no weight file is refused");
    assert!(error.message.contains("ggml-*.bin"));
}

#[test]
fn a_path_that_is_not_a_directory_is_refused_first() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let error =
        GgmlModel::resolve(&dir.path().join("nope")).expect_err("a missing directory is refused");
    assert!(error.message.contains("is not a directory"));
}

#[test]
fn auto_is_spelled_auto_and_every_language_has_a_code() {
    assert_eq!(whisper_code(Requested::Auto), "auto");
    let mut codes: Vec<&str> = VoiceLanguage::ALL
        .iter()
        .map(|&l| whisper_code(Requested::Fixed(l)))
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, ["ar", "de", "en", "es", "fr", "ja", "pt", "zh"]);
    // And no language can produce the value that means "detect one".
    assert!(codes.iter().all(|&c| c != "auto"));
}

#[test]
fn the_two_codes_that_drop_a_subtag_drop_it_visibly() {
    // Whisper has no Brazilian Portuguese and no script distinction. Pinned so
    // Plan 0.4 reads a `pt-BR` or `zh-Hans` row as what it is.
    assert_eq!(whisper_code(Requested::Fixed(VoiceLanguage::PtBr)), "pt");
    assert_eq!(whisper_code(Requested::Fixed(VoiceLanguage::ZhHans)), "zh");
}
