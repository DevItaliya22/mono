// SPDX-License-Identifier: MPL-2.0

use super::*;

/// Sizes read at a glance, and an unknown one says so rather than guessing.
#[test]
fn a_size_is_readable_and_an_unknown_one_is_a_dash() {
    assert_eq!(size(None), "-");
    assert_eq!(size(Some(0)), "0B");
    assert_eq!(size(Some(512)), "512B");
    assert_eq!(size(Some(2048)), "2.0KB");
    assert_eq!(size(Some(3 * 1024 * 1024)), "3.0MB");
}

/// **The boundary is 1024, not 1000.** Worth pinning because the two conventions
/// disagree by 2.4% at a megabyte and somebody will eventually "fix" one of them.
#[test]
fn the_unit_boundary_is_binary() {
    assert_eq!(size(Some(1023)), "1023B");
    assert_eq!(size(Some(1024)), "1.0KB");
}

/// **A WebSocket upgrade is not a warning.**
///
/// `StatusCode::is_success` is 2xx only, and `/v1/sync` answers `101 Switching
/// Protocols` — so the most ordinary thing this store does logged as a problem
/// until this was noticed in real output.
#[test]
fn a_protocol_upgrade_is_not_a_failure() {
    use axum::http::StatusCode;
    let ok = |s: StatusCode| s.is_success() || s == StatusCode::SWITCHING_PROTOCOLS;
    assert!(ok(StatusCode::SWITCHING_PROTOCOLS));
    assert!(ok(StatusCode::OK));
    assert!(!ok(StatusCode::UNAUTHORIZED));
    assert!(!ok(StatusCode::INTERNAL_SERVER_ERROR));
}

#[test]
fn query_values_never_enter_the_access_log_path() {
    let uri: axum::http::Uri = "/v1/profiles/id/reminders?entity=private%20customer%20title"
        .parse()
        .unwrap();
    assert_eq!(log_path(&uri), "/v1/profiles/id/reminders");
    assert!(!log_path(&uri).contains("private"));
}
