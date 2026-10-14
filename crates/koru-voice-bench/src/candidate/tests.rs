// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn the_table_is_the_protocols_four() {
    assert_eq!(CANDIDATES.len(), 4);
    let numbers: Vec<u8> = CANDIDATES.iter().map(|c| c.number).collect();
    assert_eq!(numbers, [1, 2, 3, 4]);
}

#[test]
fn candidates_one_and_two_are_two_model_directories() {
    // **The single most likely thing to get wrong**, and it would show up as a
    // silent 560 ms result in a row labelled 160 ms. The chunk size is fixed at
    // ONNX export, so the two candidates differ by which directory is opened
    // and by nothing else.
    let one = Candidate::by_number(1).expect("candidate 1");
    let two = Candidate::by_number(2).expect("candidate 2");
    assert_ne!(one.model_dir_name, two.model_dir_name);
    assert!(one.model_dir_name.contains("160ms"));
    assert!(two.model_dir_name.contains("560ms"));
    assert_eq!(one.api, two.api);
    assert_eq!(one.runtime, two.runtime);
}

#[test]
fn every_model_id_is_a_legal_path_component() {
    // Plan 1.2 stores installed bytes under `<id>/<version>/<digest>/`, and a
    // catalog entry named `../../etc` must never become a directory.
    for candidate in &CANDIDATES {
        koru_voice::ModelId::parse(candidate.model_id)
            .unwrap_or_else(|e| panic!("{}: {e}", candidate.model_id));
    }
}

#[test]
fn only_the_offline_candidates_run_our_policy() {
    // Partials from a streaming recogniser are the recogniser's; partials from
    // an offline one exist only because of a policy we chose, and a poor
    // latency number for those two is evidence about the policy.
    assert!(!Candidate::by_number(1).unwrap().our_policy);
    assert!(!Candidate::by_number(2).unwrap().our_policy);
    assert!(Candidate::by_number(3).unwrap().our_policy);
    assert!(Candidate::by_number(4).unwrap().our_policy);
}

#[test]
fn h_is_declared_once_and_only_the_offline_candidates_carry_it() {
    assert_eq!(REDECODE_INTERVAL, Duration::from_millis(500));
    assert_eq!(Candidate::by_number(1).unwrap().partial_every(), None);
    assert_eq!(
        Candidate::by_number(3).unwrap().partial_every(),
        Some(REDECODE_INTERVAL)
    );
    assert_eq!(
        Candidate::by_number(4).unwrap().partial_every(),
        Candidate::by_number(3).unwrap().partial_every(),
        "the two offline candidates must take one number from one place, or \
         they cannot be compared to each other"
    );
}

#[test]
fn no_runtime_is_compiled_into_an_ordinary_build() {
    // The build-script trap, at the level this crate can see it. With neither
    // feature on, nothing links sherpa-onnx or whisper.cpp — so no build script
    // of theirs runs and none of them reaches the network.
    assert_eq!(Runtime::Sherpa.is_compiled_in(), cfg!(feature = "sherpa"));
    assert_eq!(Runtime::Whisper.is_compiled_in(), cfg!(feature = "whisper"));
}

#[test]
fn a_candidate_whose_runtime_is_absent_says_which_feature_would_build_it() {
    let dir = tempfile::tempdir().expect("a temp dir");
    for candidate in &CANDIDATES {
        // `expect_err` would need `Debug` on `dyn SpeechEngine`, which the
        // trait deliberately does not require — an engine is a native handle.
        let Err(reason) = open(candidate, dir.path(), 2) else {
            panic!("{} was built, but nothing is provisioned", candidate.name);
        };
        let text = reason.to_string();
        // **Both facts, always.** Nothing is provisioned here, so the message
        // has to name the missing model directory whether or not the runtime
        // was compiled in — otherwise somebody rebuilds with a feature and
        // discovers the second missing thing an hour later.
        assert!(text.contains("model missing"), "{text}");
        assert!(text.contains(candidate.model_dir_name), "{text}");
        if !candidate.runtime.is_compiled_in() {
            assert!(text.contains("runtime not built"), "{text}");
            assert!(text.contains(candidate.runtime.feature()), "{text}");
        }
    }
}

#[test]
fn a_model_directory_is_named_under_the_models_root() {
    let candidate = Candidate::by_number(3).unwrap();
    let dir = candidate.model_dir(std::path::Path::new("/models"));
    assert!(dir.ends_with(candidate.model_dir_name));
}

#[test]
fn the_whisper_contingency_is_a_directory_swap_and_nothing_else() {
    // The protocol substitutes `medium` q5_0 if turbo cannot keep up on
    // `W-base`, and records the substitution in the run header.
    assert_eq!(WHISPER_CONTINGENCY_DIR, "whisper-medium-q5_0");
    assert_ne!(
        WHISPER_CONTINGENCY_DIR,
        Candidate::by_number(4).unwrap().model_dir_name
    );
}
