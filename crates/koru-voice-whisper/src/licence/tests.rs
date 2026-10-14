// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn nothing_we_would_redistribute_blocks_shipping() {
    // Q-V5's whole question, as an assertion. If this ever fails, Whisper is
    // usable in the benchmark — a local evaluation redistributes nothing — and
    // not usable as a product fallback, and ADR-253's fallback choice becomes a
    // second sherpa model or none.
    assert!(may_be_shipped());
}

#[test]
fn exactly_one_artifact_is_redistributed_and_it_is_the_weight_file() {
    // The runtime and the bindings are *linked*, not hosted. Only a file Koru
    // would serve and sign can block packaging, and there is one of those.
    let hosted: Vec<&str> = FACTS
        .iter()
        .filter(|f| f.redistributed)
        .map(|f| f.artifact)
        .collect();
    assert_eq!(
        hosted,
        ["ggerganov/whisper.cpp ggml-large-v3-turbo-q5_0.bin"]
    );
}

#[test]
fn every_fact_names_where_it_was_checked() {
    // A licence table without sources is an assertion; with them it is
    // something a second person can repeat. That is the difference between
    // this module and a sentence in a roadmap.
    for fact in &FACTS {
        assert!(!fact.source.is_empty(), "{} has no source", fact.artifact);
        assert!(!fact.spdx.is_empty(), "{} has no licence", fact.artifact);
    }
}

#[test]
fn the_unlicense_rows_are_the_bindings_and_only_the_bindings() {
    // Unlicense is a public-domain dedication rather than a permissive licence,
    // and some policies treat the two differently. Pinned so that a future
    // dependency arriving under it is a visible change rather than a quiet one.
    let unlicensed: Vec<&str> = FACTS
        .iter()
        .filter(|f| f.spdx == "Unlicense")
        .map(|f| f.artifact)
        .collect();
    assert_eq!(unlicensed, ["whisper-rs", "whisper-rs-sys"]);
    assert!(
        FACTS
            .iter()
            .filter(|f| f.spdx == "Unlicense")
            .all(|f| !f.redistributed)
    );
}
