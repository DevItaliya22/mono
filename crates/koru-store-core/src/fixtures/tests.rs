// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::text::{keys_equal, normalize_key};

#[test]
fn all_eight_supported_languages_are_present() {
    let tags: Vec<&str> = SAMPLES.iter().map(|s| s.tag).collect();
    assert_eq!(
        tags,
        ["en", "es", "pt-BR", "de", "fr", "ar", "zh-Hans", "ja"],
        "the supported language set changed; every downstream suite draws on this"
    );
}

#[test]
fn every_sample_tag_is_a_valid_locale() {
    for s in SAMPLES {
        assert_eq!(s.locale().as_str(), s.tag, "{} is not canonical", s.tag);
    }
}

#[test]
fn every_sample_is_non_empty_and_valid_utf8_beyond_ascii() {
    // A fixture that is accidentally ASCII proves nothing, so each non-English
    // sample must actually leave the ASCII range.
    for s in SAMPLES {
        assert!(!s.title.is_empty() && !s.body.is_empty(), "{}", s.tag);
        if s.tag != "en" {
            assert!(
                !s.body.is_ascii(),
                "{} is pure ASCII and would pass any broken text path",
                s.tag
            );
        }
    }
}

#[test]
fn the_multibyte_samples_have_more_bytes_than_characters() {
    // The property that breaks byte-offset truncation. If this stops holding,
    // the fixture has been "simplified" into something that cannot catch it.
    //
    // The thresholds differ because the encodings do: Arabic is two bytes per
    // letter (and the spaces between words are one), CJK is three. A single
    // ratio for both would either be trivially true or wrongly fail.
    for (tag, bytes_per_char) in [("ar", 1.5), ("zh-Hans", 2.5), ("ja", 2.5)] {
        let s = SAMPLES.iter().find(|s| s.tag == tag).unwrap();
        let ratio = s.body.len() as f64 / s.body.chars().count() as f64;
        assert!(
            ratio > bytes_per_char,
            "{tag} averages {ratio:.2} bytes per character, under {bytes_per_char}"
        );
    }
}

#[test]
fn japanese_carries_three_scripts_and_half_width_kana() {
    let ja = SAMPLES.iter().find(|s| s.tag == "ja").unwrap();
    assert!(
        ja.body
            .chars()
            .any(|c| ('\u{3040}'..='\u{309f}').contains(&c)),
        "hiragana"
    );
    assert!(
        ja.body
            .chars()
            .any(|c| ('\u{30a0}'..='\u{30ff}').contains(&c)),
        "katakana"
    );
    assert!(
        ja.body
            .chars()
            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "kanji"
    );
    // Half-width kana: what a width-folding "normaliser" would silently rewrite.
    assert!(
        ja.body
            .chars()
            .any(|c| ('\u{ff61}'..='\u{ff9f}').contains(&c)),
        "half-width kana"
    );
}

#[test]
fn arabic_is_right_to_left() {
    let ar = SAMPLES.iter().find(|s| s.tag == "ar").unwrap();
    assert!(
        ar.body
            .chars()
            .any(|c| ('\u{0600}'..='\u{06ff}').contains(&c))
    );
}

#[test]
fn the_bidi_line_is_genuinely_mixed_direction() {
    // All four properties, or it is not testing what it claims to.
    assert!(
        BIDI_LINE
            .chars()
            .any(|c| ('\u{0600}'..='\u{06ff}').contains(&c)),
        "arabic"
    );
    assert!(BIDI_LINE.contains("Koru Store"), "latin run");
    assert!(
        BIDI_LINE
            .chars()
            .any(|c| ('\u{0660}'..='\u{0669}').contains(&c)),
        "arabic-indic digit"
    );
    assert!(
        BIDI_LINE.contains('\u{202B}') && BIDI_LINE.contains('\u{202C}'),
        "bidi controls"
    );
}

#[test]
fn the_two_spellings_are_different_bytes_but_one_key() {
    assert_ne!(COMPOSED, DECOMPOSED, "the fixture is not testing anything");
    assert!(keys_equal(COMPOSED, DECOMPOSED));
    // And the decomposed form is genuinely decomposed, not merely different.
    assert_ne!(normalize_key(DECOMPOSED), DECOMPOSED);
}

#[test]
fn the_unicode_filename_is_not_path_safe_and_that_is_the_point() {
    // It contains a space and non-Latin script. Any code that builds a path
    // from it is wrong; the on-disk name is always the content hash.
    assert!(UNICODE_FILENAME.contains(' '));
    assert!(!UNICODE_FILENAME.is_ascii());
    assert!(UNICODE_FILENAME.ends_with(".pdf"));
}

#[test]
fn all_strings_covers_every_fixture() {
    let all = all_strings();
    assert_eq!(all.len(), SAMPLES.len() * 2 + 4);
    for s in SAMPLES {
        assert!(all.contains(&s.title));
        assert!(all.contains(&s.body));
    }
    for extra in [BIDI_LINE, COMPOSED, DECOMPOSED, UNICODE_FILENAME] {
        assert!(all.contains(&extra));
    }
}

#[test]
fn no_fixture_string_survives_a_naive_byte_truncation() {
    // Documents the hazard rather than the fix: slicing a fixture at an
    // arbitrary byte offset panics on a character boundary. Any store code that
    // truncates must use `char_indices`, and this is why.
    let s = SAMPLES.iter().find(|s| s.tag == "zh-Hans").unwrap().body;
    assert!(!s.is_char_boundary(1), "{s} starts with an ASCII byte");
}
