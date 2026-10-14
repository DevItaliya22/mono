// SPDX-License-Identifier: MPL-2.0

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use uuid::{Uuid, Variant, Version};

use crate::{AuthIdentityId, DeviceId, KoruIdentityId, ProfileId, UnixMillis};

pub const MAX_PROFILE_LIFECYCLE_RESOLVE_IDS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProfileLifecycleState {
    Active,
    Deleted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ProfileLifecycleRevision(u64);

impl ProfileLifecycleRevision {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl<'de> Deserialize<'de> for ProfileLifecycleRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(u64::deserialize(deserializer)?))
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for ProfileLifecycleRevision {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ProfileLifecycleRevision")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "integer", "minimum": 0})
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProfileLifecycleMutationId(Uuid);

impl ProfileLifecycleMutationId {
    pub fn mint() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Debug for ProfileLifecycleMutationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "profile-lifecycle-operation({:.8})",
            self.0.as_hyphenated()
        )
    }
}

impl fmt::Display for ProfileLifecycleMutationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ProfileLifecycleMutationId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parsed = Uuid::parse_str(value).map_err(|_| "invalid lifecycle operation id")?;
        if parsed.hyphenated().to_string() != value
            || parsed.get_version() != Some(Version::Random)
            || parsed.get_variant() != Variant::RFC4122
        {
            return Err("lifecycle operation id must be a canonical UUIDv4");
        }
        Ok(Self(parsed))
    }
}

impl Serialize for ProfileLifecycleMutationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ProfileLifecycleMutationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for ProfileLifecycleMutationId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ProfileLifecycleMutationId")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "format": "uuid",
            "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProfileLifecycle {
    pub profile_id: ProfileId,
    pub koru_identity_id: KoruIdentityId,
    pub state: ProfileLifecycleState,
    pub revision: ProfileLifecycleRevision,
    pub operation_id: Option<ProfileLifecycleMutationId>,
    pub lifecycle_at_millis: Option<UnixMillis>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteProfileRequest {
    pub koru_identity_id: KoruIdentityId,
    pub profile_id: ProfileId,
    pub authenticated_by: AuthIdentityId,
    pub device_id: DeviceId,
    pub operation_id: ProfileLifecycleMutationId,
    pub expected_revision: ProfileLifecycleRevision,
    pub now_millis: UnixMillis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreProfileRequest {
    pub koru_identity_id: KoruIdentityId,
    pub profile_id: ProfileId,
    pub authenticated_by: AuthIdentityId,
    pub device_id: DeviceId,
    pub operation_id: ProfileLifecycleMutationId,
    pub expected_revision: ProfileLifecycleRevision,
    pub discovery_name_snapshot: Option<String>,
    pub locale_snapshot: Option<String>,
    pub now_millis: UnixMillis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileLifecycleMutationStatus {
    Applied,
    ExactRetry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileLifecycleMutationOutcome {
    pub status: ProfileLifecycleMutationStatus,
    pub lifecycle: ProfileLifecycle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileLifecycleMutationError {
    StaleRevision,
    AlreadyDeleted,
    AlreadyActive,
    WrongIdentity,
    MissingRestoreSource,
    FreshAuthenticationRequired,
    PermissionDenied,
    Unavailable,
    Corrupt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_rejects_negative_json() {
        assert!(serde_json::from_str::<ProfileLifecycleRevision>("-1").is_err());
    }

    #[test]
    fn operation_id_requires_canonical_uuid_v4() {
        assert!(
            "00000000-0000-0000-0000-000000000000"
                .parse::<ProfileLifecycleMutationId>()
                .is_err()
        );
        let id = ProfileLifecycleMutationId::mint();
        assert_eq!(id.to_string().parse(), Ok(id));
    }
}
