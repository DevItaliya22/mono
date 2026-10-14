// SPDX-License-Identifier: MPL-2.0

//! The permission vocabulary — modeled now, owner-only reachable (D11).
//!
//! M7 replicates artifact metadata across devices and transfers content
//! permission-aware. That milestone needs words for *who* may do *what* to a
//! family, and the words are cheaper to fix now than to retrofit under rows
//! that already exist. So the vocabulary is here, `artifacts.db` carries a
//! permission row per family, and every read and write on the port is
//! decided by [`authorize`] against those rows.
//!
//! What is deliberately **not** here is a way to reach any principal but
//! the local owner. There is no route that grants, no principal a gateway
//! session can present but [`Principal::LocalOwner`], and
//! [`Principal::reachable`] says so in code so a later change is a diff to
//! one function rather than a discovery.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{ArtifactError, ArtifactFamilyId, DeviceId, ProfileId, RunId};

/// What a principal may do with a family. `Owner` permits everything `Read`
/// permits; a closed domain the database `CHECK`s too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Owner,
    Read,
}

impl Access {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Read => "read",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "read" => Some(Self::Read),
            _ => None,
        }
    }

    /// Whether holding `self` is enough for an operation that needs
    /// `required`.
    pub fn permits(self, required: Access) -> bool {
        match (self, required) {
            (Self::Owner, _) | (Self::Read, Self::Read) => true,
            (Self::Read, Self::Owner) => false,
        }
    }
}

/// Who holds an access. Four shapes, one reachable.
///
/// The storage spelling is `local`, `profile:<uuid>`, `device:<uuid>` or
/// `run:<uuid>` — a token, so the database can key on it and a reader can
/// tell the kinds apart without a second column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Principal {
    /// The person at this device, through a local gateway session. The one
    /// principal M1 can name, and the owner of every family it creates.
    LocalOwner,
    /// A profile on any device — what M7 will grant to.
    Profile { profile: ProfileId },
    /// One device, for content transfer that is per-device.
    Device {
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        device: DeviceId,
    },
    /// A run, for the read grant §7.3 gives an agent over its inputs.
    Run { run: RunId },
}

impl Principal {
    /// Whether M1 can present this principal at all (D11). Only the local
    /// owner; the rest are vocabulary for M7.
    pub fn reachable(self) -> bool {
        matches!(self, Self::LocalOwner)
    }

    pub fn to_storage(self) -> String {
        match self {
            Self::LocalOwner => "local".to_owned(),
            Self::Profile { profile } => format!("profile:{}", profile.to_storage()),
            Self::Device { device } => format!("device:{}", device.to_storage()),
            Self::Run { run } => format!("run:{}", run.to_storage()),
        }
    }

    pub fn parse(value: &str) -> Result<Self, PrincipalParseError> {
        if value == "local" {
            return Ok(Self::LocalOwner);
        }
        let Some((kind, id)) = value.split_once(':') else {
            return Err(PrincipalParseError);
        };
        match kind {
            "profile" => ProfileId::parse(id)
                .map(|profile| Self::Profile { profile })
                .ok_or(PrincipalParseError),
            "device" => DeviceId::parse(id)
                .map(|device| Self::Device { device })
                .ok_or(PrincipalParseError),
            "run" => id
                .parse()
                .map(|run| Self::Run { run })
                .map_err(|_| PrincipalParseError),
            _ => Err(PrincipalParseError),
        }
    }
}

/// The storage spelling was not one of the four shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrincipalParseError;

impl fmt::Display for PrincipalParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected local, profile:<id>, device:<id> or run:<id>")
    }
}

impl std::error::Error for PrincipalParseError {}

/// One grant: a principal holds an access on a family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Permission {
    pub family: ArtifactFamilyId,
    pub principal: Principal,
    pub access: Access,
    pub granted_at_ms: i64,
}

/// The one decision: may `principal` do something that needs `required` on
/// the family these permissions belong to?
///
/// Pure, so the rule is testable without a database and the store and the
/// service cannot drift into two rules. Refused with
/// [`ArtifactError::PermissionDenied`], which never names the principal.
pub fn authorize(
    permissions: &[Permission],
    principal: Principal,
    required: Access,
) -> crate::Result<()> {
    if permissions
        .iter()
        .any(|held| held.principal == principal && held.access.permits(required))
    {
        Ok(())
    } else {
        Err(ArtifactError::PermissionDenied)
    }
}
