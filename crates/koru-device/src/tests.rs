// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};

use koru_store_core::{ProfileId, WorkspaceId};

use crate::*;

fn device(name: &str, created: i64) -> (DeviceSigningKey, DeviceDescriptorV1) {
    let signing = DeviceSigningKey::generate();
    let endpoint = ReplicationEndpointSecret::generate();
    let descriptor = DeviceDescriptorV1::new(
        DeviceId::new(),
        name.to_owned(),
        &signing,
        endpoint.endpoint_id(),
        created,
    );
    (signing, descriptor)
}

/// A completed pairing: the relationship plus both directional all-data
/// grants, exactly what `commit_pairing` persists on each side.
fn pair(
    a: &DeviceSigningKey,
    a_descriptor: &DeviceDescriptorV1,
    b: &DeviceSigningKey,
    b_descriptor: &DeviceDescriptorV1,
) -> (TrustRelationshipV1, AllDataGrantV1, AllDataGrantV1) {
    let relationship_id = RelationshipId::new();
    let relationship = TrustRelationshipV1::sign(
        relationship_id,
        a_descriptor.clone(),
        b_descriptor.clone(),
        transcript_hash(relationship_id.to_storage().as_bytes()),
        3,
        a,
        b,
    );
    let a_grant = AllDataGrantV1::sign(
        relationship_id,
        b_descriptor.device_id,
        a_descriptor.device_id,
        RecordId::new(),
        4,
        a,
    );
    let b_grant = AllDataGrantV1::sign(
        relationship_id,
        a_descriptor.device_id,
        b_descriptor.device_id,
        RecordId::new(),
        4,
        b,
    );
    (relationship, a_grant, b_grant)
}

fn view_of(
    local: DeviceDescriptorV1,
    relationships: Vec<TrustRelationshipV1>,
    grants: Vec<AllDataGrantV1>,
    revocations: Vec<RevocationV1>,
) -> TrustView {
    let mut devices = Vec::new();
    for relationship in &relationships {
        devices.push(relationship.inviter.clone());
        devices.push(relationship.joiner.clone());
    }
    devices.retain(|descriptor| descriptor.device_id != local.device_id);
    TrustView {
        local_device: local,
        devices,
        relationships,
        profile_grants: Vec::new(),
        grants,
        revocations,
        names: Vec::new(),
        builds: Vec::new(),
        lifecycles: BTreeMap::new(),
        effective_revision: 1,
    }
}

fn owned(
    owner: ProfileId,
    workspaces: impl IntoIterator<Item = WorkspaceId>,
) -> BTreeMap<WorkspaceId, ProfileId> {
    workspaces
        .into_iter()
        .map(|workspace| (workspace, owner))
        .collect()
}

fn profile_pair(
    relationship: &TrustRelationshipV1,
    profile_id: ProfileId,
    a: &DeviceSigningKey,
    b: &DeviceSigningKey,
) -> (ProfileGrantV1, ProfileGrantV1) {
    (
        ProfileGrantV1::sign(
            relationship.relationship_id,
            RecordId::new(),
            profile_id,
            relationship.joiner.device_id,
            relationship.inviter.device_id,
            4,
            a,
        ),
        ProfileGrantV1::sign(
            relationship.relationship_id,
            RecordId::new(),
            profile_id,
            relationship.inviter.device_id,
            relationship.joiner.device_id,
            4,
            b,
        ),
    )
}

#[test]
fn secret_types_are_redacted_and_keys_are_distinct() {
    let signing = DeviceSigningKey::generate();
    let endpoint = ReplicationEndpointSecret::generate();
    assert_ne!(signing.secret_bytes(), endpoint.secret_bytes());
    assert_eq!(format!("{signing:?}"), "DeviceSigningKey([REDACTED])");
    assert_eq!(
        format!("{endpoint:?}"),
        "ReplicationEndpointSecret([REDACTED])"
    );
    assert_eq!(
        format!("{:?}", SecretBytes([7; 32])),
        "SecretBytes([REDACTED])"
    );
}

#[test]
fn every_signed_field_is_authenticated() {
    let (key, descriptor) = device("A", 10);
    descriptor.verify().unwrap();

    let grant = AllDataGrantV1::sign(
        RelationshipId::new(),
        DeviceId::new(),
        descriptor.device_id,
        RecordId::new(),
        20,
        &key,
    );
    grant.clone().verify(descriptor.signing_public_key).unwrap();

    let mut tampered = grant;
    tampered.issued_at_millis += 1;
    assert_eq!(
        tampered.verify(descriptor.signing_public_key),
        Err(VerifyError::InvalidSignature)
    );
}

#[test]
fn profile_grant_signs_every_authority_field_and_rejects_wrong_parties() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (_c_key, c) = device("C", 3);
    let (relationship, _, _) = pair(&a_key, &a, &b_key, &b);
    let profile_id = ProfileId::new();
    let grant = ProfileGrantV1::sign(
        relationship.relationship_id,
        RecordId::new(),
        profile_id,
        b.device_id,
        a.device_id,
        10,
        &a_key,
    );
    grant.clone().verify(a.signing_public_key).unwrap();
    let mut unsupported = grant.clone();
    unsupported.version += 1;
    assert_eq!(
        unsupported.verify(a.signing_public_key),
        Err(VerifyError::UnsupportedVersion)
    );

    let signed = grant.signing_bytes();
    let mut changes = Vec::new();
    let mut changed = grant.clone();
    changed.relationship_id = RelationshipId::new();
    changes.push(changed.signing_bytes());
    let mut changed = grant.clone();
    changed.record_id = RecordId::new();
    changes.push(changed.signing_bytes());
    let mut changed = grant.clone();
    changed.profile_id = ProfileId::new();
    changes.push(changed.signing_bytes());
    let mut changed = grant.clone();
    changed.grantee_device_id = c.device_id;
    changes.push(changed.signing_bytes());
    let mut changed = grant.clone();
    changed.issued_by = b.device_id;
    changes.push(changed.signing_bytes());
    let mut changed = grant.clone();
    changed.issued_at_millis += 1;
    changes.push(changed.signing_bytes());
    assert!(changes.into_iter().all(|bytes| bytes != signed));

    let view = view_of(
        a.clone(),
        vec![relationship.clone()],
        Vec::new(),
        Vec::new(),
    );
    verify_gossip_record(&view, TrustRecord::ProfileGrant(grant.clone()))
        .expect("the exact relationship parties verify");
    let wrong_parties = ProfileGrantV1::sign(
        relationship.relationship_id,
        RecordId::new(),
        profile_id,
        c.device_id,
        a.device_id,
        10,
        &a_key,
    );
    assert_eq!(
        verify_gossip_record(&view, TrustRecord::ProfileGrant(wrong_parties)),
        Err(GossipRejection::WrongParties)
    );
    let forged = ProfileGrantV1::sign(
        relationship.relationship_id,
        RecordId::new(),
        profile_id,
        b.device_id,
        a.device_id,
        10,
        &b_key,
    );
    assert!(matches!(
        verify_gossip_record(&view, TrustRecord::ProfileGrant(forged)),
        Err(GossipRejection::Malformed(VerifyError::InvalidSignature))
    ));
}

#[test]
fn profile_authorization_is_reciprocal_scope_specific_and_transitive() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (c_key, c) = device("C", 3);
    let (ab, ab_all_a, ab_all_b) = pair(&a_key, &a, &b_key, &b);
    let (bc, _, _) = pair(&b_key, &b, &c_key, &c);
    let profile_a = ProfileId::new();
    let profile_b = ProfileId::new();
    let (bc_a, bc_b) = profile_pair(&bc, profile_a, &b_key, &c_key);
    let (ab_other_a, ab_other_b) = profile_pair(&ab, profile_b, &a_key, &b_key);

    // A-B's all-data edge composes with B-C's exact Profile A edge. The
    // unrelated Profile B grant does not leak across B-C.
    let mut view = view_of(
        a.clone(),
        vec![ab.clone(), bc.clone()],
        vec![ab_all_a, ab_all_b],
        Vec::new(),
    );
    view.profile_grants = vec![bc_a, bc_b, ab_other_a, ab_other_b];
    let peers = Authorization::devices(&view);
    let b_peer = peers
        .iter()
        .find(|peer| peer.device_id == b.device_id)
        .unwrap();
    assert!(b_peer.all_data);
    assert_eq!(
        b_peer.authorized_profiles,
        BTreeSet::from([profile_a, profile_b])
    );
    let c_peer = peers
        .iter()
        .find(|peer| peer.device_id == c.device_id)
        .unwrap();
    assert!(!c_peer.all_data);
    assert_eq!(c_peer.authorized_profiles, BTreeSet::from([profile_a]));
    let profile_only_workspace = WorkspaceId::new();
    assert_eq!(
        Authorization::build(&view, &owned(profile_a, [profile_only_workspace]))
            .check(c.endpoint_id, profile_only_workspace),
        AuthorizationDecision::Denied(DenyReason::UntrustedEndpoint),
        "Profile scope does not become a workspace grant before Plan 4.2"
    );

    // Replacing the A-B all-data edge with Profile B only breaks the Profile
    // A path entirely: differently scoped edges never compose.
    view.grants.clear();
    let peers = Authorization::devices(&view);
    assert!(peers.iter().any(|peer| peer.device_id == b.device_id));
    assert!(!peers.iter().any(|peer| peer.device_id == c.device_id));

    // One directional grant is never an active edge.
    view.profile_grants = vec![view.profile_grants[2].clone()];
    assert!(Authorization::devices(&view).is_empty());
}

#[test]
fn profile_revocation_is_relationship_scoped_and_repair_uses_the_new_relationship() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (old, _, _) = pair(&a_key, &a, &b_key, &b);
    let (fresh, _, _) = pair(&a_key, &a, &b_key, &b);
    let profile = ProfileId::new();
    let (old_a, old_b) = profile_pair(&old, profile, &a_key, &b_key);
    let (fresh_a, fresh_b) = profile_pair(&fresh, profile, &a_key, &b_key);
    let mut view = view_of(
        a.clone(),
        vec![old.clone(), fresh.clone()],
        Vec::new(),
        vec![RevocationV1::sign(
            old.relationship_id,
            b.device_id,
            a.device_id,
            RecordId::new(),
            9,
            &a_key,
        )],
    );
    view.profile_grants = vec![old_a, old_b, fresh_a, fresh_b];
    let peers = Authorization::devices(&view);
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].relationship_id, fresh.relationship_id);
    assert!(!peers[0].all_data);
    assert_eq!(peers[0].authorized_profiles, BTreeSet::from([profile]));
}

#[test]
fn authorization_requires_direct_relationship_grant_and_exact_workspace() {
    let (local_key, local) = device("A", 1);
    let (peer_key, peer) = device("B", 2);
    let relationship_id = RelationshipId::new();
    let relationship = TrustRelationshipV1::sign(
        relationship_id,
        local.clone(),
        peer.clone(),
        transcript_hash(b"pairing transcript"),
        3,
        &local_key,
        &peer_key,
    );
    relationship.clone().verify().unwrap();

    let workspace = WorkspaceId::new();
    let other = WorkspaceId::new();
    let exportable = owned(ProfileId::new(), [workspace]);
    let mut view = TrustView {
        local_device: local.clone(),
        devices: vec![peer.clone()],
        relationships: vec![relationship],
        profile_grants: Vec::new(),
        grants: Vec::new(),
        revocations: Vec::new(),
        names: Vec::new(),
        builds: Vec::new(),
        lifecycles: BTreeMap::new(),
        effective_revision: 1,
    };

    let without_grant = Authorization::build(&view, &exportable);
    assert_eq!(
        without_grant.check(peer.endpoint_id, workspace),
        AuthorizationDecision::Denied(DenyReason::UntrustedEndpoint)
    );

    view.grants.push(AllDataGrantV1::sign(
        relationship_id,
        peer.device_id,
        local.device_id,
        RecordId::new(),
        4,
        &local_key,
    ));
    view.grants.push(AllDataGrantV1::sign(
        relationship_id,
        local.device_id,
        peer.device_id,
        RecordId::new(),
        4,
        &peer_key,
    ));
    view.effective_revision = 2;
    let allowed = Authorization::build(&view, &exportable);
    assert!(matches!(
        allowed.check(peer.endpoint_id, workspace),
        AuthorizationDecision::Allowed(_)
    ));
    assert_eq!(
        allowed.check(peer.endpoint_id, other),
        AuthorizationDecision::Denied(DenyReason::WorkspaceNotGranted)
    );

    view.revocations.push(RevocationV1::sign(
        relationship_id,
        peer.device_id,
        local.device_id,
        RecordId::new(),
        5,
        &local_key,
    ));
    view.effective_revision = 3;
    assert_eq!(
        Authorization::build(&view, &exportable).check(peer.endpoint_id, workspace),
        AuthorizationDecision::Denied(DenyReason::UntrustedEndpoint)
    );
}

#[test]
fn a_fresh_relationship_can_authorize_after_revocation() {
    let (local_key, local) = device("A", 1);
    let (peer_key, peer) = device("B", 2);
    let old_id = RelationshipId::new();
    let new_id = RelationshipId::new();
    let relationship = |id| {
        TrustRelationshipV1::sign(
            id,
            local.clone(),
            peer.clone(),
            transcript_hash(id.to_storage().as_bytes()),
            3,
            &local_key,
            &peer_key,
        )
    };
    let grant = |id| {
        AllDataGrantV1::sign(
            id,
            peer.device_id,
            local.device_id,
            RecordId::new(),
            4,
            &local_key,
        )
    };
    let peer_grant = |id| {
        AllDataGrantV1::sign(
            id,
            local.device_id,
            peer.device_id,
            RecordId::new(),
            4,
            &peer_key,
        )
    };
    let workspace = WorkspaceId::new();
    let view = TrustView {
        local_device: local.clone(),
        devices: vec![peer.clone()],
        relationships: vec![relationship(old_id), relationship(new_id)],
        profile_grants: Vec::new(),
        grants: vec![
            grant(old_id),
            grant(new_id),
            peer_grant(old_id),
            peer_grant(new_id),
        ],
        revocations: vec![RevocationV1::sign(
            old_id,
            peer.device_id,
            local.device_id,
            RecordId::new(),
            5,
            &local_key,
        )],
        names: Vec::new(),
        builds: Vec::new(),
        lifecycles: BTreeMap::new(),
        effective_revision: 5,
    };
    assert!(matches!(
        Authorization::build(&view, &owned(ProfileId::new(), [workspace]))
            .check(peer.endpoint_id, workspace),
        AuthorizationDecision::Allowed(AuthorizedReplica {
            relationship_id,
            ..
        }) if relationship_id == new_id
    ));
}

#[test]
fn a_peer_of_a_paired_peer_is_authorized_through_the_mesh() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (c_key, c) = device("C", 3);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let (bc, bc_b, bc_c) = pair(&b_key, &b, &c_key, &c);
    let view = view_of(
        a.clone(),
        vec![ab.clone(), bc.clone()],
        vec![ab_a, ab_b, bc_b, bc_c],
        Vec::new(),
    );

    let peers = Authorization::devices(&view);
    assert_eq!(peers.len(), 2);
    let direct = peers
        .iter()
        .find(|peer| peer.device_id == b.device_id)
        .expect("the direct peer is authorized");
    assert_eq!(direct.relationship_id, ab.relationship_id);
    assert_eq!(direct.via, None);
    let transitive = peers
        .iter()
        .find(|peer| peer.device_id == c.device_id)
        .expect("the mesh peer is authorized");
    assert_eq!(transitive.relationship_id, bc.relationship_id);
    assert_eq!(transitive.via, Some(b.device_id));
    assert_eq!(transitive.endpoint_id, c.endpoint_id);
}

#[test]
fn a_half_signed_edge_does_not_extend_the_mesh() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (_c_key, c) = device("C", 3);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let (bc, bc_b, _bc_c) = pair(&b_key, &b, &_c_key, &c);
    // C never completed its side of the pairing: only B's grant exists.
    let view = view_of(a.clone(), vec![ab, bc], vec![ab_a, ab_b, bc_b], Vec::new());
    let peers = Authorization::devices(&view);
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].device_id, b.device_id);
}

#[test]
fn a_revoked_edge_cuts_everything_behind_it() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (c_key, c) = device("C", 3);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let (bc, bc_b, bc_c) = pair(&b_key, &b, &c_key, &c);
    let revocation = RevocationV1::sign(
        bc.relationship_id,
        c.device_id,
        b.device_id,
        RecordId::new(),
        9,
        &b_key,
    );
    let view = view_of(
        a.clone(),
        vec![ab.clone(), bc],
        vec![ab_a, ab_b, bc_b, bc_c],
        vec![revocation],
    );
    let peers = Authorization::devices(&view);
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].device_id, b.device_id);
}

#[test]
fn an_alternate_path_survives_a_revocation() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (c_key, c) = device("C", 3);
    let (d_key, d) = device("D", 4);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let (bc, bc_b, bc_c) = pair(&b_key, &b, &c_key, &c);
    let (ad, ad_a, ad_d) = pair(&a_key, &a, &d_key, &d);
    let (dc, dc_d, dc_c) = pair(&d_key, &d, &c_key, &c);
    let revocation = RevocationV1::sign(
        bc.relationship_id,
        c.device_id,
        b.device_id,
        RecordId::new(),
        9,
        &b_key,
    );
    let view = view_of(
        a.clone(),
        vec![ab, bc, ad, dc.clone()],
        vec![ab_a, ab_b, bc_b, bc_c, ad_a, ad_d, dc_d, dc_c],
        vec![revocation],
    );
    let peers = Authorization::devices(&view);
    assert_eq!(peers.len(), 3);
    let c_peer = peers
        .iter()
        .find(|peer| peer.device_id == c.device_id)
        .expect("C is still reachable through D");
    assert_eq!(c_peer.relationship_id, dc.relationship_id);
    assert_eq!(c_peer.via, Some(d.device_id));
}

#[test]
fn gossiped_records_verify_against_the_receivers_own_view() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (c_key, c) = device("C", 3);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let (bc, bc_b, bc_c) = pair(&b_key, &b, &c_key, &c);
    // A's view after pairing with B, before any gossip arrives.
    let mut view = view_of(a.clone(), vec![ab], vec![ab_a, ab_b], Vec::new());

    // The B⇔C relationship is self-contained and verifies on its own.
    let relationship = verify_gossip_record(&view, TrustRecord::Relationship(bc.clone()))
        .expect("a third-party relationship verifies from its own signatures");
    assert_eq!(
        relationship.into_record(),
        TrustRecord::Relationship(bc.clone())
    );
    // Once it is held, its grants verify against the embedded descriptors.
    view.relationships.push(bc.clone());
    for grant in [bc_b.clone(), bc_c.clone()] {
        verify_gossip_record(&view, TrustRecord::AllDataGrant(grant))
            .expect("a grant verifies against its relationship");
    }

    // A grant that names no parties of its relationship is refused.
    let stray = AllDataGrantV1::sign(
        bc.relationship_id,
        a.device_id,
        b.device_id,
        RecordId::new(),
        5,
        &b_key,
    );
    assert_eq!(
        verify_gossip_record(&view, TrustRecord::AllDataGrant(stray)),
        Err(GossipRejection::WrongParties)
    );

    // A grant for a relationship the receiver has never seen cannot be
    // verified yet — the sender's canonical ordering prevents this in
    // practice.
    let unknown = AllDataGrantV1::sign(
        RelationshipId::new(),
        c.device_id,
        b.device_id,
        RecordId::new(),
        5,
        &b_key,
    );
    assert_eq!(
        verify_gossip_record(&view, TrustRecord::AllDataGrant(unknown)),
        Err(GossipRejection::UnknownRelationship)
    );

    // A forged grant — signed by a key that is not the issuer's — is refused.
    let forged = AllDataGrantV1::sign(
        bc.relationship_id,
        c.device_id,
        b.device_id,
        RecordId::new(),
        5,
        &c_key,
    );
    assert!(matches!(
        verify_gossip_record(&view, TrustRecord::AllDataGrant(forged)),
        Err(GossipRejection::Malformed(VerifyError::InvalidSignature))
    ));

    // A revocation signed by a relationship party verifies even when it names
    // a third party relative to the receiver.
    let revocation = RevocationV1::sign(
        bc.relationship_id,
        c.device_id,
        b.device_id,
        RecordId::new(),
        6,
        &b_key,
    );
    verify_gossip_record(&view, TrustRecord::Revocation(revocation))
        .expect("a third-party revocation verifies");
}

#[test]
fn a_rename_is_self_signed_and_the_latest_wins() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let mut view = view_of(a.clone(), vec![ab], vec![ab_a, ab_b], Vec::new());

    // Only the named device's own key can rename it.
    let forged = DeviceNameV1::sign(RecordId::new(), b.device_id, "Fake".into(), 10, &a_key);
    assert!(matches!(
        verify_gossip_record(&view, TrustRecord::DeviceName(forged)),
        Err(GossipRejection::Malformed(VerifyError::InvalidSignature))
    ));

    let apply_name = |view: &mut TrustView, name: DeviceNameV1| {
        let verified = verify_gossip_record(view, TrustRecord::DeviceName(name)).unwrap();
        match verified.into_record() {
            TrustRecord::DeviceName(name) => view.names.push(name),
            _ => unreachable!(),
        }
    };
    apply_name(
        &mut view,
        DeviceNameV1::sign(RecordId::new(), b.device_id, "Workshop".into(), 10, &b_key),
    );
    assert_eq!(
        view.display_name_of(b.device_id).as_deref(),
        Some("Workshop")
    );

    apply_name(
        &mut view,
        DeviceNameV1::sign(RecordId::new(), b.device_id, "Studio".into(), 20, &b_key),
    );
    assert_eq!(view.display_name_of(b.device_id).as_deref(), Some("Studio"));

    // A device with no rename keeps the name its pairing carried.
    assert_eq!(view.display_name_of(a.device_id).as_deref(), Some("A"));
}

fn payload(version: &str, os: Option<&str>) -> BuildPayload {
    BuildPayload {
        app_version: version.into(),
        channel: "nightly".into(),
        branch: Some("replicate-devices".into()),
        commit: None,
        os: os.map(str::to_owned),
        os_version: None,
        peer_protocol: None,
    }
}

#[test]
fn a_build_is_self_signed_the_latest_wins_and_an_unchanged_one_is_current() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let mut view = view_of(a.clone(), vec![ab], vec![ab_a, ab_b], Vec::new());

    let forged = DeviceBuildV1::sign(
        RecordId::new(),
        b.device_id,
        &payload("9.9.9", None),
        10,
        &a_key,
    );
    assert!(matches!(
        verify_gossip_record(&view, TrustRecord::DeviceBuild(forged)),
        Err(GossipRejection::Malformed(VerifyError::InvalidSignature))
    ));

    let apply = |view: &mut TrustView, build: DeviceBuildV1| {
        let verified = verify_gossip_record(view, TrustRecord::DeviceBuild(build)).unwrap();
        match verified.into_record() {
            TrustRecord::DeviceBuild(build) => view.builds.push(build),
            _ => unreachable!(),
        }
    };
    let first = payload("0.0.3", Some("Windows 11"));
    apply(
        &mut view,
        DeviceBuildV1::sign(RecordId::new(), b.device_id, &first, 10, &b_key),
    );
    assert_eq!(view.build_of(b.device_id).as_ref(), Some(&first));
    assert!(view.build_is_current(b.device_id, &first));
    assert_eq!(
        first.label(),
        "0.0.3 · nightly · replicate-devices · Windows 11"
    );

    let second = payload("0.0.4", Some("Windows 11"));
    assert!(!view.build_is_current(b.device_id, &second));
    apply(
        &mut view,
        DeviceBuildV1::sign(RecordId::new(), b.device_id, &second, 20, &b_key),
    );
    assert_eq!(view.build_of(b.device_id).as_ref(), Some(&second));

    // A device that never said anything has no build, not a default one.
    assert_eq!(view.build_of(a.device_id), None);
}

#[test]
fn drift_is_semver_with_the_protocol_number_overriding_it() {
    let at = |version: &str, protocol: Option<u32>| {
        let mut build = payload(version, None);
        build.peer_protocol = protocol;
        build
    };
    let drift = |a: &BuildPayload, b: &BuildPayload| {
        let forward = a.drift_from(b);
        assert_eq!(
            forward.severity(),
            b.drift_from(a).severity(),
            "drift is symmetric"
        );
        forward
    };

    // Patch drift is semver's promise of compatibility and is not shown.
    assert_eq!(
        drift(&at("1.0.0", None), &at("1.0.1", None)),
        BuildDrift::Patch
    );
    assert_eq!(BuildDrift::Patch.severity(), DriftSeverity::None);
    // A minor is a warning, a major is danger.
    assert_eq!(
        drift(&at("1.0.1", None), &at("1.1.0", None)),
        BuildDrift::Minor
    );
    assert_eq!(BuildDrift::Minor.severity(), DriftSeverity::Warning);
    assert_eq!(
        drift(&at("1.4.2", None), &at("2.0.0", None)),
        BuildDrift::Major
    );
    assert_eq!(BuildDrift::Major.severity(), DriftSeverity::Danger);
    // Distance within minors does not escalate.
    assert_eq!(
        drift(&at("1.0.0", None), &at("1.9.0", None)),
        BuildDrift::Minor
    );

    // Only the leading three numbers are read: Gradle's "(1)", a nightly's
    // pre-release and build metadata, and a different commit all vanish.
    assert_eq!(
        drift(&at("0.1.0 (1)", None), &at("0.1.0", None)),
        BuildDrift::Same
    );
    assert_eq!(
        drift(
            &at("0.0.3-nightly.20260816+abc", None),
            &at("0.0.3-nightly.20260817+def", None)
        ),
        BuildDrift::Same
    );
    let mut phone = at("0.1.0 (1)", None);
    phone.channel = "debug".into();
    phone.os = Some("Android 17".into());
    phone.commit = Some("111111".into());
    let mut desktop = at("0.1.0", None);
    desktop.channel = "internal".into();
    desktop.commit = Some("222222".into());
    assert_eq!(drift(&phone, &desktop), BuildDrift::Same);
    assert_eq!(
        drift(&at("garbage", None), &at("1.0.0", None)),
        BuildDrift::Unknown
    );

    // The protocol number overrides everything: equal versions, different
    // numbers, and the two do not sync.
    let incompatible = drift(&at("1.0.0", Some(1)), &at("1.0.0", Some(2)));
    assert_eq!(
        incompatible,
        BuildDrift::Incompatible { ours: 1, theirs: 2 }
    );
    assert!(incompatible.is_incompatible());
    assert_eq!(incompatible.severity(), DriftSeverity::Danger);
    assert_eq!(incompatible.as_str(), "incompatible");
    // A device from before the number is compared on its version as usual,
    // not locked out by the build that introduced the number.
    assert_eq!(
        drift(&at("1.0.0", None), &at("1.0.0", Some(2))),
        BuildDrift::Same
    );
    assert_eq!(
        drift(&at("1.0.0", None), &at("1.1.0", Some(2))),
        BuildDrift::Minor
    );
}

#[test]
fn a_payload_with_fields_this_build_does_not_know_still_verifies() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let view = view_of(a, vec![ab], vec![ab_a, ab_b], Vec::new());

    // What a newer device would sign: a field added after this build shipped.
    let mut future = DeviceBuildV1::sign(
        RecordId::new(),
        b.device_id,
        &payload("1.0.0", None),
        10,
        &b_key,
    );
    future.payload = future
        .payload
        .replacen('{', "{\"gpu\":\"vello on dx12\",", 1);
    future.signature = b_key.sign(&future.signing_bytes());
    let verified = verify_gossip_record(&view, TrustRecord::DeviceBuild(future))
        .expect("unknown payload fields are ignored, not refused");
    let TrustRecord::DeviceBuild(build) = verified.into_record() else {
        unreachable!()
    };
    assert_eq!(build.payload().unwrap().app_version, "1.0.0");

    // But not unbounded: the payload has a byte ceiling and each field a
    // character one.
    let mut huge = payload("1.0.0", None);
    huge.os_version = Some("x".repeat(MAX_BUILD_FIELD_CHARS + 1));
    let huge = DeviceBuildV1::sign(RecordId::new(), b.device_id, &huge, 10, &b_key);
    assert!(matches!(
        verify_gossip_record(&view, TrustRecord::DeviceBuild(huge)),
        Err(GossipRejection::Malformed(VerifyError::Oversized))
    ));
}

#[test]
fn a_record_kind_this_build_does_not_know_decodes_and_is_refused_alone() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (ab, ab_a, ab_b) = pair(&a_key, &a, &b_key, &b);
    let view = view_of(a, vec![ab], vec![ab_a, ab_b], Vec::new());

    let name = DeviceNameV1::sign(RecordId::new(), b.device_id, "Studio".into(), 10, &b_key);
    let json = format!(
        "[{{\"kind\":\"device_hologram\",\"shape\":\"cube\"}},{}]",
        serde_json::to_string(&TrustRecord::DeviceName(name)).unwrap()
    );
    let records: Vec<TrustRecord> =
        serde_json::from_str(&json).expect("a batch with a newer kind still decodes");
    assert_eq!(records.len(), 2);
    assert!(matches!(records[0], TrustRecord::Unknown));
    assert!(matches!(
        verify_gossip_record(&view, records[0].clone()),
        Err(GossipRejection::UnknownKind)
    ));
    verify_gossip_record(&view, records[1].clone()).expect("the known record beside it applies");

    let retired: TrustRecord =
        serde_json::from_str(r#"{"kind":"profile_membership","version":1,"record_id":"retired"}"#)
            .expect("the retired membership authority is handled as an unknown record");
    assert!(matches!(retired, TrustRecord::Unknown));
}

#[test]
fn gossip_order_puts_relationships_before_the_records_that_reference_them() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (relationship, a_grant, b_grant) = pair(&a_key, &a, &b_key, &b);
    let revocation = RevocationV1::sign(
        relationship.relationship_id,
        b.device_id,
        a.device_id,
        RecordId::new(),
        9,
        &a_key,
    );
    let mut records = vec![
        TrustRecord::Revocation(revocation),
        TrustRecord::AllDataGrant(b_grant),
        TrustRecord::AllDataGrant(a_grant),
        TrustRecord::Relationship(relationship),
    ];
    sort_for_gossip(&mut records);
    assert!(matches!(records[0], TrustRecord::Relationship(_)));
    assert!(matches!(records[1], TrustRecord::AllDataGrant(_)));
    assert!(matches!(records[2], TrustRecord::AllDataGrant(_)));
    assert!(matches!(records[3], TrustRecord::Revocation(_)));
}

fn lifecycle(
    key: &DeviceSigningKey,
    issuer: &DeviceDescriptorV1,
    profile: ProfileId,
    state: ProfileLifecycleStateV1,
    revision: u64,
    operation: uuid::Uuid,
) -> ProfileLifecycleV1 {
    ProfileLifecycleV1::sign(
        RecordId::new(),
        issuer.device_id,
        profile,
        state,
        revision,
        operation,
        50,
        key,
    )
}

#[test]
fn profile_lifecycle_authenticates_authority_and_ordering_fields() {
    let (key, issuer) = device("Issuer", 1);
    let profile = ProfileId::new();
    let signed = lifecycle(
        &key,
        &issuer,
        profile,
        ProfileLifecycleStateV1::Deleted,
        7,
        uuid::Uuid::new_v4(),
    );
    signed
        .clone()
        .verify(issuer.signing_public_key)
        .expect("the signed lifecycle verifies");

    let mut tampered = Vec::new();
    let mut value = signed.clone();
    value.profile_id = ProfileId::new();
    tampered.push(value);
    let mut value = signed.clone();
    value.issued_by = DeviceId::new();
    tampered.push(value);
    let mut value = signed.clone();
    value.state = ProfileLifecycleStateV1::Active;
    tampered.push(value);
    let mut value = signed.clone();
    value.lifecycle_revision += 1;
    tampered.push(value);
    let mut value = signed.clone();
    value.operation_id = uuid::Uuid::new_v4();
    tampered.push(value);
    let mut value = signed;
    value.issued_at_millis += 1;
    tampered.push(value);
    assert!(tampered.into_iter().all(|value| {
        value.verify(issuer.signing_public_key) == Err(VerifyError::InvalidSignature)
    }));
}

#[test]
fn profile_lifecycle_requires_an_existing_exact_authorization_path() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (relationship, _, _) = pair(&a_key, &a, &b_key, &b);
    let allowed_profile = ProfileId::new();
    let other_profile = ProfileId::new();
    let (a_grant, b_grant) = profile_pair(&relationship, allowed_profile, &a_key, &b_key);
    let mut view = view_of(a, vec![relationship], Vec::new(), Vec::new());
    view.profile_grants = vec![a_grant, b_grant];

    let allowed = lifecycle(
        &b_key,
        &b,
        allowed_profile,
        ProfileLifecycleStateV1::Deleted,
        1,
        uuid::Uuid::new_v4(),
    );
    verify_gossip_record(&view, TrustRecord::ProfileLifecycle(allowed))
        .expect("the reciprocal exact-Profile path authorizes its lifecycle");

    let unauthorized = lifecycle(
        &b_key,
        &b,
        other_profile,
        ProfileLifecycleStateV1::Deleted,
        1,
        uuid::Uuid::new_v4(),
    );
    assert_eq!(
        verify_gossip_record(&view, TrustRecord::ProfileLifecycle(unauthorized)),
        Err(GossipRejection::UnauthorizedProfile)
    );
}

#[test]
fn profile_lifecycle_order_is_hosted_revision_then_operation() {
    let (key, issuer) = device("Issuer", 1);
    let profile = ProfileId::new();
    let operation = uuid::Uuid::new_v4();
    let current = lifecycle(
        &key,
        &issuer,
        profile,
        ProfileLifecycleStateV1::Deleted,
        4,
        operation,
    );
    assert_eq!(
        compare_profile_lifecycle(None, &current),
        ProfileLifecycleOrder::Apply
    );
    let duplicate = lifecycle(
        &key,
        &issuer,
        profile,
        ProfileLifecycleStateV1::Deleted,
        4,
        operation,
    );
    assert_eq!(
        compare_profile_lifecycle(Some(&current), &duplicate),
        ProfileLifecycleOrder::Duplicate
    );
    let stale = lifecycle(
        &key,
        &issuer,
        profile,
        ProfileLifecycleStateV1::Deleted,
        3,
        uuid::Uuid::new_v4(),
    );
    assert_eq!(
        compare_profile_lifecycle(Some(&current), &stale),
        ProfileLifecycleOrder::Stale
    );
    let newer = lifecycle(
        &key,
        &issuer,
        profile,
        ProfileLifecycleStateV1::Active,
        5,
        uuid::Uuid::new_v4(),
    );
    assert_eq!(
        compare_profile_lifecycle(Some(&current), &newer),
        ProfileLifecycleOrder::Apply
    );
    let conflict = lifecycle(
        &key,
        &issuer,
        profile,
        ProfileLifecycleStateV1::Active,
        4,
        uuid::Uuid::new_v4(),
    );
    assert_eq!(
        compare_profile_lifecycle(Some(&current), &conflict),
        ProfileLifecycleOrder::Conflict
    );
}

#[test]
fn deleted_profile_is_removed_from_profile_and_workspace_authorization() {
    let (a_key, a) = device("A", 1);
    let (b_key, b) = device("B", 2);
    let (relationship, a_all, b_all) = pair(&a_key, &a, &b_key, &b);
    let profile = ProfileId::new();
    let workspace = WorkspaceId::new();
    let mut view = view_of(
        a,
        vec![relationship.clone()],
        vec![a_all, b_all],
        Vec::new(),
    );
    let (a_profile, b_profile) = profile_pair(&relationship, profile, &a_key, &b_key);
    view.profile_grants = vec![a_profile, b_profile];
    assert!(matches!(
        Authorization::build(&view, &owned(profile, [workspace])).check(b.endpoint_id, workspace),
        AuthorizationDecision::Allowed(_)
    ));

    view.lifecycles.insert(
        profile,
        lifecycle(
            &b_key,
            &b,
            profile,
            ProfileLifecycleStateV1::Deleted,
            1,
            uuid::Uuid::new_v4(),
        ),
    );
    assert!(matches!(
        Authorization::build(&view, &owned(profile, [workspace])).check(b.endpoint_id, workspace),
        AuthorizationDecision::Denied(_)
    ));
    assert!(
        Authorization::devices(&view)
            .iter()
            .all(|peer| !peer.authorized_profiles.contains(&profile))
    );

    // Each root must independently accept the higher hosted lifecycle before
    // its authorization view resumes sync. Accepting it on one root cannot
    // silently authorize the other.
    let restored = lifecycle(
        &b_key,
        &b,
        profile,
        ProfileLifecycleStateV1::Active,
        2,
        uuid::Uuid::new_v4(),
    );
    let mut root_a = view.clone();
    let mut root_b = view;
    root_a.lifecycles.insert(profile, restored.clone());
    assert!(matches!(
        Authorization::build(&root_a, &owned(profile, [workspace])).check(b.endpoint_id, workspace),
        AuthorizationDecision::Allowed(_)
    ));
    assert!(matches!(
        Authorization::build(&root_b, &owned(profile, [workspace])).check(b.endpoint_id, workspace),
        AuthorizationDecision::Denied(_)
    ));
    root_b.lifecycles.insert(profile, restored);
    assert!(matches!(
        Authorization::build(&root_b, &owned(profile, [workspace])).check(b.endpoint_id, workspace),
        AuthorizationDecision::Allowed(_)
    ));
}
