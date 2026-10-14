// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::cloud::object::Unit as Object;

fn workspace_scope() -> Scope {
    Scope {
        principal: CloudReplicaPrincipalV1::Workspace {
            workspace_id: id(1),
            incarnation_id: id(3),
            logical_replica_id: id(4),
        },
        eligibility_workspace: ws(),
        profile: id(5),
        manifest: doc(),
    }
}

#[test]
fn five_unit_mappings_keep_standard_loro_ack_rooms_and_correlation() {
    let workspace = workspace_scope();
    let profile = Scope {
        principal: CloudReplicaPrincipalV1::Profile {
            profile_id: id(5),
            lineage_id: id(6),
            logical_replica_id: id(7),
        },
        ..workspace
    };
    let cases = [
        (workspace, unit(), Object::WorkspaceManifest),
        (
            workspace,
            Unit::Document {
                workspace_id: id(1),
                document_id: id(8),
            },
            Object::WorkspaceDocument { document_id: id(8) },
        ),
        (
            profile,
            Unit::Identity { profile_id: id(5) },
            Object::ProfileIdentity,
        ),
        (
            profile,
            Unit::Private { profile_id: id(5) },
            Object::ProfilePrivate,
        ),
        (
            profile,
            Unit::Personal {
                profile_id: id(5),
                workspace_id: id(1),
            },
            Object::ProfilePersonal {
                workspace_id: id(1),
            },
        ),
    ];
    for (scope, unit, expected) in cases {
        assert_eq!(scope.object_unit(unit).unwrap(), expected);
        let Record::Peer(PeerFrame::Loro {
            workspace_id,
            message,
        }) = scope.committed_ack(unit, [19; 8]).unwrap()
        else {
            panic!("ordinary metadata frame expected")
        };
        assert_eq!(workspace_id, ws());
        let loro_protocol::ProtocolMessage::Ack {
            crdt,
            room_id,
            ref_id,
            status,
        } = loro_protocol::encoding::try_decode(&message).unwrap()
        else {
            panic!("standard Loro Ack expected")
        };
        assert!(matches!(crdt, loro_protocol::CrdtType::Loro));
        assert_eq!(room_id, scope.room(unit).unwrap());
        assert_eq!(ref_id.0, [19; 8]);
        assert!(matches!(status, loro_protocol::UpdateStatusCode::Ok));
    }
}

#[test]
fn catalog_mapping_and_ack_refuse_a_unit_outside_the_admitted_scope() {
    let scope = workspace_scope();
    for unit in [
        Unit::Document {
            workspace_id: id(9),
            document_id: id(2),
        },
        Unit::Identity { profile_id: id(5) },
        Unit::Private { profile_id: id(5) },
        Unit::Personal {
            profile_id: id(5),
            workspace_id: id(1),
        },
    ] {
        assert!(scope.object_unit(unit).is_err());
        assert!(scope.committed_ack(unit, [0; 8]).is_err());
    }
}
