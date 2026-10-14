// SPDX-License-Identifier: MPL-2.0

//! The relay's configuration: a small TOML file, and what it becomes.
//!
//! **`hostnames` is required and has no default**: they are the names the
//! relay orders its certificate for, so they can only be the operator's own.
//! Every other key is optional and defaults to a public deployment — ports
//! 80/443, QAD on UDP 7842, the ACME cache under `/var/lib/koru-relay/acme`,
//! metrics private on loopback 9090. A developer's `--local` run needs no file
//! at all and never reaches this schema's TLS half.
//!
//! The same doctrine as the command line: a key that is not understood is not
//! run (`deny_unknown_fields`), because a typo like `qic_port` silently
//! ignored is a relay listening on a port nobody meant.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use iroh_relay::server::{
    AcmeConfig, CertConfig, QuicConfig, RelayConfig, ServerConfig, TlsConfig,
};
use serde::Deserialize;

/// A worked example, shown by `--help`. Equivalent to a file holding only its
/// `hostnames` line: every other value written out here is the default, which
/// a test asserts.
pub const EXAMPLE: &str = r#"# Required: the hostnames this relay serves, as public DNS names that point
# at it. One server, one certificate: every name here becomes a
# subjectAltName of the single ACME certificate. The ACME cache is keyed by
# the hash of this whole list, so adding, removing or reordering a name
# orders a fresh certificate.
hostnames = ["relay.example.org"]

# TCP ports, bound on all interfaces. With TLS configured the relay itself
# and /healthz are served on the HTTPS port; the HTTP port answers only the
# captive-portal probe (/generate_204). ACME validation is TLS-ALPN-01, so
# port 443 must be reachable from the internet before a certificate can be
# issued.
http_port = 80
https_port = 443

# The QUIC address discovery (QAD) UDP port, serving the same certificate.
quic_port = 7842

# Where ACME certificates are cached across restarts.
acme_cache_dir = "/var/lib/koru-relay/acme"

# Optional contact email for the ACME account.
# acme_contact = "ops@example.org"

# true is Let's Encrypt production; false is the staging directory, whose
# certificates no client trusts but whose rate limits forgive rehearsals.
acme_production = true

# The Prometheus metrics bind. Loopback keeps it private without firewall
# help; it answers every path with the OpenMetrics text encoding.
metrics_bind = "127.0.0.1:9090"
"#;

/// The parsed file. See [`EXAMPLE`] for what each field means. `hostnames`
/// defaults to empty only so that its absence is refused by [`parse`] with a
/// message naming it, rather than by serde with a generic one.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub hostnames: Vec<String>,
    pub http_port: u16,
    pub https_port: u16,
    pub quic_port: u16,
    pub acme_cache_dir: PathBuf,
    pub acme_contact: Option<String>,
    pub acme_production: bool,
    pub metrics_bind: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hostnames: Vec::new(),
            http_port: 80,
            https_port: 443,
            quic_port: 7842,
            acme_cache_dir: PathBuf::from("/var/lib/koru-relay/acme"),
            acme_contact: None,
            acme_production: true,
            metrics_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090),
        }
    }
}

/// Why a file was refused. Both halves name what is wrong, because the person
/// reading them is editing the deployment's config at midnight.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// The TOML did not parse, or named a key the schema does not have.
    Syntax(String),
    /// Parsed, but describes a deployment that cannot be served.
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Syntax(message) => write!(f, "{message}"),
            ConfigError::Invalid(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Parses and validates one TOML document. No disk: the caller reads the
/// file, so every rule here is testable from a string.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let config: Config =
        toml::from_str(text).map_err(|error| ConfigError::Syntax(error.to_string()))?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &Config) -> Result<(), ConfigError> {
    if config.hostnames.is_empty() {
        return Err(ConfigError::Invalid(
            "`hostnames` is required and has no default: name the public hostname(s) \
             this relay serves, e.g. hostnames = [\"relay.example.org\"]"
                .to_string(),
        ));
    }
    for hostname in &config.hostnames {
        // A pasted URL or a stray port is the likely mistake; a hostname is
        // bare. ACME would refuse it much later and much less clearly.
        if hostname.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "hostnames must not contain an empty name".to_string(),
            ));
        }
        if hostname.contains('/') || hostname.contains(':') || hostname.contains(' ') {
            return Err(ConfigError::Invalid(format!(
                "`{hostname}` is not a bare hostname — no scheme, port or path"
            )));
        }
    }
    for (index, hostname) in config.hostnames.iter().enumerate() {
        if config.hostnames[..index].contains(hostname) {
            return Err(ConfigError::Invalid(format!(
                "`{hostname}` appears twice in hostnames"
            )));
        }
    }
    if config.http_port == config.https_port {
        return Err(ConfigError::Invalid(format!(
            "http_port and https_port are both {} — the captive-portal probe \
             must stay plain HTTP on its own port",
            config.http_port
        )));
    }
    if let Some(contact) = &config.acme_contact {
        if !contact.contains('@') {
            return Err(ConfigError::Invalid(format!(
                "acme_contact `{contact}` is not an email address"
            )));
        }
    }
    Ok(())
}

/// All interfaces: the container's published ports are the firewall.
const PUBLIC: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

/// Sized for a public relay rather than the library's small default; the
/// cache holds parsed client keys, a few hundred bytes each.
const KEY_CACHE_CAPACITY: usize = 1024;

impl Config {
    /// The upstream server configuration this file describes: HTTPS relay
    /// with one ACME certificate naming every hostname, QAD on UDP sharing
    /// that certificate, and a private metrics server.
    pub fn server_config(&self) -> ServerConfig {
        let mut relay = RelayConfig::new(SocketAddr::new(PUBLIC, self.http_port));
        relay.key_cache_capacity = Some(KEY_CACHE_CAPACITY);

        // The builder iroh-relay wants from us; the ACME resolver is
        // injected by the server. `ring` explicitly, because with several
        // rustls providers compiled in there is no default to rely on.
        let server_config_builder = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("ring supports rustls' default protocol versions")
        .with_no_client_auth();

        // One order naming every hostname: all of them become SANs of a
        // single certificate, so both public names share one handshake.
        let mut acme_config = AcmeConfig::letsencrypt(self.acme_production)
            .domains(self.hostnames.clone())
            .cache_path(self.acme_cache_dir.clone());
        if let Some(contact) = &self.acme_contact {
            acme_config = acme_config.contact(vec![format!("mailto:{contact}")]);
        }
        relay.tls = Some(TlsConfig::new(
            SocketAddr::new(PUBLIC, self.https_port),
            CertConfig::LetsEncrypt {
                acme_config,
                server_config_builder,
            },
        ));

        // No `server_config` of its own, so QAD inherits the relay's TLS —
        // including the ACME resolver and therefore the same certificate.
        let quic = QuicConfig::new(SocketAddr::new(PUBLIC, self.quic_port));

        let mut server = ServerConfig::default();
        server.relay = Some(relay);
        server.quic = Some(quic);
        server.metrics_addr = Some(self.metrics_bind);
        server
    }
}

/// The `--local` server: loopback, ephemeral port, plain HTTP, no QAD (which
/// requires TLS). `/healthz` answers on the one HTTP bind, exactly as the
/// test-support relay serves it. The metrics address is chosen by the caller
/// because the library does not report where metrics bound, so an ephemeral
/// port has to be picked before spawning.
pub fn local_server_config(metrics_addr: SocketAddr) -> ServerConfig {
    let mut relay = RelayConfig::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0));
    relay.key_cache_capacity = Some(KEY_CACHE_CAPACITY);
    let mut server = ServerConfig::default();
    server.relay = Some(relay);
    server.metrics_addr = Some(metrics_addr);
    server
}

#[cfg(test)]
mod tests;
