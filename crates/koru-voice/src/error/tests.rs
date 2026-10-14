// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::collections::HashSet;

/// The plan's own test. Three claims in one, and each has a different failure:
/// a code that is not lowercase ASCII breaks a log grep and a future
/// translation key; a duplicate makes two situations indistinguishable to a
/// client that branches on it.
#[test]
fn every_code_is_lowercase_ascii_and_unique() {
    let mut seen = HashSet::new();
    for code in ErrorCode::ALL {
        let token = code.as_str();
        assert!(!token.is_empty(), "{code:?} has an empty token");
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{token} is not lowercase ASCII with underscores"
        );
        assert!(
            !token.starts_with('_') && !token.ends_with('_'),
            "{token} has a leading or trailing underscore"
        );
        assert!(seen.insert(token), "{token} is declared twice");
    }
    assert_eq!(seen.len(), ErrorCode::ALL.len());
}

/// `as_str` and serde must be one string. Two spellings of one code is the
/// exact drift this crate exists to prevent, and it is invisible until a client
/// matches on the wire form.
#[test]
fn the_serde_spelling_is_the_stable_token() {
    for code in ErrorCode::ALL {
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, format!("\"{}\"", code.as_str()));
        let back: ErrorCode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, code);
    }
}

/// A token read back out of a log or a stored diagnostic finds its code, and a
/// token nobody declared finds nothing rather than a plausible neighbour.
#[test]
fn a_token_parses_back_and_an_unknown_one_does_not() {
    for code in ErrorCode::ALL {
        assert_eq!(ErrorCode::parse(code.as_str()), Some(code));
    }
    assert_eq!(ErrorCode::parse("busy "), None);
    assert_eq!(ErrorCode::parse("Busy"), None);
    assert_eq!(ErrorCode::parse(""), None);
    assert_eq!(ErrorCode::parse("model_load_fail"), None);
}

/// `ALL` is what every walk over the codes uses, so a code missing from it is a
/// code no test covers. Counting the arms is the cheapest way to notice.
#[test]
fn all_holds_every_declared_code() {
    // Adding an arm to `ErrorCode` without adding it to `ALL` leaves this
    // number behind, and the exhaustive `match` in `as_str` catches the rest.
    assert_eq!(ErrorCode::ALL.len(), 14);
}

#[test]
fn an_error_displays_as_code_then_message() {
    let error = ErrorCode::Busy.with("a session is already live");
    assert_eq!(error.to_string(), "busy: a session is already live");
    assert_eq!(error.code, ErrorCode::Busy);
}

/// **Every code has a sentence, and it is the same sentence everywhere.**
///
/// Two surfaces show one: the document's refusal toast and the notification
/// area's voice submenu. They live in different crates and cannot see each
/// other, so a copy of this mapping in each is a pair that drifts silently —
/// which is why the mapping is the *code's* and this walks it rather than
/// spot-checking the interesting arms.
#[test]
fn every_code_has_a_sentence_to_show_a_person() {
    for code in ErrorCode::ALL {
        let sentence = koru_i18n::t(koru_i18n::Lang::En, code.sentence_key());
        assert!(!sentence.trim().is_empty(), "{code} has no sentence");
        // A sentence, not the token. `model_load_failed` on a toast would be
        // the developer text this module exists to keep away from people.
        assert!(
            !sentence.contains(code.as_str()),
            "{code} shows its own token: {sentence}"
        );
    }
}

/// **Two rows for one code**, because the sentence names where the switch is
/// and that is a different place on each platform. One string covering both
/// would be wrong on one of them, which is worse than a vague sentence:
/// somebody sent to the wrong Settings page concludes the feature is broken
/// (Q-V75).
#[test]
fn the_permission_sentence_names_this_platforms_settings() {
    let key = ErrorCode::PermissionDenied.sentence_key();
    assert_eq!(
        key,
        if cfg!(target_os = "macos") {
            koru_i18n::Key::VoiceErrorPermissionMac
        } else {
            koru_i18n::Key::VoiceErrorPermissionWindows
        }
    );
    assert_ne!(
        koru_i18n::t(koru_i18n::Lang::En, koru_i18n::Key::VoiceErrorPermissionMac),
        koru_i18n::t(
            koru_i18n::Lang::En,
            koru_i18n::Key::VoiceErrorPermissionWindows
        )
    );
}
