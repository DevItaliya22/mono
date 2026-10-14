// SPDX-License-Identifier: MPL-2.0

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use super::*;

/// The one required line, for tests about the other keys.
const HOSTNAMES: &str = "hostnames = [\"relay.example.org\"]\n";

/// There is no default hostname: a relay run with an empty file would
/// otherwise order a certificate for names that are not its operator's.
#[test]
fn an_empty_file_is_refused_naming_hostnames() {
    let error = parse("").unwrap_err();
    let ConfigError::Invalid(message) = &error else {
        panic!("expected an invalid-config refusal, got {error:?}");
    };
    assert!(
        message.contains("hostnames"),
        "setting not named: {message}"
    );
    assert!(message.contains("required"), "{message}");
}

#[test]
fn a_file_with_only_hostnames_takes_every_other_default() {
    let config = parse(HOSTNAMES).unwrap();
    assert_eq!(config.hostnames, vec!["relay.example.org"]);
    assert_eq!(config.http_port, 80);
    assert_eq!(config.https_port, 443);
    assert_eq!(config.quic_port, 7842);
    assert_eq!(
        config.acme_cache_dir,
        PathBuf::from("/var/lib/koru-relay/acme")
    );
    assert_eq!(config.acme_contact, None);
    assert!(config.acme_production);
    assert_eq!(
        config.metrics_bind,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090)
    );
}

/// The example in `--help` claims every value it writes out besides
/// `hostnames` is the default. This is that claim, held.
#[test]
fn the_example_is_equivalent_to_its_hostnames_line_alone() {
    assert_eq!(parse(EXAMPLE).unwrap(), parse(HOSTNAMES).unwrap());
}

#[test]
fn every_field_can_be_overridden() {
    let config = parse(
        r#"
hostnames = ["relay.example.org"]
http_port = 8080
https_port = 8443
quic_port = 4242
acme_cache_dir = "/tmp/acme"
acme_contact = "ops@example.org"
acme_production = false
metrics_bind = "127.0.0.1:9999"
"#,
    )
    .unwrap();
    assert_eq!(config.hostnames, vec!["relay.example.org"]);
    assert_eq!(config.http_port, 8080);
    assert_eq!(config.https_port, 8443);
    assert_eq!(config.quic_port, 4242);
    assert_eq!(config.acme_cache_dir, PathBuf::from("/tmp/acme"));
    assert_eq!(config.acme_contact.as_deref(), Some("ops@example.org"));
    assert!(!config.acme_production);
    assert_eq!(
        config.metrics_bind,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9999)
    );
}

/// A key that is not understood is not run — and the refusal names the key.
#[test]
fn an_unknown_key_is_refused_by_name() {
    let error = parse(&format!("{HOSTNAMES}qic_port = 7842\n")).unwrap_err();
    let ConfigError::Syntax(message) = &error else {
        panic!("expected a syntax refusal, got {error:?}");
    };
    assert!(message.contains("qic_port"), "key not named: {message}");
}

#[test]
fn broken_toml_is_a_syntax_error() {
    assert!(matches!(
        parse("hostnames = [").unwrap_err(),
        ConfigError::Syntax(_)
    ));
}

#[test]
fn no_hostnames_cannot_be_served() {
    assert!(matches!(
        parse("hostnames = []\n").unwrap_err(),
        ConfigError::Invalid(_)
    ));
}

#[test]
fn a_hostname_must_be_bare() {
    for bad in [
        "hostnames = [\"https://relay.example.org\"]\n",
        "hostnames = [\"relay.example.org:443\"]\n",
        "hostnames = [\"relay.example.org/relay\"]\n",
        "hostnames = [\"\"]\n",
    ] {
        let error = parse(bad).unwrap_err();
        assert!(
            matches!(error, ConfigError::Invalid(_)),
            "{bad:?} was not refused as invalid: {error:?}"
        );
    }
}

#[test]
fn a_duplicate_hostname_is_refused() {
    let error = parse("hostnames = [\"a.example.org\", \"a.example.org\"]\n").unwrap_err();
    assert!(matches!(error, ConfigError::Invalid(_)), "{error:?}");
}

#[test]
fn http_and_https_cannot_share_a_port() {
    let error = parse(&format!("{HOSTNAMES}http_port = 443\n")).unwrap_err();
    let ConfigError::Invalid(message) = &error else {
        panic!("expected an invalid-config refusal, got {error:?}");
    };
    assert!(message.contains("http_port"), "{message}");
}

#[test]
fn a_contact_that_is_not_an_email_is_refused() {
    let error = parse(&format!("{HOSTNAMES}acme_contact = \"ops.example.org\"\n")).unwrap_err();
    let ConfigError::Invalid(message) = &error else {
        panic!("expected an invalid-config refusal, got {error:?}");
    };
    assert!(message.contains("acme_contact"), "{message}");
}

#[test]
fn the_server_config_binds_where_the_file_says() {
    let config = parse(
        r#"
hostnames = ["relay.example.org"]
http_port = 8080
https_port = 8443
quic_port = 4242
metrics_bind = "127.0.0.1:9999"
"#,
    )
    .unwrap();
    let server = config.server_config();
    let relay = server.relay.expect("a relay is configured");
    assert_eq!(relay.http_bind_addr.port(), 8080);
    assert!(relay.http_bind_addr.ip().is_unspecified());
    let tls = relay.tls.expect("production config always has TLS");
    assert_eq!(tls.https_bind_addr.port(), 8443);
    assert!(tls.https_bind_addr.ip().is_unspecified());
    let quic = server.quic.expect("QAD is configured");
    assert_eq!(quic.bind_addr.port(), 4242);
    // No TLS config of its own: QAD inherits the relay's ACME certificate.
    assert!(quic.server_config.is_none());
    assert_eq!(
        server.metrics_addr,
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9999))
    );
}

/// The one-certificate-both-hostnames wiring. `AcmeConfig`'s fields are
/// `pub(crate)` upstream, so the only external window onto the domain list
/// is its `Debug` rendering — a deliberate trade: if upstream changes that
/// rendering this fails loudly and gets re-verified, which is exactly what a
/// certificate-scope regression deserves.
#[test]
fn every_hostname_reaches_the_one_acme_order() {
    let server = parse("hostnames = [\"relay.example.org\", \"relay2.example.org\"]\n")
        .unwrap()
        .server_config();
    let tls = server
        .relay
        .expect("a relay is configured")
        .tls
        .expect("production config always has TLS");
    let rendered = format!("{:?}", tls.cert);
    assert!(rendered.contains("LetsEncrypt"), "{rendered}");
    for hostname in ["relay.example.org", "relay2.example.org"] {
        assert!(
            rendered.contains(hostname),
            "{hostname} missing: {rendered}"
        );
    }
    assert!(
        rendered.contains("/var/lib/koru-relay/acme"),
        "cache dir missing: {rendered}"
    );
}

#[test]
fn the_local_config_is_loopback_plain_http_without_qad() {
    let metrics = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 39999);
    let server = local_server_config(metrics);
    let relay = server.relay.expect("a relay is configured");
    assert!(relay.http_bind_addr.ip().is_loopback());
    assert_eq!(
        relay.http_bind_addr.port(),
        0,
        "ephemeral, not a fixed port"
    );
    assert!(relay.tls.is_none(), "--local never touches TLS");
    assert!(server.quic.is_none(), "QAD requires TLS");
    assert_eq!(server.metrics_addr, Some(metrics));
}
