// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn cloud_id_display_preserves_existing_canonical_uuid_grammar() {
    for text in [
        "00000000-0000-0000-0000-000000000000",
        "abcdefab-cdef-1abc-8def-abcdefabcdef",
    ] {
        let id: CloudId = serde_json::from_str(&format!("\"{text}\"")).unwrap();
        assert_eq!(id.to_string(), text);
        assert_eq!(serde_json::to_value(id).unwrap(), text);
    }
}

#[test]
fn frozen_principals_are_strict_distinct_and_never_devices_ds_d12() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../contracts/vectors/cloud-sync-admission-v1/foundation.json"
    ))
    .unwrap();
    for session in vectors["sessions"].as_array().unwrap() {
        let dataset = session["dataset"].clone();
        let principal: CloudReplicaPrincipalV1 = serde_json::from_value(dataset.clone()).unwrap();
        assert_eq!(serde_json::to_value(principal).unwrap(), dataset);
        assert_eq!(
            principal.admit_on_device_path(),
            Err("principal_not_allowed")
        );
        let mut foreign = dataset;
        foreign["device_id"] = serde_json::json!("11111111-1111-1111-1111-111111111111");
        assert!(serde_json::from_value::<CloudReplicaPrincipalV1>(foreign).is_err());
    }
    for invalid in [
        "null",
        r#"{"kind":"workspace"}"#,
        r#"{"kind":"profile","profile_id":"AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA","lineage_id":"11111111-1111-1111-1111-111111111111","logical_replica_id":"11111111-1111-1111-1111-111111111111"}"#,
    ] {
        assert!(serde_json::from_str::<CloudReplicaPrincipalV1>(invalid).is_err());
    }
}
