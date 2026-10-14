// SPDX-License-Identifier: MPL-2.0

use super::*;

fn options() -> WaitOptions<'static> {
    WaitOptions {
        expected_state: "expected-state",
        timeout: Duration::from_secs(2),
        poll: Duration::from_millis(1),
        max_request_bytes: 1024,
        success_html: "received",
        error_html: "refused",
    }
}

fn send(address: SocketAddr, target: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(stream, "GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

#[test]
fn localhost_accepts_both_loopback_families_on_one_port() {
    for ipv6 in [false, true] {
        let listener = CallbackListener::bind_localhost("/oauth/callback", 0).unwrap();
        let port = listener.local_addr().port();
        assert_eq!(
            listener.redirect_uri(),
            format!("http://localhost:{port}/oauth/callback")
        );
        let addresses: Vec<_> = listener
            .listeners
            .iter()
            .map(|socket| socket.local_addr().unwrap())
            .collect();
        assert!(
            addresses
                .iter()
                .all(|address| address.ip().is_loopback() && address.port() == port)
        );
        let Some(address) = addresses
            .into_iter()
            .find(|address| address.is_ipv6() == ipv6)
        else {
            assert!(ipv6, "IPv4 listener must always be present");
            // The constructor permits this only when the OS reports IPv6
            // unsupported or unavailable; fallback policy is tested below.
            continue;
        };
        let browser = std::thread::spawn(move || {
            send(
                address,
                "/oauth/callback?state=expected-state&code=accepted",
            )
        });
        let callback = listener.wait(&options(), &|| false).unwrap();
        assert_eq!(callback.value("code"), Some("accepted"));
        assert!(browser.join().unwrap().contains("received"));
    }
}

#[test]
fn localhost_still_checks_path_and_state_on_each_family() {
    for ipv6 in [false, true] {
        for (target, expected) in [
            ("/wrong?state=expected-state&code=no", ErrorKind::WrongPath),
            (
                "/oauth/callback?state=wrong-state&code=no",
                ErrorKind::StateMismatch,
            ),
        ] {
            let listener = CallbackListener::bind_localhost("/oauth/callback", 0).unwrap();
            let Some(address) = listener
                .listeners
                .iter()
                .map(|socket| socket.local_addr().unwrap())
                .find(|address| address.is_ipv6() == ipv6)
            else {
                continue;
            };
            let browser = std::thread::spawn(move || send(address, target));
            assert_eq!(
                listener.wait(&options(), &|| false).unwrap_err().kind(),
                expected
            );
            assert!(browser.join().unwrap().contains("refused"));
        }
    }
}

#[test]
fn localhost_wait_remains_cancellable_without_a_browser() {
    let listener = CallbackListener::bind_localhost("/oauth/callback", 0).unwrap();
    assert_eq!(
        listener.wait(&options(), &|| true).unwrap_err().kind(),
        ErrorKind::Cancelled
    );
}

#[test]
fn ipv6_fallback_refuses_occupied_ports_permission_and_resource_errors() {
    for kind in [
        std::io::ErrorKind::Unsupported,
        std::io::ErrorKind::AddrNotAvailable,
    ] {
        assert!(ipv6_unavailable(&std::io::Error::from(kind)));
    }
    for kind in [
        std::io::ErrorKind::AddrInUse,
        std::io::ErrorKind::PermissionDenied,
        std::io::ErrorKind::OutOfMemory,
        std::io::ErrorKind::Other,
    ] {
        assert!(!ipv6_unavailable(&std::io::Error::from(kind)));
    }
    #[cfg(windows)]
    {
        assert!(ipv6_unavailable(&std::io::Error::from_raw_os_error(10047)));
        assert!(ipv6_unavailable(&std::io::Error::from_raw_os_error(10043)));
        assert!(!ipv6_unavailable(&std::io::Error::from_raw_os_error(10048)));
    }
}

#[test]
fn an_owned_ipv6_port_refuses_the_whole_localhost_bind() {
    let owned = match TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0)) {
        Ok(listener) => listener,
        Err(error) if ipv6_unavailable(&error) => return,
        Err(error) => panic!("could not reserve test IPv6 socket: {error}"),
    };
    let port = owned.local_addr().unwrap().port();
    // This listener is owned by the fixture, so the refusal cannot depend on
    // an ambient process or whether somebody has Koru running.
    let error = match CallbackListener::bind_localhost("/oauth/callback", port) {
        Ok(_) => panic!("must not advertise localhost with one family owned elsewhere"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), ErrorKind::Bind);
}
