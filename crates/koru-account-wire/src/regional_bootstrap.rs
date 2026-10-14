// SPDX-License-Identifier: MPL-2.0

//! Strict candidate routing metadata. These values grant no dataset access.
//! Decoding checks shape; trusted runtime preparation checks deployment policy.

use std::net::SocketAddr;

use koru_account_core::{KoruIdentityId, workspace_authorization::Region};
use koru_device::ReplicationEndpointId;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};
use url::Url;

use crate::Version1;

pub const MAX_RELAY_URLS: usize = 8;
pub const MAX_RELAY_URL_BYTES: usize = 2048;
pub const MAX_DIRECT_ADDRESSES: usize = 8;
pub const MAX_DIRECT_ADDRESS_BYTES: usize = 128;

/// Canonical serialization comes from the same URL parser as account transport.
pub fn canonical_url(value: &str) -> Result<String, &'static str> {
    let url = Url::parse(value).map_err(|_| "invalid absolute URL")?;
    if url.host_str().is_none() || !matches!(url.scheme(), "https" | "http") {
        return Err("invalid HTTP URL");
    }
    Ok(url.to_string())
}

/// An absolute API origin, never a URL carrying credentials or a resource path.
pub fn canonical_api_origin(value: &str) -> Result<String, &'static str> {
    let value = canonical_url(value)?;
    let url = Url::parse(&value).map_err(|_| "invalid API origin")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("invalid API origin");
    }
    Ok(value)
}

pub fn canonical_relay_urls(values: &[String]) -> Result<Vec<String>, &'static str> {
    if values.len() > MAX_RELAY_URLS {
        return Err("too many relay URLs");
    }
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let value = canonical_relay_url(value)?;
        if result.contains(&value) {
            return Err("duplicate relay URL");
        }
        result.push(value);
    }
    Ok(result)
}

fn canonical_relay_url(value: &str) -> Result<String, &'static str> {
    if value.len() > MAX_RELAY_URL_BYTES {
        return Err("relay URL too long");
    }
    let canonical = canonical_url(value)?;
    // Percent encoding can expand Unicode. Serialized output must still obey
    // the reader's bound when the next endpoint decodes it.
    if canonical.len() > MAX_RELAY_URL_BYTES {
        return Err("canonical relay URL too long");
    }
    Ok(canonical)
}

fn nullable_relay_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "anyOf": [
        { "type": "string", "minLength": 1, "maxLength": MAX_RELAY_URL_BYTES },
        { "type": "null" }
    ] })
}

/// Exact endpoint and its advertised paths, not a signed assignment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "RouteFields", deny_unknown_fields)]
pub struct CloudSyncRouteV1 {
    #[schemars(with = "[u8; 32]")]
    endpoint_id: ReplicationEndpointId,
    #[schemars(length(min = 1, max = 8))]
    direct_addresses: Vec<String>,
    #[schemars(required, schema_with = "nullable_relay_schema")]
    relay_url: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "CloudSyncRouteV1")]
struct RouteFields {
    #[schemars(with = "[u8; 32]")]
    endpoint_id: ReplicationEndpointId,
    #[schemars(length(min = 1, max = 8))]
    direct_addresses: Vec<String>,
    // deserialize_with deliberately makes presence mandatory even for Option.
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required, schema_with = "nullable_relay_schema")]
    relay_url: Option<String>,
}

fn required_nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

impl TryFrom<RouteFields> for CloudSyncRouteV1 {
    type Error = &'static str;

    fn try_from(value: RouteFields) -> Result<Self, Self::Error> {
        Self::new(value.endpoint_id, value.direct_addresses, value.relay_url)
    }
}

impl CloudSyncRouteV1 {
    pub fn new(
        endpoint_id: ReplicationEndpointId,
        direct_addresses: Vec<String>,
        relay_url: Option<String>,
    ) -> Result<Self, &'static str> {
        endpoint_id.validate_cloud_key()?;
        if direct_addresses.is_empty() || direct_addresses.len() > MAX_DIRECT_ADDRESSES {
            return Err("invalid direct address count");
        }
        let mut addresses = Vec::with_capacity(direct_addresses.len());
        for value in direct_addresses {
            if value.len() > MAX_DIRECT_ADDRESS_BYTES {
                return Err("direct address too long");
            }
            let address: SocketAddr = value.parse().map_err(|_| "invalid direct address")?;
            if address.port() == 0 {
                return Err("zero direct port");
            }
            let address = address.to_string();
            if addresses.contains(&address) {
                return Err("duplicate direct address");
            }
            addresses.push(address);
        }
        let relay_url = relay_url
            .map(|value| canonical_relay_url(&value))
            .transpose()?;
        Ok(Self {
            endpoint_id,
            direct_addresses: addresses,
            relay_url,
        })
    }

    pub fn endpoint_id(&self) -> ReplicationEndpointId {
        self.endpoint_id
    }
    pub fn direct_addresses(&self) -> &[String] {
        &self.direct_addresses
    }
    pub fn relay_url(&self) -> Option<&str> {
        self.relay_url.as_deref()
    }
}

/// A candidate ready seed or explicit downtime; never a list or null.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CloudSyncSeedV1 {
    Available {
        seed_worker: CloudSyncRouteV1,
    },
    Unavailable {
        #[serde(deserialize_with = "retry_seconds")]
        #[schemars(range(min = 1, max = 300))]
        retry_after_seconds: u16,
    },
}

fn retry_seconds<'de, D: Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    let value = u16::deserialize(d)?;
    if !(1..=300).contains(&value) {
        return Err(de::Error::custom("invalid retry interval"));
    }
    Ok(value)
}

/// Authenticated metadata. Current session/account correlation is mandatory
/// in the resolver before any consumer can use the returned candidate route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "BootstrapFields", deny_unknown_fields)]
pub struct RegionalBootstrapV1 {
    pub version: Version1,
    pub koru_identity_id: KoruIdentityId,
    #[schemars(schema_with = "us_region_schema")]
    pub home_region: Region,
    #[serde(deserialize_with = "api_origin")]
    pub api_origin: String,
    pub sync: CloudSyncSeedV1,
    #[serde(deserialize_with = "relay_urls")]
    #[schemars(length(max = 8))]
    pub relay_urls: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "RegionalBootstrapV1")]
struct BootstrapFields {
    version: Version1,
    koru_identity_id: KoruIdentityId,
    #[schemars(schema_with = "us_region_schema")]
    home_region: String,
    #[serde(deserialize_with = "api_origin")]
    api_origin: String,
    sync: CloudSyncSeedV1,
    #[serde(deserialize_with = "relay_urls")]
    #[schemars(length(max = 8))]
    relay_urls: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapDecodeError {
    InvalidMessage,
    RegionUnavailable { koru_identity_id: KoruIdentityId },
}

impl std::fmt::Display for BootstrapDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidMessage => "invalid bootstrap message",
            Self::RegionUnavailable { .. } => "region_unavailable",
        })
    }
}

impl TryFrom<BootstrapFields> for RegionalBootstrapV1 {
    type Error = BootstrapDecodeError;
    fn try_from(value: BootstrapFields) -> Result<Self, Self::Error> {
        if value.home_region != "US" {
            return Err(BootstrapDecodeError::RegionUnavailable {
                koru_identity_id: value.koru_identity_id,
            });
        }
        Ok(Self {
            version: value.version,
            koru_identity_id: value.koru_identity_id,
            home_region: Region::Us,
            api_origin: value.api_origin,
            sync: value.sync,
            relay_urls: value.relay_urls,
        })
    }
}

impl RegionalBootstrapV1 {
    /// Decode every strict field before classifying unsupported home region.
    /// Carry the decoded account so a resolver cannot treat a different
    /// account's unavailable region as current authenticated account authority.
    pub fn decode(body: &[u8]) -> Result<Self, BootstrapDecodeError> {
        let fields: BootstrapFields =
            serde_json::from_slice(body).map_err(|_| BootstrapDecodeError::InvalidMessage)?;
        fields.try_into()
    }
}

fn us_region_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": "string", "const": "US" })
}

fn api_origin<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    canonical_api_origin(&String::deserialize(d)?).map_err(de::Error::custom)
}

fn relay_urls<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    canonical_relay_urls(&Vec::<String>::deserialize(d)?).map_err(de::Error::custom)
}

#[cfg(test)]
mod tests;
