// SPDX-License-Identifier: MPL-2.0

use super::*;

const DEVICE: &str = "30000000-0000-4000-8000-000000000003";
const ACCOUNT: &str = "10000000-0000-4000-8000-000000000001";

#[test]
fn provider_actor_is_separate_opaque_and_preserves_installation_scope() {
    let make = |root: &str| {
        CredentialContext::new("nightly", Path::new(root), DeviceId::parse(DEVICE).unwrap())
            .unwrap()
            .entry("https://api.x.com")
            .unwrap()
    };
    let base = make("synthetic-root");
    let actor = base.for_provider_actor("123456789").unwrap();
    assert_eq!(
        actor,
        make("synthetic-root")
            .for_provider_actor("123456789")
            .unwrap()
    );
    assert_ne!(
        actor,
        make("other-root").for_provider_actor("123456789").unwrap()
    );
    assert_ne!(actor, base.for_provider_actor("987654321").unwrap());
    assert_ne!(actor, base.for_account(ACCOUNT).unwrap());
    assert!(!actor.account().contains("123456789"));
    assert!(actor.account().len() < 200);
    assert!(actor.for_provider_actor("123456789").is_err());
    for invalid in ["", "an actor", "\n", "tab\tactor"] {
        assert!(base.for_provider_actor(invalid).is_err());
    }
}

#[test]
fn account_key_extends_exact_legacy_components_and_namespace_is_scope_isolated() {
    let make = |channel: &str, root: &str, device: &str, origin: &str| {
        CredentialContext::new(channel, Path::new(root), DeviceId::parse(device).unwrap())
            .unwrap()
            .entry(origin)
            .unwrap()
    };
    let legacy = make(
        "nightly",
        "/injected/root",
        DEVICE,
        "https://account.invalid",
    );
    let entry = legacy.for_account(ACCOUNT).unwrap();
    assert_eq!(
        entry.account(),
        format!("v2:{}:{ACCOUNT}", &legacy.account()[3..])
    );
    assert_eq!(legacy.account().split(':').collect::<Vec<_>>()[4], DEVICE);
    assert_eq!(
        legacy.namespace(),
        make(
            "nightly",
            "/injected/root",
            DEVICE,
            "https://account.invalid/"
        )
        .namespace()
    );
    for other in [
        make(
            "stable",
            "/injected/root",
            DEVICE,
            "https://account.invalid",
        ),
        make(
            "nightly",
            "/injected/other",
            DEVICE,
            "https://account.invalid",
        ),
        make(
            "nightly",
            "/injected/root",
            "30000000-0000-4000-8000-000000000004",
            "https://account.invalid",
        ),
        make("nightly", "/injected/root", DEVICE, "https://other.invalid"),
    ] {
        assert_ne!(legacy.namespace(), other.namespace());
        assert_ne!(entry, other.for_account(ACCOUNT).unwrap());
    }
    assert_ne!(
        entry,
        legacy
            .for_account("10000000-0000-4000-8000-000000000002")
            .unwrap()
    );
    assert!(entry.for_account(ACCOUNT).is_err());
    assert_eq!(format!("{entry:?}"), "CredentialEntry(REDACTED)");
}

#[test]
fn new_account_coordinate_is_canonical_v4_and_matches_android_bounds() {
    let legacy = CredentialContext::new(
        &"a".repeat(40),
        Path::new("/injected/root"),
        DeviceId::parse(DEVICE).unwrap(),
    )
    .unwrap()
    .entry("https://account.invalid")
    .unwrap();
    assert_eq!(legacy.account().len(), 210);
    assert_eq!(legacy.for_account(ACCOUNT).unwrap().account().len(), 247);
    for invalid in [
        "name@example.com",
        "10000000-0000-1000-8000-000000000001",
        "10000000-0000-4000-0000-000000000001",
        "../escape",
        "",
    ] {
        assert!(legacy.for_account(invalid).is_err());
    }
}
