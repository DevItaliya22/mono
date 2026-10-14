// SPDX-License-Identifier: MPL-2.0

use super::*;

use koru_voice::VoiceLanguage::*;

#[test]
fn levenshtein_counts_the_three_edit_kinds() {
    let a: Vec<char> = "kitten".chars().collect();
    let b: Vec<char> = "sitting".chars().collect();
    assert_eq!(levenshtein(&a, &b), 3);
    assert_eq!(levenshtein::<char>(&[], &b), 7);
    assert_eq!(levenshtein(&a, &[]), 6);
}

#[test]
fn a_word_error_rate_counts_words_and_a_character_one_counts_graphemes() {
    let words = score("open the document", "open a document", Unit::Word);
    assert_eq!(words.edits, 1);
    assert_eq!(words.reference_units, 3);

    let graphemes = score("打开文档", "打開文档", Unit::Grapheme);
    assert_eq!(graphemes.edits, 1);
    assert_eq!(graphemes.reference_units, 4);
}

#[test]
fn a_combining_sequence_is_one_grapheme_and_not_several_edits() {
    // Counting `char`s would score this as several edits, which is the exact
    // confusion the protocol's "extended grapheme clusters" sentence prevents.
    let combined = "e\u{0301}"; // e + combining acute
    assert_eq!(units(combined, Unit::Grapheme).len(), 1);
}

#[test]
fn the_corpus_rate_is_total_edits_over_total_units_not_a_mean_of_rates() {
    // A mean of rates lets a three-word utterance weigh as much as a
    // thirty-word one, which is not what anybody means by "the error rate on
    // this corpus".
    let counts = [
        ErrorCount {
            edits: 3,
            reference_units: 3,
        }, // 100%
        ErrorCount {
            edits: 0,
            reference_units: 97,
        }, // 0%
    ];
    let corpus = corpus_rate(&counts).expect("there are units");
    assert!((corpus - 0.03).abs() < 1e-12, "{corpus}");
    // The mean of the per-utterance rates would be 50%.
    assert!(corpus < 0.5);
}

#[test]
fn a_rate_with_no_reference_units_is_absent_rather_than_zero() {
    // Printing 0.0% for an empty corpus is how a broken sample looks like a
    // perfect score.
    assert_eq!(corpus_rate(&[]), None);
    assert_eq!(
        corpus_rate(&[ErrorCount {
            edits: 0,
            reference_units: 0
        }]),
        None
    );
}

#[test]
fn a_partial_must_end_on_a_token_boundary_in_the_whitespace_languages() {
    // Without this, "Creat" counts as a useful prefix of "Create a".
    assert!(!qualifies("creat", "create a document", En));
    assert!(qualifies("create a", "create a document", En));
    assert!(qualifies("create a document", "create a document", En));
}

#[test]
fn the_grapheme_minimum_is_per_language_and_is_about_useful_text() {
    // N characters is a word in English and a sentence in Chinese; the target
    // is the same amount of useful text, roughly two words.
    assert_eq!(minimum_graphemes(En), 8);
    assert_eq!(minimum_graphemes(Ar), 6);
    assert_eq!(minimum_graphemes(Ja), 4);
    assert_eq!(minimum_graphemes(ZhHans), 3);

    // Three Chinese characters qualify; seven English ones do not.
    assert!(qualifies("打开文", "打开文档并添加", ZhHans));
    assert!(!qualifies("打开", "打开文档并添加", ZhHans));
    assert!(!qualifies("create", "create a document", En));
}

#[test]
fn a_partial_that_is_not_a_prefix_never_qualifies() {
    assert!(!qualifies("open a doc", "create a document", En));
    assert!(!qualifies(
        "create a document and more",
        "create a document",
        En
    ));
}

#[test]
fn a_revision_is_a_partial_that_did_not_extend_its_predecessor() {
    assert!(extends("create", "create a"));
    assert!(extends("create", "create"));
    assert!(!extends("create a", "create"));
    // "creat" *is* a grapheme prefix of "create a", so it extends — which is
    // exactly why `extends` is not `qualifies`: a prefix that stops mid-word is
    // a legal extension and is not a useful partial.
    assert!(extends("creat", "create a"));
    assert!(!extends("open the", "create a"));

    let partials: Vec<String> = ["create", "create a", "make a", "make a doc"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(revisions(&partials), 1);
}

#[test]
fn a_stream_that_only_ever_grows_records_no_revisions() {
    let partials: Vec<String> = ["op", "open", "open the", "open the doc"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(revisions(&partials), 0);
}

#[test]
fn the_unit_and_the_metric_name_follow_the_language() {
    for language in [En, Es, PtBr, De, Fr, Ar] {
        assert_eq!(Unit::of(language), Unit::Word);
        assert_eq!(Unit::of(language).metric(), "WER");
    }
    for language in [ZhHans, Ja] {
        assert_eq!(Unit::of(language), Unit::Grapheme);
        assert_eq!(Unit::of(language).metric(), "CER");
    }
}

#[test]
fn a_prefix_carries_a_word_count_even_where_the_band_does_not_apply() {
    // Reported rather than silently skipped: a missing cell is a question and a
    // 0.0 is an answer.
    assert_eq!(words_in("create a"), 2);
    assert_eq!(words_in("打开文档"), 1);
    assert_eq!(words_in(""), 0);
}
