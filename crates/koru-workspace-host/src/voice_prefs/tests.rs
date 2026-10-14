// SPDX-License-Identifier: MPL-2.0

//! The three preferences, their tokens, and the one thing a settings file is
//! allowed to do to a build that does not understand it: nothing.
//!
//! **These take a lock.** The values are process globals, so a test that sets
//! one races every other test in this binary that reads one — the same rule the
//! theme's `THEME_LOCK` exists for.

use super::*;

static PREFS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    PREFS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Every language round-trips through the atomic, `Auto` included — which is
/// the one that would be silently lost by an index scheme with no zero.
#[test]
fn every_language_round_trips_through_the_global() {
    let _guard = lock();
    let was = language();
    set_language(Requested::Auto);
    assert_eq!(language(), Requested::Auto);
    for l in VoiceLanguage::ALL {
        set_language(Requested::Fixed(l));
        assert_eq!(language(), Requested::Fixed(l), "{l:?}");
    }
    set_language(was);
}

#[test]
fn every_tier_round_trips_through_the_global() {
    let _guard = lock();
    let was = tier();
    for t in ModelTier::ALL {
        set_tier(t);
        assert_eq!(tier(), t, "{t:?}");
    }
    set_tier(was);
}

#[test]
fn the_gesture_round_trips_and_defaults_to_push_to_talk() {
    let _guard = lock();
    let was = toggle();
    set_toggle(false);
    assert!(!toggle(), "push-to-talk is the default gesture");
    set_toggle(true);
    assert!(toggle());
    set_toggle(was);
}

/// The tokens are BCP 47 and `auto`, and they round-trip. **Not the label** —
/// a stored display string means something different in another language.
#[test]
fn a_language_token_is_a_tag_and_never_a_label() {
    assert_eq!(language_token(Requested::Auto), "auto");
    for l in VoiceLanguage::ALL {
        let token = language_token(Requested::Fixed(l));
        assert_eq!(token, l.tag(), "{l:?}");
        assert_ne!(token, l.endonym(), "the endonym is not a storage token");
        assert_eq!(parse_language(token), Requested::Fixed(l), "{l:?}");
    }
}

/// **A file this build does not understand opens on the defaults.** A ninth
/// language, a quality tier from a later catalog, a truncated line: each is the
/// default rather than a refusal to open Settings, and detecting the language
/// is the answer that is wrong for nobody.
#[test]
fn an_unreadable_preference_is_the_default_rather_than_a_failure() {
    assert_eq!(parse_language(""), Requested::Auto);
    assert_eq!(parse_language("   "), Requested::Auto);
    assert_eq!(parse_language("klingon"), Requested::Auto);
    assert_eq!(parse_language("AUTO"), Requested::Auto);
    assert_eq!(
        parse_language(" pt-BR "),
        Requested::Fixed(VoiceLanguage::PtBr)
    );

    assert_eq!(parse_tier(""), ModelTier::default());
    assert_eq!(parse_tier("enormous"), ModelTier::default());
    assert_eq!(parse_tier(" high_accuracy "), ModelTier::HighAccuracy);
}

/// `zh-Hant` is Traditional Chinese and is **not** quietly read as Simplified —
/// `koru_i18n::Lang::matching`'s rule, inherited rather than re-implemented.
#[test]
fn a_script_is_not_a_dialect() {
    assert_eq!(
        parse_language("zh-Hans"),
        Requested::Fixed(VoiceLanguage::ZhHans)
    );
    assert_eq!(parse_language("zh-Hant"), Requested::Auto);
}

/// The snapshot moves when any one of the three does — the property the mouse
/// guard depends on.
#[test]
fn the_snapshot_moves_with_every_one_of_the_three() {
    let _guard = lock();
    let was = (language(), tier(), toggle());
    set_language(Requested::Auto);
    set_tier(ModelTier::Compact);
    set_toggle(false);
    let base = snapshot();

    set_language(Requested::Fixed(VoiceLanguage::Ja));
    assert_ne!(snapshot(), base, "a language change is visible");
    set_language(Requested::Auto);

    set_tier(ModelTier::HighAccuracy);
    assert_ne!(snapshot(), base, "a quality change is visible");
    set_tier(ModelTier::Compact);

    set_toggle(true);
    assert_ne!(snapshot(), base, "a gesture change is visible");

    set_language(was.0);
    set_tier(was.1);
    set_toggle(was.2);
}
