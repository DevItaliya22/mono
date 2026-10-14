// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::TestRelayServer;

/// The server answers `/healthz` over plain HTTP — the raw-socket request
/// keeps this crate free of an HTTP client dependency, and doubles as the
/// recorded evidence that iroh-relay ships a health endpoint
/// (guide/contributing/deps/IROH_RELAY.md).
#[tokio::test]
async fn started_server_answers_healthz_over_plain_http() {
    let relay = TestRelayServer::start().await.expect("relay starts");
    let authority = relay
        .url()
        .as_str()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    let mut stream = TcpStream::connect(&authority).await.expect("connects");
    stream
        .write_all(
            format!("GET /healthz HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .expect("request written");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("response read");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected response: {response}"
    );
    assert!(response.contains(r#""status":"ok""#), "body: {response}");
    relay.shutdown().await.expect("graceful shutdown");
}

/// Teardown is the charter: after an explicit shutdown the port no longer
/// accepts connections, so a suite cannot leak relay listeners.
#[tokio::test]
async fn shutdown_releases_the_listener() {
    let relay = TestRelayServer::start().await.expect("relay starts");
    let authority = relay
        .url()
        .as_str()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    // Prove it was listening at all, so the refusal below means something.
    TcpStream::connect(&authority)
        .await
        .expect("listening before shutdown");
    relay.shutdown().await.expect("graceful shutdown");
    // The listener task is awaited by shutdown, but give the OS a moment to
    // fully retire the socket before asserting.
    for _ in 0..50 {
        if TcpStream::connect(&authority).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("relay port still accepting connections after shutdown");
}

/// Dropping the handle without calling shutdown must also reap the server —
/// a panicking test cannot be relied on to tear down politely.
#[tokio::test]
async fn dropping_the_handle_reaps_the_server() {
    let relay = TestRelayServer::start().await.expect("relay starts");
    let authority = relay
        .url()
        .as_str()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    TcpStream::connect(&authority)
        .await
        .expect("listening before drop");
    drop(relay);
    for _ in 0..50 {
        if TcpStream::connect(&authority).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("relay port still accepting connections after drop");
}
