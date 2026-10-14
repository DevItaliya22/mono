// SPDX-License-Identifier: MPL-2.0

//! Safe retained causes: never a provider message, path, or arbitrary error code.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunFailureReason {
    CapabilityNotNegotiated,
    ProviderModelUnavailable,
    ProviderIncompatible,
    ProviderUnavailable,
    ProviderFailed,
    AuthenticationRequired,
    TimedOut,
    Disconnected,
    Backpressure,
    InvalidRequest,
    #[serde(other)]
    Unknown,
}

impl RunFailureReason {
    pub fn from_code(code: &str) -> Self {
        match code {
            "capability_not_negotiated"
            | "not_managed"
            | "not_delegatable"
            | "protocol_incompatible" => Self::CapabilityNotNegotiated,
            "provider_model_unavailable" => Self::ProviderModelUnavailable,
            "provider_incompatible" | "profile_incompatible" => Self::ProviderIncompatible,
            "provider_unavailable" | "profile_missing" => Self::ProviderUnavailable,
            "provider_failed" => Self::ProviderFailed,
            "authentication_required" | "unauthenticated" => Self::AuthenticationRequired,
            "timed_out" | "timeout" | "deadline_exceeded" => Self::TimedOut,
            "disconnected" | "host_unavailable" | "unavailable" => Self::Disconnected,
            "backpressure" => Self::Backpressure,
            "invalid_request" => Self::InvalidRequest,
            _ => Self::Unknown,
        }
    }
}
