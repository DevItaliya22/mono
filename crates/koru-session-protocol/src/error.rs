// SPDX-License-Identifier: MPL-2.0

use std::{borrow::Cow, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Broad transport-neutral handling class. It never turns an error into success.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    InvalidRequest,
    Conflict,
    NotFound,
    Unavailable,
    Unsupported,
    Forbidden,
    Internal,
}

/// Stable append-only error code. Unknown future codes remain explicit errors.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorCode {
    InvalidId,
    InvalidRequest,
    InvalidTransition,
    InvalidSize,
    ProtocolIncompatible,
    CapabilityNotNegotiated,
    SessionNotFound,
    ProfileNotFound,
    AttachmentNotFound,
    TurnNotFound,
    ApprovalNotFound,
    ProfileMissing,
    ProfileIncompatible,
    IdempotencyConflict,
    ControllerConflict,
    ControllerRequired,
    StaleLease,
    StaleGeneration,
    StreamGap,
    SpawnFailed,
    SessionExited,
    HostUnavailable,
    InputTooLarge,
    Backpressure,
    AttachmentLimit,
    UnsupportedMedia,
    InvalidImage,
    DeliveryUnsupported,
    DeliveryFailed,
    ProviderUnavailable,
    ProviderIncompatible,
    ProviderModelUnavailable,
    ProviderFailed,
    ApprovalStale,
    ProjectNotFound,
    /// Deletion is bookkeeping, never a process operation in disguise: a
    /// project with running sessions is refused until they are moved or ended.
    ProjectHasSessions,
    /// A session was asked to join a project other than its parent's. A
    /// subtree shares one project; moving the parent moves all of it.
    ProjectMismatch,
    /// A creation carried a key that a fresh attempt has already retired. A
    /// read alone cannot exclude the future; this is the fence that does
    /// (protocol v14, `retire_creation`).
    CreationRetired,
    /// A chat-origin creation arrived from a connection whose creation
    /// generation is no longer current: a successor `ai_control` hello has
    /// been accepted since. The old worker's create is refused whenever it
    /// reaches admission, even from inside a handler that was already
    /// waiting (protocol v16, `SessionOrigin`).
    StaleAuthority,
    /// A chat-origin creation for a chat that already has a live session or
    /// an in-flight reservation under a different key. One conversation per
    /// chat is the host's invariant; the caller looks the origin up and
    /// reattaches or waits (protocol v16, `SessionOrigin`).
    OriginOwned,
    Forbidden,
    Internal,
    Unknown(String),
}

impl ErrorCode {
    pub const ALL: &'static [Self] = &[
        Self::InvalidId,
        Self::InvalidRequest,
        Self::InvalidTransition,
        Self::InvalidSize,
        Self::ProtocolIncompatible,
        Self::CapabilityNotNegotiated,
        Self::SessionNotFound,
        Self::ProfileNotFound,
        Self::AttachmentNotFound,
        Self::TurnNotFound,
        Self::ApprovalNotFound,
        Self::ProfileMissing,
        Self::ProfileIncompatible,
        Self::IdempotencyConflict,
        Self::ControllerConflict,
        Self::ControllerRequired,
        Self::StaleLease,
        Self::StaleGeneration,
        Self::StreamGap,
        Self::SpawnFailed,
        Self::SessionExited,
        Self::HostUnavailable,
        Self::InputTooLarge,
        Self::Backpressure,
        Self::AttachmentLimit,
        Self::UnsupportedMedia,
        Self::InvalidImage,
        Self::DeliveryUnsupported,
        Self::DeliveryFailed,
        Self::ProviderUnavailable,
        Self::ProviderIncompatible,
        Self::ProviderModelUnavailable,
        Self::ProviderFailed,
        Self::ApprovalStale,
        Self::ProjectNotFound,
        Self::ProjectHasSessions,
        Self::ProjectMismatch,
        Self::CreationRetired,
        Self::StaleAuthority,
        Self::OriginOwned,
        Self::Forbidden,
        Self::Internal,
    ];

    pub fn as_str(&self) -> &str {
        match self {
            Self::InvalidId => "invalid_id",
            Self::InvalidRequest => "invalid_request",
            Self::InvalidTransition => "invalid_transition",
            Self::InvalidSize => "invalid_size",
            Self::ProtocolIncompatible => "protocol_incompatible",
            Self::CapabilityNotNegotiated => "capability_not_negotiated",
            Self::SessionNotFound => "session_not_found",
            Self::ProfileNotFound => "profile_not_found",
            Self::AttachmentNotFound => "attachment_not_found",
            Self::TurnNotFound => "turn_not_found",
            Self::ApprovalNotFound => "approval_not_found",
            Self::ProfileMissing => "profile_missing",
            Self::ProfileIncompatible => "profile_incompatible",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::ControllerConflict => "controller_conflict",
            Self::ControllerRequired => "controller_required",
            Self::StaleLease => "stale_lease",
            Self::StaleGeneration => "stale_generation",
            Self::StreamGap => "stream_gap",
            Self::SpawnFailed => "spawn_failed",
            Self::SessionExited => "session_exited",
            Self::HostUnavailable => "host_unavailable",
            Self::InputTooLarge => "input_too_large",
            Self::Backpressure => "backpressure",
            Self::AttachmentLimit => "attachment_limit",
            Self::UnsupportedMedia => "unsupported_media",
            Self::InvalidImage => "invalid_image",
            Self::DeliveryUnsupported => "delivery_unsupported",
            Self::DeliveryFailed => "delivery_failed",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderIncompatible => "provider_incompatible",
            Self::ProviderModelUnavailable => "provider_model_unavailable",
            Self::ProviderFailed => "provider_failed",
            Self::ApprovalStale => "approval_stale",
            Self::ProjectNotFound => "project_not_found",
            Self::ProjectHasSessions => "project_has_sessions",
            Self::ProjectMismatch => "project_mismatch",
            Self::CreationRetired => "creation_retired",
            Self::StaleAuthority => "stale_authority",
            Self::OriginOwned => "origin_owned",
            Self::Forbidden => "forbidden",
            Self::Internal => "internal",
            Self::Unknown(value) => value,
        }
    }

    pub fn class(&self) -> ErrorClass {
        match self {
            Self::InvalidId
            | Self::InvalidRequest
            | Self::InvalidTransition
            | Self::InvalidSize => ErrorClass::InvalidRequest,
            Self::IdempotencyConflict
            | Self::ControllerConflict
            | Self::StaleLease
            | Self::StaleGeneration
            | Self::StreamGap
            | Self::ApprovalStale
            | Self::ProjectHasSessions
            | Self::ProjectMismatch
            | Self::CreationRetired
            | Self::StaleAuthority
            | Self::OriginOwned => ErrorClass::Conflict,
            Self::SessionNotFound
            | Self::ProfileNotFound
            | Self::AttachmentNotFound
            | Self::TurnNotFound
            | Self::ApprovalNotFound
            | Self::ProjectNotFound => ErrorClass::NotFound,
            Self::ProfileMissing
            | Self::HostUnavailable
            | Self::ProviderUnavailable
            | Self::ProviderModelUnavailable
            | Self::Backpressure => ErrorClass::Unavailable,
            Self::ProtocolIncompatible
            | Self::CapabilityNotNegotiated
            | Self::ProfileIncompatible
            | Self::DeliveryUnsupported
            | Self::UnsupportedMedia
            | Self::ProviderIncompatible => ErrorClass::Unsupported,
            Self::Forbidden => ErrorClass::Forbidden,
            _ => ErrorClass::Internal,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(serde::de::Error::custom("invalid stable error token"));
        }
        Ok(Self::ALL
            .iter()
            .find(|code| code.as_str() == value)
            .cloned()
            .unwrap_or(Self::Unknown(value)))
    }
}

impl JsonSchema for ErrorCode {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ErrorCode")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({ "type": "string", "minLength": 1, "description": "Stable error token; unknown values remain errors." })
    }
}

/// A content-free developer diagnostic. UI maps `code` to localized copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionFailure {
    pub code: ErrorCode,
    pub message: String,
}
