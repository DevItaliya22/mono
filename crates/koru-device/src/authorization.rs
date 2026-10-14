// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use koru_store_core::{ProfileId, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::{
    AllDataGrantV1, BuildPayload, DeviceBuildV1, DeviceDescriptorV1, DeviceId, DeviceNameV1,
    ProfileGrantV1, ProfileLifecycleStateV1, ProfileLifecycleV1, RelationshipId,
    ReplicationEndpointId, RevocationV1, TrustRecord, TrustRelationshipV1, VerifiedTrustRecord,
    VerifyError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustView {
    pub local_device: DeviceDescriptorV1,
    pub devices: Vec<DeviceDescriptorV1>,
    pub relationships: Vec<TrustRelationshipV1>,
    pub profile_grants: Vec<ProfileGrantV1>,
    pub grants: Vec<AllDataGrantV1>,
    pub revocations: Vec<RevocationV1>,
    /// Self-signed renames, carried by gossip so a device can be known by its
    /// current name mesh-wide rather than by the name it paired with.
    pub names: Vec<DeviceNameV1>,
    /// Self-signed build descriptions, one kept per device — the latest
    /// supersedes, so a list can say what every peer runs.
    pub builds: Vec<DeviceBuildV1>,
    /// Highest accepted hosted lifecycle revision for each exact Profile.
    pub lifecycles: BTreeMap<ProfileId, ProfileLifecycleV1>,
    pub effective_revision: u64,
}

impl TrustView {
    /// The descriptor this view can vouch for, wherever it was learned: the
    /// local identity, the projected device list, or a relationship record
    /// that embeds it. Mesh gossip is exactly the case where a device is
    /// known only through someone else's relationship.
    pub fn descriptor_of(&self, device: DeviceId) -> Option<&DeviceDescriptorV1> {
        if self.local_device.device_id == device {
            return Some(&self.local_device);
        }
        self.devices
            .iter()
            .find(|descriptor| descriptor.device_id == device)
            .or_else(|| {
                self.relationships.iter().find_map(|relationship| {
                    if relationship.inviter.device_id == device {
                        Some(&relationship.inviter)
                    } else if relationship.joiner.device_id == device {
                        Some(&relationship.joiner)
                    } else {
                        None
                    }
                })
            })
    }

    pub fn relationship(&self, id: RelationshipId) -> Option<&TrustRelationshipV1> {
        self.relationships
            .iter()
            .find(|relationship| relationship.relationship_id == id)
    }

    /// The name a device is currently known by: its latest self-signed
    /// rename, falling back to the descriptor a pairing carried.
    pub fn display_name_of(&self, device: DeviceId) -> Option<String> {
        self.names
            .iter()
            .filter(|name| name.device_id == device)
            .max_by_key(|name| (name.issued_at_millis, name.record_id))
            .map(|name| name.display_name.clone())
            .or_else(|| {
                self.descriptor_of(device)
                    .map(|descriptor| descriptor.display_name.clone())
            })
    }

    /// The build a device last said it runs — its latest self-signed
    /// description, by the same clock-then-id rule a rename uses. `None` for
    /// a device that predates build records or whose payload this build
    /// cannot read at all.
    pub fn build_of(&self, device: DeviceId) -> Option<BuildPayload> {
        self.builds
            .iter()
            .filter(|build| build.device_id == device)
            .max_by_key(|build| (build.issued_at_millis, build.record_id))
            .and_then(DeviceBuildV1::payload)
    }

    /// Whether the latest description a device signed already says exactly
    /// this — the guard that keeps a launch on an unchanged build from
    /// writing a record.
    pub fn build_is_current(&self, device: DeviceId, payload: &BuildPayload) -> bool {
        self.build_of(device).as_ref() == Some(payload)
    }

    pub fn profile_is_deleted(&self, profile: ProfileId) -> bool {
        self.lifecycles
            .get(&profile)
            .is_some_and(|record| record.state == ProfileLifecycleStateV1::Deleted)
    }

    pub fn deleted_profiles(&self) -> BTreeSet<ProfileId> {
        self.lifecycles
            .iter()
            .filter_map(|(profile, record)| {
                (record.state == ProfileLifecycleStateV1::Deleted).then_some(*profile)
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileLifecycleOrder {
    Apply,
    Duplicate,
    Stale,
    Conflict,
}

/// Compare only hosted ordering fields. Wall time, record id, and delivery
/// path never decide lifecycle authority.
pub fn compare_profile_lifecycle(
    current: Option<&ProfileLifecycleV1>,
    incoming: &ProfileLifecycleV1,
) -> ProfileLifecycleOrder {
    let Some(current) = current else {
        return ProfileLifecycleOrder::Apply;
    };
    match incoming.lifecycle_revision.cmp(&current.lifecycle_revision) {
        std::cmp::Ordering::Greater => ProfileLifecycleOrder::Apply,
        std::cmp::Ordering::Less => ProfileLifecycleOrder::Stale,
        std::cmp::Ordering::Equal
            if incoming.operation_id == current.operation_id && incoming.state == current.state =>
        {
            ProfileLifecycleOrder::Duplicate
        }
        std::cmp::Ordering::Equal => ProfileLifecycleOrder::Conflict,
    }
}

#[derive(Clone, Debug)]
pub struct Authorization {
    snapshot: AuthorizationSnapshot,
}

impl Authorization {
    pub fn build(view: &TrustView, exportable: &BTreeMap<WorkspaceId, ProfileId>) -> Self {
        let peers = Self::devices(view);
        let mut entries = Vec::new();
        for peer in peers {
            if !peer.all_data {
                continue;
            }
            for (workspace_id, owner) in exportable {
                if view.profile_is_deleted(*owner) {
                    continue;
                }
                entries.push(AuthorizationEntry {
                    relationship_id: peer.relationship_id,
                    device_id: peer.device_id,
                    endpoint_id: peer.endpoint_id,
                    workspace_id: *workspace_id,
                });
            }
        }
        entries.sort();
        entries.dedup();
        Self {
            snapshot: AuthorizationSnapshot {
                generation: view.effective_revision,
                entries,
            },
        }
    }

    /// Every device reachable from the local device in at least one active
    /// authorization scope, independent of which workspace ids are known.
    ///
    /// All-data uses only unrevoked relationships with reciprocal
    /// [`AllDataGrantV1`] records. Each Profile has a separate graph whose
    /// edges have reciprocal all-data grants or reciprocal [`ProfileGrantV1`]
    /// records for that exact Profile. This lets an all-data edge compose with
    /// a Profile edge while preventing two different Profile scopes from
    /// composing. A peer's `all_data` bit is set only by the all-data graph;
    /// Profile reachability is collected separately in `authorized_profiles`.
    /// Sharing a workspace with another person is a separate, non-transitive
    /// path and must never enter this device graph.
    pub fn devices(view: &TrustView) -> Vec<AuthorizedDevice> {
        let revoked: BTreeSet<RelationshipId> = view
            .revocations
            .iter()
            .map(|revocation| revocation.relationship_id)
            .collect();
        let adjacency_for = |profile: Option<ProfileId>| {
            let mut adjacency: BTreeMap<DeviceId, Vec<(DeviceId, RelationshipId)>> =
                BTreeMap::new();
            for relationship in &view.relationships {
                if revoked.contains(&relationship.relationship_id) {
                    continue;
                }
                let a = relationship.inviter.device_id;
                let b = relationship.joiner.device_id;
                let all_data = |from: DeviceId, to: DeviceId| {
                    view.grants.iter().any(|grant| {
                        grant.relationship_id == relationship.relationship_id
                            && grant.issued_by == from
                            && grant.device_id == to
                    })
                };
                let profile_granted = |from: DeviceId, to: DeviceId, profile_id: ProfileId| {
                    view.profile_grants.iter().any(|grant| {
                        grant.relationship_id == relationship.relationship_id
                            && grant.profile_id == profile_id
                            && grant.issued_by == from
                            && grant.grantee_device_id == to
                    })
                };
                let granted = |from, to| {
                    all_data(from, to) || profile.is_some_and(|id| profile_granted(from, to, id))
                };
                if granted(a, b) && granted(b, a) {
                    adjacency
                        .entry(a)
                        .or_default()
                        .push((b, relationship.relationship_id));
                    adjacency
                        .entry(b)
                        .or_default()
                        .push((a, relationship.relationship_id));
                }
            }
            adjacency
        };
        // Breadth-first from the local device. `via` is the first hop — the
        // direct peer that introduced this one — while `relationship_id` is
        // the edge adjacent to the peer, so the Store journal names the
        // pairing the remote device actually holds.
        let walk = |adjacency: &BTreeMap<DeviceId, Vec<(DeviceId, RelationshipId)>>| {
            let mut routes = Vec::new();
            let mut visited = BTreeSet::from([view.local_device.device_id]);
            let mut queue = VecDeque::from([(view.local_device.device_id, None)]);
            while let Some((current, first_hop)) = queue.pop_front() {
                let mut neighbors = adjacency.get(&current).cloned().unwrap_or_default();
                neighbors.sort();
                for (next, relationship_id) in neighbors {
                    if visited.contains(&next) {
                        continue;
                    }
                    let Some(descriptor) = view.descriptor_of(next) else {
                        continue;
                    };
                    visited.insert(next);
                    let direct = current == view.local_device.device_id;
                    let via = if direct { None } else { first_hop };
                    routes.push((relationship_id, next, descriptor.endpoint_id, via));
                    queue.push_back((next, via.or(Some(next))));
                }
            }
            routes
        };

        let mut peers = BTreeMap::<DeviceId, AuthorizedDevice>::new();
        for (relationship_id, device_id, endpoint_id, via) in walk(&adjacency_for(None)) {
            peers.insert(
                device_id,
                AuthorizedDevice {
                    relationship_id,
                    device_id,
                    endpoint_id,
                    via,
                    all_data: true,
                    authorized_profiles: BTreeSet::new(),
                },
            );
        }
        let profiles: BTreeSet<ProfileId> = view
            .profile_grants
            .iter()
            .map(|grant| grant.profile_id)
            .filter(|profile| !view.profile_is_deleted(*profile))
            .collect();
        for profile_id in profiles {
            for (relationship_id, device_id, endpoint_id, via) in
                walk(&adjacency_for(Some(profile_id)))
            {
                peers
                    .entry(device_id)
                    .and_modify(|peer| {
                        peer.authorized_profiles.insert(profile_id);
                    })
                    .or_insert_with(|| AuthorizedDevice {
                        relationship_id,
                        device_id,
                        endpoint_id,
                        via,
                        all_data: false,
                        authorized_profiles: BTreeSet::from([profile_id]),
                    });
            }
        }
        peers.into_values().collect()
    }

    pub fn snapshot(&self) -> &AuthorizationSnapshot {
        &self.snapshot
    }

    pub fn check(
        &self,
        endpoint_id: ReplicationEndpointId,
        workspace_id: WorkspaceId,
    ) -> AuthorizationDecision {
        match self
            .snapshot
            .entries
            .iter()
            .find(|entry| entry.endpoint_id == endpoint_id && entry.workspace_id == workspace_id)
        {
            Some(entry) => AuthorizationDecision::Allowed(AuthorizedReplica {
                relationship_id: entry.relationship_id,
                device_id: entry.device_id,
                endpoint_id,
                workspace_id,
            }),
            None => AuthorizationDecision::Denied(
                if self
                    .snapshot
                    .entries
                    .iter()
                    .any(|entry| entry.endpoint_id == endpoint_id)
                {
                    DenyReason::WorkspaceNotGranted
                } else {
                    DenyReason::UntrustedEndpoint
                },
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedDevice {
    pub relationship_id: RelationshipId,
    pub device_id: DeviceId,
    pub endpoint_id: ReplicationEndpointId,
    /// The directly paired device that introduced this one, when it is not
    /// itself directly paired. `None` for a first-hop peer.
    pub via: Option<DeviceId>,
    /// Derived only from a path of reciprocal all-data edges.
    pub all_data: bool,
    /// Each Profile reachable in that Profile's reciprocal component.
    pub authorized_profiles: BTreeSet<ProfileId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationSnapshot {
    pub generation: u64,
    pub entries: Vec<AuthorizationEntry>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AuthorizationEntry {
    pub relationship_id: RelationshipId,
    pub device_id: DeviceId,
    pub endpoint_id: ReplicationEndpointId,
    pub workspace_id: WorkspaceId,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedReplica {
    pub relationship_id: RelationshipId,
    pub device_id: DeviceId,
    pub endpoint_id: ReplicationEndpointId,
    pub workspace_id: WorkspaceId,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AuthorizationDecision {
    Allowed(AuthorizedReplica),
    Denied(DenyReason),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DenyReason {
    UntrustedEndpoint,
    WorkspaceNotGranted,
}

/// Why a gossiped trust record was refused. Gossiped records routinely name
/// third parties — that is their purpose — so verification is purely
/// cryptographic against the receiver's own view of the trust graph.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GossipRejection {
    Malformed(VerifyError),
    UnknownRelationship,
    WrongParties,
    UnknownDevice,
    KeyMismatch,
    /// The issuer had no reciprocal all-data or exact-Profile path before the
    /// lifecycle fact was considered.
    UnauthorizedProfile,
    /// A record kind this build does not know; the sender is newer. Skipped,
    /// and the rest of its batch still applies.
    UnknownKind,
}

impl std::fmt::Display for GossipRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Malformed(error) => return write!(f, "malformed_record: {error}"),
                Self::UnknownRelationship => "unknown_relationship",
                Self::WrongParties => "wrong_parties",
                Self::UnknownDevice => "unknown_device",
                Self::KeyMismatch => "key_mismatch",
                Self::UnauthorizedProfile => "unauthorized_profile_lifecycle",
                Self::UnknownKind => "unknown_record_kind",
            }
        )
    }
}

impl std::error::Error for GossipRejection {}

/// Verify one gossiped trust record against the receiver's own trust view.
///
/// A relationship is self-contained — both descriptors are embedded and both
/// signatures cover the transcript hash. Every other record verifies against
/// the issuer's key, resolved from relationships the view already holds
/// (gossip is sent in [`crate::sort_for_gossip`] order so a batch is
/// verifiable in one pass). Nothing here trusts the *sender*: a forged or
/// misattributed record simply fails its signature.
pub fn verify_gossip_record(
    view: &TrustView,
    record: TrustRecord,
) -> Result<VerifiedTrustRecord, GossipRejection> {
    match record {
        TrustRecord::Relationship(relationship) => {
            relationship.verify().map_err(GossipRejection::Malformed)
        }
        TrustRecord::AllDataGrant(grant) => {
            let relationship = view
                .relationship(grant.relationship_id)
                .ok_or(GossipRejection::UnknownRelationship)?;
            let issuer = if relationship.inviter.device_id == grant.issued_by
                && relationship.joiner.device_id == grant.device_id
            {
                &relationship.inviter
            } else if relationship.joiner.device_id == grant.issued_by
                && relationship.inviter.device_id == grant.device_id
            {
                &relationship.joiner
            } else {
                return Err(GossipRejection::WrongParties);
            };
            grant
                .verify(issuer.signing_public_key)
                .map_err(GossipRejection::Malformed)
        }
        TrustRecord::ProfileGrant(grant) => {
            let relationship = view
                .relationship(grant.relationship_id)
                .ok_or(GossipRejection::UnknownRelationship)?;
            let issuer = if relationship.inviter.device_id == grant.issued_by
                && relationship.joiner.device_id == grant.grantee_device_id
            {
                &relationship.inviter
            } else if relationship.joiner.device_id == grant.issued_by
                && relationship.inviter.device_id == grant.grantee_device_id
            {
                &relationship.joiner
            } else {
                return Err(GossipRejection::WrongParties);
            };
            grant
                .verify(issuer.signing_public_key)
                .map_err(GossipRejection::Malformed)
        }
        TrustRecord::ProfileLifecycle(lifecycle) => {
            let issuer = view
                .descriptor_of(lifecycle.issued_by)
                .ok_or(GossipRejection::UnknownDevice)?;
            let authorized = Authorization::devices(view).into_iter().any(|peer| {
                peer.device_id == lifecycle.issued_by
                    && (peer.all_data || peer.authorized_profiles.contains(&lifecycle.profile_id))
            });
            if !authorized {
                return Err(GossipRejection::UnauthorizedProfile);
            }
            lifecycle
                .verify(issuer.signing_public_key)
                .map_err(GossipRejection::Malformed)
        }
        TrustRecord::Revocation(revocation) => {
            let relationship = view
                .relationship(revocation.relationship_id)
                .ok_or(GossipRejection::UnknownRelationship)?;
            let issuer = if relationship.inviter.device_id == revocation.issued_by {
                &relationship.inviter
            } else if relationship.joiner.device_id == revocation.issued_by {
                &relationship.joiner
            } else {
                return Err(GossipRejection::WrongParties);
            };
            revocation
                .verify(issuer.signing_public_key)
                .map_err(GossipRejection::Malformed)
        }
        TrustRecord::EndpointBinding(binding) => {
            let descriptor = view
                .descriptor_of(binding.device_id)
                .ok_or(GossipRejection::UnknownDevice)?;
            if descriptor.signing_public_key != binding.device_public_key {
                return Err(GossipRejection::KeyMismatch);
            }
            binding.verify().map_err(GossipRejection::Malformed)
        }
        TrustRecord::DeviceName(name) => {
            // A rename is authentic only from the named device's own key.
            let descriptor = view
                .descriptor_of(name.device_id)
                .ok_or(GossipRejection::UnknownDevice)?;
            name.verify(descriptor.signing_public_key)
                .map_err(GossipRejection::Malformed)
        }
        TrustRecord::DeviceBuild(build) => {
            // Like a rename: only the described device's own key makes it.
            let descriptor = view
                .descriptor_of(build.device_id)
                .ok_or(GossipRejection::UnknownDevice)?;
            build
                .verify(descriptor.signing_public_key)
                .map_err(GossipRejection::Malformed)
        }
        TrustRecord::Unknown => Err(GossipRejection::UnknownKind),
    }
}
