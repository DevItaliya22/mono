// SPDX-License-Identifier: MPL-2.0

//! Purpose-specific, coherent read boundary for native Cloud credential issuance.
//! A snapshot is evidence at its read point, never authority for later worker use.

use koru_device::{ReplicationEndpointId, cloud::CloudReplicaPrincipalV1};
use uuid::Uuid;

use crate::workspace_authorization::Region;
use crate::{AccessTokenClaims, DeviceId, KoruIdentityId, PortFuture, ProfileId, WorkspaceId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncIssuanceError {
    Unauthenticated,
    PermissionDenied,
    WrongRegion,
    DeviceRevoked,
    EndpointMismatch,
    WorkspaceNotAuthorized,
    CloudDisabled,
    CloudDeleting,
    CloudAbsent,
    AuthorizationStale,
    CloudIncarnationStale,
    ReplicaMismatch,
    SyncUnavailable,
    AuthorizationUnavailable,
}

impl SyncIssuanceError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::PermissionDenied => "permission_denied",
            Self::WrongRegion => "wrong_region",
            Self::DeviceRevoked => "device_revoked",
            Self::EndpointMismatch => "endpoint_mismatch",
            Self::WorkspaceNotAuthorized => "workspace_not_authorized",
            Self::CloudDisabled => "cloud_disabled",
            Self::CloudDeleting => "cloud_deleting",
            Self::CloudAbsent => "cloud_absent",
            Self::AuthorizationStale => "authorization_stale",
            Self::CloudIncarnationStale => "cloud_incarnation_stale",
            Self::ReplicaMismatch => "replica_mismatch",
            Self::SyncUnavailable => "sync_unavailable",
            Self::AuthorizationUnavailable => "authorization_unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeSyncDevice {
    pub identity: KoruIdentityId,
    pub device: DeviceId,
    pub endpoint: ReplicationEndpointId,
    /// Earliest native bearer/session deadline; rechecked before signing.
    pub valid_until_millis: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncDatasetAssignment {
    pub dataset: CloudReplicaPrincipalV1,
    pub region: Region,
    pub worker_id: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncIssuanceAuthority {
    pub authorization_version: i64,
    pub membership_epoch: i64,
    pub cloud_access_epoch: i64,
    pub workspace: SyncDatasetAssignment,
    pub profile: SyncDatasetAssignment,
}

/// The adapter owns one read-only snapshot. Dropping it on any refusal must
/// roll back/release it. Only native/session/device facts may precede the rate
/// write; this port cannot accept a Workspace until after that write completes.
pub trait SyncIssuanceSnapshot: Send {
    fn native_device(&self) -> NativeSyncDevice;
    /// Consumes and releases the snapshot on success or refusal. Reads exact
    /// Profile membership, lifecycle and assignments in that same snapshot.
    fn authority(
        self: Box<Self>,
        profile: ProfileId,
        workspace: WorkspaceId,
    ) -> PortFuture<'static, Result<SyncIssuanceAuthority, SyncIssuanceError>>;
}

pub trait SyncIssuanceRepository: Send + Sync {
    /// Claims have already passed the isolated native access verifier. This
    /// begins the coherent read and validates current native/session/device
    /// facts only, without a Workspace or chosen Profile lookup.
    fn begin_sync_issuance<'a>(
        &'a self,
        claims: AccessTokenClaims,
        now_millis: i64,
    ) -> PortFuture<'a, Result<Box<dyn SyncIssuanceSnapshot>, SyncIssuanceError>>;
}

/// Placement of an authorized pair's two datasets onto a ready worker when
/// they have no assignment yet (the Plan 12 `place` operation, taken by the
/// issuer on a pair's first credential). The issuer calls it only after the
/// pair's current authorization was read and the assignment alone was found
/// absent; it is idempotent, and a dataset that already has a replica is left
/// exactly as it is. Placement grants no data-write lease: the worker still
/// acquires under its own readiness.
pub trait SyncPlacement: Send + Sync {
    fn place<'a>(
        &'a self,
        profile: ProfileId,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, Result<(), SyncIssuanceError>>;
}
