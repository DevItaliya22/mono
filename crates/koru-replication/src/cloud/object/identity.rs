// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::profile_units::ProfileUnitRef;

/// Stored-object kinds, not the four-kind transfer carrier's JSON Unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    WorkspaceManifest,
    WorkspaceDocument { document_id: CloudId },
    ProfileIdentity,
    ProfilePrivate,
    ProfilePersonal { workspace_id: CloudId },
}
impl Unit {
    pub(super) fn tag(self) -> u8 {
        match self {
            Self::WorkspaceManifest => 1,
            Self::WorkspaceDocument { .. } => 2,
            Self::ProfileIdentity => 3,
            Self::ProfilePrivate => 4,
            Self::ProfilePersonal { .. } => 5,
        }
    }
    pub(super) fn subject(self) -> Option<CloudId> {
        match self {
            Self::WorkspaceDocument { document_id } => Some(document_id),
            Self::ProfilePersonal { workspace_id } => Some(workspace_id),
            _ => None,
        }
    }
    pub(super) fn parse(tag: u8, subject: Option<CloudId>) -> Result<Self, Error> {
        match (tag, subject) {
            (1, None) => Ok(Self::WorkspaceManifest),
            (2, Some(document_id)) => Ok(Self::WorkspaceDocument { document_id }),
            (3, None) => Ok(Self::ProfileIdentity),
            (4, None) => Ok(Self::ProfilePrivate),
            (5, Some(workspace_id)) => Ok(Self::ProfilePersonal { workspace_id }),
            _ => Err(Error::Unit),
        }
    }
    pub(super) fn validate(self, dataset: CloudReplicaPrincipalV1) -> Result<(), Error> {
        if matches!(dataset, CloudReplicaPrincipalV1::Workspace { .. }) != (self.tag() <= 2) {
            Err(Error::Unit)
        } else {
            Ok(())
        }
    }
    pub fn digest(self, dataset: CloudReplicaPrincipalV1) -> Result<Hash, Error> {
        self.validate(dataset)?;
        let (tag, authority, _) = parts(dataset);
        let mut digest = Sha256::new();
        digest.update(b"koru/cloud/unit-ref/v1\0");
        digest.update([tag]);
        digest.update(authority.as_bytes());
        digest.update([self.tag(), u8::from(self.subject().is_some())]);
        if let Some(subject) = self.subject() {
            digest.update(subject.as_bytes());
        }
        Ok(digest.finalize().into())
    }
    pub fn profile_unit(self, dataset: CloudReplicaPrincipalV1) -> Result<ProfileUnitRef, Error> {
        self.validate(dataset)?;
        let CloudReplicaPrincipalV1::Profile { profile_id, .. } = dataset else {
            return Err(Error::Unit);
        };
        let profile =
            koru_store_core::ProfileId::parse(&uuid_text(profile_id)).ok_or(Error::Unit)?;
        match self {
            Self::ProfileIdentity => Ok(ProfileUnitRef::Identity { profile }),
            Self::ProfilePrivate => Ok(ProfileUnitRef::Private { profile }),
            Self::ProfilePersonal { workspace_id } => Ok(ProfileUnitRef::Personal {
                owner: profile,
                workspace: koru_store_core::WorkspaceId::parse(&uuid_text(workspace_id))
                    .ok_or(Error::Unit)?,
            }),
            _ => Err(Error::Unit),
        }
    }
    fn token(self) -> String {
        match self {
            Self::WorkspaceManifest => "manifest".into(),
            Self::WorkspaceDocument { document_id } => uuid_text(document_id),
            Self::ProfileIdentity => "identity".into(),
            Self::ProfilePrivate => "private".into(),
            Self::ProfilePersonal { workspace_id } => {
                format!("personal-{}", uuid_text(workspace_id))
            }
        }
    }
}

pub(super) fn parts(dataset: CloudReplicaPrincipalV1) -> (u8, CloudId, CloudId) {
    match dataset {
        CloudReplicaPrincipalV1::Workspace {
            workspace_id,
            incarnation_id,
            ..
        } => (1, workspace_id, incarnation_id),
        CloudReplicaPrincipalV1::Profile {
            profile_id,
            lineage_id,
            ..
        } => (2, profile_id, lineage_id),
    }
}
pub(super) fn uuid_text(id: CloudId) -> String {
    id.to_string()
}

pub fn logical_key(metadata: Metadata) -> Result<String, Error> {
    metadata.validate()?;
    let (tag, authority, lineage) = parts(metadata.dataset);
    let key = format!(
        "{}/{}/{}/{}-{}-{}.loro",
        if tag == 1 { "w" } else { "p" },
        uuid_text(authority),
        metadata.unit.token(),
        uuid_text(lineage),
        if metadata.kind == Kind::Checkpoint {
            "c"
        } else {
            "s"
        },
        uuid_text(metadata.object_id)
    );
    if key.len() > KEY_MAX {
        Err(Error::TooLarge)
    } else {
        Ok(key)
    }
}

/// Diagnostic naming only: this type cannot authorize cleanup or open a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingName(String);
impl StagingName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
pub fn staging_name(metadata: Metadata, nonce: [u8; 16]) -> Result<StagingName, Error> {
    metadata.validate()?;
    let (tag, authority, lineage) = parts(metadata.dataset);
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let name = format!(
        "staging/{}/{}/{}/{}/{}/{}/{}-{}/{}-{}.tmp",
        if tag == 1 { "w" } else { "p" },
        uuid_text(authority),
        uuid_text(lineage),
        metadata.unit.token(),
        uuid_text(metadata.dataset.logical_replica_id()),
        uuid_text(metadata.writer_worker_id),
        uuid_text(metadata.writer_lease_id),
        metadata.writer_lease_epoch,
        uuid_text(metadata.object_id),
        nonce
    );
    if name.len() > STAGING_MAX {
        Err(Error::TooLarge)
    } else {
        Ok(StagingName(name))
    }
}

#[cfg(test)]
mod tests;
