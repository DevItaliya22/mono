// SPDX-License-Identifier: MPL-2.0

//! Blocking invalidation watch for the Store's workspace catalog.
//!
//! This socket carries no catalog rows, workspace ids or document content: one
//! process-local generation says only that the caller should list workspaces
//! again. It is separate from both standard Loro sync rooms and the telemetry
//! event journal.

use std::net::TcpStream;

use koru_store_core::errors::{Code, Error, Result};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::http::Discovery;

/// A blocking subscription to workspace-catalog invalidations.
pub struct CatalogWatch {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl CatalogWatch {
    /// Open the dedicated Store-owned watch, authenticating on the upgrade.
    pub fn open(discovery: &Discovery) -> Result<Self> {
        use tungstenite::client::IntoClientRequest;

        let mut request = discovery
            .catalog_watch_url()
            .into_client_request()
            .map_err(|e| Error::new(Code::InvalidRequest, format!("catalog watch url: {e}")))?;
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", discovery.token).parse().map_err(|_| {
                Error::new(Code::Unauthenticated, "the token is not a header value")
            })?,
        );
        let (socket, _response) = tungstenite::connect(request).map_err(|e| {
            Error::new(
                Code::StorageFailed,
                format!("could not open the catalog watch: {e}"),
            )
        })?;
        Ok(Self { socket })
    }

    /// A handle that interrupts [`next_generation`](Self::next_generation)
    /// from another thread.
    ///
    /// The watch has no heartbeat or healthy-idle timeout: its read blocks for
    /// its lifetime. Shutting down this cloned socket is therefore the explicit
    /// close path used by an owner that is stopping or rebuilding its watches.
    pub fn abort_handle(&self) -> Option<TcpStream> {
        match self.socket.get_ref() {
            MaybeTlsStream::Plain(stream) => stream.try_clone().ok(),
            _ => None,
        }
    }

    /// Block until the current generation arrives or the watch fails.
    ///
    /// The Store sends one generation immediately after the upgrade, then one
    /// after each coalesced catalog change. The caller always reconciles from
    /// the catalog; the number is ordering evidence, never catalog content.
    pub fn next_generation(&mut self) -> Result<u64> {
        loop {
            let message = self.socket.read().map_err(|error| {
                Error::new(
                    Code::StorageFailed,
                    format!("the catalog watch ended: {error}"),
                )
            })?;
            match message {
                Message::Binary(bytes) => {
                    let frame: [u8; 8] = bytes.as_ref().try_into().map_err(|_| {
                        Error::new(
                            Code::CorruptState,
                            "the Store sent an invalid catalog generation",
                        )
                    })?;
                    return Ok(u64::from_le_bytes(frame));
                }
                Message::Close(_) => {
                    return Err(Error::new(
                        Code::StorageFailed,
                        "the Store closed the catalog watch",
                    ));
                }
                // Ping, Pong, Text and Frame are not catalog traffic.
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
