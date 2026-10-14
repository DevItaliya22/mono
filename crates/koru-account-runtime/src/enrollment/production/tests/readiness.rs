// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::enrollment::member::Effects;
use crate::identity::AccountOwner;
use koru_account_wire::{AccessToken, AuthIdentitySummaryV1, ProfileSummaryV1, SessionSummaryV1};

// Actual directory transactions and production authorization checks; only the
// outbound effect is injected. Member retains the real signed ceremony proof.
struct ReadyPort {
    repository: SqliteTrustRepository,
    cache: ownership::AuthorizationCache,
    device: koru_account_core::DeviceId,
    profile: ProfileId,
    attempt: ProductionAttempt,
    candidate: EnrollmentCandidateV1,
    discoveries: usize,
    sent: Vec<SignedDeviceAttestationV1>,
}

impl ReadyPort {
    fn verified(&self, credential: &EnrollmentAuthorization) -> ownership::AuthorizedProfiles {
        ownership::AuthorizedProfiles::verify(
            credential,
            self.device,
            &SessionSummaryV1 {
                version: Version1,
                session_id: credential.session_id.parse().unwrap(),
                koru_identity_id: credential.owner.account,
                auth_identity_id: credential.owner.auth_identity,
                device_id: Some(self.device),
                expires_at_millis: EXPIRES,
                recent_auth_at_millis: NOW,
            },
            &[ProfileSummaryV1 {
                version: Version1,
                koru_identity_id: credential.owner.account,
                profile_id: self.profile,
                name_snapshot: None,
                locale_snapshot: None,
                auth_identities: vec![AuthIdentitySummaryV1 {
                    version: Version1,
                    auth_identity_id: credential.owner.auth_identity,
                    provider: koru_account_core::AuthProvider::Google,
                    verified_email_snapshot: None,
                }],
                lifecycle_revision: Default::default(),
            }],
            &koru_store_wire::ProfileLifecycleInventory { profiles: vec![] },
        )
        .unwrap()
    }
}

impl Port for ReadyPort {
    type Scope = (String, koru_account_core::KoruIdentityId);
    type Credential = EnrollmentAuthorization;
    type Candidate = EnrollmentCandidateV1;
    type Attempt = ProductionAttempt;
    type Completion = ();
    fn fork_effects(&self) -> Result<Self, Failure> {
        unreachable!("direct member fixture")
    }
    fn scope(value: &Self::Credential) -> Self::Scope {
        ProductionPort::scope(value)
    }
    fn same_authority(previous: &Self::Credential, next: &Self::Credential) -> bool {
        ProductionPort::same_authority(previous, next)
    }
    fn refresh_authority(
        &mut self,
        previous: &Self::Credential,
        next: &Self::Credential,
        expected: &BTreeSet<ProfileId>,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        if !ProductionPort::refresh_continuity(previous, next) {
            return Err(Failure::SessionExpired);
        }
        self.repository
            .validate_account_targets(&next.namespace, next.owner, next.record_version, expected)
            .map_err(|_| Failure::Persistence)?;
        self.cache.publish(self.verified(next))?;
        Ok(BTreeSet::new())
    }
    fn restored_targets(&mut self, _: &Self::Credential) -> Result<BTreeSet<ProfileId>, Failure> {
        unreachable!()
    }
    fn replace_targets(
        &mut self,
        _: &Self::Credential,
        _: &BTreeSet<ProfileId>,
        _: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        unreachable!()
    }
    fn authorize_profile(
        &mut self,
        profile: ProfileId,
        credential: &Self::Credential,
    ) -> Result<bool, Failure> {
        Ok(self.cache.allows(credential, profile))
    }
    fn prepare_effect(
        &mut self,
        profile: ProfileId,
        credential: &Self::Credential,
    ) -> Result<(), Failure> {
        self.cache.authorize_effect(credential, profile)?;
        verify_effect_target(&mut self.repository, credential, profile)
    }
    fn reconcile(&mut self, _: ProfileId) -> Result<Option<Completion<()>>, Failure> {
        Ok(None)
    }
    fn discover(
        &mut self,
        _: ProfileId,
        _: &Self::Credential,
        _: &AtomicBool,
    ) -> Result<Discovery<Self::Candidate, Self::Attempt>, Failure> {
        self.discoveries += 1;
        Ok(Discovery {
            candidates: vec![self.candidate.clone()],
            only_older_peers: false,
            attempt: self.attempt.clone(),
        })
    }
    fn enroll(
        &mut self,
        _: ProfileId,
        candidate: &Self::Candidate,
        attempt: &Self::Attempt,
        _: &AtomicBool,
    ) -> Result<Completion<()>, Failure> {
        assert_eq!(candidate, &self.candidate);
        assert_eq!(attempt.joining_challenge, self.attempt.joining_challenge);
        self.sent.push(attempt.evidence.clone());
        Ok(Completion {
            value: (),
            durable_revision: 17,
        })
    }
    fn provision(&mut self, _: ProfileId, _: &(), _: &Self::Credential) -> Result<(), Failure> {
        Ok(())
    }
    fn discard(&mut self, _: ProfileId) {
        panic!("temporary readiness refusal cannot discard proof")
    }
}

#[test]
fn pending_successor_refuses_outbound_then_acknowledged_refresh_resumes_exact_proof() {
    let fixture = Fixture::new();
    let profile = fixture.profile;
    let mut authority = EnrollmentAuthorization {
        access: AccessToken("injected-current-access".into()),
        namespace: "a".repeat(64),
        owner: AccountOwner {
            account: "30000000-0000-4000-8000-000000000001".parse().unwrap(),
            auth_identity: "60000000-0000-4000-8000-000000000001".parse().unwrap(),
        },
        record_version: 0,
        session_id: "40000000-0000-4000-8000-000000000001".into(),
    };
    let mut port = ReadyPort {
        repository: fixture.joining_repository,
        cache: Default::default(),
        device: fixture.joining.device_id,
        profile,
        attempt: fixture.attempt,
        candidate: fixture.candidate,
        discoveries: 0,
        sent: vec![],
    };
    for _ in 0..3 {
        let row = port
            .repository
            .begin_account_write(&authority.namespace, authority.owner)
            .unwrap();
        port.repository
            .finish_account_write(&authority.namespace, &row, authority.owner, true, false)
            .unwrap();
        authority.record_version = row.version;
    }
    assert_eq!(authority.record_version, 3);
    let targets = BTreeSet::from([profile]);
    port.repository
        .replace_account_targets(
            &authority.namespace,
            authority.owner,
            authority.record_version,
            &BTreeSet::new(),
            &targets,
        )
        .unwrap();
    port.cache.publish(port.verified(&authority)).unwrap();
    let cancel = AtomicBool::new(false);
    let reconciled = Effects::<ReadyPort>::default().step(&mut port, profile, &authority, &cancel);
    assert_eq!(reconciled.failure, None);
    let discovered = reconciled
        .effects
        .step(&mut port, profile, &authority, &cancel);
    assert_eq!(discovered.failure, None);
    assert_eq!(port.discoveries, 1);
    let pending = port
        .repository
        .begin_account_write(&authority.namespace, authority.owner)
        .unwrap();
    assert_eq!(pending.version, 4);
    assert!(
        port.cache.allows(&authority, profile),
        "old hosted cache still exists while vault ACK is held"
    );
    let refused = discovered
        .effects
        .step(&mut port, profile, &authority, &cancel);
    assert_eq!(refused.failure, Some(Failure::Unavailable));
    assert!(port.sent.is_empty());
    assert_eq!(port.discoveries, 1);
    port.repository
        .finish_account_write(&authority.namespace, &pending, authority.owner, true, false)
        .unwrap();
    let mut next = authority.clone();
    next.record_version = pending.version;
    next.access = AccessToken("injected-acknowledged-access".into());
    assert!(
        port.refresh_authority(&authority, &next, &targets)
            .unwrap()
            .is_empty()
    );
    let completed = refused.effects.step(&mut port, profile, &next, &cancel);
    assert_eq!(completed.failure, None);
    assert_eq!(completed.durable_revision, Some(17));
    assert_eq!(
        port.discoveries, 1,
        "refresh must not request another attestation"
    );
    assert_eq!(port.sent, vec![port.attempt.evidence.clone()]);
    assert_eq!(
        port.repository
            .account_targets(&next.namespace, next.owner.account)
            .unwrap(),
        targets
    );
}
