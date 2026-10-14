// SPDX-License-Identifier: MPL-2.0

//! Transport-neutral device network-edge contracts.
//!
//! This crate contains no Iroh, QUIC, gateway, Store, JNI, or C ABI type. A
//! device host authenticates a connection and hands the resulting principal,
//! protocol, path metadata, and bounded byte streams to a product server.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use koru_device::cloud::{CloudPrincipalV1, CloudReplicaPrincipalV1};
use koru_device::{DeviceId, RelationshipId, ReplicationEndpointId};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

/// A product protocol selected at the authenticated transport boundary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProtocolToken(String);

impl ProtocolToken {
    pub fn parse(value: impl Into<String>) -> Result<Self, EdgeError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 || !value.is_ascii() {
            return Err(EdgeError::InvalidProtocol);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identity and authorization facts established by the device host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicePrincipal {
    pub device_id: DeviceId,
    pub relationship_id: RelationshipId,
    pub endpoint_id: ReplicationEndpointId,
    /// Monotonic revision of the authorization snapshot used for admission.
    pub authorization_revision: u64,
}

/// Who is on the other end of an authenticated edge session: a paired device,
/// or a Cloud authority context (HOST-CONTROL §2).
///
/// The JSON is the externally specified tagged form, with no untagged
/// fallback: exactly `{"kind":"device","device":{…}}` or
/// `{"kind":"cloud","cloud":{…}}`. An unknown tag or an unknown field closes
/// admission rather than selecting the other arm.
///
/// **A device-only handler rejects Cloud; it never downcasts it.** That is why
/// this is a union at the edge rather than a `DevicePrincipal` with optional
/// Cloud fields — there is no shape in which a Cloud context can be read as a
/// bilateral device trust principal by accident.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkPrincipal {
    Device { device: DevicePrincipal },
    Cloud { cloud: CloudPrincipalV1 },
}

impl NetworkPrincipal {
    pub fn device(&self) -> Option<&DevicePrincipal> {
        match self {
            Self::Device { device } => Some(device),
            Self::Cloud { .. } => None,
        }
    }

    pub fn cloud(&self) -> Option<&CloudPrincipalV1> {
        match self {
            Self::Cloud { cloud } => Some(cloud),
            Self::Device { .. } => None,
        }
    }

    pub fn is_cloud(&self) -> bool {
        matches!(self, Self::Cloud { .. })
    }
}

impl From<DevicePrincipal> for NetworkPrincipal {
    fn from(device: DevicePrincipal) -> Self {
        Self::Device { device }
    }
}

impl From<CloudPrincipalV1> for NetworkPrincipal {
    fn from(cloud: CloudPrincipalV1) -> Self {
        Self::Cloud { cloud }
    }
}

/// What an egress open is asked to reach. A Cloud target carries no
/// caller-supplied socket, issuer or credential: the Agent resolves its own
/// in-memory permit, rechecks generations and current authority, and dials its
/// authenticated route. A permit cannot authorize a peer protocol token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkTarget {
    Device {
        device_id: DeviceId,
    },
    Cloud {
        permit_id: Uuid,
        dataset: CloudReplicaPrincipalV1,
    },
}

impl NetworkTarget {
    pub fn device_id(&self) -> Option<DeviceId> {
        match self {
            Self::Device { device_id } => Some(*device_id),
            Self::Cloud { .. } => None,
        }
    }
}

impl From<DeviceId> for NetworkTarget {
    fn from(device_id: DeviceId) -> Self {
        Self::Device { device_id }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathKind {
    Direct,
    Relay,
    Unknown,
}

/// Observable transport facts. They carry no routable Iroh address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMetadata {
    pub kind: PathKind,
    pub outbound: bool,
    /// **The path that was actually carrying application data** when this
    /// session or connection was observed — D246's byte gate, and a different
    /// question from [`Self::kind`].
    ///
    /// `kind` answers *"is a direct path available at all"*, which reads
    /// `Direct` whenever any IP path exists whether or not a packet has ever
    /// gone over it. That is the right answer for connection reporting and the
    /// wrong one for admitting a person's bytes, so the two are separate
    /// fields rather than one field with two readers.
    ///
    /// `#[serde(default)]` — **false when absent**. An older peer of this
    /// bridge does not send it, and the safe reading of "I could not observe
    /// the selected path" is "do not carry blob bytes over it".
    #[serde(default)]
    pub selected_is_ip: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionMetadata {
    pub connection_id: u64,
    pub principal: NetworkPrincipal,
    pub path: PathMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionEvent {
    Connected(ConnectionMetadata),
    Disconnected { connection_id: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalError {
    Cancelled,
    Reset(u64),
    Transport(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeError {
    InvalidProtocol,
    Closed,
    Cancelled,
    Reset(u64),
}

impl fmt::Display for EdgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProtocol => formatter.write_str("invalid network protocol token"),
            Self::Closed => formatter.write_str("network session is closed"),
            Self::Cancelled => formatter.write_str("network session was cancelled"),
            Self::Reset(code) => write!(formatter, "network session was reset ({code})"),
        }
    }
}

impl std::error::Error for EdgeError {}

/// Bounded admission of authenticated product sessions into a host.
pub trait NetworkIngress: Send + Sync + 'static {
    fn admit(&self, session: TransportSession) -> Result<(), EdgeError>;

    fn connected(&self, _connection: ConnectionMetadata) -> Result<(), EdgeError> {
        Ok(())
    }

    fn disconnected(&self, _connection_id: u64) {}
}

/// Opens a new product stream through the transport owner without exposing a
/// routable address or transport-specific connection handle.
pub trait NetworkEgress: Send + Sync + 'static {
    fn open(
        &self,
        target: NetworkTarget,
        protocol: ProtocolToken,
    ) -> Pin<Box<dyn Future<Output = Result<TransportSession, EdgeError>> + Send + '_>>;
}

/// One bounded, ordered byte direction. Dropping the sender half-closes it;
/// reset and cancellation are out-of-band and cannot sit behind payload.
#[derive(Debug)]
pub struct ByteReceiver {
    receiver: mpsc::Receiver<Vec<u8>>,
    terminal: Arc<std::sync::Mutex<Option<TerminalError>>>,
}

#[derive(Clone, Debug)]
pub struct ByteSender {
    sender: mpsc::Sender<Vec<u8>>,
    terminal: Arc<std::sync::Mutex<Option<TerminalError>>>,
}

impl ByteSender {
    /// Wake the transport pump when its consumer leaves, even if the remote
    /// stream is idle and never sends another byte.
    pub async fn closed(&self) {
        self.sender.closed().await
    }

    pub async fn send(&self, bytes: Vec<u8>) -> Result<(), EdgeError> {
        if let Some(error) = self.terminal.lock().expect("terminal state").clone() {
            return Err(edge_error(error));
        }
        self.sender.send(bytes).await.map_err(|_| EdgeError::Closed)
    }

    pub fn try_send(&self, bytes: Vec<u8>) -> Result<(), mpsc::error::TrySendError<Vec<u8>>> {
        self.sender.try_send(bytes)
    }

    pub fn blocking_send(&self, bytes: Vec<u8>) -> Result<(), EdgeError> {
        if let Some(error) = self.terminal.lock().expect("terminal state").clone() {
            return Err(edge_error(error));
        }
        self.sender
            .blocking_send(bytes)
            .map_err(|_| EdgeError::Closed)
    }
}

impl ByteReceiver {
    pub async fn recv(&mut self) -> Result<Option<Vec<u8>>, EdgeError> {
        match self.receiver.recv().await {
            Some(bytes) => Ok(Some(bytes)),
            None => match self.terminal.lock().expect("terminal state").clone() {
                Some(error) => Err(edge_error(error)),
                None => Ok(None),
            },
        }
    }
}

fn edge_error(error: TerminalError) -> EdgeError {
    match error {
        TerminalError::Cancelled => EdgeError::Cancelled,
        TerminalError::Reset(code) => EdgeError::Reset(code),
        TerminalError::Transport(_) => EdgeError::Closed,
    }
}

/// A transport-neutral accepted or outbound product session.
#[derive(Debug)]
pub struct TransportSession {
    /// Correlates this logical byte stream across a process bridge. It carries
    /// no document or protocol meaning and may be reminted after a restart.
    pub session_id: u64,
    /// Correlates streams sharing one authenticated transport connection.
    pub connection_id: u64,
    pub principal: NetworkPrincipal,
    pub protocol: ProtocolToken,
    pub path: PathMetadata,
    pub send: ByteSender,
    pub receive: ByteReceiver,
    cancelled: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub struct SessionControl {
    terminal: Arc<std::sync::Mutex<Option<TerminalError>>>,
    cancelled: Arc<AtomicBool>,
}

impl SessionControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        *self.terminal.lock().expect("terminal state") = Some(TerminalError::Cancelled);
    }

    pub fn reset(&self, code: u64) {
        *self.terminal.lock().expect("terminal state") = Some(TerminalError::Reset(code));
    }
}

impl TransportSession {
    pub fn pair(
        a_principal: NetworkPrincipal,
        b_principal: NetworkPrincipal,
        protocol: ProtocolToken,
        capacity: usize,
    ) -> (Self, Self) {
        assert!(
            capacity > 0,
            "network-edge stream capacity must be non-zero"
        );
        let (a_to_b_tx, a_to_b_rx) = mpsc::channel(capacity);
        let (b_to_a_tx, b_to_a_rx) = mpsc::channel(capacity);
        let terminal = Arc::new(std::sync::Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        (
            Self {
                session_id: 0,
                connection_id: 0,
                principal: a_principal,
                protocol: protocol.clone(),
                path: PathMetadata {
                    kind: PathKind::Unknown,
                    outbound: true,
                    selected_is_ip: false,
                },
                send: ByteSender {
                    sender: a_to_b_tx,
                    terminal: Arc::clone(&terminal),
                },
                receive: ByteReceiver {
                    receiver: b_to_a_rx,
                    terminal: Arc::clone(&terminal),
                },
                cancelled: Arc::clone(&cancelled),
            },
            Self {
                session_id: 0,
                connection_id: 0,
                principal: b_principal,
                protocol,
                path: PathMetadata {
                    kind: PathKind::Unknown,
                    outbound: false,
                    selected_is_ip: false,
                },
                send: ByteSender {
                    sender: b_to_a_tx,
                    terminal: Arc::clone(&terminal),
                },
                receive: ByteReceiver {
                    receiver: a_to_b_rx,
                    terminal: Arc::clone(&terminal),
                },
                cancelled,
            },
        )
    }

    pub fn cancel(&self) {
        self.control().cancel();
    }

    pub fn reset(&self, code: u64) {
        self.control().reset(code);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn control(&self) -> SessionControl {
        SessionControl {
            terminal: Arc::clone(&self.send.terminal),
            cancelled: Arc::clone(&self.cancelled),
        }
    }
}

#[cfg(test)]
mod tests;
