// SPDX-License-Identifier: MPL-2.0

use std::str::FromStr;

use crate::*;

fn capability(value: &str) -> CapabilityName {
    CapabilityName::from_str(value).unwrap()
}

#[test]
fn negotiation_selects_highest_common_version_and_intersection() {
    let client = ProtocolOffer {
        versions: ProtocolRange { min: 1, max: 3 },
        capabilities: CapabilitySet::new([
            capability("render-deltas"),
            capability("future-client-feature"),
        ]),
    };
    let server = ProtocolOffer {
        versions: ProtocolRange { min: 2, max: 4 },
        capabilities: CapabilitySet::new([
            capability("render-deltas"),
            capability("future-server-feature"),
        ]),
    };
    let agreement = negotiate(&client, &server).unwrap();
    assert_eq!(agreement.version, 3);
    assert!(
        agreement
            .capabilities
            .contains(&capability("render-deltas"))
    );
    assert!(
        !agreement
            .capabilities
            .contains(&capability("future-client-feature"))
    );
    assert!(
        !agreement
            .capabilities
            .contains(&capability("future-server-feature"))
    );
}

#[test]
fn incompatible_ranges_name_both_sides_and_stay_an_error() {
    let client = ProtocolOffer {
        versions: ProtocolRange { min: 1, max: 1 },
        capabilities: CapabilitySet::default(),
    };
    let server = ProtocolOffer {
        versions: ProtocolRange { min: 2, max: 3 },
        capabilities: CapabilitySet::default(),
    };
    let refusal = negotiate(&client, &server).unwrap_err();
    assert_eq!(refusal.client, client.versions);
    assert_eq!(refusal.server, server.versions);
    assert_eq!(refusal.error.code, ErrorCode::ProtocolIncompatible);
}

#[test]
fn capability_tokens_are_additive_and_validated() {
    let future = capability("new-provider-mode");
    let json = serde_json::to_string(&future).unwrap();
    assert_eq!(
        serde_json::from_str::<CapabilityName>(&json).unwrap(),
        future
    );
    assert!(CapabilityName::from_str("Native Image").is_err());
    assert!(CapabilityName::from_str("native_image").is_err());
}
