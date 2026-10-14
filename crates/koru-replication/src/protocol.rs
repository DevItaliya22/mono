// SPDX-License-Identifier: MPL-2.0

//! Transport-neutral replication rooms carried over authenticated streams.
//!
//! The inner messages are the published `loro-protocol` vocabulary unchanged.
//! Koru contributes only an authorized workspace envelope so a globally unique
//! document room can be checked against the exact grant before any bytes move.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use koru_device::{DeviceId, RelationshipId, ReplicationEndpointId};
use koru_store_core::model::{Blob, BlobHash};
use koru_store_core::{DocumentId, ProfileId, WorkspaceId};
use loro_protocol::protocol::{
    BatchId, CrdtType, JoinErrorCode, Permission, ProtocolMessage, RoomErrorCode, UpdateStatusCode,
};
use serde::{Deserialize, Serialize};

pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub use koru_store_core::profile_units::ProfileUnitRef;
/// The peer-to-peer protocol number this runtime speaks; declared in
/// `koru-sync-control` (its doc says when to bump it and why it is not the
/// ALPN) and re-exported here because this is the crate that enforces it.
pub use koru_sync_control::PEER_PROTOCOL_VERSION;

#[derive(Clone, Debug)]
pub struct AuthenticatedPeer {
    pub relationship_id: RelationshipId,
    pub device_id: DeviceId,
    pub endpoint_id: ReplicationEndpointId,
    pub all_data: bool,
    pub authorized_profiles: BTreeSet<ProfileId>,
    pub authorized_workspaces: BTreeSet<WorkspaceId>,
    pub authorization_fence: AuthorizationFence,
    /// The peer's display name from its trust records, for log lines. `None`
    /// only when the authorization snapshot does not carry one.
    pub display_name: Option<String>,
    /// The peer advertises a different [`PEER_PROTOCOL_VERSION`] from ours.
    /// The connection stays — trust gossip is how the older side learns it
    /// must update — but no sync pass starts toward it and every data-plane
    /// request is refused; `peer_protocol` distinguishes an older unit-less
    /// peer from a newer unknown protocol.
    pub incompatible: bool,
    /// Signed peer protocol observation. Missing/older observations cannot
    /// safely exchange independently addressed Profile documents.
    pub peer_protocol: Option<u32>,
}

/// Why this runtime refuses a peer's data plane, from what the peer
/// advertises: `peer_protocol` is its signed [`PEER_PROTOCOL_VERSION`] and
/// `incompatible` is the build-drift verdict. `None` means the two sync.
///
/// A free function so a caller that has not authenticated a connection —
/// the dialer deciding how long to back off — applies the same rule as the
/// one refusing frames on it.
pub fn protocol_refusal(peer_protocol: Option<u32>, incompatible: bool) -> Option<&'static str> {
    match peer_protocol {
        None | Some(0..=4) => Some("profile_units_required"),
        Some(version) if version != PEER_PROTOCOL_VERSION || incompatible => {
            Some("incompatible_protocol")
        }
        _ => None,
    }
}

impl AuthenticatedPeer {
    pub fn protocol_refusal(&self) -> Option<&'static str> {
        protocol_refusal(self.peer_protocol, self.incompatible)
    }

    /// The peer's display name for log lines — a person debugging a mesh
    /// knows their devices as "Pixel8" and "Win11", never by uuid.
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or("?")
    }

    pub fn update_origin(&self) -> koru_store_core::ports::UpdateOrigin {
        koru_store_core::ports::UpdateOrigin::RemoteDevice {
            relationship_id: *self.relationship_id.as_uuid().as_bytes(),
            device_id: *self.device_id.as_uuid().as_bytes(),
            endpoint_id: *self.endpoint_id.as_bytes(),
        }
    }

    pub fn with_authorized_workspace<T>(
        &self,
        workspace: WorkspaceId,
        operation: impl FnOnce() -> Result<T, StoreFailure>,
    ) -> Result<T, StoreFailure> {
        self.check_authorized_workspace(workspace)?;
        self.with_active_fence(operation)
    }

    pub fn check_authorized_workspace(&self, workspace: WorkspaceId) -> Result<(), StoreFailure> {
        if !self.all_data && !self.authorized_workspaces.contains(&workspace) {
            return Err(StoreFailure::new("unauthorized_workspace"));
        }
        self.check_active()
    }

    /// Run one Profile operation while this relationship still authorizes the
    /// exact Profile. All-data is the legacy superset; a Profile grant never
    /// implies authority over another Profile.
    pub fn with_authorized_profile<T>(
        &self,
        profile: ProfileId,
        operation: impl FnOnce() -> Result<T, StoreFailure>,
    ) -> Result<T, StoreFailure> {
        self.check_authorized_profile(profile)?;
        self.with_active_fence(operation)
    }

    pub fn check_authorized_profile(&self, profile: ProfileId) -> Result<(), StoreFailure> {
        if !self.all_data && !self.authorized_profiles.contains(&profile) {
            return Err(StoreFailure::new("unauthorized_profile"));
        }
        self.check_active()
    }

    pub fn check_authorized_profile_unit(&self, unit: ProfileUnitRef) -> Result<(), StoreFailure> {
        if let Some(code) = self.protocol_refusal() {
            return Err(StoreFailure::new(code));
        }
        match unit {
            ProfileUnitRef::LegacyPrivate { .. } if !self.all_data => {
                Err(StoreFailure::new("profile_metadata_not_authorized"))
            }
            ProfileUnitRef::Personal { owner, workspace } => {
                self.check_authorized_profile(owner)?;
                self.check_authorized_workspace_profile(workspace, owner)
            }
            _ => self.check_authorized_profile(unit.profile()),
        }
    }

    pub fn with_authorized_profile_unit<T>(
        &self,
        unit: ProfileUnitRef,
        operation: impl FnOnce() -> Result<T, StoreFailure>,
    ) -> Result<T, StoreFailure> {
        self.check_authorized_profile_unit(unit)?;
        self.with_active_fence(operation)
    }

    /// Run one Workspace operation when the peer has either the exact
    /// Workspace grant or authority over the Workspace's immutable owner
    /// Profile. Adapters resolve `profile` from durable Store state, except
    /// replica creation where the incoming descriptor supplies the owner and
    /// the Store collision checks prove it before committing.
    pub fn with_authorized_workspace_profile<T>(
        &self,
        workspace: WorkspaceId,
        profile: ProfileId,
        operation: impl FnOnce() -> Result<T, StoreFailure>,
    ) -> Result<T, StoreFailure> {
        self.check_authorized_workspace_profile(workspace, profile)?;
        self.with_active_fence(operation)
    }

    pub fn check_authorized_workspace_profile(
        &self,
        workspace: WorkspaceId,
        profile: ProfileId,
    ) -> Result<(), StoreFailure> {
        if !self.all_data
            && !self.authorized_workspaces.contains(&workspace)
            && !self.authorized_profiles.contains(&profile)
        {
            return Err(StoreFailure::new("unauthorized_workspace"));
        }
        self.check_active()
    }

    /// Whether a catalog view can be established at all. An empty authority
    /// must not receive a catalog or participate in the equal-digest shortcut.
    pub fn has_catalog_scope(&self) -> bool {
        self.all_data
            || !self.authorized_profiles.is_empty()
            || !self.authorized_workspaces.is_empty()
    }

    pub fn with_catalog_scope<T>(
        &self,
        operation: impl FnOnce() -> Result<T, StoreFailure>,
    ) -> Result<T, StoreFailure> {
        if !self.has_catalog_scope() {
            return Err(StoreFailure::new("unauthorized_catalog"));
        }
        self.with_active_fence(operation)
    }

    /// Run one Store operation inside the revocation barrier.
    ///
    /// **Never call this from inside another `with_*` on the same peer.** The
    /// barrier is an `RwLock`, and a second read acquisition on a thread that
    /// already holds one deadlocks the moment a revoking writer is queued:
    /// the writer waits for the outer guard, and the inner read waits behind
    /// the writer. That is why [`Self::check_active`] reads the flag and not
    /// the lock — every `check_authorized_*` runs inside operations like
    /// `profiles` and `apply_remote_profile_updates`, which are themselves
    /// already under this barrier.
    fn with_active_fence<T>(
        &self,
        operation: impl FnOnce() -> Result<T, StoreFailure>,
    ) -> Result<T, StoreFailure> {
        // Hold the read side through the Store operation. Revocation takes the
        // write side, so when replacement returns every earlier commit has
        // either completed durably or was rejected before mutation.
        let _admitted = self
            .authorization_fence
            .barrier
            .read()
            .unwrap_or_else(|error| error.into_inner());
        // Read *inside* the barrier. `revoke` clears the flag before it takes
        // the write side, so a revocation that has already returned cannot be
        // followed by an admission here.
        self.check_active()?;
        operation()
    }

    /// Whether the relationship still stands, **without touching the barrier**
    /// — see [`Self::with_active_fence`] for why that matters.
    fn check_active(&self) -> Result<(), StoreFailure> {
        if self
            .authorization_fence
            .active
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            Ok(())
        } else {
            Err(StoreFailure::new("relationship_revoked"))
        }
    }
}

/// The standing of one device grant, and the barrier a revocation waits on.
///
/// **The state and the barrier are deliberately two things.** They used to be
/// one `RwLock<bool>`, read for both — and every `check_authorized_*` inside an
/// operation that was already holding the fence took a *second* read of that
/// same lock. `std::sync::RwLock` is write-preferring, so a `revoke` arriving
/// in between queued as a writer, the inner read queued behind the writer, and
/// the writer waited on the guard the same thread was still holding: a
/// three-way self-deadlock, on the thread that owns the Store lock. The
/// symptom was a phone or tablet whose every Store call timed out until the
/// process was restarted.
///
/// So: the flag is an atomic anyone may read at any depth, and the `RwLock` is
/// only ever taken once per operation, by [`AuthenticatedPeer::with_active_fence`].
#[derive(Clone, Debug)]
pub struct AuthorizationFence {
    active: Arc<std::sync::atomic::AtomicBool>,
    barrier: Arc<RwLock<()>>,
}

impl AuthorizationFence {
    pub fn active() -> Self {
        Self {
            active: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            barrier: Arc::new(RwLock::new(())),
        }
    }

    /// Refuse first, then wait for what was already admitted.
    ///
    /// The order is the contract: clearing the flag before taking the write
    /// side means no operation can be admitted after this returns, and taking
    /// the write side means every operation admitted before it has finished.
    pub fn revoke(&self) {
        self.active
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let _drained = self
            .barrier
            .write()
            .unwrap_or_else(|error| error.into_inner());
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeerFrame {
    /// A coalesced hint that the authenticated peer has durable local changes.
    /// The receiver answers immediately, then compares authoritative Store
    /// inventories; the hint itself carries no data and is not a source of
    /// truth.
    SyncRequest,
    SyncAck,
    CatalogRequest,
    Catalog {
        #[serde(default)]
        digest: Vec<u8>,
        #[serde(default)]
        profiles: Vec<ProfileReplicaState>,
        workspaces: Vec<WorkspaceCatalogState>,
    },
    ProfileRequest {
        unit: ProfileUnitRef,
        version: Vec<u8>,
    },
    ProfileUpdates {
        unit: ProfileUnitRef,
        updates: Vec<Vec<u8>>,
    },
    InventoryRequest {
        workspace_id: WorkspaceId,
    },
    Inventory(WorkspaceInventory),
    /// Cloud-only, bounded comparison of known documents inside an already
    /// admitted Workspace. Never account/catalog discovery or write authority.
    CloudCompare {
        workspace_id: WorkspaceId,
        documents: Vec<CloudDocumentDigest>,
    },
    CloudCompared {
        workspace_id: WorkspaceId,
        matching: Vec<DocumentId>,
    },
    BlobRequest {
        workspace_id: WorkspaceId,
        hash: BlobHash,
        offset: u64,
        max_bytes: u32,
    },
    BlobChunk(BlobChunk),
    Loro {
        workspace_id: WorkspaceId,
        message: Vec<u8>,
    },
    LoroBatch {
        workspace_id: WorkspaceId,
        messages: Vec<Vec<u8>>,
    },
    TrustRecord {
        record: Box<koru_device::TrustRecord>,
    },
    /// The sender's whole trust-record set in canonical
    /// [`koru_device::sort_for_gossip`] order — how the device list itself
    /// replicates. Unlike [`PeerFrame::TrustRecord`], which carries a
    /// revocation and tears the connection down, a batch never touches the
    /// sender's fence: every record is third-party-verifiable, and an applied
    /// change reaches the data plane through the next authorization snapshot.
    TrustRecords {
        records: Vec<koru_device::TrustRecord>,
    },
    TrustAck,
    Error {
        code: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInventory {
    pub workspace_id: WorkspaceId,
    pub replica: ReplicaDescriptor,
    pub documents: Vec<DocumentReplicaState>,
    pub blobs: Vec<Blob>,
}

pub const CLOUD_COMPARE_LIMIT: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudDocumentDigest {
    pub document_id: DocumentId,
    pub digest: koru_store_core::sync_digest::Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileReplicaState {
    pub unit: ProfileUnitRef,
    pub version: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<ProfileIdentityState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileIdentityState {
    pub name: String,
    pub purpose: Option<String>,
    pub avatar: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub account: Option<String>,
    pub locale: String,
    pub created_at_ms: i64,
    /// Required immutable classification; a holder must never become a persona.
    pub kind: String,
}

impl ProfileReplicaState {
    pub fn validate(&self) -> Result<(), StoreFailure> {
        match (&self.unit, &self.identity) {
            (ProfileUnitRef::Identity { .. }, Some(identity))
                if matches!(identity.kind.as_str(), "normal" | "holder") => {}
            (ProfileUnitRef::Identity { .. }, _) | (_, Some(_)) => {
                return Err(StoreFailure::new("invalid_profile_identity"));
            }
            _ => {}
        }
        loro::VersionVector::decode(&self.version)
            .map_err(|_| StoreFailure::new("invalid_profile_version"))?;
        Ok(())
    }
}

/// Wire tags determine order, independently of Rust's enum declaration order.
pub fn profile_unit_sort_key(
    unit: ProfileUnitRef,
) -> (&'static str, ProfileId, Option<WorkspaceId>) {
    let tag = match unit {
        ProfileUnitRef::Identity { .. } => "identity",
        ProfileUnitRef::LegacyPrivate { .. } => "legacy_private",
        ProfileUnitRef::Personal { .. } => "personal",
        ProfileUnitRef::Private { .. } => "private",
    };
    (tag, unit.profile(), unit.workspace())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCatalogState {
    pub workspace_id: WorkspaceId,
    pub profile_id: ProfileId,
    #[serde(default)]
    pub digest: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobChunk {
    pub workspace_id: WorkspaceId,
    pub blob: Blob,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaDescriptor {
    pub profile_id: ProfileId,
    pub profile_name: String,
    pub profile_locale: String,
    pub profile_created_at_ms: i64,
    /// Bootstrap appearance for a legacy all-data peer. Scoped peers must
    /// receive these fields as `None`: appearance is Profile metadata, not
    /// authority or Workspace content.
    #[serde(default)]
    pub profile_icon: Option<String>,
    #[serde(default)]
    pub profile_color: Option<String>,
    /// Persona or legacy holder classification for all-data bootstrap.
    /// Workspace-scoped peers receive an empty token; a trusted all-data peer
    /// may carry a legacy holder, which the receiving shell still excludes
    /// from persona surfaces.
    #[serde(default)]
    pub profile_kind: String,
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub workspace_created_at_ms: i64,
    #[serde(default)]
    pub workspace_icon: Option<String>,
    #[serde(default)]
    pub workspace_color: Option<String>,
    #[serde(default)]
    pub workspace_pinned: bool,
    /// Shared workspace lifecycle state. Ordering and last-opened state remain
    /// device-local; appearance and lifecycle are projected from the manifest.
    #[serde(default)]
    pub workspace_archived: bool,
    #[serde(default)]
    pub workspace_deleted_at_ms: Option<i64>,
    pub manifest_document: DocumentId,
}

impl ReplicaDescriptor {
    pub fn redact_profile_metadata(&mut self) {
        self.profile_name.clear();
        self.profile_locale.clear();
        self.profile_created_at_ms = 0;
        self.profile_icon = None;
        self.profile_color = None;
        self.profile_kind.clear();
    }

    pub fn has_profile_metadata(&self) -> bool {
        !self.profile_name.is_empty()
            || !self.profile_locale.is_empty()
            || self.profile_created_at_ms != 0
            || self.profile_icon.is_some()
            || self.profile_color.is_some()
            || !self.profile_kind.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentReplicaState {
    pub document_id: DocumentId,
    pub room_id: String,
    pub version: Vec<u8>,
    pub is_manifest: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableAck {
    pub durable_sequence: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreFailure {
    pub code: &'static str,
    /// Privacy-safe developer detail from Store core. This is logged only on
    /// the local side and is not serialized into peer protocol errors.
    pub detail: Option<String>,
}

impl StoreFailure {
    pub const fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if !detail.is_empty() {
            self.detail = Some(detail);
        }
        self
    }
}

/// Synchronous by design: the desktop adapter locks Store only for these calls
/// and never holds that lock across network awaits. Mobile implements the same
/// port around its embedded Store.
pub trait ReplicationStore: Send + Sync + 'static {
    fn record_state_change(
        &self,
        event: &crate::StateChangeEvent,
        _retain_after_millis: i64,
    ) -> Result<crate::StateChangeEvent, StoreFailure> {
        Ok(event.clone())
    }
    fn state_changes(
        &self,
        _peer: DeviceId,
        _retain_after_millis: i64,
        _before_id: Option<i64>,
        _limit: usize,
    ) -> Result<Vec<crate::StateChangeEvent>, StoreFailure> {
        Ok(Vec::new())
    }
    fn latest_state_change_times(
        &self,
        _retain_after_millis: i64,
    ) -> Result<Vec<(DeviceId, i64)>, StoreFailure> {
        Ok(Vec::new())
    }
    fn prune_state_changes(&self, _retain_after_millis: i64) -> Result<usize, StoreFailure> {
        Ok(0)
    }
    /// Return only Profile rows authorized for `peer`. The implementation
    /// holds the relationship fence while reading and filtering.
    fn profiles(
        &self,
        _peer: &AuthenticatedPeer,
    ) -> Result<Vec<ProfileReplicaState>, StoreFailure> {
        Ok(Vec::new())
    }
    fn provision_profile(
        &self,
        _peer: &AuthenticatedPeer,
        _profile: &ProfileReplicaState,
    ) -> Result<(), StoreFailure> {
        Ok(())
    }
    fn profile_version(
        &self,
        _peer: &AuthenticatedPeer,
        _unit: ProfileUnitRef,
    ) -> Result<Vec<u8>, StoreFailure> {
        Ok(loro::VersionVector::new().encode())
    }
    fn profile_updates_since(
        &self,
        _peer: &AuthenticatedPeer,
        _unit: ProfileUnitRef,
        _version: &loro::VersionVector,
    ) -> Result<Vec<u8>, StoreFailure> {
        Err(StoreFailure::new("profile_not_found"))
    }
    fn apply_remote_profile_updates(
        &self,
        _peer: &AuthenticatedPeer,
        _unit: ProfileUnitRef,
        _updates: Vec<Vec<u8>>,
    ) -> Result<DurableAck, StoreFailure> {
        Err(StoreFailure::new("profile_not_found"))
    }
    /// Return only Workspace rows authorized through all-data, their owner
    /// Profile, or an exact Workspace grant.
    fn catalog(
        &self,
        _peer: &AuthenticatedPeer,
    ) -> Result<Vec<WorkspaceCatalogState>, StoreFailure> {
        Ok(Vec::new())
    }
    /// Resolve the immutable owner used to select the Profile or exact
    /// Workspace guard before any Workspace metadata or bytes are exposed.
    fn workspace_profile(&self, workspace: WorkspaceId) -> Result<ProfileId, StoreFailure>;
    fn provision_replica(
        &self,
        peer: &AuthenticatedPeer,
        replica: &ReplicaDescriptor,
    ) -> Result<(), StoreFailure>;
    fn provision_document(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> Result<(), StoreFailure>;
    fn inventory(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
    ) -> Result<WorkspaceInventory, StoreFailure>;
    fn locate(
        &self,
        peer: &AuthenticatedPeer,
        document: DocumentId,
    ) -> Result<Option<WorkspaceId>, StoreFailure>;
    /// Current local display name for a document. This is observational
    /// metadata for device history only; identity and routing remain the ids.
    fn document_name(
        &self,
        _peer: &AuthenticatedPeer,
        _workspace: WorkspaceId,
        _document: DocumentId,
    ) -> Option<String> {
        None
    }
    fn manifest_id(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
    ) -> Result<DocumentId, StoreFailure>;
    fn version(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> Result<Vec<u8>, StoreFailure>;
    fn updates_since(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
        document: DocumentId,
        version: &loro::VersionVector,
    ) -> Result<Vec<u8>, StoreFailure>;
    /// A full snapshot for a peer that has no history at all. A snapshot is
    /// the compact encoding of the same information — one structured blob
    /// instead of the concatenated update journal — and Loro's `import`
    /// accepts either, so the receiver needs no second code path. The default
    /// answers with the complete update history: correct, just not compact.
    fn snapshot(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> Result<Vec<u8>, StoreFailure> {
        self.updates_since(peer, workspace, document, &loro::VersionVector::new())
    }
    /// **§6's transport-mode gate, asked of the Store**: may blob bytes cross a
    /// *peer* connection for this Workspace right now?
    ///
    /// The adapter answers with `Store::admit_transport`, which applies the
    /// stored mode, the Profile ceiling and the in-memory fence in that order.
    /// Neither transport re-derives the rule — the same question asked the same
    /// way is what stops one path being the one that forgets.
    ///
    /// It exists because one of §6's five points is on the **requesting** side,
    /// before a `BlobRequest` leaves this device, and the requester is a
    /// transport rather than the Store. The other three peer points —
    /// advertising in an inventory, serving a chunk, and admitting incoming
    /// bytes — are inside the adapter's own methods, beside the Store call each
    /// one guards.
    ///
    /// **The default permits, deliberately**, on exactly D254's reasoning:
    /// migration 12 seeds every row `local_only`, so an adapter whose host
    /// gives its owner no way to promote a row would refuse every peer
    /// transfer on every existing installation the moment this landed. An
    /// adapter overrides it once its host has that way, and not before.
    /// `koru-mobile-host` is the adapter that still takes this default:
    /// Android has no Cloud sync toggle (D238) and so no Profile prompt
    /// (D258), and a phone's peer blob path is ungated in consequence.
    fn admit_peer_blob_transfer(&self, _workspace: WorkspaceId) -> Result<(), StoreFailure> {
        Ok(())
    }
    fn read_blob_chunk(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
        hash: &BlobHash,
        offset: u64,
        max_bytes: u32,
    ) -> Result<BlobChunk, StoreFailure>;
    /// The device that first stored this blob (storage-form id), when the
    /// local record says. Provenance for parking labels and holder hints —
    /// never routing authority: any holder may serve the bytes, and the digest
    /// decides whether they are accepted.
    fn blob_origin(
        &self,
        _peer: &AuthenticatedPeer,
        _workspace: WorkspaceId,
        _hash: &BlobHash,
    ) -> Option<String> {
        None
    }
    /// Record the origins an inventory advertised, so the fact survives a
    /// restart: a device that never held the bytes can still name whoever did.
    /// First record wins; blobs without an origin attribute are skipped.
    /// Returns how many new rows landed so the caller can log news, not
    /// re-advertisements. A store that cannot record this is not broken — the
    /// placeholder simply stays unnamed, as it always was.
    fn record_blob_origins(
        &self,
        _peer: &AuthenticatedPeer,
        _workspace: WorkspaceId,
        _blobs: &[Blob],
    ) -> Result<usize, StoreFailure> {
        Ok(0)
    }
    fn store_remote_blob(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
        blob: &Blob,
        bytes: Vec<u8>,
    ) -> Result<(), StoreFailure>;
    /// Must return success only after `Store::flush` has succeeded.
    fn apply_remote_updates(
        &self,
        peer: &AuthenticatedPeer,
        workspace: WorkspaceId,
        document: DocumentId,
        updates: Vec<Vec<u8>>,
    ) -> Result<DurableAck, StoreFailure>;
}

/// The workspace whose grant a frame is checked against — and, for a host
/// serving more than one replica, the replica that owns it.
///
/// `None` means account-scoped rather than unknown: a catalog, profile or
/// trust frame belongs to the whole authenticated relationship, so no single
/// workspace can answer it. [`handle_frame`] routes on exactly this, which is
/// what keeps a multi-replica host's routing and this crate's authorization
/// check from becoming two matches that can disagree.
pub fn frame_workspace(frame: &PeerFrame) -> Option<WorkspaceId> {
    match frame {
        PeerFrame::InventoryRequest { workspace_id }
        | PeerFrame::CloudCompare { workspace_id, .. }
        | PeerFrame::CloudCompared { workspace_id, .. }
        | PeerFrame::BlobRequest { workspace_id, .. }
        | PeerFrame::Loro { workspace_id, .. }
        | PeerFrame::LoroBatch { workspace_id, .. } => Some(*workspace_id),
        PeerFrame::Inventory(inventory) => Some(inventory.workspace_id),
        PeerFrame::BlobChunk(chunk) => Some(chunk.workspace_id),
        PeerFrame::SyncRequest
        | PeerFrame::SyncAck
        | PeerFrame::CatalogRequest
        | PeerFrame::Catalog { .. }
        | PeerFrame::ProfileRequest { .. }
        | PeerFrame::ProfileUpdates { .. }
        | PeerFrame::TrustRecord { .. }
        | PeerFrame::TrustRecords { .. }
        | PeerFrame::TrustAck
        | PeerFrame::Error { .. } => None,
    }
}

pub fn handle_frame(
    store: &dyn ReplicationStore,
    peer: &AuthenticatedPeer,
    frame: PeerFrame,
) -> Vec<PeerFrame> {
    if let Some(code) = peer.protocol_refusal() {
        return vec![PeerFrame::Error { code: code.into() }];
    }
    let workspace = frame_workspace(&frame);
    if matches!(frame, PeerFrame::CatalogRequest) {
        if !peer.has_catalog_scope() {
            return vec![PeerFrame::Error {
                code: "unauthorized_catalog".into(),
            }];
        }
        if let Err(error) = peer.check_active() {
            return vec![PeerFrame::Error {
                code: error.code.into(),
            }];
        }
        return handle_authorized_frame(store, peer, frame);
    }
    if let PeerFrame::ProfileRequest { unit, .. } | PeerFrame::ProfileUpdates { unit, .. } = &frame
    {
        if let Err(error) = peer.check_authorized_profile_unit(*unit) {
            return vec![PeerFrame::Error {
                code: error.code.into(),
            }];
        }
        return handle_authorized_frame(store, peer, frame);
    }
    if let Some(workspace) = workspace {
        let profile = match store.workspace_profile(workspace) {
            Ok(profile) => profile,
            Err(error) => {
                let code = if peer.all_data || peer.authorized_workspaces.contains(&workspace) {
                    error.code
                } else {
                    "unauthorized_workspace"
                };
                return vec![PeerFrame::Error { code: code.into() }];
            }
        };
        if let Err(error) = peer.check_authorized_workspace_profile(workspace, profile) {
            return vec![PeerFrame::Error {
                code: error.code.into(),
            }];
        }
        return handle_authorized_frame(store, peer, frame);
    }
    Vec::new()
}

/// Count durable document updates carried by a response without treating the
/// response itself as authoritative. Used only to make the inventory/version
/// decision observable at the data-plane boundary.
#[doc(hidden)]
pub fn doc_update_count(frame: &PeerFrame) -> usize {
    let messages: &[Vec<u8>] = match frame {
        PeerFrame::Loro { message, .. } => std::slice::from_ref(message),
        PeerFrame::LoroBatch { messages, .. } => messages,
        _ => return 0,
    };
    messages
        .iter()
        .filter(|encoded| {
            matches!(
                loro_protocol::encoding::try_decode(encoded),
                // A fragment header is one update that arrives in pieces —
                // counting only whole `DocUpdate`s made a fragmented answer
                // read as an empty one.
                Some(
                    ProtocolMessage::DocUpdate { .. }
                        | ProtocolMessage::DocUpdateFragmentHeader { .. }
                )
            )
        })
        .count()
}

/// Documents a request-shaped Loro frame can durably update, with the number
/// of update groups aimed at each one. The caller snapshots their versions
/// around `handle_frame`, so an accepted no-op or an incomplete fragment group
/// never becomes a state-change event.
#[doc(hidden)]
pub fn doc_update_targets(
    store: &dyn ReplicationStore,
    peer: &AuthenticatedPeer,
    frame: &PeerFrame,
) -> Vec<(WorkspaceId, DocumentId, usize)> {
    let (workspace, messages): (WorkspaceId, &[Vec<u8>]) = match frame {
        PeerFrame::Loro {
            workspace_id,
            message,
        } => (*workspace_id, std::slice::from_ref(message)),
        PeerFrame::LoroBatch {
            workspace_id,
            messages,
        } => (*workspace_id, messages),
        _ => return Vec::new(),
    };
    let mut targets = BTreeMap::<DocumentId, usize>::new();
    for encoded in messages {
        let Some(message) = loro_protocol::encoding::try_decode(encoded) else {
            continue;
        };
        let room_id = match message {
            ProtocolMessage::DocUpdate { room_id, .. }
            | ProtocolMessage::DocUpdateFragmentHeader { room_id, .. } => room_id,
            _ => continue,
        };
        if let Some(document) = resolve(store, peer, workspace, &room_id) {
            *targets.entry(document).or_default() += 1;
        }
    }
    targets
        .into_iter()
        .map(|(document, updates)| (workspace, document, updates))
        .collect()
}

/// A peer's refusal of a document join, if the response carries one:
/// `(room_id, code)`. Without this the refusal is decoded by `handle_frame`
/// and dropped, which is exactly how a document that can never be pulled used
/// to pass as a completed workspace.
#[doc(hidden)]
pub fn join_refusal(frame: &PeerFrame) -> Option<(String, String)> {
    let messages: &[Vec<u8>] = match frame {
        PeerFrame::Loro { message, .. } => std::slice::from_ref(message),
        PeerFrame::LoroBatch { messages, .. } => messages,
        _ => return None,
    };
    messages.iter().find_map(
        |encoded| match loro_protocol::encoding::try_decode(encoded)? {
            ProtocolMessage::JoinError {
                room_id,
                message,
                app_code,
                ..
            } => Some((room_id, app_code.unwrap_or(message))),
            _ => None,
        },
    )
}

fn handle_authorized_frame(
    store: &dyn ReplicationStore,
    peer: &AuthenticatedPeer,
    frame: PeerFrame,
) -> Vec<PeerFrame> {
    match frame {
        PeerFrame::CatalogRequest => match store.catalog(peer) {
            Ok(mut workspaces) => {
                workspaces.sort_by_key(|workspace| workspace.workspace_id);
                workspaces.dedup_by_key(|workspace| workspace.workspace_id);
                match store.profiles(peer) {
                    Ok(mut profiles) => {
                        profiles.sort_by_key(|profile| profile_unit_sort_key(profile.unit));
                        let Some(digest) = catalog_digest(peer, &profiles, &workspaces) else {
                            return vec![PeerFrame::Error {
                                code: "unauthorized_catalog".into(),
                            }];
                        };
                        vec![PeerFrame::Catalog {
                            digest,
                            profiles,
                            workspaces,
                        }]
                    }
                    Err(error) => vec![PeerFrame::Error {
                        code: error.code.into(),
                    }],
                }
            }
            Err(error) => vec![PeerFrame::Error {
                code: error.code.into(),
            }],
        },
        PeerFrame::InventoryRequest { workspace_id } => match store.inventory(peer, workspace_id) {
            Ok(inventory) => vec![PeerFrame::Inventory(inventory)],
            Err(error) => {
                tracing::warn!(
                    peer = %peer.device_id.to_storage(),
                    peer_name = %peer.label(),
                    workspace = %workspace_id.to_storage(),
                    error_code = error.code,
                    error_detail = error.detail.as_deref().unwrap_or("-"),
                    "sync inventory request failed"
                );
                vec![PeerFrame::Error {
                    code: error.code.into(),
                }]
            }
        },
        PeerFrame::ProfileRequest { unit, version } => {
            let Ok(version) = loro::VersionVector::decode(&version) else {
                return vec![PeerFrame::Error {
                    code: "invalid_profile_version".into(),
                }];
            };
            match store.profile_updates_since(peer, unit, &version) {
                Ok(update) => vec![PeerFrame::ProfileUpdates {
                    unit,
                    updates: vec![update],
                }],
                Err(error) => vec![PeerFrame::Error {
                    code: error.code.into(),
                }],
            }
        }
        PeerFrame::BlobRequest {
            workspace_id,
            hash,
            offset,
            max_bytes,
        } => match store.read_blob_chunk(peer, workspace_id, &hash, offset, max_bytes) {
            Ok(chunk) => vec![PeerFrame::BlobChunk(chunk)],
            Err(error) => vec![PeerFrame::Error {
                code: error.code.into(),
            }],
        },
        PeerFrame::Loro {
            workspace_id,
            message,
        } => handle_loro(store, peer, workspace_id, &message),
        PeerFrame::LoroBatch {
            workspace_id,
            messages,
        } => {
            let mut decoded = Vec::with_capacity(messages.len());
            let mut undecodable = 0usize;
            for encoded in &messages {
                match loro_protocol::encoding::try_decode(encoded) {
                    Some(message) => decoded.push(message),
                    None => undecodable += 1,
                }
            }
            if undecodable > 0 {
                tracing::warn!(
                    peer = %peer.device_id.to_storage(),
                    peer_name = %peer.label(),
                    workspace = %workspace_id.to_storage(),
                    undecodable,
                    total = messages.len(),
                    "sync batch carried undecodable messages; skipped"
                );
            }
            let replies = fragments::reassemble(peer, workspace_id, decoded)
                .into_iter()
                .flat_map(|message| handle_loro_message(store, peer, workspace_id, message))
                .collect();
            encode_loro_replies(peer, workspace_id, replies)
        }
        PeerFrame::SyncRequest
        | PeerFrame::SyncAck
        | PeerFrame::Catalog { .. }
        | PeerFrame::CloudCompare { .. }
        | PeerFrame::CloudCompared { .. }
        | PeerFrame::ProfileUpdates { .. }
        | PeerFrame::Inventory(_)
        | PeerFrame::BlobChunk(_)
        | PeerFrame::TrustRecord { .. }
        | PeerFrame::TrustRecords { .. }
        | PeerFrame::TrustAck
        | PeerFrame::Error { .. } => Vec::new(),
    }
}

/// Root of the disposable anti-entropy tree. The already-filtered Profile rows
/// and their independent versions, per-workspace roots, and exact
/// normalized scope are authoritative inputs; this value is only a shortcut
/// for proving that recursive inventory exchange has nothing to do.
pub fn catalog_digest(
    peer: &AuthenticatedPeer,
    profiles: &[ProfileReplicaState],
    workspaces: &[WorkspaceCatalogState],
) -> Option<Vec<u8>> {
    if !peer.has_catalog_scope()
        || peer.protocol_refusal().is_some()
        || peer.check_active().is_err()
        || profiles.iter().any(|profile| {
            profile.validate().is_err() || peer.check_authorized_profile_unit(profile.unit).is_err()
        })
    {
        return None;
    }
    let mut profiles = profiles.iter().collect::<Vec<_>>();
    profiles.sort_by_key(|profile| profile_unit_sort_key(profile.unit));
    if profiles.windows(2).any(|pair| pair[0].unit == pair[1].unit) {
        return None;
    }
    let mut workspaces = workspaces.iter().collect::<Vec<_>>();
    workspaces.sort_by_key(|workspace| workspace.workspace_id);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"koru-sync-catalog-v3\0");
    hasher.update(&[u8::from(peer.all_data)]);
    if !peer.all_data {
        hasher.update(&(peer.authorized_profiles.len() as u64).to_le_bytes());
        for profile in &peer.authorized_profiles {
            hasher.update(profile.as_uuid().as_bytes());
        }
        hasher.update(&(peer.authorized_workspaces.len() as u64).to_le_bytes());
        for workspace in &peer.authorized_workspaces {
            hasher.update(workspace.as_uuid().as_bytes());
        }
    }
    for profile in profiles {
        let (tag, owner, workspace) = profile_unit_sort_key(profile.unit);
        hasher.update(&(tag.len() as u64).to_le_bytes());
        hasher.update(tag.as_bytes());
        hasher.update(owner.as_uuid().as_bytes());
        if let Some(workspace) = workspace {
            hasher.update(workspace.as_uuid().as_bytes());
        }
        hasher.update(&(profile.version.len() as u64).to_le_bytes());
        hasher.update(&profile.version);
    }
    for workspace in workspaces {
        hasher.update(workspace.workspace_id.as_uuid().as_bytes());
        hasher.update(workspace.profile_id.as_uuid().as_bytes());
        hasher.update(&(workspace.digest.len() as u64).to_le_bytes());
        hasher.update(&workspace.digest);
    }
    Some(hasher.finalize().as_bytes().to_vec())
}

/// Encode replies for the wire, loudly. A reply that fails to encode used to
/// be dropped by a `filter_map`, which is how an oversized manifest catch-up
/// read as "peer already up to date" for a whole evening.
fn encode_loro_replies(
    peer: &AuthenticatedPeer,
    workspace: WorkspaceId,
    replies: Vec<ProtocolMessage>,
) -> Vec<PeerFrame> {
    let mut messages = Vec::with_capacity(replies.len());
    for reply in replies {
        match loro_protocol::encoding::encode(&reply) {
            Ok(encoded) => messages.push(encoded),
            Err(error) => tracing::warn!(
                peer = %peer.device_id.to_storage(),
                peer_name = %peer.label(),
                workspace = %workspace.to_storage(),
                error,
                "sync reply dropped: message failed to encode"
            ),
        }
    }
    vec![PeerFrame::LoroBatch {
        workspace_id: workspace,
        messages,
    }]
}

fn handle_loro(
    store: &dyn ReplicationStore,
    peer: &AuthenticatedPeer,
    workspace: WorkspaceId,
    encoded: &[u8],
) -> Vec<PeerFrame> {
    let Some(message) = loro_protocol::encoding::try_decode(encoded) else {
        tracing::warn!(
            peer = %peer.device_id.to_storage(),
            peer_name = %peer.label(),
            workspace = %workspace.to_storage(),
            bytes = encoded.len(),
            "sync loro message refused: undecodable"
        );
        return vec![PeerFrame::Error {
            code: "invalid_loro_frame".into(),
        }];
    };
    let replies = handle_loro_message(store, peer, workspace, message);
    encode_loro_replies(peer, workspace, replies)
}

fn handle_loro_message(
    store: &dyn ReplicationStore,
    peer: &AuthenticatedPeer,
    workspace: WorkspaceId,
    message: ProtocolMessage,
) -> Vec<ProtocolMessage> {
    match message {
        ProtocolMessage::JoinRequest {
            crdt,
            room_id,
            version,
            ..
        } => join(store, peer, workspace, crdt, room_id, version),
        ProtocolMessage::DocUpdate {
            crdt,
            room_id,
            updates,
            batch_id,
        } => update(store, peer, workspace, crdt, room_id, updates, batch_id),
        ProtocolMessage::Leave { .. }
        | ProtocolMessage::JoinResponseOk { .. }
        | ProtocolMessage::JoinError { .. }
        | ProtocolMessage::Ack { .. }
        | ProtocolMessage::RoomError { .. } => Vec::new(),
        // A bare `Loro` frame carries one message and cannot hold a whole
        // fragment group; fragments are reassembled at the batch boundary.
        ProtocolMessage::DocUpdateFragmentHeader { crdt, room_id, .. }
        | ProtocolMessage::DocUpdateFragment { crdt, room_id, .. } => vec![room_error(
            crdt,
            room_id,
            "fragmented updates arrive only inside a batch",
        )],
    }
}

fn join(
    store: &dyn ReplicationStore,
    peer: &AuthenticatedPeer,
    workspace: WorkspaceId,
    crdt: CrdtType,
    room_id: String,
    version: Vec<u8>,
) -> Vec<ProtocolMessage> {
    let Some(document) = resolve(store, peer, workspace, &room_id) else {
        // The one refusal a peer cannot retry its way out of: our location
        // projection does not know this document. Log it loudly — a store
        // that cannot serve what its inventory advertises is a repair
        // signal, not routine traffic.
        tracing::warn!(
            peer = %peer.device_id.to_storage(),
            peer_name = %peer.label(),
            workspace = %workspace.to_storage(),
            room_id,
            "sync room join refused: document not located"
        );
        return vec![join_error(crdt, room_id, "no such authorized room")];
    };
    let Ok(ours) = store.version(peer, workspace, document) else {
        tracing::warn!(
            peer = %peer.device_id.to_storage(),
            peer_name = %peer.label(),
            workspace = %workspace.to_storage(),
            room_id,
            "sync room join refused: document listed but unreadable"
        );
        return vec![join_error(crdt, room_id, "could not read room")];
    };
    let ours_vv = loro::VersionVector::decode(&ours).unwrap_or_default();
    let theirs = loro::VersionVector::decode(&version).unwrap_or_default();
    let behind = !theirs.includes_vv(&ours_vv);
    let mut replies = vec![ProtocolMessage::JoinResponseOk {
        crdt,
        room_id: room_id.clone(),
        permission: Permission::Write,
        version: ours,
        extra: None,
    }];
    if behind {
        // A peer with no history at all gets a snapshot: the compact encoding
        // of the same information. An op-history export replays every edit
        // ever made — 301b's manifest was 1.7MB of journal for a document
        // whose state fits in a fraction of that — while `import` on the
        // receiving side accepts either encoding without a second code path.
        let (bytes, shape) = if theirs.is_empty() {
            (store.snapshot(peer, workspace, document), "snapshot")
        } else {
            (
                store.updates_since(peer, workspace, document, &theirs),
                "updates",
            )
        };
        match bytes {
            Ok(bytes) => {
                let update_bytes = bytes.len();
                let messages = fragments::doc_update_messages(crdt, &room_id, bytes);
                // What this side actually sends, per answered join — the
                // sending half of the exchange was invisible, and a dropped
                // or missing answer could only be inferred from the peer.
                tracing::info!(
                    peer = %peer.device_id.to_storage(),
                    peer_name = %peer.label(),
                    workspace = %workspace.to_storage(),
                    document = %document.to_storage(),
                    bytes = update_bytes,
                    fragments = messages.len().saturating_sub(1),
                    shape,
                    theirs = %crate::vv_tag(&version),
                    "workspace_sync direction=local_to_peer action=join_answered"
                );
                replies.extend(messages);
            }
            // A join we answered with no updates: the peer stays behind
            // forever and retries, so this must never be silent.
            Err(error) => tracing::warn!(
                peer = %peer.device_id.to_storage(),
                peer_name = %peer.label(),
                workspace = %workspace.to_storage(),
                document = %document.to_storage(),
                shape,
                error_code = error.code,
                "sync join answered with no updates: export failed"
            ),
        }
    } else {
        tracing::info!(
            peer = %peer.device_id.to_storage(),
            peer_name = %peer.label(),
            workspace = %workspace.to_storage(),
            document = %document.to_storage(),
            theirs = %crate::vv_tag(&version),
            "workspace_sync direction=local_to_peer action=join_answered_up_to_date"
        );
    }
    replies
}

fn update(
    store: &dyn ReplicationStore,
    peer: &AuthenticatedPeer,
    workspace: WorkspaceId,
    crdt: CrdtType,
    room_id: String,
    updates: Vec<Vec<u8>>,
    batch_id: BatchId,
) -> Vec<ProtocolMessage> {
    let Some(document) = resolve(store, peer, workspace, &room_id) else {
        tracing::warn!(
            peer = %peer.device_id.to_storage(),
            peer_name = %peer.label(),
            workspace = %workspace.to_storage(),
            room_id,
            "sync room update refused: document not located"
        );
        return vec![room_error(crdt, room_id, "no such authorized room")];
    };
    let update_count = updates.len();
    let update_bytes: usize = updates.iter().map(Vec::len).sum();
    let manifest = room_id.ends_with(":manifest");
    let status = match store.apply_remote_updates(peer, workspace, document, updates) {
        Ok(_) => {
            // The applying half of the exchange: bytes arrived, decoded, and
            // committed durably. A manifest apply is the moment a workspace's
            // document list changes, so it logs at info; ordinary documents
            // at debug, or a busy pass floods the log.
            if manifest {
                tracing::info!(
                    peer = %peer.device_id.to_storage(),
                    peer_name = %peer.label(),
                    workspace = %workspace.to_storage(),
                    document = %document.to_storage(),
                    updates = update_count,
                    bytes = update_bytes,
                    "workspace_sync direction=peer_to_local object=manifest action=applied"
                );
            } else {
                tracing::debug!(
                    peer = %peer.device_id.to_storage(),
                    peer_name = %peer.label(),
                    workspace = %workspace.to_storage(),
                    document = %document.to_storage(),
                    updates = update_count,
                    bytes = update_bytes,
                    "sync remote update applied"
                );
            }
            UpdateStatusCode::Ok
        }
        // An apply that fails is answered with a status the puller never
        // checks — so the refusal has to live in the log, or it lives nowhere.
        Err(error) => {
            tracing::warn!(
                peer = %peer.device_id.to_storage(),
                peer_name = %peer.label(),
                workspace = %workspace.to_storage(),
                document = %document.to_storage(),
                updates = update_count,
                bytes = update_bytes,
                error_code = error.code,
                error_detail = error.detail.as_deref().unwrap_or("-"),
                "sync remote update apply failed"
            );
            if error.code == "crdt_rejected" {
                UpdateStatusCode::InvalidUpdate
            } else if error.code == "payload_too_large" {
                UpdateStatusCode::PayloadTooLarge
            } else {
                UpdateStatusCode::Unknown
            }
        }
    };
    vec![ProtocolMessage::Ack {
        crdt,
        room_id,
        ref_id: batch_id,
        status,
    }]
}

fn resolve(
    store: &dyn ReplicationStore,
    peer: &AuthenticatedPeer,
    workspace: WorkspaceId,
    room_id: &str,
) -> Option<DocumentId> {
    if let Some(value) = room_id
        .strip_prefix("workspace:")
        .and_then(|value| value.strip_suffix(":manifest"))
    {
        return (WorkspaceId::parse(value) == Some(workspace))
            .then(|| store.manifest_id(peer, workspace).ok())
            .flatten();
    }
    let document = DocumentId::parse(room_id.strip_prefix("document:")?)?;
    (store.locate(peer, document).ok().flatten() == Some(workspace)).then_some(document)
}

fn join_error(crdt: CrdtType, room_id: String, message: &str) -> ProtocolMessage {
    ProtocolMessage::JoinError {
        crdt,
        room_id,
        code: JoinErrorCode::AppError,
        message: message.into(),
        receiver_version: None,
        app_code: Some("document_not_found".into()),
    }
}

fn room_error(crdt: CrdtType, room_id: String, message: &str) -> ProtocolMessage {
    ProtocolMessage::RoomError {
        crdt,
        room_id,
        code: RoomErrorCode::Unknown,
        message: message.into(),
    }
}

mod fragments;

#[cfg(test)]
mod tests;
