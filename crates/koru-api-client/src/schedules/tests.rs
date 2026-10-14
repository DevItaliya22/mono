// SPDX-License-Identifier: MPL-2.0

use super::*;

fn gateway(endpoint: String) -> koru_api_discovery::GatewayHello {
    koru_api_discovery::GatewayHello {
        instance_id: "test-gateway".into(),
        supervisor_launch_id: None,
        gateway_version: "test".into(),
        segment: "test".into(),
        api_version: 1,
        api_endpoint: endpoint,
        capabilities: vec![koru_delegate_api::CAPABILITY.into()],
        session: koru_api_discovery::SessionCredential::new("schedule-test".into(), i64::MAX),
    }
}

#[test]
fn schedule_paths_keep_the_workspace_and_complete_generation() {
    let workspace = WorkspaceId::new();
    let change: ScheduleChange = serde_json::from_value(serde_json::json!({
        "instance": "00000000-0000-4000-8000-000000000001",
        "generation": 7
    }))
    .unwrap();
    assert_eq!(
        stream_path(workspace, Some(&change)),
        format!(
            "/v1/schedules/stream?workspace={}&after_instance={}&after_generation=7",
            workspace.to_storage(),
            change.instance
        )
    );
    assert_eq!(
        stream_path(workspace, None),
        format!("/v1/schedules/stream?workspace={}", workspace.to_storage())
    );
}

fn idle_schedule_server() -> (
    Client,
    std::sync::mpsc::Receiver<()>,
    std::thread::JoinHandle<bool>,
) {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (opened, wait_opened) = std::sync::mpsc::sync_channel(1);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                return false;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        assert!(String::from_utf8_lossy(&request).starts_with("GET /v1/schedules/stream?"));
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
            )
            .unwrap();
        opened.send(()).unwrap();
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => return true,
                Ok(_) => continue,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return false;
                }
                Err(_) => return true,
            }
        }
    });
    let client = Client::from_gateway(gateway(endpoint)).unwrap();
    (client, wait_opened, server)
}

#[test]
fn stopping_an_idle_schedule_stream_cancels_the_http_read() {
    let (client, opened, server) = idle_schedule_server();
    let subscription = client
        .schedules()
        .watch(WorkspaceId::new(), None, |_| {})
        .unwrap();
    opened
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();

    let started = std::time::Instant::now();
    subscription.stop();

    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(
        server.join().unwrap(),
        "the idle HTTP connection stayed open"
    );
}

#[test]
fn dropping_an_idle_schedule_stream_cancels_the_http_read() {
    let (client, opened, server) = idle_schedule_server();
    let subscription = client
        .schedules()
        .watch(WorkspaceId::new(), None, |_| {})
        .unwrap();
    opened
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();

    let started = std::time::Instant::now();
    drop(subscription);

    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(
        server.join().unwrap(),
        "the idle HTTP connection stayed open"
    );
}

#[test]
fn repeated_idle_schedule_subscriptions_leave_no_transport_threads() {
    for _ in 0..8 {
        let (client, opened, server) = idle_schedule_server();
        let subscription = client
            .schedules()
            .watch(WorkspaceId::new(), None, |_| {})
            .unwrap();
        opened
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();

        drop(subscription);

        assert!(server.join().unwrap());
    }
}

#[test]
fn fragmented_crlf_schedule_event_reaches_the_typed_handler() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::from_gateway(gateway(format!(
        "http://{}",
        listener.local_addr().unwrap()
    )))
    .unwrap();
    let expected: ScheduleChange = serde_json::from_value(serde_json::json!({
        "instance": "00000000-0000-0000-0000-00000000000b",
        "generation": 23
    }))
    .unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            assert_ne!(read, 0);
            request.extend_from_slice(&buffer[..read]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
            )
            .unwrap();
        for part in [
            "event: schedule_change\r\n",
            "data: {\"instance\":\"00000000-0000-0000-0000-00000000000b\",",
            "\"generation\":23}\r\n\r\n",
        ] {
            write!(stream, "{:x}\r\n{part}\r\n", part.len()).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        matches!(stream.read(&mut buffer), Ok(0) | Err(_))
    });
    let (events, received) = std::sync::mpsc::channel();
    let subscription = client
        .schedules()
        .watch(WorkspaceId::new(), None, move |event| {
            let _ = events.send(event);
        })
        .unwrap();

    let change = received
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(change, expected);

    drop(subscription);
    assert!(server.join().unwrap(), "the decoded stream stayed open");
}

#[test]
fn unchanged_generation_is_reported_after_a_healthy_reconnect() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::from_gateway(gateway(format!(
        "http://{}",
        listener.local_addr().unwrap()
    )))
    .unwrap();
    let expected: ScheduleChange = serde_json::from_value(serde_json::json!({
        "instance": "00000000-0000-0000-0000-00000000000c",
        "generation": 29
    }))
    .unwrap();
    let expected_for_server = expected.clone();
    let (reconnected, wait_reconnected) = std::sync::mpsc::sync_channel(1);
    let server = std::thread::spawn(move || {
        let read_request = |stream: &mut std::net::TcpStream| {
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                assert_ne!(read, 0);
                request.extend_from_slice(&buffer[..read]);
            }
            String::from_utf8(request).unwrap()
        };

        let (mut first, _) = listener.accept().unwrap();
        let first_request = read_request(&mut first);
        assert!(!first_request.contains("after_generation"));
        let body = format!(
            "data: {{\"instance\":\"{}\",\"generation\":29}}\n\n",
            expected_for_server.instance
        );
        write!(
            first,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        first.flush().unwrap();
        drop(first);

        let (mut second, _) = listener.accept().unwrap();
        let second_request = read_request(&mut second);
        assert!(second_request.contains("after_generation=29"));
        second
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
            )
            .unwrap();
        second.flush().unwrap();
        reconnected.send(()).unwrap();
        second
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut buffer = [0_u8; 32];
        matches!(second.read(&mut buffer), Ok(0) | Err(_))
    });
    let (events, received) = std::sync::mpsc::channel();
    let subscription = client
        .schedules()
        .watch(WorkspaceId::new(), None, move |event| {
            let _ = events.send(event);
        })
        .unwrap();

    assert_eq!(
        received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        expected
    );
    assert!(
        received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .is_err(),
        "the closed first stream reports its retryable error"
    );
    wait_reconnected
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(
        received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        expected,
        "the successful reconnect repeats the cursor even without a mutation"
    );

    drop(subscription);
    assert!(server.join().unwrap());
}

#[test]
fn schedule_stream_header_handshake_is_bounded() {
    use std::io::Read as _;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let hello = gateway(format!("http://{}", listener.local_addr().unwrap()));
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer).unwrap();
        // Accept the request but never answer its headers. The timeout must
        // drop the request future and close this transport.
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        matches!(stream.read(&mut buffer), Ok(0) | Err(_))
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let http = reqwest::Client::new();

    let error = runtime
        .block_on(send_stream_request(
            &http,
            &hello,
            "/v1/schedules/stream",
            std::time::Duration::from_millis(50),
        ))
        .unwrap_err();

    assert_eq!(error.code, "gateway_unavailable");
    assert!(server.join().unwrap());
}

#[test]
fn schedule_stream_requires_event_stream_content_type() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let hello = gateway(format!("http://{}", listener.local_addr().unwrap()));
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            )
            .unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let http = reqwest::Client::new();

    let error = runtime.block_on(async {
        let response = send_stream_request(
            &http,
            &hello,
            "/v1/schedules/stream",
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap();
        checked_stream(response).await.unwrap_err()
    });

    assert_eq!(error.code, "invalid_response");
    server.join().unwrap();
}
