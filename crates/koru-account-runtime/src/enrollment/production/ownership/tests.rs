// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_account_wire::{AccessToken, AuthIdentitySummaryV1, Version1};
use koru_store_wire::ProfileLifecycleInventoryItem;

fn credential(account: u8, auth: u8) -> EnrollmentAuthorization {
    EnrollmentAuthorization {
        access: AccessToken("fixture-access".into()),
        namespace: "a".repeat(64),
        record_version: 1,
        session_id: "50000000-0000-4000-8000-000000000001".into(),
        owner: AccountOwner {
            account: format!("10000000-0000-4000-8000-{account:012}")
                .parse()
                .unwrap(),
            auth_identity: format!("20000000-0000-4000-8000-{auth:012}")
                .parse()
                .unwrap(),
        },
    }
}
fn profile() -> ProfileId {
    ProfileId::parse("30000000-0000-4000-8000-000000000001").unwrap()
}
fn device() -> DeviceId {
    DeviceId::parse("40000000-0000-4000-8000-000000000001").unwrap()
}
fn session(credential: &EnrollmentAuthorization) -> SessionSummaryV1 {
    SessionSummaryV1 {
        version: Version1,
        session_id: "50000000-0000-4000-8000-000000000001".parse().unwrap(),
        koru_identity_id: credential.owner.account,
        auth_identity_id: credential.owner.auth_identity,
        device_id: Some(device()),
        expires_at_millis: 100_000,
        recent_auth_at_millis: 1,
    }
}
fn hosted(credential: &EnrollmentAuthorization) -> ProfileSummaryV1 {
    ProfileSummaryV1 {
        version: Version1,
        koru_identity_id: credential.owner.account,
        profile_id: profile(),
        name_snapshot: None,
        locale_snapshot: None,
        auth_identities: vec![AuthIdentitySummaryV1 {
            version: Version1,
            auth_identity_id: credential.owner.auth_identity,
            provider: koru_account_core::AuthProvider::Google,
            verified_email_snapshot: None,
        }],
        lifecycle_revision: Default::default(),
    }
}
fn inventory(binding: Option<String>) -> ProfileLifecycleInventory {
    ProfileLifecycleInventory {
        profiles: binding
            .into_iter()
            .map(|account_binding| ProfileLifecycleInventoryItem {
                profile_id: profile(),
                account_binding,
                removed_at_millis: None,
            })
            .collect(),
    }
}
fn ready(repository: &mut SqliteTrustRepository, credential: &mut EnrollmentAuthorization) {
    let record = repository
        .begin_account_write(&credential.namespace, credential.owner)
        .unwrap();
    credential.record_version = record.version;
    repository
        .finish_account_write(
            &credential.namespace,
            &record,
            credential.owner,
            true,
            false,
        )
        .unwrap();
}

#[test]
fn delayed_old_load_cannot_clear_or_replace_new_ready_authorization() {
    let old = credential(1, 1);
    let mut new = old.clone();
    new.record_version = old.record_version + 2;
    new.session_id = "50000000-0000-4000-8000-000000000099".into();
    let verified = |credential: &EnrollmentAuthorization| {
        let mut current = session(credential);
        current.session_id = credential.session_id.parse().unwrap();
        AuthorizedProfiles::verify(
            credential,
            device(),
            &current,
            &[hosted(credential)],
            &inventory(None),
        )
        .unwrap()
    };
    let mut cache = AuthorizationCache::default();
    cache.begin(&old).unwrap();
    let delayed = verified(&old);
    // The new ready incarnation finishes while the older hosted load waits.
    cache.begin(&new).unwrap();
    cache.publish(verified(&new)).unwrap();
    assert!(cache.allows(&new, profile()));
    cache.begin(&new).unwrap();
    assert!(
        cache.allows(&new, profile()),
        "same-authority revalidation must not manufacture revocation"
    );
    assert_eq!(cache.publish(delayed), Err(Failure::Persistence));
    assert_eq!(cache.begin(&old), Err(Failure::Persistence));
    assert!(cache.allows(&new, profile()));
    assert!(!cache.allows(&old, profile()));
    let mut substituted = new.clone();
    substituted.session_id = old.session_id.clone();
    assert!(!cache.allows(&substituted, profile()));
    assert_eq!(cache.begin(&substituted), Err(Failure::Persistence));
    assert_eq!(
        cache.publish(verified(&substituted)),
        Err(Failure::Persistence)
    );
    substituted = new.clone();
    substituted.owner.auth_identity = credential(1, 2).owner.auth_identity;
    assert!(!cache.allows(&substituted, profile()));
    assert_eq!(cache.begin(&substituted), Err(Failure::Persistence));

    // Even if a newer load fails before publication, an older ACK cannot
    // restore authorization behind that ready revision fence.
    let mut newest = new.clone();
    newest.record_version += 1;
    cache.begin(&newest).unwrap();
    let mut substituted_pending = newest.clone();
    substituted_pending.session_id = old.session_id.clone();
    assert_eq!(cache.begin(&substituted_pending), Err(Failure::Persistence));
    assert_eq!(cache.publish(verified(&new)), Err(Failure::Persistence));
    assert!(!cache.allows(&new, profile()));
    assert!(!cache.allows(&newest, profile()));

    let other = credential(2, 2);
    cache.begin(&other).unwrap();
    cache.publish(verified(&other)).unwrap();
    assert!(cache.allows(&other, profile()));
}

#[test]
fn legacy_claim_requires_ready_directory_exact_session_grant_and_matching_local_binding() {
    let credential = credential(1, 1);
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    repository.set_enrollment_target(Some(profile())).unwrap();
    let verified = AuthorizedProfiles::verify(
        &credential,
        device(),
        &session(&credential),
        &[hosted(&credential)],
        &inventory(Some(format!("koru:{}", credential.owner.account))),
    )
    .unwrap();
    let pending = repository
        .begin_account_write(&credential.namespace, credential.owner)
        .unwrap();
    assert_eq!(
        verified.claim_and_load(&mut repository),
        Err(Failure::Persistence)
    );
    assert_eq!(repository.enrollment_target().unwrap(), Some(profile()));
    repository
        .finish_account_write(
            &credential.namespace,
            &pending,
            credential.owner,
            true,
            false,
        )
        .unwrap();
    assert_eq!(
        verified.claim_and_load(&mut repository).unwrap(),
        BTreeSet::from([profile()])
    );
    assert_eq!(repository.enrollment_target().unwrap(), None);
    assert_eq!(
        verified.claim_and_load(&mut repository).unwrap(),
        BTreeSet::from([profile()])
    );
}

#[test]
fn signing_in_to_another_account_or_auth_identity_does_not_adopt_legacy_work() {
    let original = credential(1, 1);
    for mut next in [credential(2, 2), credential(1, 2)] {
        let mut repository = SqliteTrustRepository::in_memory().unwrap();
        repository.set_enrollment_target(Some(profile())).unwrap();
        ready(&mut repository, &mut next);
        let mut row = hosted(&next);
        row.auth_identities = hosted(&original).auth_identities;
        let verified =
            AuthorizedProfiles::verify(&next, device(), &session(&next), &[row], &inventory(None))
                .unwrap();
        assert!(!verified.allows(&next, profile()));
        assert!(verified.claim_and_load(&mut repository).unwrap().is_empty());
        assert_eq!(repository.enrollment_target().unwrap(), Some(profile()));
    }
}

#[test]
fn session_account_auth_device_and_profile_owner_substitution_fail_closed() {
    let expected = credential(1, 1);
    for altered in [
        SessionSummaryV1 {
            session_id: "50000000-0000-4000-8000-000000000002".parse().unwrap(),
            ..session(&expected)
        },
        session(&credential(2, 1)),
        session(&credential(1, 2)),
        SessionSummaryV1 {
            device_id: None,
            ..session(&expected)
        },
        SessionSummaryV1 {
            device_id: Some(DeviceId::parse("40000000-0000-4000-8000-000000000002").unwrap()),
            ..session(&expected)
        },
    ] {
        assert!(matches!(
            AuthorizedProfiles::verify(
                &expected,
                device(),
                &altered,
                &[hosted(&expected)],
                &inventory(None)
            ),
            Err(Failure::SessionExpired)
        ));
    }
    let verified = AuthorizedProfiles::verify(
        &expected,
        device(),
        &session(&expected),
        &[hosted(&expected)],
        &inventory(None),
    )
    .unwrap();
    let mut replaced_session = expected.clone();
    replaced_session.session_id = "50000000-0000-4000-8000-000000000002".into();
    assert!(!verified.allows(&replaced_session, profile()));
    assert!(matches!(
        AuthorizedProfiles::verify(
            &expected,
            device(),
            &session(&expected),
            &[hosted(&credential(2, 1))],
            &inventory(None)
        ),
        Err(Failure::AccessRevoked)
    ));
}

#[test]
fn canonical_local_contradiction_and_wrong_namespace_never_claim_or_erase() {
    let mut expected = credential(1, 1);
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    repository.set_enrollment_target(Some(profile())).unwrap();
    ready(&mut repository, &mut expected);
    for binding in [
        format!("koru:{}", credential(2, 2).owner.account),
        "koru:malformed".into(),
    ] {
        let verified = AuthorizedProfiles::verify(
            &expected,
            device(),
            &session(&expected),
            &[hosted(&expected)],
            &inventory(Some(binding)),
        )
        .unwrap();
        assert!(!verified.allows(&expected, profile()));
        assert!(verified.claim_and_load(&mut repository).unwrap().is_empty());
        assert_eq!(repository.enrollment_target().unwrap(), Some(profile()));
    }
    let mut other_namespace = expected.clone();
    other_namespace.namespace = "b".repeat(64);
    let verified = AuthorizedProfiles::verify(
        &other_namespace,
        device(),
        &session(&expected),
        &[hosted(&expected)],
        &inventory(None),
    )
    .unwrap();
    assert_eq!(
        verified.claim_and_load(&mut repository),
        Err(Failure::Persistence)
    );
    assert_eq!(repository.enrollment_target().unwrap(), Some(profile()));
}

#[test]
fn account_directory_auth_replacement_invalidates_old_claim_evidence() {
    let mut previous = credential(1, 1);
    let mut next = credential(1, 2);
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    repository.set_enrollment_target(Some(profile())).unwrap();
    ready(&mut repository, &mut previous);
    let verified = AuthorizedProfiles::verify(
        &previous,
        device(),
        &session(&previous),
        &[hosted(&previous)],
        &inventory(None),
    )
    .unwrap();
    ready(&mut repository, &mut next);
    assert_eq!(
        verified.claim_and_load(&mut repository),
        Err(Failure::Persistence)
    );
    assert_eq!(repository.enrollment_target().unwrap(), Some(profile()));
    assert!(!verified.allows(&next, profile()));
}

#[test]
fn cancellation_after_account_removal_is_idempotent_and_preserves_other_accounts() {
    let mut original = credential(1, 1);
    let mut other = credential(2, 2);
    let expected = BTreeSet::from([profile()]);
    let empty = BTreeSet::new();
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    for owner in [&mut original, &mut other] {
        ready(&mut repository, owner);
        repository
            .replace_account_targets(
                &owner.namespace,
                owner.owner,
                owner.record_version,
                &empty,
                &expected,
            )
            .unwrap();
    }
    let removed = repository
        .account_directory(&original.namespace)
        .unwrap()
        .records
        .into_iter()
        .find(|record| record.owner == original.owner)
        .unwrap();
    repository
        .remove_account(&original.namespace, &removed)
        .unwrap();
    for _ in 0..2 {
        repository
            .replace_account_targets(
                &original.namespace,
                original.owner,
                original.record_version,
                &expected,
                &empty,
            )
            .unwrap();
    }
    assert!(
        repository
            .account_targets(&original.namespace, original.owner.account)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repository
            .account_targets(&other.namespace, other.owner.account)
            .unwrap(),
        expected
    );
    assert_eq!(
        repository
            .account_directory(&original.namespace)
            .unwrap()
            .records
            .len(),
        1
    );
}

#[test]
fn cancellation_refuses_recreated_accounts_even_with_identical_owner_and_target_set() {
    let mut original = credential(1, 1);
    let expected = BTreeSet::from([profile()]);
    let empty = BTreeSet::new();
    let different = ProfileId::parse("30000000-0000-4000-8000-000000000002").unwrap();
    for (mut replacement, next) in [
        (credential(1, 2), expected.clone()),
        (credential(1, 1), BTreeSet::from([different])),
        (credential(1, 1), expected.clone()),
    ] {
        let mut repository = SqliteTrustRepository::in_memory().unwrap();
        ready(&mut repository, &mut original);
        repository
            .replace_account_targets(
                &original.namespace,
                original.owner,
                original.record_version,
                &empty,
                &expected,
            )
            .unwrap();
        let removed = repository
            .account_directory(&original.namespace)
            .unwrap()
            .records
            .remove(0);
        repository
            .remove_account(&original.namespace, &removed)
            .unwrap();
        ready(&mut repository, &mut replacement);
        assert!(replacement.record_version > original.record_version);
        repository
            .replace_account_targets(
                &replacement.namespace,
                replacement.owner,
                replacement.record_version,
                &empty,
                &next,
            )
            .unwrap();
        assert!(
            repository
                .replace_account_targets(
                    &original.namespace,
                    original.owner,
                    original.record_version,
                    &expected,
                    &empty
                )
                .is_err()
        );
        assert_eq!(
            repository
                .account_targets(&replacement.namespace, replacement.owner.account)
                .unwrap(),
            next
        );
    }
}

#[test]
fn ready_revision_change_requires_fresh_ownership_evidence_before_legacy_claim() {
    let mut original = credential(1, 1);
    let mut repository = SqliteTrustRepository::in_memory().unwrap();
    ready(&mut repository, &mut original);
    let verified = AuthorizedProfiles::verify(
        &original,
        device(),
        &session(&original),
        &[hosted(&original)],
        &inventory(None),
    )
    .unwrap();
    let mut renewed = original.clone();
    ready(&mut repository, &mut renewed);
    repository.set_enrollment_target(Some(profile())).unwrap();
    assert!(!verified.allows(&renewed, profile()));
    assert_eq!(
        verified.claim_and_load(&mut repository),
        Err(Failure::Persistence)
    );
    assert_eq!(repository.enrollment_target().unwrap(), Some(profile()));
    let verified = AuthorizedProfiles::verify(
        &renewed,
        device(),
        &session(&renewed),
        &[hosted(&renewed)],
        &inventory(None),
    )
    .unwrap();
    assert_eq!(
        verified.claim_and_load(&mut repository).unwrap(),
        BTreeSet::from([profile()])
    );
}
