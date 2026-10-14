// SPDX-License-Identifier: MPL-2.0

//! The library half of the `koru-relay` binary: the relay probe.
//!
//! The probe answers one question — *does this relay actually relay?* — by
//! standing up two fresh iroh endpoints whose only transport is the relay
//! under test, connecting one to the other, and exchanging a nonce. Nothing
//! short of the relay forwarding real traffic can make it pass: there are no
//! IP transports to fall back to and no address lookup to route around a dead
//! relay.
//!
//! This is production code, not a test helper. The `probe` subcommand
//! (Plan 1.3) and the deployment gates and external monitor (Milestone 3)
//! run exactly this path, which is why it lives here and the tests exercise
//! it rather than a copy.

use std::time::{Duration, Instant};

use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl, SecretKey};

/// The ALPN the probe's two endpoints speak to each other.
///
/// The relay never sees it — ALPN is inside the end-to-end QUIC handshake —
/// so this is not a wire contract with the relay, only with other probes.
pub const PROBE_ALPN: &[u8] = b"koru/relay-probe/1";

/// The nonce proves the bytes crossed the relay in both directions and were
/// not answered by a stale or foreign responder.
const NONCE_LEN: usize = 32;

/// How long a probe may take end to end before it is reported as timed out.
///
/// Covers both endpoints binding, the responder registering with the relay,
/// the connection, and the nonce round trip.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// A successful probe: the relay forwarded a nonce there and back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// The relay that was probed.
    pub relay_url: RelayUrl,
    /// Wall time from starting the endpoints to the verified echo.
    pub elapsed: Duration,
}

impl ProbeReport {
    /// The plain one-line success result (Plan 1.3's output contract).
    pub fn one_line(&self) -> String {
        format!(
            "ok {} relayed a nonce round trip in {}ms",
            self.relay_url,
            self.elapsed.as_millis()
        )
    }
}

/// Where a failed probe failed. Stages, not prose — the message beside it is
/// developer text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStage {
    /// Binding one of the two local endpoints.
    Bind,
    /// The responder endpoint registering with the relay as its home relay.
    RelayRegistration,
    /// Connecting to the responder through the relay.
    Connect,
    /// The nonce exchange over the established connection.
    Exchange,
}

impl ProbeStage {
    fn name(self) -> &'static str {
        match self {
            ProbeStage::Bind => "bind",
            ProbeStage::RelayRegistration => "relay-registration",
            ProbeStage::Connect => "connect",
            ProbeStage::Exchange => "exchange",
        }
    }
}

/// A failed probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The probe did not finish inside its time budget.
    Timeout {
        /// The budget that was exceeded.
        limit: Duration,
    },
    /// The echoed nonce did not match the one sent — something answered, but
    /// not our responder's bytes.
    NonceMismatch,
    /// The connection carries a non-relay path, so the probe would be
    /// vouching for a route it did not take.
    NotRelayed,
    /// A stage failed outright.
    Failed {
        /// The stage that failed.
        stage: ProbeStage,
        /// Developer text from the underlying error. Not stable, not for
        /// branching.
        message: String,
    },
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::Timeout { limit } => {
                write!(f, "failed: timed out after {}ms", limit.as_millis())
            }
            ProbeError::NonceMismatch => write!(f, "failed: echoed nonce did not match"),
            ProbeError::NotRelayed => {
                write!(f, "failed: connection carries a non-relay path")
            }
            ProbeError::Failed { stage, message } => {
                write!(f, "failed at {}: {message}", stage.name())
            }
        }
    }
}

impl std::error::Error for ProbeError {}

fn stage_error(stage: ProbeStage) -> impl FnOnce(&dyn std::fmt::Display) -> ProbeError {
    move |source| ProbeError::Failed {
        stage,
        message: source.to_string(),
    }
}

/// Builds an endpoint whose **only** transport is `relay_url`.
///
/// IP transports are cleared and no address lookup is configured, so every
/// connection this endpoint makes or accepts goes through the relay — there
/// is nothing else it could use. This is the construction the probe stands
/// on, exposed for anything else that needs a relay-only route.
pub async fn relay_only_endpoint(
    relay_url: RelayUrl,
    alpns: Vec<Vec<u8>>,
) -> Result<Endpoint, ProbeError> {
    Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Custom(RelayMap::from(relay_url)))
        .clear_ip_transports()
        .alpns(alpns)
        .bind()
        .await
        .map_err(|error| stage_error(ProbeStage::Bind)(&error))
}

/// Asserts that every path of `connection` is a relay path.
///
/// With IP transports cleared this cannot fail, which is exactly why it is
/// checked: if it ever does, the probe's premise is broken and the result
/// must not be trusted.
pub fn require_relayed(connection: &Connection) -> Result<(), ProbeError> {
    let paths = connection.paths();
    let mut saw_relay = false;
    for path in paths.iter() {
        if path.is_ip() {
            return Err(ProbeError::NotRelayed);
        }
        saw_relay |= path.is_relay();
    }
    if saw_relay {
        Ok(())
    } else {
        Err(ProbeError::NotRelayed)
    }
}

/// Probes `relay_url` end to end within `timeout`.
///
/// Two relay-only endpoints are created; one accepts, the other connects to
/// it through the relay and sends a random nonce, which the responder echoes
/// back. Success means the relay forwarded application traffic in both
/// directions just now.
pub async fn probe_relay(
    relay_url: RelayUrl,
    timeout: Duration,
) -> Result<ProbeReport, ProbeError> {
    let started = Instant::now();
    match tokio::time::timeout(timeout, probe_exchange(relay_url.clone())).await {
        Ok(Ok(())) => Ok(ProbeReport {
            relay_url,
            elapsed: started.elapsed(),
        }),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(ProbeError::Timeout { limit: timeout }),
    }
}

/// One line for one probe, success or failure (Plan 1.3's output contract).
pub fn one_line_result(result: &Result<ProbeReport, ProbeError>) -> String {
    match result {
        Ok(report) => report.one_line(),
        Err(error) => error.to_string(),
    }
}

async fn probe_exchange(relay_url: RelayUrl) -> Result<(), ProbeError> {
    let responder = relay_only_endpoint(relay_url.clone(), vec![PROBE_ALPN.to_vec()]).await?;
    let prober = relay_only_endpoint(relay_url.clone(), Vec::new()).await?;

    // The responder must be registered with the relay before the prober
    // dials, or the relay has nowhere to forward the handshake.
    responder.online().await;

    let responder_task = tokio::spawn(echo_once(responder.clone()));

    // The address is built from the URL under test — deliberately not from
    // `responder.addr()` — so the probe proves *this* relay, not whatever
    // the endpoint happens to advertise.
    let responder_addr = EndpointAddr::new(responder.id()).with_relay_url(relay_url);

    let exchange = async {
        let connection = prober
            .connect(responder_addr, PROBE_ALPN)
            .await
            .map_err(|error| stage_error(ProbeStage::Connect)(&error))?;
        require_relayed(&connection)?;

        // Random bytes from the same OS entropy source iroh keys use; this
        // avoids a direct rand dependency for one array.
        let nonce = SecretKey::generate().to_bytes();
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
        send.write_all(&nonce)
            .await
            .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
        send.finish()
            .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
        let mut echoed = [0u8; NONCE_LEN];
        recv.read_exact(&mut echoed)
            .await
            .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
        if echoed != nonce {
            return Err(ProbeError::NonceMismatch);
        }
        // The echo went through the relay too, or the responder invented it.
        require_relayed(&connection)?;
        connection.close(0u32.into(), b"probe complete");
        Ok(())
    };
    let result = exchange.await;

    responder_task.abort();
    prober.close().await;
    responder.close().await;
    result
}

/// Accepts one probe connection and echoes one nonce back.
async fn echo_once(endpoint: Endpoint) -> Result<(), ProbeError> {
    let incoming = endpoint
        .accept()
        .await
        .ok_or_else(|| stage_error(ProbeStage::Exchange)(&"endpoint closed while accepting"))?;
    let connection = incoming
        .await
        .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
    let (mut send, mut recv) = connection
        .accept_bi()
        .await
        .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
    let mut nonce = [0u8; NONCE_LEN];
    recv.read_exact(&mut nonce)
        .await
        .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
    send.write_all(&nonce)
        .await
        .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
    send.finish()
        .map_err(|error| stage_error(ProbeStage::Exchange)(&error))?;
    // Let the close come from the prober once it has read the echo.
    connection.closed().await;
    Ok(())
}

#[cfg(test)]
mod tests;
