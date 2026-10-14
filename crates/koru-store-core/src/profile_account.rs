// SPDX-License-Identifier: MPL-2.0

//! The validated Store spelling of a hosted Koru Identity binding.
//!
//! `koru-account-core` depends on this crate for local [`ProfileId`](crate::ProfileId),
//! so Store core cannot name its `KoruIdentityId` without a dependency cycle.
//! This boundary type deliberately applies the identical canonical v4 UUID rules
//! and owns only the persisted `koru:<identity-uuid>` spelling.

use uuid::{Uuid, Variant, Version};

use crate::errors::{Code, Error, Result};

pub const KORU_ACCOUNT_PREFIX: &str = "koru:";

/// One validated hosted Koru Identity as stored in [`Profile::account`](crate::Profile::account).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProfileAccountBinding(Uuid);

/// Attribution for local catalog presentation, never authorization or access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalProfileAttribution {
    Unbound,
    Account(ProfileAccountBinding),
    Invalid,
}

impl LocalProfileAttribution {
    pub fn from_binding(binding: &str) -> Self {
        if binding.is_empty() {
            return Self::Unbound;
        }
        ProfileAccountBinding::parse_storage(binding)
            .map(Self::Account)
            .unwrap_or(Self::Invalid)
    }

    /// Apply after lifecycle filtering. With no active account all retained
    /// local Profiles remain visible; malformed nonempty values are not unbound.
    pub fn visible_for(self, active: Option<ProfileAccountBinding>) -> bool {
        match (active, self) {
            (None, _) | (_, Self::Unbound) => true,
            (Some(active), Self::Account(owner)) => active == owner,
            (Some(_), Self::Invalid) => false,
        }
    }
}

impl ProfileAccountBinding {
    /// Parse the identity-id part supplied by an authenticated caller.
    pub fn parse_identity_id(value: &str) -> Result<Self> {
        let parsed = Uuid::parse_str(value).map_err(|_| invalid_identity_id())?;
        if parsed.hyphenated().to_string() != value
            || parsed.get_version() != Some(Version::Random)
            || parsed.get_variant() != Variant::RFC4122
        {
            return Err(invalid_identity_id());
        }
        Ok(Self(parsed))
    }

    /// Construct from the canonical account layer's already-validated UUID.
    pub fn from_identity_uuid(value: Uuid) -> Result<Self> {
        Self::parse_identity_id(&value.hyphenated().to_string())
    }

    pub fn identity_id(self) -> String {
        self.0.hyphenated().to_string()
    }

    pub fn to_storage(self) -> String {
        format!("{KORU_ACCOUNT_PREFIX}{}", self.0.hyphenated())
    }

    /// Read an existing Profile field. A non-canonical stored value is corrupt
    /// state, never something a compare-and-set may silently reinterpret.
    pub fn parse_storage(value: &str) -> Result<Self> {
        let Some(identity) = value.strip_prefix(KORU_ACCOUNT_PREFIX) else {
            return Err(invalid_stored_binding());
        };
        let binding = Self::parse_identity_id(identity).map_err(|_| invalid_stored_binding())?;
        if binding.to_storage() != value {
            return Err(invalid_stored_binding());
        }
        Ok(binding)
    }
}

/// Durable result of one Profile account compare-and-set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileAccountBindingOutcome {
    Bound,
    AlreadyBound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileAccountClearOutcome {
    Cleared,
    AlreadyOffline,
}

fn invalid_identity_id() -> Error {
    Error::new(
        Code::InvalidId,
        "Koru identity id must be a canonical lowercase hyphenated v4 UUID",
    )
}

fn invalid_stored_binding() -> Error {
    Error::new(
        Code::CorruptState,
        "Profile account must be exactly koru:<canonical-v4-identity-uuid>",
    )
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod visibility_tests;
