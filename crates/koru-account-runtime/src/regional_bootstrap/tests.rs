// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_network_iroh::{AddressLookupConfig, RelayProviderConfig};

const VECTORS: &str =
    include_str!("../../../../contracts/vectors/regional-bootstrap-resolver-v1/cases.json");

#[derive(Deserialize)]
struct Case {
    id: String,
    mode: String,
    activity: String,
    expected: String,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    config: serde_json::Value,
    response_json: String,
    request_account: KoruIdentityId,
    current_account: Option<KoruIdentityId>,
    request_generation: u64,
    current_generation: u64,
}

fn cases() -> Vec<Case> {
    let vectors: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
    let mut cases: Vec<Case> = serde_json::from_value(vectors["resolve"].clone()).unwrap();
    for input in [
        include_str!("../../../../contracts/vectors/regional-bootstrap-resolver-v1/erratum-1.json"),
        include_str!("../../../../contracts/vectors/regional-bootstrap-resolver-v1/erratum-2.json"),
        include_str!("../../../../contracts/vectors/regional-bootstrap-resolver-v1/erratum-3.json"),
    ] {
        let erratum: serde_json::Value = serde_json::from_str(input).unwrap();
        for case in &mut cases {
            if let Some(expected) = erratum["expected_overrides"][&case.id].as_str() {
                case.expected = expected.to_owned();
            }
        }
        cases.extend(serde_json::from_value::<Vec<Case>>(erratum["resolve"].clone()).unwrap());
    }
    cases
}

#[test]
fn unsupported_authoritative_region_is_typed_in_production_and_mismatch_under_root() {
    let mut case = cases()
        .into_iter()
        .find(|c| c.id == "hostile_region")
        .unwrap();
    let owner = BootstrapOwner {
        account: case.request_account,
        generation: 7,
    };
    let production = prepare(&case, true).unwrap().unwrap();
    assert_eq!(
        production
            .resolve(case.response_json.as_bytes(), owner, Some(owner))
            .unwrap_err(),
        BootstrapError::RegionUnavailable
    );
    case.mode = "isolated".into();
    let isolated = prepare(&case, true).unwrap().unwrap();
    assert_eq!(
        isolated
            .resolve(case.response_json.as_bytes(), owner, Some(owner))
            .unwrap_err(),
        BootstrapError::Mismatch
    );
}

fn prepare(
    case: &Case,
    relay_enabled: bool,
) -> Result<Option<PreparedAccountEntry>, BootstrapError> {
    // Old frozen production fixtures carry a sync_pool only because the first
    // harness conflated production and isolated inputs. Project their account
    // fields here; production AccountEndpoints has no serde/wire representation.
    #[derive(Deserialize)]
    struct AccountFixture {
        region: Region,
        api_origin: String,
        relay_urls: Vec<String>,
    }
    let configuration = if case.mode == "production" {
        EndpointDeployment::Production {
            channel: if case.channel.as_deref() == Some("internal") {
                RelayChannelKind::Internal
            } else {
                RelayChannelKind::Production
            },
            endpoints: serde_json::from_value::<AccountFixture>(case.config.clone())
                .ok()
                .map(|value| AccountEndpoints {
                    region: value.region,
                    api_origin: value.api_origin,
                    relay_urls: value.relay_urls,
                }),
        }
    } else {
        EndpointDeployment::IsolatedRoot(serde_json::from_value(case.config.clone()).ok())
    };
    PreparedAccountEntry::prepare(
        if case.activity == "local" {
            AccountActivity::LocalOnly
        } else {
            AccountActivity::AccountEntry
        },
        configuration,
        relay_enabled,
    )
}

#[test]
fn frozen_resolver_vectors_use_real_preparation_and_resolution() {
    for case in cases() {
        for relay_enabled in [false, true] {
            let result = prepare(&case, relay_enabled).and_then(|prepared| {
                let Some(prepared) = prepared else {
                    return Ok("local_only");
                };
                assert_eq!(prepared.region(), Region::Us);
                if case.phase.as_deref() == Some("prepare") {
                    assert_eq!(prepared.endpoints().api_origin, "https://api.koru.do/");
                    return Ok("prepared");
                }
                let requested = BootstrapOwner {
                    account: case.request_account,
                    generation: case.request_generation,
                };
                let current = case.current_account.map(|account| BootstrapOwner {
                    account,
                    generation: case.current_generation,
                });
                let resolved =
                    prepared.resolve(case.response_json.as_bytes(), requested, current)?;
                let response = resolved.response(current)?;
                Ok(match response.sync {
                    CloudSyncSeedV1::Available { .. } => "available",
                    CloudSyncSeedV1::Unavailable { .. } => "unavailable",
                })
            });
            let actual = result.unwrap_or_else(BootstrapError::code);
            assert_eq!(
                actual, case.expected,
                "{} relay_enabled={relay_enabled}",
                case.id
            );
        }
    }
}

#[test]
fn bootstrap_failure_does_not_remove_prepared_account_routing() {
    let case = cases()
        .into_iter()
        .find(|case| case.id == "production_prepares_without_worker_pool")
        .unwrap();
    let prepared = prepare(&case, true).unwrap().unwrap();
    let owner = BootstrapOwner {
        account: case.request_account,
        generation: case.request_generation,
    };
    assert_eq!(
        prepared
            .resolve(b"route not found", owner, Some(owner))
            .unwrap_err(),
        BootstrapError::Mismatch
    );
    assert_eq!(prepared.endpoints().api_origin, "https://api.koru.do/");
    assert_eq!(prepared.region(), Region::Us);
}

#[test]
fn isolated_configuration_disables_public_lookup_before_startup_and_honors_relay_preference() {
    for case in cases()
        .iter()
        .filter(|c| c.mode == "isolated" && c.expected == "available")
    {
        for enabled in [false, true] {
            let prepared = prepare(case, enabled).unwrap().unwrap();
            let network = prepared.network_config();
            assert_eq!(network.address_lookup, AddressLookupConfig::Disabled);
            if !enabled || prepared.endpoints().relay_urls.is_empty() {
                assert_eq!(network.relay_provider, RelayProviderConfig::Disabled);
            } else {
                assert_eq!(
                    network.relay_provider,
                    RelayProviderConfig::CustomUrls(prepared.endpoints().relay_urls.clone())
                );
            }
        }
    }
}

#[test]
fn resolved_result_rechecks_existing_owner_at_use() {
    let case = cases().remove(0);
    let prepared = prepare(&case, true).unwrap().unwrap();
    let owner = BootstrapOwner {
        account: case.request_account,
        generation: 7,
    };
    let result = prepared
        .resolve(case.response_json.as_bytes(), owner, Some(owner))
        .unwrap();
    assert!(result.response(Some(owner)).is_ok());
    assert_eq!(result.response(None).unwrap_err(), BootstrapError::Mismatch);
    assert_eq!(
        result
            .response(Some(BootstrapOwner {
                generation: 8,
                ..owner
            }))
            .unwrap_err(),
        BootstrapError::Mismatch
    );
    let oversized = vec![b' '; koru_account_client::MAX_RESPONSE_BODY_BYTES + 1];
    assert_eq!(
        prepared
            .resolve(&oversized, owner, Some(owner))
            .unwrap_err(),
        BootstrapError::Mismatch
    );
}
