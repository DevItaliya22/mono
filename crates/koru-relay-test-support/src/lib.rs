// SPDX-License-Identifier: MPL-2.0

//! In-memory iroh relay servers for tests.
//!
//! Charter: this crate starts and tears down in-memory iroh relay servers for
//! tests, returning the relay URL/handle, and nothing else. It is consumed
//! only as a dev-dependency; nothing shipped links it.
//!
//! The server binds plain HTTP on a `127.0.0.1` OS-assigned port — no TLS, no
//! ACME, no QUIC address discovery, no metrics — which keeps repository tests
//! off disk, DNS and the internet. iroh endpoints accept an `http://` relay
//! URL directly (the client speaks `ws://` to it), so no certificate skipping
//! is needed anywhere.
//!
//! Teardown is guaranteed twice over: [`TestRelayServer::shutdown`] stops the
//! server gracefully and waits for its tasks, and simply dropping the handle
//! aborts every server task — the upstream server is written in structured-
//! concurrency style where tasks cannot outlive their handle. A test that
//! panics therefore cannot leak a relay.

use std::net::Ipv4Addr;

use iroh::RelayUrl;
use iroh_relay::server::{RelayConfig, Server, ServerConfig, SpawnError, SupervisorError};

/// A running in-memory relay server for tests.
///
/// Start one with [`TestRelayServer::start`], hand [`TestRelayServer::url`]
/// to the endpoints under test, and either call
/// [`TestRelayServer::shutdown`] or let the drop reap it.
#[derive(Debug)]
pub struct TestRelayServer {
    url: RelayUrl,
    server: Server,
}

impl TestRelayServer {
    /// Starts a relay server on a `127.0.0.1` OS-assigned port, plain HTTP.
    pub async fn start() -> Result<Self, SpawnError> {
        let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
        // The default is sized for a public relay; a test sees a handful of
        // keys. Same value iroh's own test helpers use.
        relay.key_cache_capacity = Some(1024);
        let mut config = ServerConfig::default();
        config.relay = Some(relay);
        let server = Server::spawn(config).await?;
        let addr = server
            .http_addr()
            .expect("a relay server spawned with a relay config has an HTTP address");
        let url: RelayUrl = format!("http://{addr}")
            .parse()
            .expect("a bound socket address forms a valid URL");
        Ok(Self { url, server })
    }

    /// The relay's URL, e.g. `http://127.0.0.1:54321`.
    ///
    /// Usable directly as an iroh relay URL: in a custom relay map, or in
    /// Koru terms as a `RelayProviderConfig::CustomUrls` entry.
    pub fn url(&self) -> &RelayUrl {
        &self.url
    }

    /// Gracefully shuts the server down and waits for its tasks to finish.
    ///
    /// Dropping the handle instead also reaps every server task, just
    /// without waiting.
    pub async fn shutdown(self) -> Result<(), SupervisorError> {
        self.server.shutdown().await
    }
}

#[cfg(test)]
mod tests;
