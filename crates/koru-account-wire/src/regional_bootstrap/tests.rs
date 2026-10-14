// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn relay_schema_is_required_and_nullable_directly_and_in_seed() {
    let schema = serde_json::to_value(schemars::schema_for!(CloudSyncRouteV1)).unwrap();
    let seed = serde_json::to_value(schemars::schema_for!(CloudSyncSeedV1)).unwrap();
    for route in [&schema, &seed["$defs"]["CloudSyncRouteV1"]] {
        assert!(
            route["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "relay_url")
        );
        let variants = route["properties"]["relay_url"]["anyOf"]
            .as_array()
            .unwrap();
        assert!(variants.iter().any(|v| v["type"] == "null"));
        assert!(variants.iter().any(|v| v["type"] == "string"));
    }
}

#[test]
fn canonical_relay_limit_is_roundtrip_safe() {
    let prefix = "https://relay.test/";
    let exact = format!("{prefix}{}", "x".repeat(MAX_RELAY_URL_BYTES - prefix.len()));
    assert_eq!(canonical_relay_url(&exact).unwrap(), exact);
    assert!(canonical_relay_url(&format!("{exact}x")).is_err());
    let erratum: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../contracts/vectors/regional-bootstrap-resolver-v1/erratum-1.json"
    ))
    .unwrap();
    let case = &erratum["relay_expansion"];
    let expanding = format!(
        "{}{}",
        case["prefix"].as_str().unwrap(),
        case["character"]
            .as_str()
            .unwrap()
            .repeat(case["repeat"].as_u64().unwrap() as usize)
    );
    assert!(expanding.len() < MAX_RELAY_URL_BYTES);
    assert_eq!(
        canonical_relay_url(&expanding).is_ok(),
        case["accept"].as_bool().unwrap()
    );
    assert!(canonical_relay_urls(&[expanding]).is_err());
}

const VECTORS: &str =
    include_str!("../../../../contracts/vectors/regional-bootstrap-resolver-v1/cases.json");

#[test]
fn frozen_raw_wire_vectors_use_production_decoders() {
    let vectors: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
    for case in vectors["wire"].as_array().unwrap() {
        let json = case["json"].as_str().unwrap();
        let accepted = match case["target"].as_str().unwrap() {
            "route" => serde_json::from_str::<CloudSyncRouteV1>(json).is_ok(),
            "bootstrap" => serde_json::from_str::<RegionalBootstrapV1>(json).is_ok(),
            other => panic!("unknown target {other}"),
        };
        assert_eq!(
            accepted,
            case["accept"].as_bool().unwrap(),
            "{}: {json}",
            case["id"]
        );
    }
}

#[test]
fn canonical_roundtrip_preserves_required_null_and_native_key_bytes() {
    let vectors: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
    for case in vectors["wire"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["target"] == "route" && c["accept"] == true)
    {
        let route: CloudSyncRouteV1 = serde_json::from_str(case["json"].as_str().unwrap()).unwrap();
        let json = serde_json::to_string(&route).unwrap();
        assert!(json.contains("\"relay_url\":"));
        assert_eq!(route, serde_json::from_str(&json).unwrap());
    }
}

/// **The spelling a deployment must carry, pinned.**
///
/// `CloudSyncIssuer::new` canonicalises its relay URLs and then refuses the
/// configuration unless the result is byte-identical to what it was given, so
/// a value that merely *parses* is not enough. `koru-network-iroh` spells the
/// same relay without a trailing slash — correct for a client that
/// canonicalises before use, and wrong for `KORU_IROH_RELAYS`, which is
/// compared verbatim. Copying the constant into the deployment took the API
/// down with `account_configuration_failed`, which names neither the variable
/// nor the slash.
#[test]
fn a_relay_url_is_canonical_only_with_its_trailing_slash() {
    let bare = "https://use1-1.relay.koru.do";
    let canonical = "https://use1-1.relay.koru.do/";
    assert_eq!(canonical_url(bare).unwrap(), canonical);
    assert_eq!(canonical_url(canonical).unwrap(), canonical);
    // What the issuer actually asserts: canonicalising the configured value
    // must be a no-op, which the bare form is not.
    let configured = vec![bare.to_owned()];
    assert_ne!(canonical_relay_urls(&configured).unwrap(), configured);
    let configured = vec![canonical.to_owned()];
    assert_eq!(canonical_relay_urls(&configured).unwrap(), configured);
    // The same holds for the internal alias, so neither build's constant is
    // safe to paste into a deployment unchanged.
    assert_eq!(
        canonical_url(koru_network_relay_alias()).unwrap(),
        "https://dev1.relay.koru.do/"
    );
}

/// Spelled here rather than depending on `koru-network-iroh`, which this crate
/// does not and should not know about.
fn koru_network_relay_alias() -> &'static str {
    "https://dev1.relay.koru.do"
}
