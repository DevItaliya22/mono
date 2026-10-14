// SPDX-License-Identifier: MPL-2.0

//! The dedicated worker process's one endpoint, using Iroh's existing transport.
//! This module does not load secrets, register readiness or grant authority.

use std::net::SocketAddr;

use iroh::{Endpoint, RelayMap, RelayMode, endpoint::presets};
use koru_device::ReplicationEndpointSecret;
use koru_replication::cloud::CLOUD_ALPN_V1;

use crate::RelayProviderConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerBindError {
    InvalidBindAddress,
    InvalidAdvertisedAddress,
    InvalidRelayConfiguration,
    BindFailed,
}

/// Bind exactly the configured UDP socket and advertise the supplied mapped
/// address. Port zero is refused: a Docker mapping must not target a random port.
///
/// The caller supplies the protected endpoint secret, not the device signing
/// key. It must retain the returned endpoint and await its close on shutdown.
/// The bound socket is only one readiness prerequisite, never an authority permit.
///
/// Public address lookup and implicit/default relays are never installed.
/// Disabled relays and development HTTP relays support explicit isolated
/// deployments; the production host must require its trusted HTTPS relay list.
/// This is not a second endpoint for desktop/mobile clients.
pub async fn bind(
    secret: &ReplicationEndpointSecret,
    bind_addr: SocketAddr,
    advertised_addr: SocketAddr,
    relay_provider: &RelayProviderConfig,
) -> Result<Endpoint, WorkerBindError> {
    if bind_addr.port() == 0 || bind_addr.ip().is_multicast() {
        return Err(WorkerBindError::InvalidBindAddress);
    }
    if advertised_addr.port() == 0
        || advertised_addr.ip().is_unspecified()
        || advertised_addr.ip().is_multicast()
        || advertised_addr.is_ipv4() != bind_addr.is_ipv4()
        || matches!(advertised_addr.ip(), std::net::IpAddr::V4(ip) if ip.is_broadcast())
    {
        return Err(WorkerBindError::InvalidAdvertisedAddress);
    }
    let relay_mode = match relay_provider {
        RelayProviderConfig::Disabled => RelayMode::Disabled,
        RelayProviderConfig::CustomUrls(urls) if !urls.is_empty() => RelayMode::Custom(
            RelayMap::try_from_iter(urls.iter().map(String::as_str))
                .map_err(|_| WorkerBindError::InvalidRelayConfiguration)?,
        ),
        _ => return Err(WorkerBindError::InvalidRelayConfiguration),
    };
    // Clear both default address families: bind_addr alone replaces only its
    // own family, leaving an unconfigured ephemeral socket in the other.
    Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .bind_addr(bind_addr)
        .map_err(|_| WorkerBindError::InvalidBindAddress)?
        .external_addr(advertised_addr)
        .relay_mode(relay_mode)
        .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
        .secret_key(iroh::SecretKey::from_bytes(&secret.secret_bytes()))
        .alpns(vec![CLOUD_ALPN_V1.to_vec()])
        .bind()
        .await
        .map_err(|_| WorkerBindError::BindFailed)
}
