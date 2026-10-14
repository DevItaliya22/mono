// SPDX-License-Identifier: MPL-2.0

use super::*;

use koru_voice::VoiceLanguage::*;

#[test]
fn case_and_punctuation_go_and_diacritics_stay() {
    assert_eq!(normalise("Hello, World!", En), "hello world");
    // Diacritics are orthography, not decoration: `añade` and `anade` are
    // different words and only one of them is what was said.
    assert_eq!(normalise("Añade una línea.", Es), "añade una línea");
}

#[test]
fn an_apostrophe_between_two_letters_survives_and_a_quote_does_not() {
    // *don't* and *dont* are different words; French elision is not
    // punctuation. This is the one exception in rule 3 and it is why the rule
    // cannot be a `retain`.
    assert_eq!(normalise("don't", En), "don't");
    assert_eq!(normalise("don\u{2019}t", En), "don't");
    assert_eq!(normalise("l'eau", Fr), "l'eau");
    assert_eq!(normalise("'quoted'", En), "quoted");
    assert_eq!(normalise("dogs' bowls", En), "dogs bowls");
}

#[test]
fn whitespace_collapses_for_latin_and_vanishes_for_cjk() {
    assert_eq!(normalise("a   b \t c", En), "a b c");
    // Chinese and Japanese reference text has no whitespace and a recogniser
    // may insert some.
    assert_eq!(normalise("打开 文档 并", ZhHans), "打开文档并");
    assert_eq!(normalise("書類 を 開いて", Ja), "書類を開いて");
}

#[test]
fn nfkc_folds_halfwidth_katakana_and_fullwidth_ascii() {
    // Rule 1 does most of the Japanese work on its own, which is why rule 5
    // for Japanese is "nothing else".
    assert_eq!(normalise("ｶﾀｶﾅ", Ja), normalise("カタカナ", Ja));
    assert_eq!(normalise("ＡＢＣ", En), "abc");
}

#[test]
fn japanese_does_not_convert_kana_to_kanji_or_the_other_way() {
    // A recogniser that writes かんじ where the reference has 漢字 has produced
    // text the person would have to correct, so for a dictation product that is
    // an error. This makes our Japanese CER pessimistic against published
    // numbers scored on kana readings, and that comparison is invalid.
    assert_ne!(normalise("かんじ", Ja), normalise("漢字", Ja));
}

#[test]
fn arabic_strips_the_harakat_and_folds_the_alef_variants() {
    // Harakat are optional in writing; a recogniser that emits them and a
    // reference that omits them are not disagreeing.
    assert_eq!(normalise("كَتَبَ", Ar), normalise("كتب", Ar));
    assert_eq!(normalise("أحمد", Ar), normalise("احمد", Ar));
    assert_eq!(normalise("إسلام", Ar), normalise("اسلام", Ar));
    assert_eq!(normalise("آمن", Ar), normalise("امن", Ar));
    // Tatweel is a stretching glyph, not a letter.
    assert_eq!(normalise("كــتــب", Ar), normalise("كتب", Ar));
}

#[test]
fn arabic_leaves_teh_marbuta_alone() {
    // **A decision, and it makes our Arabic WER slightly higher than a
    // paper's.** Merging `ة` with `ه` changes word identity, and a person
    // dictating would have to fix it.
    assert_ne!(normalise("مدرسة", Ar), normalise("مدرسه", Ar));
}

#[test]
fn arabic_punctuation_goes_with_the_rest() {
    assert_eq!(normalise("كتب، ثم قرأ؟", Ar), normalise("كتب ثم قرأ", Ar));
}

#[test]
fn cjk_punctuation_goes_too() {
    assert_eq!(normalise("打开文档。", ZhHans), "打开文档");
    assert_eq!(normalise("Koru、Loro", ZhHans), "koruloro");
}

#[test]
fn without_a_table_no_traditional_form_is_converted_and_that_is_reportable() {
    // The honest third option. A zh-Hans figure produced without the table
    // scores a model's Traditional output as wrong, which is a real difference
    // from the protocol's number and must not be invisible.
    let plain = Normaliser::without_t2s();
    assert!(!plain.has_t2s());
    assert_eq!(plain.normalise("開啟文檔", ZhHans), "開啟文檔");
}

#[test]
fn with_a_table_the_conversion_happens_and_the_longest_key_wins() {
    let table = T2sTable::parse("開\t开\n啟\t启\n文檔\t文档\n").expect("a table parses");
    let normaliser = Normaliser::new(table);
    assert!(normaliser.has_t2s());
    assert_eq!(normaliser.normalise("開啟文檔", ZhHans), "开启文档");
}

#[test]
fn the_five_latin_languages_share_one_body() {
    // Rule set 1-4 and nothing else: no stemming, no German compound
    // splitting, no lexicon.
    for language in [En, Es, PtBr, De, Fr] {
        assert_eq!(normalise("Hallo, Welt!", language), "hallo welt");
    }
    // German compounds are one word here and stay one word.
    assert_eq!(normalise("Donaudampfschiff", De), "donaudampfschiff");
}

#[test]
fn the_unit_a_language_is_scored_in_follows_its_whitespace() {
    for language in [En, Es, PtBr, De, Fr, Ar] {
        assert!(is_whitespace_separated(language), "{}", language.tag());
    }
    assert!(!is_whitespace_separated(ZhHans));
    assert!(!is_whitespace_separated(Ja));
}

#[test]
fn normalisation_is_idempotent() {
    // The same function is applied to the reference, to the hypothesis and to
    // every partial; one of those has usually been through it once already in a
    // caller's head. Running it twice must change nothing, or the prefix test
    // and the error rate would disagree about what a string is.
    for language in koru_voice::VoiceLanguage::ALL {
        for text in [
            "Hello, World! don't",
            "打开文档。",
            "كَتَبَ، ثم",
            "ＡＢＣ ｶﾀｶﾅ",
        ] {
            let once = normalise(text, language);
            let twice = normalise(&once, language);
            assert_eq!(once, twice, "{} over {text:?}", language.tag());
        }
    }
}

#[test]
fn an_emoji_is_a_symbol_and_goes() {
    // A recogniser will not emit one and a reference should not carry one, but
    // the category rule says S\* and this is the part of S\* a reader would
    // wonder about.
    assert_eq!(normalise("hello 👋 world", En), "hello world");
}
