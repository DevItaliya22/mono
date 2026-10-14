// SPDX-License-Identifier: MPL-2.0

use super::*;

const ID: &str = "10000000-0000-4000-8000-000000000001";

#[test]
fn the_binding_has_one_exact_storage_spelling() {
    let binding = ProfileAccountBinding::parse_identity_id(ID).unwrap();
    assert_eq!(binding.identity_id(), ID);
    assert_eq!(binding.to_storage(), format!("koru:{ID}"));
    assert_eq!(
        ProfileAccountBinding::parse_storage(&format!("koru:{ID}")).unwrap(),
        binding
    );
}

#[test]
fn names_emails_provider_ids_and_noncanonical_uuids_are_never_inferred() {
    for invalid in [
        "steven@example.com",
        "google:10000000-0000-4000-8000-000000000001",
        "10000000-0000-1000-8000-000000000001",
        "10000000-0000-4000-8000-000000000001 ",
        "10000000-0000-4000-8000-00000000000A",
    ] {
        assert_eq!(
            ProfileAccountBinding::parse_identity_id(invalid)
                .unwrap_err()
                .code(),
            Code::InvalidId,
            "{invalid}"
        );
    }
}

#[test]
fn a_stored_binding_requires_the_koru_prefix_and_canonical_identity() {
    for invalid in [
        ID,
        "google:10000000-0000-4000-8000-000000000001",
        "koru:steven@example.com",
        "koru:10000000-0000-1000-8000-000000000001",
    ] {
        assert_eq!(
            ProfileAccountBinding::parse_storage(invalid)
                .unwrap_err()
                .code(),
            Code::CorruptState,
            "{invalid}"
        );
    }
}
