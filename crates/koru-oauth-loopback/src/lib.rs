// SPDX-License-Identifier: MPL-2.0

//! The native OAuth callback mechanism shared by desktop OAuth owners.
//!
//! This crate knows how a browser returns to a native process and nothing
//! about Google, Picker, PKCE, grants, accounts, or token exchange. A listener
//! binds explicit loopback addresses, consumes exactly one
//! callback, bounds the HTTP header, validates the exact path and OAuth state,
//! percent-decodes form query values, and always attempts a small browser
//! response. Consuming [`CallbackListener`] in [`CallbackListener::wait`] is
//! the single-completion boundary.

use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};
use zeroize::Zeroize;

pub const DEFAULT_MAX_REQUEST_BYTES: usize = 8 * 1024;
pub const DEFAULT_POLL: Duration = Duration::from_millis(100);

pub struct WaitOptions<'a> {
    pub expected_state: &'a str,
    pub timeout: Duration,
    pub poll: Duration,
    pub max_request_bytes: usize,
    pub success_html: &'a str,
    pub error_html: &'a str,
}

#[derive(PartialEq, Eq)]
pub struct Callback {
    query: Vec<(String, String)>,
}

impl fmt::Debug for Callback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Callback")
            .field("field_count", &self.query.len())
            .finish_non_exhaustive()
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        zeroize_query_values(&mut self.query);
    }
}

fn zeroize_query_values(query: &mut [(String, String)]) {
    for (name, value) in query {
        name.zeroize();
        value.zeroize();
    }
}

impl Callback {
    /// Borrow each decoded field once, preserving duplicate names and order.
    pub fn fields(&self) -> impl Iterator<Item = (&str, &str)> {
        self.query
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    pub fn field_names(&self) -> impl Iterator<Item = &str> {
        self.query.iter().map(|(name, _)| name.as_str())
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
    }

    pub fn values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.query
            .iter()
            .filter_map(move |(key, value)| (key == name).then_some(value.as_str()))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Bind,
    Address,
    InvalidPath,
    InvalidState,
    InvalidTimeout,
    Arm,
    Accept,
    Read,
    Oversized,
    Malformed,
    WrongPath,
    MissingState,
    StateMismatch,
    Cancelled,
    TimedOut,
    AlreadyCompleted,
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    detail: String,
}

impl Error {
    fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for Error {}

pub struct CallbackListener {
    listeners: Vec<TcpListener>,
    callback_path: String,
    address: SocketAddr,
    redirect_host: &'static str,
}

impl CallbackListener {
    pub fn bind(callback_path: &str) -> Result<Self, Error> {
        Self::bind_port(callback_path, 0)
    }

    /// Bind an exact IPv4 loopback port for providers requiring an exact
    /// registered redirect URI. Zero retains the ephemeral-port behavior.
    pub fn bind_port(callback_path: &str, port: u16) -> Result<Self, Error> {
        if !valid_callback_path(callback_path) {
            return Err(Error::new(
                ErrorKind::InvalidPath,
                "the OAuth callback path must be one absolute path with no query or fragment",
            ));
        }
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).map_err(|error| {
                Error::new(
                    ErrorKind::Bind,
                    format!("could not open the OAuth callback listener: {error}"),
                )
            })?;
        let address = listener.local_addr().map_err(|error| {
            Error::new(
                ErrorKind::Address,
                format!("could not read the OAuth callback address: {error}"),
            )
        })?;
        Ok(Self {
            listeners: vec![listener],
            callback_path: callback_path.to_owned(),
            address,
            redirect_host: "127.0.0.1",
        })
    }

    /// Bind both explicit loopback families for a registered `localhost`
    /// callback. No DNS lookup or wildcard-interface bind is performed.
    ///
    /// IPv6 may be absent on the host; only unsupported/address-unavailable
    /// errors permit IPv4 alone. An occupied IPv6 port fails the entire bind
    /// so another process cannot receive one family's callback. Zero chooses
    /// one ephemeral port on IPv4 and uses that same port on IPv6.
    pub fn bind_localhost(callback_path: &str, port: u16) -> Result<Self, Error> {
        let mut bound = Self::bind_port(callback_path, port)?;
        let ipv6 = TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, bound.address.port()));
        match ipv6 {
            Ok(listener) => bound.listeners.push(listener),
            Err(error) if ipv6_unavailable(&error) => {}
            Err(error) => {
                return Err(Error::new(
                    ErrorKind::Bind,
                    format!("could not open the IPv6 OAuth callback listener: {error}"),
                ));
            }
        }
        bound.redirect_host = "localhost";
        Ok(bound)
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    pub fn redirect_uri(&self) -> String {
        format!(
            "http://{}:{}{}",
            self.redirect_host,
            self.address.port(),
            self.callback_path
        )
    }

    pub fn wait(
        self,
        options: &WaitOptions<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Callback, Error> {
        self.wait_one(options, cancelled)
    }

    pub fn into_two_stage(self) -> TwoStageCallbackListener {
        TwoStageCallbackListener {
            inner: self,
            completed: 0,
        }
    }

    fn wait_one(
        &self,
        options: &WaitOptions<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Callback, Error> {
        if options.expected_state.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidState,
                "the expected OAuth state must not be empty",
            ));
        }
        if options.max_request_bytes == 0 {
            return Err(Error::new(
                ErrorKind::Oversized,
                "the OAuth callback request budget is zero",
            ));
        }
        for listener in &self.listeners {
            listener.set_nonblocking(true).map_err(|error| {
                Error::new(
                    ErrorKind::Arm,
                    format!("could not arm the OAuth callback listener: {error}"),
                )
            })?;
        }
        let deadline = Instant::now().checked_add(options.timeout).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidTimeout,
                "the OAuth callback timeout is outside the supported range",
            )
        })?;
        let mut stream = 'accept: loop {
            for listener in &self.listeners {
                match listener.accept() {
                    Ok((stream, _)) => break 'accept stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        return Err(Error::new(
                            ErrorKind::Accept,
                            format!("the OAuth callback listener failed: {error}"),
                        ));
                    }
                }
            }
            wait_for_poll(deadline, options.poll, cancelled)?;
        };
        stream.set_nonblocking(true).map_err(|error| {
            Error::new(
                ErrorKind::Arm,
                format!("could not arm the OAuth callback connection: {error}"),
            )
        })?;

        let parsed = read_request(
            &mut stream,
            &self.callback_path,
            options.max_request_bytes,
            deadline,
            options.poll,
            cancelled,
        );
        let callback = parsed.and_then(|query| validate_state(query, options.expected_state));
        let body = if callback.is_ok() {
            options.success_html
        } else {
            options.error_html
        };
        write_browser_response(&mut stream, body);
        callback
    }
}

fn ipv6_unavailable(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::Unsupported | std::io::ErrorKind::AddrNotAvailable
    ) {
        return true;
    }
    // std does not consistently classify EAFNOSUPPORT/EPROTONOSUPPORT on all
    // supported hosts. These are the native socket codes, never EADDRINUSE.
    #[cfg(windows)]
    let unsupported = matches!(error.raw_os_error(), Some(10047 | 10043));
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let unsupported = matches!(error.raw_os_error(), Some(97 | 93));
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    let unsupported = matches!(error.raw_os_error(), Some(47 | 43));
    #[cfg(not(any(
        windows,
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    let unsupported = false;
    unsupported
}

#[cfg(test)]
mod localhost_tests;

/// Exactly two independently state-authenticated callbacks on one bound port.
pub struct TwoStageCallbackListener {
    inner: CallbackListener,
    completed: u8,
}

impl TwoStageCallbackListener {
    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr()
    }
    pub fn redirect_uri(&self) -> String {
        self.inner.redirect_uri()
    }
    pub fn wait_stage(
        &mut self,
        options: &WaitOptions<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Callback, Error> {
        if self.completed >= 2 {
            return Err(Error::new(
                ErrorKind::AlreadyCompleted,
                "the two-stage OAuth callback lifecycle is complete",
            ));
        }
        let callback = self.inner.wait_one(options, cancelled)?;
        self.completed += 1;
        Ok(callback)
    }
}

fn valid_callback_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && !path.starts_with("//")
        && !path.contains('?')
        && !path.contains('#')
        && !path.chars().any(char::is_whitespace)
}

fn wait_for_poll(
    deadline: Instant,
    poll: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    if cancelled() {
        return Err(Error::new(
            ErrorKind::Cancelled,
            "the OAuth callback was cancelled",
        ));
    }
    let now = Instant::now();
    if now >= deadline {
        return Err(Error::new(
            ErrorKind::TimedOut,
            "timed out waiting for the OAuth callback",
        ));
    }
    std::thread::sleep(poll.min(deadline.saturating_duration_since(now)));
    Ok(())
}

fn read_request(
    stream: &mut TcpStream,
    expected_path: &str,
    max_bytes: usize,
    deadline: Instant,
    poll: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<(String, String)>, Error> {
    let mut bytes = Vec::with_capacity(max_bytes.min(1024));
    let mut chunk = [0_u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => {
                return Err(Error::new(
                    ErrorKind::Malformed,
                    "the OAuth callback ended before its HTTP header was complete",
                ));
            }
            Ok(read) => {
                if bytes.len().saturating_add(read) > max_bytes {
                    return Err(Error::new(
                        ErrorKind::Oversized,
                        "the OAuth callback request exceeded its byte limit",
                    ));
                }
                bytes.extend_from_slice(&chunk[..read]);
                if header_end(&bytes).is_some() {
                    return parse_request(&bytes, expected_path);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_for_poll(deadline, poll, cancelled)?;
            }
            Err(error) => {
                return Err(Error::new(
                    ErrorKind::Read,
                    format!("could not read the OAuth callback: {error}"),
                ));
            }
        }
    }
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_request(bytes: &[u8], expected_path: &str) -> Result<Vec<(String, String)>, Error> {
    let end = header_end(bytes).ok_or_else(|| {
        Error::new(
            ErrorKind::Malformed,
            "the OAuth callback had no complete HTTP header",
        )
    })?;
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| {
        Error::new(
            ErrorKind::Malformed,
            "the OAuth callback HTTP header was not UTF-8",
        )
    })?;
    let line = header.lines().next().ok_or_else(|| {
        Error::new(
            ErrorKind::Malformed,
            "the OAuth callback had no HTTP request line",
        )
    })?;
    let mut fields = line.split_whitespace();
    let method = fields.next();
    let target = fields.next();
    let version = fields.next();
    if method != Some("GET")
        || target.is_none()
        || !matches!(version, Some("HTTP/1.0" | "HTTP/1.1"))
        || fields.next().is_some()
    {
        return Err(Error::new(
            ErrorKind::Malformed,
            "the OAuth callback request line was malformed",
        ));
    }
    let target = target.unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != expected_path || query.contains('#') {
        return Err(Error::new(
            ErrorKind::WrongPath,
            "the OAuth callback arrived on an unexpected path",
        ));
    }
    if !valid_percent_encoding(query.as_bytes()) {
        return Err(Error::new(
            ErrorKind::Malformed,
            "the OAuth callback query contained malformed percent encoding",
        ));
    }
    Ok(url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect())
}

fn valid_percent_encoding(value: &[u8]) -> bool {
    let mut index = 0;
    while index < value.len() {
        if value[index] == b'%' {
            if index + 2 >= value.len()
                || !value[index + 1].is_ascii_hexdigit()
                || !value[index + 2].is_ascii_hexdigit()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

fn validate_state(query: Vec<(String, String)>, expected_state: &str) -> Result<Callback, Error> {
    let states: Vec<&str> = query
        .iter()
        .filter_map(|(key, value)| (key == "state").then_some(value.as_str()))
        .collect();
    match states.as_slice() {
        [] => Err(Error::new(
            ErrorKind::MissingState,
            "the OAuth callback carried no state",
        )),
        [state] if *state == expected_state => Ok(Callback { query }),
        _ => Err(Error::new(
            ErrorKind::StateMismatch,
            "the OAuth callback state did not match",
        )),
    }
}

fn write_browser_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = stream.write_all(response.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS: &str = "callback accepted";
    const FAILURE: &str = "callback refused";

    fn options<'a>(state: &'a str) -> WaitOptions<'a> {
        WaitOptions {
            expected_state: state,
            timeout: Duration::from_secs(1),
            poll: Duration::from_millis(1),
            max_request_bytes: 512,
            success_html: SUCCESS,
            error_html: FAILURE,
        }
    }

    fn send(address: SocketAddr, target: &str) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn listener_is_explicit_ipv4_loopback_with_an_ephemeral_port() {
        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        assert_eq!(listener.local_addr().ip().to_string(), "127.0.0.1");
        assert_ne!(listener.local_addr().port(), 0);
        assert_eq!(
            listener.redirect_uri(),
            format!(
                "http://127.0.0.1:{}/oauth/callback",
                listener.local_addr().port()
            )
        );
    }

    #[test]
    fn callback_decodes_form_values_and_answers_the_browser() {
        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        let address = listener.local_addr();
        let browser = std::thread::spawn(move || {
            send(
                address,
                "/oauth/callback?state=s&code=4%2F0Aan+RRrs&picked=id%20a",
            )
        });
        let callback = listener.wait(&options("s"), &|| false).unwrap();
        assert_eq!(callback.value("code"), Some("4/0Aan RRrs"));
        assert_eq!(callback.value("picked"), Some("id a"));
        let response = browser.join().unwrap();
        assert!(response.contains(SUCCESS));
        assert!(response.contains("Cache-Control: no-store"));
        assert!(response.contains("Referrer-Policy: no-referrer"));
    }

    /// A browser opens the connection and sends the request a moment later.
    /// On macOS the accepted socket inherits the listener's `O_NONBLOCK` (BSD
    /// does; POSIX says not to and Linux obeys), so a blocking-style read here
    /// once answered `WouldBlock` — "Resource temporarily unavailable" — and
    /// the sign-in failed with nothing a person could act on. The read is
    /// polled against one deadline on every platform now; the delay is what
    /// makes this deterministic, because every other test writes before
    /// `accept` returns and all of them passed while that bug was live.
    #[test]
    fn a_callback_that_arrives_after_the_connection_is_still_read() {
        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        let address = listener.local_addr();
        let browser = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            std::thread::sleep(Duration::from_millis(250));
            write!(
                stream,
                "GET /oauth/callback?code=late&state=s HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        let callback = listener.wait(&options("s"), &|| false).unwrap();
        assert_eq!(callback.value("code"), Some("late"));
        assert!(browser.join().unwrap().contains(SUCCESS));
    }

    #[test]
    fn wrong_path_and_state_are_refused_with_a_browser_response() {
        for (target, kind) in [
            ("/favicon.ico?state=s", ErrorKind::WrongPath),
            (
                "/oauth/callback?state=attacker&code=x",
                ErrorKind::StateMismatch,
            ),
        ] {
            let listener = CallbackListener::bind("/oauth/callback").unwrap();
            let address = listener.local_addr();
            let browser = std::thread::spawn(move || send(address, target));
            assert_eq!(
                listener.wait(&options("s"), &|| false).unwrap_err().kind(),
                kind
            );
            assert!(browser.join().unwrap().contains(FAILURE));
        }
    }

    #[test]
    fn oversized_and_malformed_requests_are_bounded_and_refused() {
        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        let address = listener.local_addr();
        let browser = std::thread::spawn(move || {
            send(
                address,
                &format!("/oauth/callback?state=s&padding={}", "x".repeat(600)),
            )
        });
        assert_eq!(
            listener.wait(&options("s"), &|| false).unwrap_err().kind(),
            ErrorKind::Oversized
        );
        assert!(browser.join().unwrap().contains(FAILURE));

        assert_eq!(
            parse_request(b"POST /oauth/callback HTTP/1.1\r\n\r\n", "/oauth/callback")
                .unwrap_err()
                .kind(),
            ErrorKind::Malformed
        );
        for request in [
            b"GET /oauth/callback?state=s HTTP/1.evil\r\n\r\n".as_slice(),
            b"GET /oauth/callback?state=%GG HTTP/1.1\r\n\r\n".as_slice(),
            b"GET /oauth/callback?state=%2 HTTP/1.1\r\n\r\n".as_slice(),
        ] {
            assert_eq!(
                parse_request(request, "/oauth/callback")
                    .unwrap_err()
                    .kind(),
                ErrorKind::Malformed
            );
        }
    }

    #[test]
    fn timeout_and_cancel_are_distinct_and_poll_without_a_connection() {
        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        let mut timed = options("s");
        timed.timeout = Duration::from_millis(5);
        assert_eq!(
            listener.wait(&timed, &|| false).unwrap_err().kind(),
            ErrorKind::TimedOut
        );

        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        assert_eq!(
            listener.wait(&options("s"), &|| true).unwrap_err().kind(),
            ErrorKind::Cancelled
        );
    }

    #[test]
    fn empty_state_and_unrepresentable_deadline_are_rejected_before_waiting() {
        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        assert_eq!(
            listener.wait(&options(""), &|| false).unwrap_err().kind(),
            ErrorKind::InvalidState
        );

        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        let mut options = options("s");
        options.timeout = Duration::MAX;
        assert_eq!(
            listener.wait(&options, &|| false).unwrap_err().kind(),
            ErrorKind::InvalidTimeout
        );
    }

    #[test]
    fn callback_debug_is_seeded_and_values_are_zeroized() {
        let mut callback = Callback {
            query: vec![
                ("state".into(), "state-never-log".into()),
                ("code".into(), "code-never-log".into()),
                ("secret-key-never-log".into(), "ordinary".into()),
            ],
        };
        let debug = format!("{callback:?}");
        assert!(debug.contains("field_count: 3"));
        assert!(!debug.contains("state-never-log"));
        assert!(!debug.contains("code-never-log"));
        assert!(!debug.contains("secret-key-never-log"));

        zeroize_query_values(&mut callback.query);
        assert!(
            callback
                .query
                .iter()
                .all(|(name, value)| name.is_empty() && value.is_empty())
        );
    }

    #[test]
    fn one_listener_completes_with_exactly_the_first_callback() {
        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        let address = listener.local_addr();
        let first = std::thread::spawn(move || send(address, "/oauth/callback?state=s&code=first"));
        let callback = listener.wait(&options("s"), &|| false).unwrap();
        assert_eq!(callback.value("code"), Some("first"));
        assert!(first.join().unwrap().contains(SUCCESS));
        // `wait` consumed the listener. There is no API through which a second
        // callback can complete this rendezvous.
    }

    #[test]
    fn collision_uses_two_authenticated_stages_on_the_same_port() {
        let listener = CallbackListener::bind("/oauth/callback").unwrap();
        let address = listener.local_addr();
        let mut listener = listener.into_two_stage();
        let first = std::thread::spawn(move || {
            send(address, "/oauth/callback?state=original&grant=phase1")
        });
        let callback = listener
            .wait_stage(&options("original"), &|| false)
            .unwrap();
        assert_eq!(callback.value("grant"), Some("phase1"));
        assert!(first.join().unwrap().contains(SUCCESS));

        let second =
            std::thread::spawn(move || send(address, "/oauth/callback?state=fresh&grant=phase2"));
        let callback = listener.wait_stage(&options("fresh"), &|| false).unwrap();
        assert_eq!(callback.value("grant"), Some("phase2"));
        assert!(second.join().unwrap().contains(SUCCESS));
        assert_eq!(listener.local_addr(), address);
        assert_eq!(
            listener
                .wait_stage(&options("third"), &|| false)
                .unwrap_err()
                .kind(),
            ErrorKind::AlreadyCompleted
        );
    }
}
