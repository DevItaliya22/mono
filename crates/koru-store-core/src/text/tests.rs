// SPDX-License-Identifier: MPL-2.0

use super::*;

// ---------------------------------------------------------------- normalisation

#[test]
fn ascii_is_returned_unchanged() {
    assert_eq!(normalize_key("workspace"), "workspace");
}

#[test]
fn decomposed_and_composed_spellings_become_one_key() {
    let composed = "café"; // U+00E9
    let decomposed = "cafe\u{0301}"; // e + combining acute
    assert_ne!(composed, decomposed, "the fixture is wrong");
    assert_eq!(normalize_key(composed), normalize_key(decomposed));
    assert!(keys_equal(composed, decomposed));
}

#[test]
fn german_and_french_keys_compose() {
    assert!(keys_equal("Grüße", "Gru\u{0308}ße"));
    assert!(keys_equal("Élève", "E\u{0301}le\u{0300}ve"));
}

#[test]
fn korean_jamo_compose_to_syllables() {
    // NFC composes conjoining jamo. A different spelling of the same word must
    // not become a second row.
    let jamo = "\u{1100}\u{1161}"; // ᄀ + ᅡ
    assert_eq!(normalize_key(jamo), "가");
}

#[test]
fn normalisation_does_not_touch_case_or_width() {
    // If any of these ever change, someone has added case folding or width
    // folding to a function that must only compose.
    assert_eq!(normalize_key("Workspace"), "Workspace");
    assert_eq!(normalize_key("ＷＯＲＫ"), "ＷＯＲＫ");
    assert_eq!(normalize_key("ｶﾀｶﾅ"), "ｶﾀｶﾅ");
}

#[test]
fn arabic_and_cjk_survive_untouched() {
    for s in [
        "مساحة العمل",
        "工作区",
        "ワークスペース",
        "Área de trabalho",
    ] {
        assert_eq!(normalize_key(s), s.nfc().collect::<String>());
    }
}

#[test]
fn bidi_controls_are_preserved_not_stripped() {
    // Direction is a rendering decision. The store neither strips nor inserts.
    let mixed = "مرحبا \u{202B}Koru\u{202C} بالعالم";
    assert!(normalize_key(mixed).contains('\u{202B}'));
    assert!(normalize_key(mixed).contains('\u{202C}'));
}

// ---------------------------------------------------------------- locale

#[test]
fn every_supported_language_parses() {
    for tag in ["en", "es", "pt-BR", "de", "fr", "ar", "zh-Hans", "ja"] {
        assert!(Locale::parse(tag).is_some(), "{tag} did not parse");
    }
}

#[test]
fn a_tag_is_stored_in_its_conventional_spelling() {
    assert_eq!(Locale::parse("PT-br").unwrap().as_str(), "pt-BR");
    assert_eq!(Locale::parse("zh-HANS").unwrap().as_str(), "zh-Hans");
    assert_eq!(Locale::parse("EN").unwrap().as_str(), "en");
    assert_eq!(Locale::parse("es-419").unwrap().as_str(), "es-419");
}

#[test]
fn the_primary_language_is_reachable() {
    assert_eq!(Locale::parse("pt-BR").unwrap().language(), "pt");
    assert_eq!(Locale::parse("zh-Hans").unwrap().language(), "zh");
    assert_eq!(Locale::parse("ja").unwrap().language(), "ja");
}

#[test]
fn rubbish_is_rejected() {
    for tag in [
        "",
        " ",
        "-",
        "e",
        "en-",
        "en--US",
        "en_US",
        "1234",
        "en-ünicode",
    ] {
        assert!(Locale::parse(tag).is_none(), "{tag:?} should not parse");
    }
}

#[test]
fn an_unknown_but_well_formed_tag_is_accepted() {
    // Structure, not a registry. Shipping a stale registry would lock someone
    // out of their own language.
    assert!(Locale::parse("tlh-Piqd").is_some());
}

#[test]
fn the_default_is_undetermined_not_english() {
    assert_eq!(Locale::default().as_str(), "und");
}

#[test]
fn a_locale_round_trips_through_serde() {
    let l = Locale::parse("pt-BR").unwrap();
    let json = serde_json::to_string(&l).unwrap();
    assert_eq!(json, "\"pt-BR\"");
    assert_eq!(serde_json::from_str::<Locale>(&json).unwrap(), l);
    assert!(serde_json::from_str::<Locale>("\"Portugues\"").is_err());
}
