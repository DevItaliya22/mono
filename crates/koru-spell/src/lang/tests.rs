// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn every_tag_resolves_back_to_its_language() {
    for lang in SpellLang::ALL {
        assert_eq!(SpellLang::from_locale_tag(lang.tag()), Some(lang));
    }
}

#[test]
fn resolution_is_case_insensitive_and_tolerates_underscores() {
    assert_eq!(SpellLang::from_locale_tag("EN-us"), Some(SpellLang::EnUs));
    assert_eq!(SpellLang::from_locale_tag("pt_BR"), Some(SpellLang::PtBr));
}

#[test]
fn bare_english_is_us_and_named_regions_are_uk() {
    assert_eq!(SpellLang::from_locale_tag("en"), Some(SpellLang::EnUs));
    assert_eq!(SpellLang::from_locale_tag("en-AU"), Some(SpellLang::EnGb));
    assert_eq!(SpellLang::from_locale_tag("en-NZ"), Some(SpellLang::EnGb));
    assert_eq!(SpellLang::from_locale_tag("en-GB"), Some(SpellLang::EnGb));
}

#[test]
fn other_regions_fall_back_to_the_base_language() {
    assert_eq!(SpellLang::from_locale_tag("es-MX"), Some(SpellLang::Es));
    assert_eq!(SpellLang::from_locale_tag("fr-CA"), Some(SpellLang::Fr));
    assert_eq!(SpellLang::from_locale_tag("pt-PT"), Some(SpellLang::PtBr));
    assert_eq!(SpellLang::from_locale_tag("de-AT"), Some(SpellLang::De));
    assert_eq!(SpellLang::from_locale_tag("ar-EG"), Some(SpellLang::Ar));
}

/// The D6 half of the contract: an unsupported locale is `None`, never
/// English. `ko-KR` prose checked against en-US would squiggle every word.
#[test]
fn an_unsupported_locale_is_none_not_english() {
    assert_eq!(SpellLang::from_locale_tag("ko-KR"), None);
    assert_eq!(SpellLang::from_locale_tag("ja"), None);
    assert_eq!(SpellLang::from_locale_tag("zh-Hans"), None);
    assert_eq!(SpellLang::from_locale_tag(""), None);
    assert_eq!(SpellLang::from_locale_tag("  "), None);
}

/// The shared tags are borrowed from `koru-i18n`, never restated — pin the
/// two that regional English does not cover from drifting.
#[test]
fn shared_tags_match_koru_i18n() {
    assert_eq!(SpellLang::PtBr.tag(), koru_i18n::Lang::PtBr.tag());
    assert_eq!(SpellLang::Ar.tag(), koru_i18n::Lang::Ar.tag());
}
