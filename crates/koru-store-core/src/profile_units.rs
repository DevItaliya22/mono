// SPDX-License-Identifier: MPL-2.0

//! Independently addressed Profile documents. Constructors author fresh lineages;
//! no unit is a filtered export of the legacy Profile document.

use loro::{ExportMode, LoroDoc, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};

use crate::errors::{Code, Error, Result};
use crate::ids::{ProfileId, WorkspaceId};
use crate::version::{
    Compatibility, PERSONAL_CRDT_SCHEMA_VERSION, PROFILE_IDENTITY_CRDT_SCHEMA_VERSION,
    PROFILE_PRIVATE_CRDT_SCHEMA_VERSION,
};

pub(crate) mod content;
mod identity;
mod personal;
mod private;
mod validation;
pub use identity::IdentityManifest;
pub use personal::PersonalManifest;
pub use private::PrivateManifest;

/// Required wire address. Unknown tags/fields never select another unit.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProfileUnitRef {
    Identity {
        profile: ProfileId,
    },
    LegacyPrivate {
        profile: ProfileId,
    },
    Personal {
        owner: ProfileId,
        workspace: WorkspaceId,
    },
    Private {
        profile: ProfileId,
    },
}

impl ProfileUnitRef {
    pub fn profile(self) -> ProfileId {
        match self {
            Self::LegacyPrivate { profile }
            | Self::Identity { profile }
            | Self::Private { profile } => profile,
            Self::Personal { owner, .. } => owner,
        }
    }

    pub fn workspace(self) -> Option<WorkspaceId> {
        match self {
            Self::Personal { workspace, .. } => Some(workspace),
            _ => None,
        }
    }

    fn schema(self) -> i64 {
        match self {
            Self::Identity { .. } => PROFILE_IDENTITY_CRDT_SCHEMA_VERSION,
            Self::Personal { .. } => PERSONAL_CRDT_SCHEMA_VERSION,
            Self::Private { .. } => PROFILE_PRIVATE_CRDT_SCHEMA_VERSION,
            Self::LegacyPrivate { .. } => unreachable!("legacy does not use fresh unit storage"),
        }
    }
}

struct UnitDoc {
    doc: LoroDoc,
    unit: ProfileUnitRef,
    compatibility: Compatibility,
}

impl UnitDoc {
    fn fresh(unit: ProfileUnitRef) -> Result<Self> {
        let doc = LoroDoc::new();
        let meta = doc.get_map("meta");
        meta.insert("schema_version", unit.schema())
            .map_err(loro_failed)?;
        match unit {
            ProfileUnitRef::Personal { owner, workspace } => {
                meta.insert("owner_profile_id", owner.to_storage())
                    .map_err(loro_failed)?;
                meta.insert("workspace_id", workspace.to_storage())
                    .map_err(loro_failed)?;
            }
            _ => meta
                .insert("profile_id", unit.profile().to_storage())
                .map_err(loro_failed)?,
        }
        doc.commit();
        Ok(Self {
            doc,
            unit,
            compatibility: Compatibility::Writable,
        })
    }

    fn load(
        bytes: &[u8],
        kind: fn(ProfileId, Option<WorkspaceId>) -> Result<ProfileUnitRef>,
    ) -> Result<Self> {
        let doc = LoroDoc::new();
        let status = doc
            .import(bytes)
            .map_err(|_| corrupt("invalid unit snapshot"))?;
        if status.pending.is_some() {
            return Err(corrupt("unit snapshot has missing causal ancestors"));
        }
        let meta = doc.get_map("meta");
        let owner = string(&meta, "owner_profile_id")
            .or_else(|| string(&meta, "profile_id"))
            .and_then(|id| ProfileId::parse(&id))
            .ok_or_else(|| corrupt("unit has no Profile scope"))?;
        let workspace = string(&meta, "workspace_id").and_then(|id| WorkspaceId::parse(&id));
        let unit = kind(owner, workspace)?;
        let compatibility = validation::validate(&doc, unit)?;
        Ok(Self {
            doc,
            unit,
            compatibility,
        })
    }

    fn require_writable(&self) -> Result<()> {
        if self.compatibility.writable() {
            Ok(())
        } else {
            Err(Error::new(
                Code::SchemaTooNew,
                "Profile unit format is newer than this build",
            ))
        }
    }

    fn snapshot(&self) -> Result<Vec<u8>> {
        self.doc
            .export(ExportMode::snapshot())
            .map_err(|_| Error::new(Code::StorageFailed, "unit snapshot export failed"))
    }

    fn staged_import(&self, frames: &[Vec<u8>]) -> Result<Self> {
        self.require_writable()?;
        let doc = LoroDoc::new();
        doc.import(&self.snapshot()?)
            .map_err(|_| corrupt("unit staging failed"))?;
        doc.set_peer_id(self.doc.peer_id()).map_err(loro_failed)?;
        for frame in frames {
            // A complete foreign snapshot can lose the metadata LWW conflict
            // while still adding its content. Validate its own scope before
            // merging; a causal delta is checked against the staged state.
            let incoming = LoroDoc::new();
            let isolated = incoming
                .import(frame)
                .map_err(|_| Error::new(Code::CrdtRejected, "unit frame rejected"))?;
            if isolated.pending.is_none()
                && integer(&incoming.get_map("meta"), "schema_version").is_some()
            {
                validation::validate(&incoming, self.unit)?;
            }
            let status = doc
                .import(frame)
                .map_err(|_| Error::new(Code::CrdtRejected, "unit frame rejected"))?;
            if status.pending.is_some() {
                return Err(Error::new(
                    Code::CrdtRejected,
                    "unit frame has missing causal ancestors",
                ));
            }
            let compatibility = validation::validate(&doc, self.unit)?;
            if !compatibility.writable() {
                return Err(Error::new(
                    Code::SchemaTooNew,
                    "incoming unit format is newer than this build",
                ));
            }
            if matches!(self.unit, ProfileUnitRef::Identity { .. }) {
                let before = self.doc.get_map("meta");
                let after = doc.get_map("meta");
                // Not a scope mismatch: the frame names this exact unit and
                // every scope check above passed. What it disagrees about is a
                // field the contract calls immutable, which can only happen
                // when a second `IdentityManifest::create` lineage exists for
                // this Profile. Saying `scope` here sends the next reader to
                // the unit reference, the export mode and the advertised
                // version — all three of which are correct. See PLAN19 round
                // five.
                //
                // **Reported, not refused.** Refusing is self-sealing: each
                // side keeps its own lineage and rejects the other in both
                // directions forever, and only a device holding no copy can
                // ever escape. Worse, one refused unit ends the whole dataset
                // pass, so a disagreement about a *name* froze the private and
                // personal units beside it on every pass, permanently.
                //
                // Merging is self-healing. Loro takes both histories,
                // last-writer-wins settles one `created_at` and one `kind`,
                // and the merged document then converges everywhere it
                // reaches. Nothing a person cannot retype is lost, and
                // nothing is dropped: a Profile's Workspace list is not in
                // this document at all.
                //
                // The signal is kept because two lineages still mean a defect
                // upstream, and this is the only place that can see it.
                identity_lineage_divergence(&self.unit, &before, &after);
            }
        }
        Ok(Self {
            doc,
            unit: self.unit,
            compatibility: self.compatibility,
        })
    }
}

// Every public manifest uses the same validated staging boundary. Replacing
// self publishes only a complete candidate; a rejected frame cannot poison it.
macro_rules! unit_methods {
    () => {
        pub fn unit(&self) -> ProfileUnitRef {
            self.inner.unit
        }
        pub fn compatibility(&self) -> Compatibility {
            self.inner.compatibility
        }
        pub fn is_read_only(&self) -> bool {
            !self.compatibility().writable()
        }
        pub fn set_peer_id(&self, peer: u64) -> Result<()> {
            self.inner.require_writable()?;
            self.inner.doc.set_peer_id(peer).map_err(loro_failed)
        }
        pub fn export_snapshot(&self) -> Result<Vec<u8>> {
            self.inner.snapshot()
        }
        pub fn export_updates_since(&self, from: &loro::VersionVector) -> Result<Vec<u8>> {
            self.inner.require_writable()?;
            self.inner
                .doc
                .export(ExportMode::updates(from))
                .map_err(|_| Error::new(Code::StorageFailed, "unit update export failed"))
        }
        pub fn version(&self) -> loro::VersionVector {
            self.inner.doc.state_vv()
        }
        pub fn staged_import(&self, frames: &[Vec<u8>]) -> Result<Self> {
            Ok(Self {
                inner: self.inner.staged_import(frames)?,
            })
        }
        pub fn import_frames(&mut self, frames: &[Vec<u8>]) -> Result<()> {
            *self = self.staged_import(frames)?;
            Ok(())
        }
    };
}
use unit_methods;

fn string(map: &loro::LoroMap, key: &str) -> Option<String> {
    match map.get(key)? {
        ValueOrContainer::Value(LoroValue::String(v)) => Some(v.to_string()),
        _ => None,
    }
}
fn integer(map: &loro::LoroMap, key: &str) -> Option<i64> {
    match map.get(key)? {
        ValueOrContainer::Value(LoroValue::I64(v)) => Some(v),
        _ => None,
    }
}
fn optional(map: &loro::LoroMap, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => map.insert(key, v).map_err(loro_failed)?,
        None if map.get(key).is_some() => map.delete(key).map_err(loro_failed)?,
        None => {}
    }
    Ok(())
}
fn loro_failed(_: loro::LoroError) -> Error {
    Error::new(Code::StorageFailed, "unit CRDT operation failed")
}
fn corrupt(message: &'static str) -> Error {
    Error::new(Code::CorruptState, message)
}
fn scope_mismatch() -> Error {
    Error::new(
        Code::ProfileUnitScopeMismatch,
        "Profile unit scope differs from its address",
    )
}
/// Two `IdentityManifest::create` lineages exist for one Profile and disagree
/// about a field the contract calls immutable.
///
/// This is a **report**, never a refusal. See the call site for why: refusing
/// is permanent and self-sealing, merging is self-healing, and the expensive
/// part was never the field itself but the whole dataset pass it ended.
///
/// Logged at `warn` with both values, because two lineages mean a defect
/// upstream — a device minted a manifest for a Profile it did not originate —
/// and this import is the only place in the system that can observe it.
fn identity_lineage_divergence(
    unit: &ProfileUnitRef,
    before: &loro::LoroMap,
    after: &loro::LoroMap,
) {
    for field in ["created_at", "kind"] {
        let (held, incoming) = match field {
            "created_at" => (
                integer(before, field).map(|v| v.to_string()),
                integer(after, field).map(|v| v.to_string()),
            ),
            _ => (string(before, field), string(after, field)),
        };
        if held != incoming {
            tracing::warn!(
                ?unit,
                field,
                held = ?held,
                incoming = ?incoming,
                code = Code::ProfileIdentityConflict.as_str(),
                "profile identity lineages disagree; merging",
            );
        }
    }
}

#[cfg(test)]
mod tests;
