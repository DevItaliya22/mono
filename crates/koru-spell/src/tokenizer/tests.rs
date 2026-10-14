// SPDX-License-Identifier: MPL-2.0

use super::*;

fn ranges(text: &str, exclusions: &[Range<usize>]) -> Vec<(String, Range<usize>)> {
    words(text, exclusions)
        .into_iter()
        .map(|token| (token.text.to_string(), token.range))
        .collect()
}

#[test]
fn ranges_are_source_bytes_and_contractions_stay_whole() {
    let text = "This isn't Steven's test-file.";
    let tokens = ranges(text, &[]);
    assert_eq!(
        tokens,
        vec![
            ("This".into(), 0..4),
            ("isn't".into(), 5..10),
            ("Steven's".into(), 11..19),
            ("test".into(), 20..24),
            ("file".into(), 25..29),
        ]
    );
    // Every range indexes the source exactly.
    for (word, range) in &tokens {
        assert_eq!(&text[range.clone()], word);
    }
}

#[test]
fn the_typographic_apostrophe_folds_in_check_but_not_in_the_range() {
    let text = "isn\u{2019}t";
    let tokens = words(text, &[]);
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].range, 0..text.len());
    assert_eq!(tokens[0].text, text);
    assert_eq!(tokens[0].check.as_ref(), "isn't");
    assert!(matches!(tokens[0].check, std::borrow::Cow::Owned(_)));
}

#[test]
fn plain_words_borrow_their_check_value() {
    let tokens = words("plain", &[]);
    assert!(matches!(tokens[0].check, std::borrow::Cow::Borrowed(_)));
}

#[test]
fn identifiers_numbers_and_domains_are_not_prose() {
    assert_eq!(ranges("foo_bar 42 4f7c2a9e IPv6", &[]), vec![]);
    assert_eq!(ranges("getDocumentById McDonald", &[]), vec![]);
    assert_eq!(ranges("api.koru.do e.g", &[]), vec![]);
}

#[test]
fn an_excluded_range_swallows_every_overlapping_token() {
    let text = "keep skip keep";
    let tokens = ranges(text, &[5..9]);
    assert_eq!(tokens, vec![("keep".into(), 0..4), ("keep".into(), 10..14)]);
    // Partial overlap excludes too — a token half inside code is not prose.
    assert_eq!(ranges(text, &[7..12]), vec![("keep".into(), 0..4)]);
}

#[test]
fn the_pill_anchor_cuts_words_apart() {
    // U+2060 is a Format character UAX #29 would ignore, welding the two
    // sides into one token; the anchor split is what keeps them apart.
    let text = "teh\u{2060}next";
    let tokens = ranges(text, &[]);
    assert_eq!(tokens, vec![("teh".into(), 0..3), ("next".into(), 6..10)]);
}

#[test]
fn punctuation_and_emoji_yield_no_tokens() {
    assert_eq!(ranges("— … 🎉 ()", &[]), vec![]);
}
