// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::cloud::object::tests::*;

#[test]
fn five_unit_digests_keys_and_staging_are_exact_frozen_values() {
    for golden in goldens() {
        let m = metadata(&golden["expected"]);
        assert_eq!(
            hex(&m.unit.digest(m.dataset).unwrap()),
            golden["expected"]["unit_digest"]
        );
        assert_eq!(logical_key(m).unwrap(), golden["expected"]["key"]);
        let name = staging_name(m, [0x11; 16]).unwrap();
        let expected = &golden["expected"];
        let token = match expected["unit_kind"].as_u64().unwrap() {
            1 => "manifest".to_owned(),
            2 => expected["subject"].as_str().unwrap().to_owned(),
            3 => "identity".to_owned(),
            4 => "private".to_owned(),
            5 => format!("personal-{}", expected["subject"].as_str().unwrap()),
            _ => unreachable!(),
        };
        assert_eq!(
            name.as_str(),
            format!(
                "staging/{}/{}/{}/{}/{}/{}/{}-{}/{}-11111111111111111111111111111111.tmp",
                if expected["dataset_kind"] == 1 {
                    "w"
                } else {
                    "p"
                },
                expected["authority"].as_str().unwrap(),
                expected["lineage"].as_str().unwrap(),
                token,
                expected["replica"].as_str().unwrap(),
                expected["worker"].as_str().unwrap(),
                expected["lease_id"].as_str().unwrap(),
                expected["lease_epoch"].as_u64().unwrap(),
                expected["object_id"].as_str().unwrap()
            )
        );
        assert!(name.as_str().len() <= STAGING_MAX);
        if m.unit.tag() > 2 {
            assert!(m.unit.profile_unit(m.dataset).is_ok());
        }
    }
}

#[test]
fn dataset_unit_subject_shape_and_non_v4_uuid_compatibility() {
    let mut m = metadata(&goldens()[0]["expected"]);
    assert_eq!(Unit::parse(1, Some(m.object_id)), Err(Error::Unit));
    assert_eq!(Unit::parse(2, None), Err(Error::Unit));
    assert_eq!(Unit::parse(5, None), Err(Error::Unit));
    assert_eq!(Unit::parse(6, None), Err(Error::Unit));
    assert_eq!(Unit::ProfilePrivate.digest(m.dataset), Err(Error::Unit));
    // Envelope UUIDs are raw UUID values; do not import transfer's v4 predicate.
    m.object_id = CloudId::from_bytes([0; 16]);
    assert!(
        logical_key(m)
            .unwrap()
            .ends_with("-00000000-0000-0000-0000-000000000000.loro")
    );
}
