// SPDX-License-Identifier: MPL-2.0

//! The shipped `probe` path, end to end: the built `koru-relay` binary run
//! against an in-memory relay from `koru-relay-test-support`. This is exactly
//! what the deploy gate and the scheduled monitor (Milestone 3) will run —
//! binary, CLI, library probe — not an imitation of it.
//!
//! Everything here stays on loopback: the relay is in-memory, the dead port
//! was ours a moment ago, and the usage cases never open a socket at all.

use std::process::{Command, Output};
use std::time::{Duration, Instant};

use koru_relay_test_support::TestRelayServer;

fn probe(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_koru-relay"))
        .arg("probe")
        .args(args)
        .output()
        .expect("koru-relay runs")
}

fn stdout_line(output: &Output) -> String {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    // The contract: exactly one line, for the humans behind the monitors.
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout is not one line: {stdout:?}"
    );
    stdout
}

/// Success through the whole stack: exit 0, one `ok` line on stdout, and
/// nothing on stderr — the probe path initialises no logging.
///
/// Multi-threaded runtime: the relay's tasks must keep running on worker
/// threads while the test thread blocks on the child process.
#[tokio::test(flavor = "multi_thread")]
async fn the_probe_subcommand_exits_zero_with_one_ok_line_against_a_live_relay() {
    let relay = TestRelayServer::start().await.expect("relay starts");
    let url = relay.url().to_string();
    let output = tokio::task::spawn_blocking(move || probe(&[&url]))
        .await
        .expect("probe task completes");
    let line = stdout_line(&output);
    assert!(
        output.status.success(),
        "exit {:?}, stdout {line:?}, stderr {:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(line.starts_with("ok "), "unexpected line: {line}");
    assert!(
        output.stderr.is_empty(),
        "probe chattered on stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    relay.shutdown().await.expect("graceful shutdown");
}

/// A dead port exits nonzero, fast, with one `failed` line. The library's
/// own tests prove the timeout mechanism; this proves the *binary* turns it
/// into the exit code the deploy gate and the monitor branch on — and that a
/// probe failure is not exit 2, which means a usage error.
#[test]
fn a_dead_port_exits_nonzero_fast_with_one_failed_line() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    let started = Instant::now();
    let output = probe(&[&format!("http://127.0.0.1:{port}"), "--timeout", "1"]);
    assert!(!output.status.success(), "a dead port probed as healthy");
    assert_ne!(
        output.status.code(),
        Some(2),
        "a failed probe reported itself as a usage error"
    );
    let line = stdout_line(&output);
    assert!(line.starts_with("failed"), "unexpected line: {line}");
    // 1s budget; generous slack for a loaded machine — the point is that it
    // did not sit out the 30s default.
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "probe took {:?} against a 1s budget",
        started.elapsed()
    );
}

/// `probe` with no URL is a usage error: exit 2, the explanation and usage
/// on stderr, and nothing on stdout for a script to misread as a result.
#[test]
fn probe_without_a_url_exits_two_with_usage() {
    let output = probe(&[]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(
        stderr.contains("probe needs a relay URL"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("Usage:"), "stderr: {stderr}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// A URL that does not parse is refused the same way — before any endpoint
/// is built, so it can never be mistaken for a relay being down.
#[test]
fn a_bad_url_exits_two_as_a_usage_error_not_a_probe_failure() {
    let output = probe(&["not a url"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("is not a relay URL"), "stderr: {stderr}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
