// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::Value;

pub(super) const OBJECTS: &str =
    include_str!("../../../../../contracts/vectors/cloud-loro-object-v1/objects.json");
pub(super) const LORO: &str =
    include_str!("../../../../../contracts/vectors/cloud-loro-object-v1/loro.json");
pub(super) const SEEDS: &str =
    include_str!("../../../../../contracts/vectors/cloud-loro-object-v1/seeds.json");
pub(super) fn limits() -> Limits {
    Limits::new(
        HEADER_MAX,
        VECTOR_MAX,
        super::super::transfer::MAX_PAYLOAD as u64,
        1024 * 1024,
    )
    .unwrap()
}
pub(super) fn bytes(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
pub(super) fn id(text: &str) -> CloudId {
    serde_json::from_value(Value::String(text.into())).unwrap()
}
pub(super) fn hex(value: &[u8]) -> String {
    value.iter().map(|v| format!("{v:02x}")).collect()
}
pub(super) fn goldens() -> Vec<Value> {
    serde_json::from_str(OBJECTS).unwrap()
}
pub(super) fn metadata(value: &Value) -> Metadata {
    let dataset = if value["dataset_kind"] == 1 {
        CloudReplicaPrincipalV1::Workspace {
            workspace_id: id(value["authority"].as_str().unwrap()),
            incarnation_id: id(value["lineage"].as_str().unwrap()),
            logical_replica_id: id(value["replica"].as_str().unwrap()),
        }
    } else {
        CloudReplicaPrincipalV1::Profile {
            profile_id: id(value["authority"].as_str().unwrap()),
            lineage_id: id(value["lineage"].as_str().unwrap()),
            logical_replica_id: id(value["replica"].as_str().unwrap()),
        }
    };
    Metadata {
        kind: if value["object_kind"] == 1 {
            Kind::Checkpoint
        } else {
            Kind::Segment
        },
        dataset,
        unit: Unit::parse(
            value["unit_kind"].as_u64().unwrap() as u8,
            value["subject"].as_str().map(id),
        )
        .unwrap(),
        cloud_unit_id: id(value["unit_id"].as_str().unwrap()),
        object_id: id(value["object_id"].as_str().unwrap()),
        sequence: value["sequence"].as_u64().unwrap(),
        writer_worker_id: id(value["worker"].as_str().unwrap()),
        writer_lease_id: id(value["lease_id"].as_str().unwrap()),
        writer_lease_epoch: value["lease_epoch"].as_u64().unwrap(),
        authority_workspace_id: id(value["authority_workspace"].as_str().unwrap()),
        authorization_version: value["authorization_version"].as_u64().unwrap(),
        membership_epoch: value["membership_epoch"].as_u64().unwrap(),
        cloud_access_epoch: value["cloud_access_epoch"].as_u64().unwrap(),
        covers_through_sequence: value["coverage"].as_u64().unwrap(),
    }
}
pub(super) fn expected(value: &Value) -> ExpectedObject<'_> {
    let h = |key: &str| -> Hash { bytes(value[key].as_str().unwrap()).try_into().unwrap() };
    let optional =
        |key: &str| -> Option<Hash> { value[key].as_str().map(|v| bytes(v).try_into().unwrap()) };
    ExpectedObject {
        metadata: metadata(value),
        logical_key: value["key"].as_str().unwrap(),
        commitments: Commitments {
            unit_ref_digest: h("unit_digest"),
            predecessor_version_digest: optional("predecessor_digest"),
            result_version_digest: h("result_digest"),
            checkpoint_frontier_digest: optional("frontier_digest"),
            payload_sha256: h("payload_sha"),
            object_sha256: h("object_sha"),
            request_digest: h("request_digest"),
            payload_byte_length: value["payload_length"].as_u64().unwrap(),
            object_byte_length: value["object_length"].as_u64().unwrap(),
        },
    }
}
/// Explicit synthetic fixture metadata. Real owners may not derive independent
/// expectations from the same untrusted bytes as this convenience test adapter.
pub(super) fn bind_fixture(bytes: &[u8]) -> BoundObject<'_> {
    let decoded = decode(bytes, limits()).unwrap();
    let key = logical_key(decoded.metadata()).unwrap();
    decoded
        .bind(ExpectedObject {
            metadata: decoded.metadata(),
            logical_key: &key,
            commitments: decoded.commitments(),
        })
        .unwrap()
}
pub(super) fn head(object: BoundObject<'_>) -> HeadCommitment {
    HeadCommitment {
        result_version_digest: object.commitments().result_version_digest,
    }
}

#[test]
fn explicit_limits_never_implicitly_accept_a_checkpoint_default() {
    assert_eq!(
        Limits::new(HEADER_MAX, VECTOR_MAX, 16 * 1024 * 1024 + 1, 1),
        Err(Error::InvalidLimits)
    );
    assert_eq!(
        Limits::new(HEADER_MAX, VECTOR_MAX, 1, CHECKPOINT_REVIEW_MAX + 1),
        Err(Error::InvalidLimits)
    );
    assert!(Limits::new(HEADER_MAX, VECTOR_MAX, 1, CHECKPOINT_REVIEW_MAX).is_ok()); // declaration only
    for candidate in [
        (0, 1, 1, 1),
        (1, 0, 1, 1),
        (1, 1, 0, 1),
        (1, 1, 1, 0),
        (HEADER_MAX + 1, 1, 1, 1),
        (1, VECTOR_MAX + 1, 1, 1),
    ] {
        assert_eq!(
            Limits::new(candidate.0, candidate.1, candidate.2, candidate.3),
            Err(Error::InvalidLimits)
        );
    }
    assert_eq!(
        Error::TooLarge.public_bound_code(),
        Some("cloud_object_too_large")
    );
    assert_eq!(Error::Binding.public_bound_code(), None);
}

#[test]
fn captured_fixture_bytes_and_source_inputs_are_immutable() {
    // Raw bytes, not EOL-normalized. These JSON fixture files are byte inputs.
    for (input, expected) in [
        (
            OBJECTS,
            "a492bb6bf9f8a2d03ad482278debfd28e4db2873d7ec0534fa23d98041afda06",
        ),
        (
            LORO,
            "c783a2df756f6555c358d6f58de4c3b19300c624e34b946f4e82379d34b2b45f",
        ),
        (
            SEEDS,
            "e9d6cc67ed03ae47327258da18122188386937f2c4f4757f77e069edb1ba0211",
        ),
    ] {
        assert_eq!(hex(&hash(input.as_bytes())), expected);
    }
}

#[test]
fn public_debug_contains_no_payload_or_vector_bytes() {
    let golden = goldens().remove(0);
    let raw = bytes(golden["envelope"].as_str().unwrap());
    let decoded = decode(&raw, limits()).unwrap();
    let debug = format!("{decoded:?}");
    assert!(!debug.contains("Fixture edit"));
    assert!(!debug.contains(golden["expected"]["payload"].as_str().unwrap()));
    assert!(!debug.contains(golden["expected"]["result"].as_str().unwrap()));
}
