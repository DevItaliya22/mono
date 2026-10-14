// SPDX-License-Identifier: MPL-2.0

//! Bounded Cloud lease vocabulary. No value here proves live authority or bytes.
//!
//! The repository must lock and recheck current hosted authority before replay.

use sha2::{Digest, Sha256};
use uuid::{Uuid, Variant, Version};

use crate::{PortFuture, ProfileId, WorkspaceId};

pub const LEASE_DURATION_MILLIS: u64 = 30_000;
pub const LEASE_RENEW_INTERVAL_MILLIS: u64 = 10_000;
pub const MAX_LEASE_INTEGER: u64 = i64::MAX as u64;

/// Current hosted eligibility to be re-read under the transaction's locks.
/// These are expected values, never caller-minted proof of authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatasetAuthorityV1 {
    Workspace {
        workspace_id: WorkspaceId,
        incarnation_id: Uuid,
        cloud_access_epoch: u64,
    },
    Profile {
        profile_id: ProfileId,
        lineage_id: Uuid,
        authority_workspace_id: WorkspaceId,
        authorization_version: u64,
        membership_epoch: u64,
        cloud_access_epoch: u64,
    },
}

impl DatasetAuthorityV1 {
    pub fn lineage_id(self) -> Uuid {
        match self {
            Self::Workspace { incarnation_id, .. } => incarnation_id,
            Self::Profile { lineage_id, .. } => lineage_id,
        }
    }

    fn identity(self) -> (u8, Uuid) {
        match self {
            Self::Workspace { workspace_id, .. } => (1, workspace_id.as_uuid()),
            Self::Profile { profile_id, .. } => (2, profile_id.as_uuid()),
        }
    }

    fn validate(self) -> Result<(), InvalidLeaseRequest> {
        valid_uuid(self.identity().1)?;
        valid_uuid(self.lineage_id())?;
        match self {
            Self::Workspace {
                cloud_access_epoch, ..
            } => positive(cloud_access_epoch),
            Self::Profile {
                authority_workspace_id,
                authorization_version,
                membership_epoch,
                cloud_access_epoch,
                ..
            } => {
                valid_uuid(authority_workspace_id.as_uuid())?;
                positive(authorization_version)?;
                positive(membership_epoch)?;
                positive(cloud_access_epoch)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedLeaseV1 {
    pub lease_id: Uuid,
    pub lease_epoch: u64,
    pub lease_expires_at_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseCommandV1 {
    Place,
    Acquire { expected_epoch: u64 },
    Renew { expected: ExpectedLeaseV1 },
    Release { expected: ExpectedLeaseV1 },
}

impl LeaseCommandV1 {
    pub fn expected_epoch(self) -> u64 {
        match self {
            Self::Place => 0,
            Self::Acquire { expected_epoch } => expected_epoch,
            Self::Renew { expected } | Self::Release { expected } => expected.lease_epoch,
        }
    }

    pub fn expected_lease(self) -> Option<ExpectedLeaseV1> {
        match self {
            Self::Renew { expected } | Self::Release { expected } => Some(expected),
            Self::Place | Self::Acquire { .. } => None,
        }
    }

    pub fn kind(self) -> LeaseOperationKindV1 {
        match self {
            Self::Place => LeaseOperationKindV1::Place,
            Self::Acquire { .. } => LeaseOperationKindV1::Acquire,
            Self::Renew { .. } => LeaseOperationKindV1::Renew,
            Self::Release { .. } => LeaseOperationKindV1::Release,
        }
    }
}

/// Move is intentionally unrepresentable in the implemented v1 command set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LeaseOperationKindV1 {
    Place = 1,
    Acquire = 2,
    Renew = 3,
    Release = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseRequestV1 {
    pub operation_id: Uuid,
    pub logical_replica_id: Uuid,
    pub authority: DatasetAuthorityV1,
    pub worker_id: Uuid,
    pub command: LeaseCommandV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidLeaseRequest {
    Uuid,
    IntegerDomain,
}

fn valid_uuid(value: Uuid) -> Result<(), InvalidLeaseRequest> {
    if value.get_version() == Some(Version::Random) && value.get_variant() == Variant::RFC4122 {
        Ok(())
    } else {
        Err(InvalidLeaseRequest::Uuid)
    }
}

fn nonnegative(value: u64) -> Result<(), InvalidLeaseRequest> {
    if value <= MAX_LEASE_INTEGER {
        Ok(())
    } else {
        Err(InvalidLeaseRequest::IntegerDomain)
    }
}

fn positive(value: u64) -> Result<(), InvalidLeaseRequest> {
    if value == 0 {
        return Err(InvalidLeaseRequest::IntegerDomain);
    }
    nonnegative(value)
}

impl LeaseRequestV1 {
    pub fn validate(self) -> Result<(), InvalidLeaseRequest> {
        valid_uuid(self.operation_id)?;
        valid_uuid(self.logical_replica_id)?;
        valid_uuid(self.worker_id)?;
        self.authority.validate()?;
        nonnegative(self.command.expected_epoch())?;
        if let Some(expected) = self.command.expected_lease() {
            valid_uuid(expected.lease_id)?;
            positive(expected.lease_epoch)?;
            positive(expected.lease_expires_at_millis)?;
        }
        Ok(())
    }

    /// Fixed, bounded binary grammar; operation UUID is the separate lookup key.
    /// Eligibility epochs remain live SQL preconditions, not the replay shortcut.
    pub fn canonical_bytes(self) -> Result<Vec<u8>, InvalidLeaseRequest> {
        self.validate()?;
        let mut bytes = Vec::with_capacity(144);
        bytes.extend_from_slice(b"koru/cloud/lease-operation/v1\0");
        bytes.push(self.command.kind() as u8);
        bytes.extend_from_slice(self.logical_replica_id.as_bytes());
        let (kind, authority_id) = self.authority.identity();
        bytes.push(kind);
        bytes.extend_from_slice(authority_id.as_bytes());
        bytes.extend_from_slice(self.authority.lineage_id().as_bytes());
        bytes.extend_from_slice(self.worker_id.as_bytes());
        bytes.extend_from_slice(&self.command.expected_epoch().to_be_bytes());
        match self.command.expected_lease() {
            None => bytes.push(0),
            Some(expected) => {
                bytes.push(1);
                bytes.extend_from_slice(expected.lease_id.as_bytes());
                bytes.extend_from_slice(&expected.lease_expires_at_millis.to_be_bytes());
            }
        }
        bytes.extend_from_slice(&[0, 0]); // No target worker or move proof in v1.
        Ok(bytes)
    }

    pub fn digest(self) -> Result<[u8; 32], InvalidLeaseRequest> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseFenceV1 {
    pub logical_replica_id: Uuid,
    pub lineage_id: Uuid,
    pub worker_id: Uuid,
    pub lease_id: Uuid,
    pub lease_epoch: u64,
    pub lease_expires_at_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseMutationV1 {
    Placed {
        logical_replica_id: Uuid,
        lineage_id: Uuid,
        worker_id: Uuid,
    },
    Acquired {
        fence: LeaseFenceV1,
    },
    Renewed {
        fence: LeaseFenceV1,
    },
    Released {
        logical_replica_id: Uuid,
        worker_id: Uuid,
        lease_epoch: u64,
    },
}

/// A retry carries the old expiry even if it is expired. Never a durable receipt.
/// ExactRetry reconstructs a retained result, not current process-generation
/// readiness. Worker composition must fence its actual generation separately
/// before using even an unexpired result as part of a data-write decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseMutationResultV1 {
    Applied(LeaseMutationV1),
    ExactRetry(LeaseMutationV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseErrorV1 {
    InvalidRequest(InvalidLeaseRequest),
    AuthorizationUnavailable,
    WorkspaceNotAuthorized,
    WrongRegion,
    CloudDisabled,
    CloudDeleting,
    CloudIncarnationStale,
    CloudAccessEpochMismatch,
    AuthorizationVersionMismatch,
    MembershipEpochMismatch,
    ReplicaExists,
    ReplicaFenced,
    LeaseHeld,
    OperationConflict,
    StorageMoveRequired,
    SyncUnavailable,
}

/// Installed only by trusted worker composition, never selected by request data.
/// Implementations recheck authority and worker readiness inside the locked SQL
/// transaction before any mutation. Missing composition is unavailable, not ready.
pub trait CloudLeaseRepository: Send + Sync {
    fn execute<'a>(
        &'a self,
        request: &'a LeaseRequestV1,
    ) -> PortFuture<'a, Result<LeaseMutationResultV1, LeaseErrorV1>>;
}

#[cfg(test)]
mod tests;
