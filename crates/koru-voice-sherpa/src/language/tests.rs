// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn auto_is_a_value_here_and_is_still_not_a_language() {
    assert_eq!(option_value(Requested::Auto), "auto");
    // The whole reason `koru-voice` has two types. `VoiceLanguage` cannot hold
    // `auto`, so no `subtag` can accidentally produce it.
    assert!(VoiceLanguage::ALL.iter().all(|&l| subtag(l) != AUTO));
}

#[test]
fn every_language_has_a_subtag_and_they_are_distinct_except_by_design() {
    let mut tags: Vec<&str> = VoiceLanguage::ALL.iter().map(|&l| subtag(l)).collect();
    tags.sort_unstable();
    assert_eq!(tags, ["ar", "de", "en", "es", "fr", "ja", "pt", "zh"]);
}

#[test]
fn the_two_that_lose_information_lose_it_deliberately() {
    // `pt-BR` becomes `pt` and `zh-Hans` becomes `zh`, because the prompt
    // dictionary is keyed by language rather than by variety or script. Pinned
    // so Plan 0.4 cannot read a Brazilian result out of a prompt that only ever
    // said Portuguese.
    assert_eq!(subtag(VoiceLanguage::PtBr), "pt");
    assert_ne!(subtag(VoiceLanguage::PtBr), VoiceLanguage::PtBr.tag());
    assert_eq!(subtag(VoiceLanguage::ZhHans), "zh");
    assert_ne!(subtag(VoiceLanguage::ZhHans), VoiceLanguage::ZhHans.tag());
}

#[test]
fn the_other_six_are_their_own_bcp_47_tag() {
    for language in [
        VoiceLanguage::En,
        VoiceLanguage::Es,
        VoiceLanguage::De,
        VoiceLanguage::Fr,
        VoiceLanguage::Ar,
        VoiceLanguage::Ja,
    ] {
        assert_eq!(subtag(language), language.tag());
    }
}

#[test]
fn the_option_key_is_sherpas_own_spelling() {
    // From the C API's own documentation comment:
    // `SherpaOnnxOfflineStreamSetOption(stream, "language", "en")`.
    assert_eq!(OPTION_KEY, "language");
}
