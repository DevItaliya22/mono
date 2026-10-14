// SPDX-License-Identifier: MPL-2.0

use std::str::FromStr;

use uuid::Version;

use super::*;

#[test]
fn account_ids_are_independent_random_v4_values() {
    let identity = KoruIdentityId::mint();
    let auth = AuthIdentityId::mint();
    let session = AccountSessionId::mint();
    let flow = AuthFlowId::mint();
    let values = [
        identity.as_uuid(),
        auth.as_uuid(),
        session.as_uuid(),
        flow.as_uuid(),
    ];
    assert!(
        values
            .iter()
            .all(|id| id.get_version() == Some(Version::Random))
    );
    for (index, left) in values.iter().enumerate() {
        assert!(values[index + 1..].iter().all(|right| left != right));
    }
}

#[test]
fn account_ids_reject_non_v4_and_noncanonical_spellings() {
    assert_eq!(
        KoruIdentityId::from_str("01890f3a-4c20-7cc0-98f6-47f8d2ca3c32"),
        Err(IdParseError::NotVersion4)
    );
    let id = KoruIdentityId::mint().to_string().to_uppercase();
    assert_eq!(
        KoruIdentityId::from_str(&id),
        Err(IdParseError::InvalidSpelling)
    );
}

#[test]
fn profile_and_koru_identity_are_distinct_types_and_values() {
    fn takes_profile(_: ProfileId) {}
    fn takes_identity(_: KoruIdentityId) {}

    let profile = ProfileId::new();
    let identity = KoruIdentityId::mint();
    takes_profile(profile);
    takes_identity(identity);
    assert_ne!(profile.as_uuid(), identity.as_uuid());
}

#[test]
fn secret_debug_output_is_redacted() {
    let sentinel = b"must-not-appear".to_vec();
    for value in [
        format!("{:?}", OpaqueSecret(sentinel.clone())),
        format!("{:?}", KeyedDigest(sentinel.clone())),
        format!("{:?}", SealedValue(sentinel)),
    ] {
        assert!(!value.contains("must-not-appear"));
        assert!(value.contains("redacted"));
    }
}

#[test]
fn proof_mutation_bindings_redact_ids_and_discovery_metadata() {
    let sentinel = "proof-binding-sentinel";
    let link = LinkProofMutationBindingV1 {
        version: 1,
        parent_flow_id: AuthFlowId::mint(),
        koru_identity_id: KoruIdentityId::mint(),
        profile_id: ProfileId::new(),
        issuer: format!("https://{sentinel}.invalid"),
        subject: sentinel.into(),
        authorized_by: AuthIdentityId::mint(),
        authenticated_at_millis: 1,
        session_id: AccountSessionId::mint(),
    };
    let rebind = RebindProofMutationBindingV1 {
        version: 1,
        parent_flow_id: AuthFlowId::mint(),
        koru_identity_id: KoruIdentityId::mint(),
        profile_id: ProfileId::new(),
        erased_identity_id: KoruIdentityId::mint(),
        authenticated_by: AuthIdentityId::mint(),
        explicit_rebind: true,
        discovery_name_snapshot: Some(sentinel.into()),
        locale_snapshot: Some(format!("{sentinel}-locale")),
    };
    for debug in [format!("{link:?}"), format!("{rebind:?}")] {
        assert!(!debug.contains(sentinel));
        assert!(debug.contains("<redacted>"));
    }
}

#[test]
fn rebind_proof_outcome_debug_is_variant_only() {
    let sentinel = "rebind-outcome-sentinel";
    let profile = ProfileBinding {
        profile_id: ProfileId::new(),
        koru_identity_id: KoruIdentityId::mint(),
        discovery_name_snapshot: Some(sentinel.into()),
        locale_snapshot: Some(format!("{sentinel}-locale")),
        lifecycle_state: ProfileLifecycleState::Active,
        lifecycle_revision: ProfileLifecycleRevision::INITIAL,
        lifecycle_operation_id: None,
        deleted_at_millis: None,
        created_at_millis: 1,
        updated_at_millis: 1,
    };
    let profile_id = profile.profile_id.to_string();
    for outcome in [
        RebindProofCommitOutcome::Committed(profile.clone()),
        RebindProofCommitOutcome::ExactRetry(profile),
    ] {
        let debug = format!("{outcome:?}");
        assert!(!debug.contains(sentinel));
        assert!(!debug.contains(&profile_id));
        assert!(debug.contains("<redacted>"));
    }
}
