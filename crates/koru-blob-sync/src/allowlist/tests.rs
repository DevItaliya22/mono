// SPDX-License-Identifier: MPL-2.0

use super::*;

fn us() -> ByteOrigins {
    ByteOrigins::for_region(Region::Us)
}

/// The two US hosts, over `https`, and nothing else.
#[test]
fn the_region_set_is_the_regions_object_hosts_over_https() {
    let origins = us();
    assert!(
        origins
            .admit("https://us.data.koru.do/v1/objects/uploads/abc")
            .is_ok()
    );
    assert!(origins.admit("https://data.koru.do/v1/objects/abc").is_ok());
    assert_eq!(
        origins.admit("https://eu.data.koru.do/v1/objects/abc"),
        Err(OriginRefused::NotAllowed)
    );
    assert_eq!(
        ByteOrigins::for_region(Region::Eu).admit("https://us.data.koru.do/v1/objects/abc"),
        Err(OriginRefused::NotAllowed)
    );
    assert_eq!(
        ByteOrigins::for_region(Region::Eu)
            .admit("https://eu.data.koru.do/v1/objects/abc")
            .map(|url| url.as_url().host_str().unwrap_or_default().to_string()),
        Ok("eu.data.koru.do".to_string())
    );
}

/// **The clause this module exists for.** A control response that names
/// somewhere else is refused before a byte moves.
#[test]
fn a_returned_url_outside_the_allowlist_is_refused() {
    let origins = us();
    for hostile in [
        "https://evil.example.com/v1/objects/uploads/abc",
        // The allowed host as a *prefix* of somebody else's.
        "https://us.data.koru.do.evil.example.com/v1/objects/abc",
        // The allowed host in the path, which is not the host at all.
        "https://evil.example.com/us.data.koru.do/v1/objects/abc",
        // The allowed host as userinfo, the oldest trick of the set.
        "https://us.data.koru.do@evil.example.com/v1/objects/abc",
        // A second port on the right host is a second service.
        "https://us.data.koru.do:8443/v1/objects/abc",
    ] {
        assert!(origins.admit(hostile).is_err(), "{hostile} was admitted");
    }
}

/// Cleartext is refused even on a host that is otherwise held: the whole
/// point of holding an origin rather than a host is that the scheme is part of
/// the promise.
#[test]
fn cleartext_on_a_held_host_is_still_refused() {
    assert_eq!(
        us().admit("http://us.data.koru.do/v1/objects/abc"),
        Err(OriginRefused::Scheme)
    );
    assert_eq!(
        us().admit("ftp://us.data.koru.do/v1/objects/abc"),
        Err(OriginRefused::Scheme)
    );
}

/// A byte route takes no query and no fragment, so one that arrived is a
/// response asking for something the contract does not describe.
#[test]
fn a_url_with_a_query_or_a_fragment_is_refused_on_an_allowed_host() {
    for shaped in [
        "https://us.data.koru.do/v1/objects/abc?redirect=https://evil.example.com",
        "https://us.data.koru.do/v1/objects/abc#fragment",
    ] {
        assert_eq!(us().admit(shaped), Err(OriginRefused::Shape), "{shaped}");
    }
    assert_eq!(us().admit("not a url"), Err(OriginRefused::Unparseable));
}

/// A local stack is admitted only when a person named it, and only on an
/// address that cannot be the public internet.
#[test]
fn a_development_origin_is_explicit_and_local_only() {
    let origins = ByteOrigins::none()
        .with_development_origins("http://127.0.0.1:8081, http://192.168.1.20:8081")
        .unwrap();
    assert!(
        origins
            .admit("http://127.0.0.1:8081/v1/objects/uploads/abc")
            .is_ok()
    );
    assert!(
        origins
            .admit("http://192.168.1.20:8081/v1/objects/abc")
            .is_ok()
    );
    // The same host on another port is another service.
    assert!(origins.admit("http://127.0.0.1:9/v1/objects/abc").is_err());
    for public in [
        "http://93.184.216.34:8081",
        "https://evil.example.com",
        "http://localhost.example.com:8081",
    ] {
        assert_eq!(
            ByteOrigins::none().with_development_origins(public),
            Err(OriginRefused::NotAllowed),
            "{public}"
        );
    }
    // An argument that is not understood is not run.
    assert!(
        ByteOrigins::none()
            .with_development_origins("127.0.0.1:8081")
            .is_err()
    );
    assert_eq!(
        ByteOrigins::none().with_development_origins("http://127.0.0.1:8081/v1"),
        Err(OriginRefused::Shape)
    );
}

/// `localhost` matched exactly, and the default port spelled one way.
#[test]
fn one_origin_has_one_spelling() {
    let origins = ByteOrigins::none()
        .with_development_origins("http://localhost:8081")
        .unwrap();
    assert_eq!(
        origins.origins().collect::<Vec<_>>(),
        ["http://localhost:8081"]
    );
    let implicit = ByteOrigins::none()
        .with_development_origins("http://localhost")
        .unwrap();
    assert!(implicit.admit("http://localhost/v1/objects/abc").is_ok());
    // `:80` on `http` is the same origin, and `url` says so by normalising it
    // away. One origin, one spelling — including when the response spells it
    // the long way.
    assert!(implicit.admit("http://localhost:80/v1/objects/abc").is_ok());
    assert!(
        implicit
            .admit("http://localhost:8080/v1/objects/abc")
            .is_err()
    );
}

/// Nothing is admitted before a region is known.
#[test]
fn an_empty_set_admits_nothing() {
    assert_eq!(
        ByteOrigins::none().admit("https://us.data.koru.do/v1/objects/abc"),
        Err(OriginRefused::NotAllowed)
    );
}

/// The refusal names the rejected value, and a hostile one cannot forge a
/// second log line or choose how long ours is.
///
/// The `warn!` itself is not asserted here — a subscriber in a unit test would
/// be testing `tracing` — but the one part with a rule of its own is the
/// rendering, and that is exactly what a control response gets to influence.
#[test]
fn a_rejected_byte_url_is_bounded_and_cannot_forge_a_log_line() {
    assert_eq!(Rejected("").to_string(), "<empty>");
    assert_eq!(Rejected("   ").to_string(), "<empty>");
    assert_eq!(
        Rejected("https://us.data.koru.do/v1/objects/abc").to_string(),
        "https://us.data.koru.do/v1/objects/abc"
    );
    // A newline, a carriage return and a NUL each become one visible marker,
    // so nothing downstream reads the tail as a record of its own.
    assert_eq!(
        Rejected("https://evil.test/a\r\nWARN forged\0b").to_string(),
        "https://evil.test/a\u{fffd}\u{fffd}WARN forged\u{fffd}b"
    );
    // Long values are truncated with a mark, so the reader knows the value
    // continued rather than believing they were shown all of it.
    let long = format!("https://evil.test/{}", "a".repeat(4096));
    let rendered = Rejected(&long).to_string();
    assert_eq!(
        rendered.chars().count(),
        257,
        "256 characters plus the mark"
    );
    assert!(rendered.ends_with('…'));
    // A value of exactly the limit is shown whole, with no mark.
    let exact = "a".repeat(256);
    let rendered = Rejected(&exact).to_string();
    assert_eq!(rendered, exact);
    assert!(!rendered.ends_with('…'));
}
