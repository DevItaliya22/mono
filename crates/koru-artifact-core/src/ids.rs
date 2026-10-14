// SPDX-License-Identifier: MPL-2.0

//! The two identities this crate mints.
//!
//! [`ArtifactVersionId`](crate::ArtifactVersionId) is deliberately *not* here:
//! `koru-delegate-core` minted it so a run could name a result before the
//! artifact service existed, and this crate speaks that id rather than a
//! second one. What is minted here is the family — the thing a person names —
//! and the registration operation, which is the idempotency key of §7.1.
//!
//! Both are client-minted v4 UUIDs with the contract every Koru id has
//! (ADR-39): canonical lowercase hyphenated spelling, never derived from a
//! name or a path, and a `Debug` of a prefix and eight hex characters.

use std::{fmt, str::FromStr};

use koru_delegate_core::IdParseError;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use uuid::{Uuid, Variant, Version};

fn parse_v4(value: &str) -> Result<Uuid, IdParseError> {
    let parsed = Uuid::parse_str(value).map_err(|_| IdParseError::InvalidSpelling)?;
    if parsed.hyphenated().to_string() != value {
        return Err(IdParseError::InvalidSpelling);
    }
    if parsed.get_version() != Some(Version::Random) || parsed.get_variant() != Variant::RFC4122 {
        return Err(IdParseError::NotVersion4);
    }
    Ok(parsed)
}

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// Mint an opaque v4 UUID. Never derived from a name or a path.
            pub fn mint() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap a UUID minted elsewhere — a fixture, a database row, a
            /// wire value already validated.
            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            pub fn as_uuid(self) -> Uuid {
                self.0
            }

            /// The one spelling stored anywhere: hyphenated and lowercase.
            pub fn to_storage(self) -> String {
                self.0.hyphenated().to_string()
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                parse_v4(value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.hyphenated().fmt(f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let full = self.0.simple().to_string();
                write!(f, "{}({})", $prefix, &full[..8])
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.to_storage())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(de::Error::custom)
            }
        }

        /// Written by hand because the serde above is: a derive would
        /// describe the `Uuid` inside rather than the string that crosses.
        #[cfg(feature = "schema")]
        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed(stringify!($name))
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
                    "type": "string",
                    "format": "uuid",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
                })
            }
        }
    };
}

uuid_id!(
    /// One artifact family: the thing a person names, under which immutable
    /// versions accumulate. Minted by the caller that creates it, so a
    /// retried creation is the same family.
    ArtifactFamilyId,
    "family"
);
uuid_id!(
    /// One registration operation — the idempotency key of §7.1's
    /// `artifacts.db` row. A retry carries the same id and finds the version
    /// the first attempt made; a different id is a different version even
    /// for the same bytes.
    RegistrationId,
    "registration"
);
