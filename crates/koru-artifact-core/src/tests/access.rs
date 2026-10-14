// SPDX-License-Identifier: MPL-2.0

use super::*;

fn grant(principal: Principal, access: Access) -> Permission {
    Permission {
        family: family_id(1),
        principal,
        access,
        granted_at_ms: 1,
    }
}

#[test]
fn owner_permits_everything_and_read_permits_only_reading() {
    assert!(Access::Owner.permits(Access::Owner));
    assert!(Access::Owner.permits(Access::Read));
    assert!(Access::Read.permits(Access::Read));
    assert!(!Access::Read.permits(Access::Owner));
}

#[test]
fn authorize_decides_by_the_principal_and_the_access_held() {
    let owner = Principal::LocalOwner;
    let reader = Principal::Profile {
        profile: ProfileId::from_uuid(v4(9)),
    };
    let stranger = Principal::Run { run: run_id(8) };
    let permissions = [grant(owner, Access::Owner), grant(reader, Access::Read)];

    assert_eq!(authorize(&permissions, owner, Access::Owner), Ok(()));
    assert_eq!(authorize(&permissions, owner, Access::Read), Ok(()));
    assert_eq!(authorize(&permissions, reader, Access::Read), Ok(()));
    assert_eq!(
        authorize(&permissions, reader, Access::Owner),
        Err(ArtifactError::PermissionDenied)
    );
    assert_eq!(
        authorize(&permissions, stranger, Access::Read),
        Err(ArtifactError::PermissionDenied)
    );
    assert_eq!(
        authorize(&[], owner, Access::Read),
        Err(ArtifactError::PermissionDenied),
        "no grant, no access — even for the owner principal"
    );
}

#[test]
fn only_the_local_owner_is_reachable_in_m1() {
    assert!(Principal::LocalOwner.reachable());
    for unreachable in [
        Principal::Profile {
            profile: ProfileId::from_uuid(v4(1)),
        },
        Principal::Device {
            device: DeviceId::from_uuid(v4(2)),
        },
        Principal::Run { run: run_id(3) },
    ] {
        assert!(!unreachable.reachable(), "{unreachable:?} is M7 vocabulary");
    }
}

#[test]
fn a_principal_round_trips_through_its_storage_token() {
    let all = [
        Principal::LocalOwner,
        Principal::Profile {
            profile: ProfileId::from_uuid(v4(1)),
        },
        Principal::Device {
            device: DeviceId::from_uuid(v4(2)),
        },
        Principal::Run { run: run_id(3) },
    ];
    for principal in all {
        let stored = principal.to_storage();
        assert_eq!(Principal::parse(&stored), Ok(principal), "{stored}");
    }
    assert_eq!(Principal::parse("local").unwrap().to_storage(), "local");
    assert_eq!(Principal::parse("owner"), Err(PrincipalParseError));
    assert_eq!(Principal::parse("profile:nope"), Err(PrincipalParseError));
    assert_eq!(Principal::parse("group:1"), Err(PrincipalParseError));
}

#[test]
fn access_and_role_tokens_are_the_checked_domains() {
    for access in [Access::Owner, Access::Read] {
        assert_eq!(Access::parse(access.as_str()), Some(access));
    }
    assert_eq!(Access::parse("write"), None);
    for role in [LinkRole::Input, LinkRole::Output] {
        assert_eq!(LinkRole::parse(role.as_str()), Some(role));
    }
    assert_eq!(LinkRole::parse("consumed"), None);
}
