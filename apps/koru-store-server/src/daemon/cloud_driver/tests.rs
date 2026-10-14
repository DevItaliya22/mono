// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_device::cloud::CloudId;
use koru_sync_control::cloud::CloudWorkspaceDescriptorV1;

fn id(byte: u8) -> CloudId {
    let mut bytes = [byte; 16];
    // Canonical v4 shape, as every permit id must be.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    CloudId::from_bytes(bytes)
}

fn workspace_permit() -> CloudPermitV1 {
    CloudPermitV1 {
        version: 1,
        permit_id: uuid::Uuid::new_v4(),
        dataset: CloudReplicaPrincipalV1::Workspace {
            workspace_id: id(1),
            incarnation_id: id(2),
            logical_replica_id: id(3),
        },
        account_id: id(4),
        profile_id: id(5),
        authority_workspace_id: id(1),
        authority_workspace_incarnation_id: id(2),
        account_generation: 1,
        expires_at_seconds: 600,
        allowed_profile_units: Vec::new(),
        workspace_descriptor: Some(CloudWorkspaceDescriptorV1 {
            version: 1,
            profile_id: id(5),
            workspace_id: id(1),
            workspace_incarnation_id: id(2),
            logical_replica_id: id(3),
            lineage_id: id(2),
            manifest_document_id: id(6),
            authorization_version: 1,
            membership_epoch: 1,
            cloud_access_epoch: 1,
        }),
    }
}

fn profile_permit(units: Vec<ProfileUnitRef>) -> CloudPermitV1 {
    CloudPermitV1 {
        dataset: CloudReplicaPrincipalV1::Profile {
            profile_id: id(5),
            lineage_id: id(7),
            logical_replica_id: id(8),
        },
        allowed_profile_units: units,
        workspace_descriptor: None,
        ..workspace_permit()
    }
}

#[test]
fn a_workspace_permit_plans_its_workspace_under_the_descriptor_manifest() {
    let permit = workspace_permit();
    let (plan, scope) = plan_for(&permit).unwrap();
    assert_eq!(
        plan,
        Plan::Workspace {
            workspace_id: WorkspaceId::from_uuid(id(1).as_uuid()),
        }
    );
    assert_eq!(scope.principal, permit.dataset);
    assert_eq!(scope.manifest, DocumentId::from_uuid(id(6).as_uuid()));
    assert_eq!(scope.profile, id(5));
    assert_eq!(
        scope.eligibility_workspace,
        WorkspaceId::from_uuid(id(1).as_uuid())
    );
}

#[test]
fn a_workspace_permit_without_a_descriptor_or_with_the_wrong_one_is_refused() {
    let mut permit = workspace_permit();
    permit.workspace_descriptor = None;
    assert_eq!(
        plan_for(&permit).map(|_| ()).unwrap_err(),
        "cloud_permit_descriptor_required"
    );
    let mut permit = workspace_permit();
    permit.workspace_descriptor.as_mut().unwrap().workspace_id = id(9);
    assert_eq!(
        plan_for(&permit).map(|_| ()).unwrap_err(),
        "cloud_permit_descriptor_mismatch"
    );
}

#[test]
fn a_profile_permit_plans_exactly_the_units_it_allows() {
    let profile = ProfileId::from_uuid(id(5).as_uuid());
    let workspace = WorkspaceId::from_uuid(id(1).as_uuid());
    let (plan, _) = plan_for(&profile_permit(vec![ProfileUnitRef::Identity { profile }])).unwrap();
    assert_eq!(
        plan,
        Plan::Profile {
            profile_id: profile,
            private: false,
            personal_workspaces: Vec::new(),
        }
    );
    let (plan, scope) = plan_for(&profile_permit(vec![
        ProfileUnitRef::Identity { profile },
        ProfileUnitRef::Private { profile },
        ProfileUnitRef::Personal {
            owner: profile,
            workspace,
        },
    ]))
    .unwrap();
    assert_eq!(
        plan,
        Plan::Profile {
            profile_id: profile,
            private: true,
            personal_workspaces: vec![workspace],
        }
    );
    assert_eq!(scope.manifest, DocumentId::from_uuid(uuid::Uuid::nil()));

    // A Personal unit of an unrelated Workspace, or another Profile's unit,
    // is not narrowed down: the whole permit is refused.
    let foreign = WorkspaceId::from_uuid(id(9).as_uuid());
    assert_eq!(
        plan_for(&profile_permit(vec![ProfileUnitRef::Personal {
            owner: profile,
            workspace: foreign,
        }]))
        .map(|_| ())
        .unwrap_err(),
        "cloud_permit_profile_units"
    );
    let other = ProfileId::from_uuid(id(9).as_uuid());
    assert_eq!(
        plan_for(&profile_permit(vec![ProfileUnitRef::Identity {
            profile: other
        }]))
        .map(|_| ())
        .unwrap_err(),
        "cloud_permit_profile_units"
    );
}

// ------------------------------------------- the document half of coverage

fn document_unit(byte: u8) -> Unit {
    Unit::Document {
        workspace_id: id(1),
        document_id: id(byte),
    }
}

fn a_state(counter: i32) -> Vec<u8> {
    let mut version = loro::VersionVector::default();
    version.set_last(loro::ID::new(1, counter));
    version.encode()
}

/// **What coverage is computed against, and nothing else.**
///
/// A document the pass carried whole arrives with the exact state it was
/// carried at; one it deferred, refused or never read arrives as `None` —
/// required, and Cloud does not have it. A Profile unit is not a document and
/// contributes nothing at all.
#[test]
fn the_document_half_of_coverage_is_what_the_pass_carried() {
    let carried = document_unit(10);
    let held = document_unit(11);
    let unread = document_unit(12);
    let outcome = cloud_sync::Outcome {
        documents: vec![
            cloud_sync::DocumentPass {
                unit: carried,
                state: a_state(7),
                carried: true,
            },
            // Settled, but its upload was held: Cloud does not have this state.
            cloud_sync::DocumentPass {
                unit: held,
                state: a_state(8),
                carried: false,
            },
            // Carried, and the version read never happened. An empty state is
            // the empty version vector, which every state includes — reporting
            // it as covered would be claiming coverage out of a missing read.
            cloud_sync::DocumentPass {
                unit: unread,
                state: Vec::new(),
                carried: true,
            },
        ],
        ..cloud_sync::Outcome::default()
    };

    let documents = covered_documents(&outcome);
    assert_eq!(documents.len(), 3, "a required document was dropped");
    assert_eq!(
        documents
            .get(&DocumentId::from_uuid(id(10).as_uuid()))
            .expect("the carried document")
            .as_ref()
            .map(|state| state.as_bytes().to_vec()),
        Some(a_state(7)),
    );
    assert_eq!(
        documents.get(&DocumentId::from_uuid(id(11).as_uuid())),
        Some(&None),
        "a held upload was reported as covered"
    );
    assert_eq!(
        documents.get(&DocumentId::from_uuid(id(12).as_uuid())),
        Some(&None),
        "a document whose state was never read was reported as covered"
    );
}

/// A Profile dataset carries no documents, so it contributes no coverage
/// input: Identity, Private and Personal are facts about the dataset and about
/// no document at all.
#[test]
fn a_pass_that_carried_no_documents_contributes_no_coverage_input() {
    assert!(covered_documents(&cloud_sync::Outcome::default()).is_empty());
}
