// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::collections::BTreeSet;

/// The tokens are matched by a caller and printed in a log, so a duplicate or a
/// stray capital is a rename waiting to happen. The same test `koru-voice`'s
/// own `ErrorCode` carries, for the same reason.
#[test]
fn every_code_is_lowercase_ascii_and_unique() {
    let mut seen = BTreeSet::new();
    for code in ModelErrorCode::ALL {
        let token = code.as_str();
        assert!(!token.is_empty(), "{code:?} has an empty token");
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
            "{token} is not lowercase ASCII with underscores"
        );
        assert!(seen.insert(token), "{token} is declared twice");
    }
    assert_eq!(seen.len(), ModelErrorCode::ALL.len());
}

/// `as_str` and serde must agree, or the wire spelling and the log spelling
/// come apart the first time somebody edits `rename_all`.
#[test]
fn serde_writes_the_stable_token() {
    for code in ModelErrorCode::ALL {
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, format!("\"{}\"", code.as_str()));
        assert_eq!(ModelErrorCode::parse(code.as_str()), Some(code));
    }
    assert_eq!(ModelErrorCode::parse("not_a_code"), None);
}

/// The split that matters is `ModelMissing`'s own doc: *only one of them is
/// fixed by downloading something*. Pinned as a table rather than left to a
/// reader of the `match`, because getting it backwards would tell a person to
/// download a model they already have.
#[test]
fn a_failure_a_download_would_fix_is_model_missing() {
    use koru_voice::ErrorCode;
    for code in [
        ModelErrorCode::NotInstalled,
        ModelErrorCode::Offline,
        ModelErrorCode::DownloadFailed,
        ModelErrorCode::InstallFailed,
    ] {
        assert_eq!(code.session_code(), ErrorCode::ModelMissing, "{code}");
    }
    for code in [
        ModelErrorCode::DigestMismatch,
        ModelErrorCode::SignatureInvalid,
        ModelErrorCode::EngineMismatch,
        ModelErrorCode::NoTrustRoot,
    ] {
        assert_eq!(code.session_code(), ErrorCode::ModelLoadFailed, "{code}");
    }
    assert_eq!(
        ModelErrorCode::LanguageUnsupported.session_code(),
        ErrorCode::LanguageUnsupported
    );
}

/// A model failure has to be answerable in the session vocabulary without the
/// worker inventing text, so the message travels and the code translates.
#[test]
fn a_model_error_becomes_a_session_error_without_losing_its_words() {
    let error = ModelErrorCode::DigestMismatch.with("expected abc, got def");
    let session = error.session_error();
    assert_eq!(session.code, koru_voice::ErrorCode::ModelLoadFailed);
    assert_eq!(session.message, "expected abc, got def");
}
