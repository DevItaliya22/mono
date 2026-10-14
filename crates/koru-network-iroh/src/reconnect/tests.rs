// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn transient_backoff_is_exponential_jittered_and_capped() {
    let peer = DeviceId::new();
    let first = reconnect_delay(0, peer);
    let second = reconnect_delay(1, peer);
    let capped = reconnect_delay(u8::MAX, peer);

    assert!((Duration::from_millis(250)..=Duration::from_millis(312)).contains(&first));
    assert!(second > first);
    assert!(capped <= Duration::from_secs(30));
    assert_eq!(
        retry_delay(RetryReason::Transient, 3, peer),
        reconnect_delay(3, peer),
        "the transient schedule is the one compatible peers always had"
    );
}

#[test]
fn incompatible_backoff_climbs_from_thirty_seconds_to_a_ten_minute_cap() {
    for _ in 0..64 {
        let peer = DeviceId::new();
        let delays: Vec<Duration> = (0..8)
            .map(|failures| retry_delay(RetryReason::Incompatible, failures, peer))
            .collect();
        for (failures, delay) in delays.iter().enumerate() {
            let base = INCOMPATIBLE_BACKOFF[failures.min(INCOMPATIBLE_BACKOFF.len() - 1)];
            assert!(
                *delay <= base && *delay >= base - base / 10,
                "attempt {failures}: {delay:?} is not within a tenth below {base:?}"
            );
        }
        assert!(delays[0] >= Duration::from_secs(27));
        assert!(
            delays
                .iter()
                .all(|delay| *delay <= Duration::from_secs(600)),
            "ten minutes is a cap: {delays:?}"
        );
        assert!(
            delays[4] >= Duration::from_secs(540),
            "the cap is reached on the fifth early death"
        );
    }
}

#[test]
fn incompatible_jitter_differs_between_attempts_and_devices() {
    // Deterministic for one (device, attempt), so a test can hold it; varied
    // across both, so stacked or meshed redials do not land together.
    let peer = DeviceId::new();
    assert_eq!(incompatible_delay(2, peer), incompatible_delay(2, peer));
    let distinct: std::collections::BTreeSet<Duration> = (0..32)
        .map(|_| incompatible_delay(4, DeviceId::new()))
        .collect();
    assert!(distinct.len() > 1, "every device would redial in lockstep");
}

#[test]
fn an_incompatible_peer_is_reported_on_entry_then_once_per_backoff_window() {
    let peer = DeviceId::new();
    let build = BuildPayload {
        app_version: "0.0.3".into(),
        peer_protocol: Some(4),
        ..Default::default()
    };
    let start = Instant::now();
    let mut reports = IncompatibleReports::default();

    assert_eq!(
        reports.connected(peer, Some(&build), start),
        IncompatibleReport::Entered
    );
    // The storm the maintainers saw: connection after connection within
    // milliseconds. None of them is news any more.
    for offset in 1..50 {
        let now = start + Duration::from_millis(offset * 10);
        assert_eq!(
            reports.connected(peer, Some(&build), now),
            IncompatibleReport::Quiet
        );
        assert_eq!(
            reports.backing_off(peer, Some(&build), now, INCOMPATIBLE_BACKOFF[0]),
            IncompatibleReport::Quiet
        );
    }

    // Once the first window has passed, the next refusal says so again and
    // opens a window as long as the backoff it announces.
    let first_retry = start + Duration::from_secs(28);
    assert_eq!(
        reports.backing_off(peer, Some(&build), first_retry, INCOMPATIBLE_BACKOFF[1]),
        IncompatibleReport::StillRefused
    );
    assert_eq!(
        reports.backing_off(
            peer,
            Some(&build),
            first_retry + Duration::from_secs(30),
            INCOMPATIBLE_BACKOFF[2]
        ),
        IncompatibleReport::Quiet,
        "inside the sixty-second window"
    );
    assert_eq!(
        reports.backing_off(
            peer,
            Some(&build),
            first_retry + Duration::from_secs(60),
            INCOMPATIBLE_BACKOFF[2]
        ),
        IncompatibleReport::StillRefused
    );
}

#[test]
fn a_new_peer_build_or_a_clear_starts_the_report_over() {
    let peer = DeviceId::new();
    let old = BuildPayload {
        app_version: "0.0.3".into(),
        peer_protocol: Some(4),
        ..Default::default()
    };
    let other_old = BuildPayload {
        app_version: "0.0.4".into(),
        peer_protocol: Some(4),
        ..Default::default()
    };
    let now = Instant::now();
    let mut reports = IncompatibleReports::default();

    assert_eq!(
        reports.connected(peer, Some(&old), now),
        IncompatibleReport::Entered
    );
    assert_eq!(
        reports.connected(peer, Some(&other_old), now),
        IncompatibleReport::Entered,
        "a different build that is still refused is news"
    );
    assert!(reports.clear(peer), "leaving the state is news");
    assert!(!reports.clear(peer), "and only once");
    assert_eq!(
        reports.connected(peer, Some(&other_old), now),
        IncompatibleReport::Entered,
        "entering it again is news again"
    );
    assert_eq!(reports.devices(), vec![peer]);
}
