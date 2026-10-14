// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::*;
use std::{path::Path, str::FromStr};

fn main() {
    let root = std::env::args().nth(1).expect("root");
    let endpoint = koru_ipc::Endpoint::for_service_channel_root(
        koru_ipc::SESSION_HOST,
        "internal-debug",
        Path::new(&root),
    )
    .unwrap();
    let names = [
        capability::CONTROLLER_LEASES,
        capability::DELEGATION,
        capability::MANAGED_SESSIONS,
        capability::MANAGED_EVENTS,
        capability::OBSERVATION_CURSORS,
    ];
    let client = koru_session_client::SessionClient::connect(
        &endpoint,
        "approval-inspection",
        ProtocolOffer {
            versions: ProtocolRange {
                min: MIN_PROTOCOL_VERSION,
                max: MAX_PROTOCOL_VERSION,
            },
            capabilities: CapabilitySet::new(
                names
                    .into_iter()
                    .map(|s| CapabilityName::from_str(s).unwrap()),
            ),
        },
    )
    .unwrap();
    for session in client.list().unwrap() {
        println!(
            "session={} handover={:?} active={:?}",
            session.id, session.handover, session.active_turn_id
        );
        if let Ok(events) = client.observe_agent_events(session.id) {
            println!(
                "truncated={} cursor={:?}",
                events.earlier_truncated, events.cursor
            );
            for event in events.events {
                if !matches!(event.event, AgentEvent::MessageDelta { .. }) {
                    println!("{:?}", event);
                }
            }
        }
    }
}
