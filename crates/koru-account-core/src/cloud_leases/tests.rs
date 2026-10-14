// SPDX-License-Identifier: MPL-2.0

use super::*;

const VECTORS: &str =
    include_str!("../../../../contracts/vectors/cloud-replica-runtime-v1/plan12.json");

fn uuid(value: &str) -> Uuid {
    value.parse().unwrap()
}

fn request() -> LeaseRequestV1 {
    LeaseRequestV1 {
        operation_id: uuid("66666666-6666-4666-8666-666666666666"),
        logical_replica_id: uuid("11111111-1111-4111-8111-111111111111"),
        authority: DatasetAuthorityV1::Workspace {
            workspace_id: WorkspaceId::from_uuid(uuid("22222222-2222-4222-8222-222222222222")),
            incarnation_id: uuid("33333333-3333-4333-8333-333333333333"),
            cloud_access_epoch: 1,
        },
        worker_id: uuid("44444444-4444-4444-8444-444444444444"),
        command: LeaseCommandV1::Renew {
            expected: ExpectedLeaseV1 {
                lease_id: uuid("55555555-5555-4555-8555-555555555555"),
                lease_epoch: 1,
                lease_expires_at_millis: 31000,
            },
        },
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn production_digest_matches_all_frozen_workspace_and_profile_vectors() {
    let vectors: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
    for vector in vectors["digest_vectors"].as_array().unwrap() {
        let mut request = request();
        if vector["dataset"] == "profile" {
            request.authority = DatasetAuthorityV1::Profile {
                profile_id: ProfileId::from_uuid(uuid("22222222-2222-4222-8222-222222222222")),
                lineage_id: request.authority.lineage_id(),
                authority_workspace_id: WorkspaceId::from_uuid(uuid(
                    "77777777-7777-4777-8777-777777777777",
                )),
                authorization_version: 1,
                membership_epoch: 1,
                cloud_access_epoch: 1,
            };
        }
        let expected = request.command.expected_lease().unwrap();
        request.command = match vector["kind"].as_str().unwrap() {
            "place" => LeaseCommandV1::Place,
            "acquire" => LeaseCommandV1::Acquire { expected_epoch: 0 },
            "renew" => LeaseCommandV1::Renew { expected },
            "release" => LeaseCommandV1::Release { expected },
            other => panic!("unimplemented vector kind {other}"),
        };
        assert_eq!(
            hex(&request.canonical_bytes().unwrap()),
            vector["canonical_hex"],
            "{}",
            vector["id"]
        );
        assert_eq!(
            hex(&request.digest().unwrap()),
            vector["sha256"],
            "{}",
            vector["id"]
        );
    }
}

#[test]
fn expected_expiry_is_bound_but_operation_lookup_key_is_not() {
    let mut request = request();
    let digest = request.digest().unwrap();
    request.operation_id = uuid("88888888-8888-4888-8888-888888888888");
    assert_eq!(digest, request.digest().unwrap());
    let mut expected = request.command.expected_lease().unwrap();
    expected.lease_expires_at_millis += 1;
    request.command = LeaseCommandV1::Renew { expected };
    assert_ne!(digest, request.digest().unwrap());
}

#[test]
fn invalid_integer_domain_and_nonpositive_fences_never_encode() {
    for value in [0, MAX_LEASE_INTEGER + 1, u64::MAX] {
        let mut request = request();
        let mut expected = request.command.expected_lease().unwrap();
        expected.lease_epoch = value;
        request.command = LeaseCommandV1::Renew { expected };
        assert_eq!(request.digest(), Err(InvalidLeaseRequest::IntegerDomain));
        expected.lease_epoch = 1;
        expected.lease_expires_at_millis = value;
        request.command = LeaseCommandV1::Release { expected };
        assert_eq!(request.digest(), Err(InvalidLeaseRequest::IntegerDomain));
    }
    let mut request = request();
    request.command = LeaseCommandV1::Acquire {
        expected_epoch: MAX_LEASE_INTEGER,
    };
    assert!(request.digest().is_ok());
    request.command = LeaseCommandV1::Acquire {
        expected_epoch: MAX_LEASE_INTEGER + 1,
    };
    assert_eq!(request.digest(), Err(InvalidLeaseRequest::IntegerDomain));
}

#[test]
fn invalid_uuid_and_authority_epochs_are_rejected() {
    let mut request = request();
    request.operation_id = Uuid::nil();
    assert_eq!(request.digest(), Err(InvalidLeaseRequest::Uuid));
    request.operation_id = uuid("66666666-6666-4666-8666-666666666666");
    if let DatasetAuthorityV1::Workspace {
        ref mut cloud_access_epoch,
        ..
    } = request.authority
    {
        *cloud_access_epoch = 0;
    }
    assert_eq!(request.digest(), Err(InvalidLeaseRequest::IntegerDomain));
}

#[test]
fn acquire_zero_is_valid_and_place_has_no_lease_fence() {
    let mut request = request();
    request.command = LeaseCommandV1::Acquire { expected_epoch: 0 };
    assert!(request.digest().is_ok());
    request.command = LeaseCommandV1::Place;
    assert_eq!(request.command.expected_epoch(), 0);
    assert_eq!(request.command.expected_lease(), None);
    assert_eq!(LEASE_DURATION_MILLIS, 30_000);
    assert_eq!(LEASE_RENEW_INTERVAL_MILLIS, 10_000);
}
