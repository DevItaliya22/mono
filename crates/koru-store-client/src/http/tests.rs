// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn an_injected_loopback_endpoint_becomes_internal_connection_config() {
    let found =
        Discovery::from_loopback_endpoint("http://127.0.0.1:51234", "a-session-token", i64::MAX)
            .unwrap();
    assert_eq!(found.port, 51234);
    assert_eq!(found.token, "a-session-token");
    assert_eq!(found.expires_at_millis, i64::MAX);
    assert_eq!(found.base_url(), "http://127.0.0.1:51234");
    assert_eq!(found.sync_url(), "ws://127.0.0.1:51234/v1/sync");
}

#[test]
fn discovery_credentials_are_redacted() {
    let discovery = Discovery {
        port: 51234,
        token: "never-print-this-token".into(),
        expires_at_millis: 42,
    };
    let debug = format!("{discovery:?}");
    assert!(!debug.contains("never-print-this-token"));
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn renewal_deadline_comes_from_the_store_granted_expiry() {
    let discovery = Discovery {
        port: 1,
        token: "test".to_owned(),
        expires_at_millis: 1_600_000,
    };
    // A ten-minute credential is renewed one minute early (ten percent), not
    // by a client copy of the Store's policy.
    assert_eq!(
        Current::at(discovery, 1_000_000).refresh_at_millis,
        1_540_000
    );
}

#[test]
fn a_short_credential_still_refreshes_before_its_expiry() {
    let discovery = Discovery {
        port: 1,
        token: "test".to_owned(),
        expires_at_millis: 1_005_000,
    };
    assert_eq!(
        Current::at(discovery, 1_000_000).refresh_at_millis,
        1_004_500
    );
}

/// `Discovery` is a plain struct with public fields, so a caller that builds
/// one by hand leaves the expiry at zero. Read as a deadline that is a deadline
/// already in the past, and every request would open the rendezvous before
/// sending — quietly, and for far longer than the call it precedes.
#[test]
fn a_credential_with_no_granted_expiry_has_no_deadline() {
    let discovery = Discovery {
        port: 1,
        token: "test".to_owned(),
        expires_at_millis: 0,
    };
    assert_eq!(
        Current::at(discovery, 1_000_000).refresh_at_millis,
        i64::MAX
    );
}

/// **A client that cannot rediscover is not a failed request.**
///
/// `Client::new` is given an address and a credential and no way back to the
/// owning boundary — several direct integration harnesses are built that way.
/// Refusing here would turn "I could not renew" into a
/// request that never left, holding a token the store would still have taken.
#[test]
fn a_client_with_no_way_back_to_the_rendezvous_still_sends() {
    let client = Client::new(Discovery {
        port: 1,
        token: "test".to_owned(),
        // Expired long ago, so the deadline is unambiguously past.
        expires_at_millis: 1,
    });
    assert!(
        client.refresh_if_due().is_ok(),
        "a client with no segment refused a request it could still have made"
    );
    assert!(
        client.refresh_after_rejection("test").is_ok(),
        "a 401 became `cannot rediscover`, hiding the real answer"
    );
}

#[test]
fn a_boundary_owned_renewal_replaces_the_rejected_credential_once() {
    let renewals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&renewals);
    let client = Client::with_renewal(
        Discovery {
            port: 1,
            token: "rejected".to_owned(),
            expires_at_millis: i64::MAX,
        },
        Arc::new(move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Discovery {
                port: 2,
                token: "gateway-renewed".to_owned(),
                expires_at_millis: i64::MAX,
            })
        }),
    );

    client.refresh_after_rejection("rejected").unwrap();
    client.refresh_after_rejection("rejected").unwrap();

    assert_eq!(renewals.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(client.discovery().port, 2);
    assert_eq!(client.discovery().token, "gateway-renewed");
}

#[test]
fn an_address_that_is_not_loopback_is_refused_rather_than_followed() {
    // The store binds `127.0.0.1` and there is no setting through which it
    // could become reachable from the network. A client that would follow any
    // address it was handed is one that can be walked off the machine by a
    // store that has been tampered with — so the invariant is checked on the
    // way in, not assumed.
    for address in [
        "http://0.0.0.0:51234",
        "http://192.168.1.10:51234",
        "http://example.com:51234",
        "https://127.0.0.1:51234",
    ] {
        let outcome = Discovery::from_loopback_endpoint(address, "token", i64::MAX);
        assert_eq!(
            outcome.unwrap_err().code(),
            Code::CorruptState,
            "{address} was accepted"
        );
    }
}

#[test]
fn an_unparseable_port_is_corrupt_state_rather_than_a_default() {
    // Falling back to a default port would dial *something*, and whatever
    // answered would be treated as the store.
    for address in [
        "http://127.0.0.1:",
        "http://127.0.0.1:99999",
        "http://127.0.0.1",
    ] {
        assert_eq!(
            Discovery::from_loopback_endpoint(address, "token", i64::MAX)
                .unwrap_err()
                .code(),
            Code::CorruptState,
            "{address} was accepted"
        );
    }
}

#[test]
fn a_stable_token_maps_back_to_this_builds_code() {
    // What lets a client branch on `workspace_not_found` rather than on 404 —
    // and therefore work over a transport with no status codes at all.
    for code in Code::ALL {
        assert_eq!(code_from(code.as_str()), *code, "{code:?}");
    }
}

#[test]
fn a_token_this_build_has_never_heard_of_is_still_a_failure() {
    // A client built against an older store must not lose the fact that
    // something failed just because it does not recognise the reason.
    assert_eq!(code_from("some_future_code"), Code::Internal);
    assert_eq!(code_from(""), Code::Internal);
}

#[test]
fn candidate_seed_is_version_fenced_before_an_old_store_can_mutate() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).unwrap();
            assert_ne!(read, 0);
            request.extend_from_slice(&chunk[..read]);
        }
        let head = String::from_utf8_lossy(&request);
        assert!(head.starts_with("POST /v2/internal/workspaces/"));
        assert!(!head.starts_with("POST /v1/internal/workspaces/"));
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let client = Client::new(Discovery {
        port,
        token: "old-store".into(),
        expires_at_millis: i64::MAX,
    });

    let error = match client.seed_document_with_candidate(
        WorkspaceId::new(),
        "schedule",
        "koru-schedule",
        None,
        None,
        DocumentId::new(),
        "schedule-id",
        "request-hash",
        vec![1],
    ) {
        Ok(_) => panic!("an old Store must not accept candidate seeding"),
        Err(error) => error,
    };

    assert_eq!(error.code(), Code::ApiVersionUnsupported);
    server.join().unwrap();
}

#[test]
fn a_streaming_blob_retry_rebuilds_the_reader_at_byte_zero() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TrackingReader {
        cursor: std::io::Cursor<Vec<u8>>,
        starts: Arc<Mutex<Vec<u64>>>,
        first_read: bool,
    }
    impl std::io::Read for TrackingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.first_read {
                self.first_read = false;
                self.starts.lock().unwrap().push(self.cursor.position());
            }
            self.cursor.read(buffer)
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let good_port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            if request.windows(5).any(|window| window == b"0\r\n\r\n") {
                break;
            }
        }
        assert!(
            request.windows(11).any(|window| window == b"retry bytes"),
            "second attempt did not stream the source"
        );
        let blob = Blob {
            hash: BlobHash::from_digest(BlobHash::BLAKE3, &"a".repeat(64)).unwrap(),
            size: 11,
            mime: "application/octet-stream".into(),
            filename: None,
            created_at: koru_store_core::model::Timestamp::from_millis(1),
            attributes: Default::default(),
        };
        let body = serde_json::to_vec(&blob).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
    });

    let failing_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let failing_port = failing_listener.local_addr().unwrap().port();
    let first_server = std::thread::spawn(move || {
        let (mut stream, _) = failing_listener.accept().unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            if request.windows(11).any(|window| window == b"retry bytes") {
                break;
            }
        }
        assert!(request.windows(11).any(|window| window == b"retry bytes"));
        stream.shutdown(std::net::Shutdown::Both).unwrap();
    });
    let renewals = Arc::new(AtomicUsize::new(0));
    let renewal_count = Arc::clone(&renewals);
    let client = Client::with_renewal(
        Discovery {
            port: failing_port,
            token: "first".into(),
            expires_at_millis: i64::MAX,
        },
        Arc::new(move || {
            renewal_count.fetch_add(1, Ordering::SeqCst);
            Ok(Discovery {
                port: good_port,
                token: "second".into(),
                expires_at_millis: i64::MAX,
            })
        }),
    );

    let starts = Arc::new(Mutex::new(Vec::new()));
    let factories = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&starts);
    let factory_count = Arc::clone(&factories);
    client
        .put_blob_from(
            WorkspaceId::new(),
            move || {
                factory_count.fetch_add(1, Ordering::SeqCst);
                Box::new(TrackingReader {
                    cursor: std::io::Cursor::new(b"retry bytes".to_vec()),
                    starts: Arc::clone(&observed),
                    first_read: true,
                })
            },
            "application/octet-stream",
            None,
        )
        .unwrap();

    first_server.join().unwrap();
    server.join().unwrap();
    assert_eq!(renewals.load(Ordering::SeqCst), 1);
    assert_eq!(factories.load(Ordering::SeqCst), 2);
    assert_eq!(*starts.lock().unwrap(), vec![0, 0]);
}
