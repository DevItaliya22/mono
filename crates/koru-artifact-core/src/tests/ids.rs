// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn ids_round_trip_through_their_storage_spelling() {
    let family = family_id(7);
    assert_eq!(
        family.to_storage().parse::<ArtifactFamilyId>().unwrap(),
        family
    );
    let registration = RegistrationId::mint();
    assert_eq!(
        registration.to_storage().parse::<RegistrationId>().unwrap(),
        registration
    );
    assert_eq!(family.to_string(), family.to_storage());
}

#[test]
fn ids_are_strict_v4_in_canonical_spelling() {
    assert!(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
            .parse::<ArtifactFamilyId>()
            .is_ok()
    );
    assert_eq!(
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA".parse::<ArtifactFamilyId>(),
        Err(IdParseError::InvalidSpelling)
    );
    assert_eq!(
        "00000000-0000-1000-8000-000000000001".parse::<RegistrationId>(),
        Err(IdParseError::NotVersion4)
    );
    assert_eq!(
        "not-an-id".parse::<ArtifactFamilyId>(),
        Err(IdParseError::InvalidSpelling)
    );
}

#[test]
fn debug_is_a_prefix_and_eight_characters() {
    assert_eq!(format!("{:?}", family_id(7)), "family(00000000)");
    assert_eq!(
        format!("{:?}", RegistrationId::from_uuid(v4(1))),
        "registration(00000000)"
    );
}

#[test]
fn minted_ids_are_distinct_and_version_four() {
    let a = ArtifactFamilyId::mint();
    let b = ArtifactFamilyId::mint();
    assert_ne!(a, b);
    assert!(a.to_storage().parse::<ArtifactFamilyId>().is_ok());
}

#[test]
fn an_ordinal_starts_at_one_and_never_wraps() {
    assert_eq!(Ordinal::FIRST.get(), 1);
    assert_eq!(Ordinal::FIRST.next().get(), 2);
    assert_eq!(Ordinal::from_raw(0), None);
    assert_eq!(Ordinal::from_raw(u32::MAX).unwrap().next().get(), u32::MAX);
}
