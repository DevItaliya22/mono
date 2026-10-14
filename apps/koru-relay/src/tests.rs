// SPDX-License-Identifier: MPL-2.0

use std::net::Ipv4Addr;
use std::time::Duration;

use koru_relay_test_support::TestRelayServer;

use super::{DEFAULT_PROBE_TIMEOUT, ProbeError, one_line_result, probe_relay};

/// The whole probe path against a live in-memory relay: two relay-only
/// endpoints, a connection whose every path is the relay, and a nonce that
/// came back intact. This is the shipped probe code — the same path the
/// deployment gates will run against the public hostnames.
#[tokio::test]
async fn probe_succeeds_through_an_in_memory_relay() {
    let relay = TestRelayServer::start().await.expect("relay starts");
    let result = probe_relay(relay.url().clone(), DEFAULT_PROBE_TIMEOUT).await;
    let report = result.expect("probe succeeds through the relay");
    assert_eq!(&report.relay_url, relay.url());
    let line = report.one_line();
    assert!(line.starts_with("ok "), "unexpected line: {line}");
    relay.shutdown().await.expect("graceful shutdown");
}

/// A relay that answers nothing must produce a bounded, explicit timeout —
/// not a hang. The port is armed to be dead: it was ours a moment ago and
/// was closed before probing.
#[tokio::test]
async fn probe_against_a_dead_port_times_out_within_its_budget() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind an ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    let url = format!("http://127.0.0.1:{port}")
        .parse()
        .expect("valid relay url");
    let budget = Duration::from_millis(750);
    let started = std::time::Instant::now();
    let result = probe_relay(url, budget).await;
    assert_eq!(result, Err(ProbeError::Timeout { limit: budget }));
    // The bound is the point: well past the budget means the timeout is not
    // actually bounding the work. Generous slack for a loaded machine.
    assert!(
        started.elapsed() < budget + Duration::from_secs(5),
        "probe took {:?} against a {budget:?} budget",
        started.elapsed()
    );
    let line = one_line_result(&result);
    assert!(line.starts_with("failed"), "unexpected line: {line}");
}
