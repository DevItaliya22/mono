// SPDX-License-Identifier: MPL-2.0

//! The device's side of one Cloud dataset dial: connect the issued route on
//! the Cloud ALPN, open the dataset stream and run the four-message
//! handshake. Shared by the desktop Agent, which then bridges the stream to
//! its gateway, and the Android host, which reconciles on it in process.
//!
//! Nothing here chooses an address, retries or caches: the route is dialed
//! as issued, the coordinator owns backoff, and a refusal is returned with
//! its stable code so the caller can tell a worker that said no from a
//! network that did not answer.
pub use iroh::endpoint::Connection;
use iroh::endpoint::{RecvStream, SendStream};
use koru_account_session::TokenCapabilityVerifier;
use koru_device::{DeviceSigningKey, ReplicationEndpointId, cloud::CloudReplicaPrincipalV1};
use koru_replication::cloud::{PreparedSession, Refusal};

use crate::{CloudRoute, RuntimeHandle};
mod pool;
pub use pool::Pool;

/// One authenticated dataset stream and the connection it lives on. The
/// caller keeps `connection`: dropping the last handle closes the QUIC
/// connection under the stream.
pub struct Dialed {
    pub connection: Connection,
    pub send: SendStream,
    pub recv: RecvStream,
    pub prepared: PreparedSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialError {
    /// The connection could not be made, or was made to the wrong identity.
    /// The code is the runtime's stable dial diagnostic.
    Connect(&'static str),
    /// The connection is up but no dataset stream could be opened on it.
    Stream,
    /// The handshake was refused, by the worker or by this side's own checks.
    Refused(Refusal),
}

impl DialError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Connect(code) => code,
            Self::Stream => "cloud_stream_failed",
            Self::Refused(refusal) => refusal.code(),
        }
    }

    /// A refusal is the worker's or the credential's answer and is not
    /// retried by dialing again; everything else is the network.
    pub fn is_refusal(self) -> bool {
        matches!(self, Self::Refused(_))
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Dial and authenticate.
///
/// A session generation is **the connection's**, not the dataset's: the worker
/// allocates it from a counter it discards when the connection goes away, so
/// the first session of every connection is generation 1. `previous_generation`
/// is therefore asked for the connection that was actually opened — given its
/// stable id, it answers the highest generation this device has acknowledged on
/// *that* connection, and 0 for one it has not seen. A worker that repeats or
/// goes backwards within a connection is refused (`handshake_replay`); a fresh
/// connection legitimately starts again at 1.
#[allow(clippy::too_many_arguments)]
pub async fn open(
    runtime: &RuntimeHandle,
    route: CloudRoute,
    credential: String,
    dataset: CloudReplicaPrincipalV1,
    local_endpoint: ReplicationEndpointId,
    verifier: &TokenCapabilityVerifier,
    device_key: &DeviceSigningKey,
    previous_generation: impl Fn(u64) -> u64,
) -> Result<Dialed, DialError> {
    let route_endpoint = route.endpoint_id;
    let connection = runtime
        .connect_cloud(route)
        .await
        .map_err(DialError::Connect)?;
    let previous_generation = previous_generation(connection.stable_id() as u64);
    open_on(
        connection,
        credential,
        dataset,
        local_endpoint,
        route_endpoint,
        verifier,
        device_key,
        previous_generation,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn open_on(
    connection: Connection,
    credential: String,
    dataset: CloudReplicaPrincipalV1,
    local_endpoint: ReplicationEndpointId,
    route_endpoint: ReplicationEndpointId,
    verifier: &TokenCapabilityVerifier,
    device_key: &DeviceSigningKey,
    previous_generation: u64,
) -> Result<Dialed, DialError> {
    let (mut send, mut recv) = match connection.open_bi().await {
        Ok(streams) => streams,
        Err(_) => {
            return Err(DialError::Stream);
        }
    };
    let prepared = match super::authenticate(
        &connection,
        &mut send,
        &mut recv,
        verifier,
        credential,
        dataset,
        local_endpoint,
        route_endpoint,
        device_key,
        previous_generation,
        now_millis,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(refusal) => {
            // A refused dataset must not close other subscriptions multiplexed
            // on this connection. Reset only the failed stream.
            let _ = send.reset(1u32.into());
            let _ = recv.stop(1u32.into());
            return Err(DialError::Refused(refusal));
        }
    };
    Ok(Dialed {
        connection,
        send,
        recv,
        prepared,
    })
}
