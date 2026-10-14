// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn every_code_is_a_stable_ascii_token() {
    // A person searching for one of these has been handed it by a program.
    for e in [
        IpcError::UnknownSegment {
            segment: "x".into(),
            expected: "stable, beta".into(),
        },
        IpcError::NotRunning("n".into()),
        IpcError::AlreadyBound("n".into()),
        IpcError::PermissionDenied("n".into()),
        IpcError::BindFailed("n".into()),
        IpcError::TransportFailed("n".into()),
    ] {
        let code = e.code();
        assert!(code.starts_with("ipc_"), "{code}");
        assert!(
            code.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{code}"
        );
    }
}

#[test]
fn only_nothing_is_listening_is_worth_retrying() {
    // Everything else needs somebody to do something first, and a caller that
    // retries those spins until a person intervenes.
    assert!(IpcError::NotRunning("n".into()).is_retryable());
    for e in [
        IpcError::AlreadyBound("n".into()),
        IpcError::PermissionDenied("n".into()),
        IpcError::BindFailed("n".into()),
        IpcError::TransportFailed("n".into()),
    ] {
        assert!(!e.is_retryable(), "{e:?}");
    }
}

#[test]
fn an_unknown_segment_says_which_ones_would_have_worked() {
    // The remedy is the whole value of the message: "not a release channel" on
    // its own leaves somebody guessing at the spelling.
    let e = IpcError::UnknownSegment {
        segment: "nightlyy".into(),
        expected: koru_channel::SEGMENTS.join(", "),
    };
    let said = e.to_string();
    assert!(said.contains("nightlyy"), "{said}");
    assert!(said.contains("nightly"), "{said}");
}
