// SPDX-License-Identifier: MPL-2.0

//! Pure account-entry placement and authenticated candidate-route resolution.
//! Hosts prepare account endpoints before PII handling, then correlate completion
//! against their existing session owner. Nothing here starts IO or enables Cloud.

use koru_account_core::{
    KoruIdentityId, regional_bootstrap::AccountDeployment, workspace_authorization::Region,
};
use koru_account_wire::regional_bootstrap::{
    BootstrapDecodeError, CloudSyncRouteV1, CloudSyncSeedV1, RegionalBootstrapV1,
    canonical_relay_urls,
};
use koru_device::cloud::CloudId;
use koru_network_iroh::{
    IrohNetworkConfig, RelayChannelKind, RelayProviderConfig, channel_relays,
    resolve_network_config,
};
use serde::Deserialize;

/// Deployment injection only; no readiness or persisted worker registration.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredCloudSyncWorkerV1 {
    pub worker_id: CloudId,
    pub route: CloudSyncRouteV1,
}

/// An isolated host supplies every field, including an explicit relay list.
/// Deserialize supports test adapters; this does not define a settings file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionalEndpoints {
    pub region: Region,
    pub api_origin: String,
    pub sync_pool: [ConfiguredCloudSyncWorkerV1; 2],
    pub relay_urls: Vec<String>,
}

/// Account routing inputs before authentication, without worker metadata.
/// In-memory host injection only: not a wire or persisted configuration format.
#[derive(Clone, Debug)]
pub struct AccountEndpoints {
    pub region: Region,
    pub api_origin: String,
    pub relay_urls: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapError {
    RegionUnavailable,
    Unconfigured,
    Mismatch,
}

impl BootstrapError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::RegionUnavailable => "region_unavailable",
            Self::Unconfigured => "regional_bootstrap_unconfigured",
            Self::Mismatch => "regional_bootstrap_mismatch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountActivity {
    LocalOnly,
    AccountEntry,
}

#[derive(Clone, Debug)]
pub enum EndpointDeployment {
    Production {
        channel: RelayChannelKind,
        endpoints: Option<AccountEndpoints>,
    },
    IsolatedRoot(Option<RegionalEndpoints>),
}

#[derive(Clone, Debug)]
enum PreparedRouting {
    Production,
    IsolatedRoot([ConfiguredCloudSyncWorkerV1; 2]),
}

/// Provisional US placement. Deliberately not serializable and not a session.
#[derive(Clone, Debug)]
pub struct PreparedAccountEntry {
    routing: PreparedRouting,
    deployment: AccountDeployment,
    endpoints: AccountEndpoints,
    network: IrohNetworkConfig,
}

impl PreparedAccountEntry {
    /// Local work returns None before examining account configuration. A root
    /// without a complete bundle refuses account entry, with no public fallback.
    /// Issuer/rollout and provider policy remain separate validated host inputs.
    pub fn prepare(
        activity: AccountActivity,
        configuration: EndpointDeployment,
        relay_enabled: bool,
    ) -> Result<Option<Self>, BootstrapError> {
        if activity == AccountActivity::LocalOnly {
            return Ok(None);
        }
        let (mut endpoints, routing, channel) = match configuration {
            EndpointDeployment::Production { channel, endpoints } => (
                endpoints.ok_or(BootstrapError::Unconfigured)?,
                PreparedRouting::Production,
                channel,
            ),
            EndpointDeployment::IsolatedRoot(endpoints) => {
                let RegionalEndpoints {
                    region,
                    api_origin,
                    sync_pool,
                    relay_urls,
                } = endpoints.ok_or(BootstrapError::Unconfigured)?;
                (
                    AccountEndpoints {
                        region,
                        api_origin,
                        relay_urls,
                    },
                    PreparedRouting::IsolatedRoot(sync_pool),
                    RelayChannelKind::Production,
                )
            }
        };
        let deployment = AccountDeployment::prepare(endpoints.region)
            .map_err(|_| BootstrapError::Unconfigured)?;
        let api = koru_account_client::validate_api_origin(&endpoints.api_origin)
            .map_err(|_| BootstrapError::Unconfigured)?;
        endpoints.api_origin = api.to_string();
        endpoints.relay_urls = canonical_relay_urls(&endpoints.relay_urls)
            .map_err(|_| BootstrapError::Unconfigured)?;
        let network = match &routing {
            PreparedRouting::Production => {
                if endpoints.api_origin != "https://api.koru.do/" {
                    return Err(BootstrapError::Unconfigured);
                }
                let RelayProviderConfig::CustomUrls(relays) = channel_relays(channel) else {
                    return Err(BootstrapError::Unconfigured);
                };
                let relays =
                    canonical_relay_urls(&relays).map_err(|_| BootstrapError::Unconfigured)?;
                if endpoints.relay_urls != relays {
                    return Err(BootstrapError::Unconfigured);
                }
                resolve_network_config(relay_enabled, channel, false, None)
            }
            PreparedRouting::IsolatedRoot(workers) => {
                let [first, second] = workers;
                if first.worker_id.as_bytes() >= second.worker_id.as_bytes()
                    || first.route.endpoint_id() == second.route.endpoint_id()
                {
                    return Err(BootstrapError::Unconfigured);
                }
                for worker in workers {
                    match worker.route.relay_url() {
                        None if endpoints.relay_urls.is_empty() => {}
                        Some(relay) if endpoints.relay_urls.iter().any(|value| value == relay) => {}
                        _ => return Err(BootstrapError::Unconfigured),
                    }
                }
                resolve_network_config(relay_enabled, channel, true, Some(&endpoints.relay_urls))
            }
        };
        Ok(Some(Self {
            routing,
            deployment,
            endpoints,
            network,
        }))
    }

    pub fn region(&self) -> Region {
        self.deployment.region()
    }
    pub fn endpoints(&self) -> &AccountEndpoints {
        &self.endpoints
    }
    /// Apply to the existing endpoint owner before startup; this call does no IO.
    /// Relay preference is honored without changing route membership validation.
    pub fn network_config(&self) -> &IrohNetworkConfig {
        &self.network
    }

    /// Consume the raw authenticated body, preserving duplicate-field checks.
    /// `current` must be read by the existing owner when delivering this result,
    /// under its normal ownership guard. No generation or session store is added.
    /// This result is still only routing, never permission to start Cloud/dial.
    pub fn resolve(
        &self,
        body: &[u8],
        requested: BootstrapOwner,
        current: Option<BootstrapOwner>,
    ) -> Result<ResolvedRegionalBootstrap, BootstrapError> {
        if current != Some(requested) {
            return Err(BootstrapError::Mismatch);
        }
        // Reuse the account client's response envelope bound before allocation.
        if body.len() > koru_account_client::MAX_RESPONSE_BODY_BYTES {
            return Err(BootstrapError::Mismatch);
        }
        let response = RegionalBootstrapV1::decode(body).map_err(|error| match error {
            BootstrapDecodeError::RegionUnavailable { koru_identity_id }
                if koru_identity_id == requested.account
                    && matches!(self.routing, PreparedRouting::Production) =>
            {
                BootstrapError::RegionUnavailable
            }
            _ => BootstrapError::Mismatch,
        })?;
        if response.koru_identity_id != requested.account
            || response.home_region != self.region()
            || response.api_origin != self.endpoints.api_origin
            || response.relay_urls != self.endpoints.relay_urls
        {
            return Err(BootstrapError::Mismatch);
        }
        if let CloudSyncSeedV1::Available { seed_worker } = &response.sync {
            let accepted = match &self.routing {
                // Production discovers the candidate from authenticated bootstrap.
                // The prepared channel list requires HTTPS and a non-null relay,
                // independently of whether the person's Use-relay setting is off.
                PreparedRouting::Production => seed_worker.relay_url().is_some_and(|relay| {
                    self.endpoints.relay_urls.iter().any(|value| value == relay)
                }),
                PreparedRouting::IsolatedRoot(workers) => {
                    workers.iter().any(|worker| &worker.route == seed_worker)
                }
            };
            if !accepted {
                return Err(BootstrapError::Mismatch);
            }
        }
        Ok(ResolvedRegionalBootstrap {
            owner: requested,
            response,
        })
    }
}

/// Correlation facts borrowed from the existing authenticated operation owner.
/// Constructing this value does not authenticate a session or mint a generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootstrapOwner {
    pub account: KoruIdentityId,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct ResolvedRegionalBootstrap {
    owner: BootstrapOwner,
    response: RegionalBootstrapV1,
}

impl ResolvedRegionalBootstrap {
    /// Recheck the existing owner at use, so holding this value across account
    /// switch/sign-out cannot grant access to even a stale candidate route.
    pub fn response(
        &self,
        current: Option<BootstrapOwner>,
    ) -> Result<&RegionalBootstrapV1, BootstrapError> {
        if current != Some(self.owner) {
            return Err(BootstrapError::Mismatch);
        }
        Ok(&self.response)
    }
}

#[cfg(test)]
mod tests;
