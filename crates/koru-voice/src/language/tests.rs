// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The plan's own test, and it is only satisfiable because `Auto` is not in
/// this enum: every language has a tag, and every tag comes back.
#[test]
fn every_language_round_trips_through_bcp_47() {
    for language in VoiceLanguage::ALL {
        let tag = language.tag();
        assert!(!tag.is_empty(), "{language:?} has an empty tag");
        assert_eq!(
            VoiceLanguage::matching(tag),
            Some(language),
            "{tag} did not come back as {language:?}"
        );
    }
}

/// The other half of the same statement, written as an assertion rather than
/// left as an absence: `Auto` has no tag, and nothing may invent one for it.
#[test]
fn auto_has_no_tag() {
    assert_eq!(Requested::Auto.tag(), None);
    assert_eq!(Requested::Auto.fixed(), None);
    for language in VoiceLanguage::ALL {
        assert_eq!(Requested::Fixed(language).tag(), Some(language.tag()));
    }
}

/// The wire spelling and the displayed spelling are one string. Without this a
/// serde rename could drift from `koru_i18n::Lang::tag` and a worker would
/// answer `zh_hans` to a client that asked about `zh-Hans`.
#[test]
fn the_serde_spelling_is_the_bcp_47_tag() {
    for language in VoiceLanguage::ALL {
        let json = serde_json::to_string(&language).unwrap();
        assert_eq!(json, format!("\"{}\"", language.tag()));
        let back: VoiceLanguage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, language);
    }
}

/// `ALL` is the picker's order and the exhaustiveness argument at once; a
/// duplicate in it would make one language unreachable in a list and would not
/// otherwise fail anything.
#[test]
fn all_holds_each_language_once() {
    for language in VoiceLanguage::ALL {
        let count = VoiceLanguage::ALL
            .iter()
            .filter(|l| **l == language)
            .count();
        assert_eq!(count, 1, "{language:?} appears {count} times in ALL");
    }
    assert_eq!(VoiceLanguage::ALL.len(), Lang::ALL.len());
}

/// The two crates' lists are the same eight, in the same order, and the
/// mapping is a bijection. A ninth language on either side lands here.
#[test]
fn the_two_language_lists_are_the_same_eight() {
    for (voice, lang) in VoiceLanguage::ALL.into_iter().zip(Lang::ALL) {
        assert_eq!(voice.lang(), lang);
        assert_eq!(VoiceLanguage::from_lang(lang), voice);
        assert_eq!(voice.endonym(), lang.endonym());
    }
}

/// Inherited from `koru-i18n` and worth pinning here, because getting it wrong
/// is not a missing translation but a model producing fluent output in a script
/// the reader does not use.
#[test]
fn traditional_chinese_does_not_become_simplified() {
    assert_eq!(VoiceLanguage::matching("zh-Hant"), None);
    assert_eq!(
        VoiceLanguage::matching("zh-CN"),
        Some(VoiceLanguage::ZhHans)
    );
    assert_eq!(VoiceLanguage::matching("en-GB"), Some(VoiceLanguage::En));
    assert_eq!(VoiceLanguage::matching("kok"), None);
}

/// `Requested` is externally tagged so `Fixed` keeps the plan's newtype shape.
/// Pinned because an internally tagged enum cannot hold a newtype over a
/// string-shaped enum at all — it compiles and fails at runtime.
#[test]
fn requested_serializes_compactly() {
    assert_eq!(serde_json::to_string(&Requested::Auto).unwrap(), "\"auto\"");
    assert_eq!(
        serde_json::to_string(&Requested::Fixed(VoiceLanguage::PtBr)).unwrap(),
        "{\"fixed\":\"pt-BR\"}"
    );
    let back: Requested = serde_json::from_str("{\"fixed\":\"ja\"}").unwrap();
    assert_eq!(back, Requested::Fixed(VoiceLanguage::Ja));
}
