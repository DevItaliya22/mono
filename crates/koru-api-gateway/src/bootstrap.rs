// SPDX-License-Identifier: MPL-2.0

//! The supervisor's bootstrap, and the two private bridges it authorizes.
//!
//! A supervised gateway is a *child*: the Agent hands it a launch bootstrap and
//! the gateway proves, before it does anything else, that the bootstrap
//! describes this root, this channel, this bridge version and **this image**.
//! Everything here is refusal machinery, and none of it reads an argument or
//! exits a process — a host passes the bootstrap in, and gets an `Err` back.
//!
//! **Ordering (3) lives in [`crate::start`], not here**: both bridges connect
//! before the rendezvous binds, so a child is never discoverable before the
//! Agent's edge into it exists.

use koru_network_bridge::LaunchBootstrap;

/// Refuse a bootstrap that does not describe this gateway.
///
/// **`root` is the *store* root, not the channel root.** The Agent fingerprints
/// the path it hands the Store, and the two halves have to agree byte for byte
/// or a supervised launch refuses; this is the one place the distinction is
/// load-bearing, so it is spelled out rather than inferred from the caller.
///
/// The image fingerprint is of `current_exe()` — whichever executable is
/// hosting this runtime. A worker role inside another image therefore proves
/// *its own* image, which is the property that makes "the Agent launched this
/// exact binary" mean something.
pub(crate) fn validate(
    store_root: &std::path::Path,
    bootstrap: Option<&LaunchBootstrap>,
    segment: &str,
) -> Result<(), String> {
    let Some(bootstrap) = bootstrap else {
        return Ok(());
    };
    if bootstrap.bridge_version != koru_network_bridge::BRIDGE_VERSION
        || bootstrap.channel != segment
        || bootstrap.root_fingerprint
            != koru_network_bridge::fingerprint(store_root.to_string_lossy().as_bytes())
    {
        return Err("Agent bootstrap does not match this gateway root/channel/version".into());
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let image = std::fs::read(executable).map_err(|error| error.to_string())?;
    if bootstrap.installation_fingerprint != koru_network_bridge::fingerprint(image) {
        return Err("Agent bootstrap belongs to a different gateway image".into());
    }
    Ok(())
}

/// Register with the Agent over the private, root-qualified bridge.
///
/// A refused registration fails the host: a supervised gateway with no edge
/// back into its parent cannot validate a grant, and admitting callers it
/// cannot vouch for is the one outcome worse than not starting.
pub(crate) async fn connect_agent_bridge(
    store_root: &std::path::Path,
    bootstrap: &LaunchBootstrap,
    segment: &str,
) -> Result<koru_network_bridge::wire::Link, String> {
    let endpoint =
        koru_ipc::Endpoint::for_service_root(koru_ipc::AGENT_GATEWAY_BRIDGE, segment, store_root)
            .map_err(|error| error.to_string())?;
    let registration = koru_network_bridge::wire::Registration {
        identity: bootstrap.identity(),
        capability: bootstrap.capability.clone(),
    };
    let stream = tokio::task::spawn_blocking(move || {
        let mut stream = koru_ipc::connect_timed(&endpoint, std::time::Duration::from_secs(10))
            .map_err(|error| error.to_string())?;
        if !koru_ipc::same_user(&stream) {
            return Err("Agent bridge belongs to another user".to_owned());
        }
        koru_ipc::frame::write_frame(
            &mut stream,
            koru_network_bridge::wire::WIRE_VERSION,
            &registration,
        )
        .map_err(|error| error.to_string())?;
        let (version, reply) = koru_ipc::frame::read_frame::<
            _,
            koru_network_bridge::wire::RegistrationReply,
        >(&mut stream)
        .map_err(|error| error.to_string())?;
        if version != koru_network_bridge::wire::WIRE_VERSION {
            return Err("Agent bridge frame version mismatch".to_owned());
        }
        match reply {
            koru_network_bridge::wire::RegistrationReply::Accepted => Ok(stream),
            koru_network_bridge::wire::RegistrationReply::Refused(reason) => {
                Err(format!("Agent bridge refused registration: {reason}"))
            }
        }
    })
    .await
    .map_err(|error| error.to_string())??;
    koru_network_bridge::wire::Link::attach(stream, koru_network_bridge::wire::Side::Gateway, None)
}

pub(crate) async fn connect_session_bridge(
    channel_root: &std::path::Path,
    segment: &str,
    lifecycle_authority: String,
) -> Result<koru_api_gateway_core::SessionBridge, String> {
    let endpoint =
        session_bridge_endpoint(channel_root, segment).map_err(|error| error.to_string())?;
    koru_api_gateway_core::SessionBridge::connect_supervised(
        endpoint,
        session_offer(),
        Some(lifecycle_authority),
    )
    .await
    .map_err(|error| format!("session bridge unavailable: {error:?}"))
}

/// A supervised Agent may deliberately have no session host in a development
/// bundle. That absence means the Gateway serves Store/API traffic without a
/// sessions adapter; it must not turn into a connection attempt that makes the
/// entire Gateway unavailable.
pub(crate) fn session_bridge_authority(bootstrap: Option<&LaunchBootstrap>) -> Option<String> {
    bootstrap?
        .session_lifecycle
        .as_ref()
        .map(|authority| authority.expose().to_owned())
}

fn session_bridge_endpoint(
    channel_root: &std::path::Path,
    segment: &str,
) -> Result<koru_ipc::Endpoint, koru_ipc::IpcError> {
    koru_ipc::Endpoint::for_service_channel_root(koru_ipc::SESSION_HOST, segment, channel_root)
}

fn session_offer() -> koru_session_protocol::ProtocolOffer {
    use koru_session_protocol::{
        CapabilityName, CapabilitySet, MAX_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION, ProtocolRange,
        capability,
    };
    use std::str::FromStr as _;

    let capabilities = [
        capability::CONTROLLER_LEASES,
        capability::SCROLLBACK_PAGES,
        capability::RAW_WRITE,
        capability::TERMINAL_KEYS,
        capability::RESIZE,
        capability::INTERRUPT,
        capability::TERMINATE,
        capability::ATTACHMENTS,
        capability::QUOTED_PATH_DELIVERY,
        capability::PROFILE_LAUNCH,
        capability::OBSERVER_OUTPUT,
        capability::OBSERVATION_CURSORS,
        capability::MANAGED_SESSIONS,
        capability::MANAGED_CANCEL,
        capability::NATIVE_ATTACHMENT_DELIVERY,
        // The two the workspace management operation needs: `projects` to read
        // which project a session sits in, and `workspace-lifecycle` to close
        // the ones a workspace owns. Negotiating them says only that this peer
        // can speak the frames; the launch-scoped secret says it may.
        capability::PROJECTS,
        capability::WORKSPACE_LIFECYCLE,
    ]
    .into_iter()
    .map(|name| CapabilityName::from_str(name).expect("built-in capability"));
    koru_session_protocol::ProtocolOffer {
        versions: ProtocolRange {
            min: MIN_PROTOCOL_VERSION,
            max: MAX_PROTOCOL_VERSION,
        },
        capabilities: CapabilitySet::new(capabilities),
    }
}

#[cfg(test)]
mod tests;
