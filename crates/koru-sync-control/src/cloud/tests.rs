// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_device::cloud::CloudId;

#[test]
fn replacement_waits_for_ack_reuses_endpoint_and_never_extends_old_expiry() {
    let endpoint = ReplicationEndpointId::from_bytes([1; 32]);
    let id = CloudId::from_bytes([2; 16]);
    let principal = CloudReplicaPrincipalV1::Workspace {
        workspace_id: id,
        incarnation_id: id,
        logical_replica_id: id,
    };
    let mut state = CloudSessionHandover::new(endpoint, principal);
    state
        .acknowledge(
            endpoint,
            principal,
            SessionGeneration {
                generation: 1,
                expires_at_seconds: 600,
            },
            0,
        )
        .unwrap();
    assert!(!state.refresh_due(479));
    assert!(state.refresh_due(480));
    assert_eq!(state.active(599).unwrap().generation, 1);
    assert_eq!(state.active(600), Err("credential_expired"));
    state
        .acknowledge(
            endpoint,
            principal,
            SessionGeneration {
                generation: 2,
                expires_at_seconds: 1080,
            },
            580,
        )
        .unwrap();
    assert_eq!(state.active(580).unwrap().generation, 2);
    assert_eq!(state.draining(580).unwrap().generation, 1);
    assert_eq!(state.draining(580).unwrap().expires_at_seconds, 600);
    assert_eq!(state.draining(600), None);
    state.drained(1);
    assert_eq!(state.draining(600), None);
    assert_eq!(
        state.acknowledge(
            endpoint,
            principal,
            SessionGeneration {
                generation: 2,
                expires_at_seconds: 2000
            },
            590
        ),
        Err("handshake_replay")
    );
    state.fence();
    assert_eq!(state.active(590), Err("credential_expired"));
    for generation in [1, 2] {
        assert_eq!(
            state.acknowledge(
                endpoint,
                principal,
                SessionGeneration {
                    generation,
                    expires_at_seconds: 2000
                },
                590
            ),
            Err("handshake_replay")
        );
    }
}

#[test]
fn old_control_is_refused_before_any_state_change_and_peer_framing_is_unchanged() {
    use crate::*;
    assert_eq!(CONTROL_PROTOCOL_VERSION, 7);
    assert_eq!(CONTROL_FRAME_VERSION, 1);
    assert_eq!(PEER_PROTOCOL_VERSION, 5);
    let mut host = ControlHost::default();
    let old = ControlHello {
        protocol_version: 6,
        session_id: ControlSessionId::new(),
        local_device: DeviceId::new(),
    };
    assert!(matches!(host.begin_session(&old), Err(HostError::Protocol)));
    assert_eq!(host.status(), ControlStatus::default());
    assert!(host.session.is_none());
}

#[test]
fn expired_drain_does_not_block_next_acknowledgement() {
    let endpoint = ReplicationEndpointId::from_bytes([1; 32]);
    let id = CloudId::from_bytes([2; 16]);
    let principal = CloudReplicaPrincipalV1::Profile {
        profile_id: id,
        lineage_id: id,
        logical_replica_id: id,
    };
    let mut state = CloudSessionHandover::new(endpoint, principal);
    for (generation, expires_at_seconds, now) in [(1, 100, 0), (2, 200, 50), (3, 300, 101)] {
        state
            .acknowledge(
                endpoint,
                principal,
                SessionGeneration {
                    generation,
                    expires_at_seconds,
                },
                now,
            )
            .unwrap();
    }
    assert_eq!(state.active(101).unwrap().generation, 3);
    assert_eq!(state.draining(101).unwrap().generation, 2);
}
