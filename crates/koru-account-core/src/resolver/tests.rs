// SPDX-License-Identifier: MPL-2.0

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    thread,
};

use super::*;

const ISSUER: &str = "https://accounts.google.com";

#[derive(Default)]
struct State {
    identities: Vec<KoruIdentity>,
    auth: Vec<AuthIdentity>,
    flows: HashMap<AuthFlowId, TemporaryAuthFlow>,
    fail_atomic_link_once: bool,
    fail_record_once: bool,
}

#[derive(Default)]
struct FakeRepository(Mutex<State>);

impl AccountExchangeRepository for FakeRepository {
    fn commit_exchange<'a>(
        &'a self,
        _request: ExchangeCommitRequest,
    ) -> PortFuture<'a, Result<ExchangeCommitOutcome, RepositoryError>> {
        Box::pin(async { Err(RepositoryError::Unavailable) })
    }
}

impl FakeRepository {
    fn seed(&self, subject: &str, email: Option<&str>, now: UnixMillis) -> KoruIdentityId {
        let identity = KoruIdentity {
            id: KoruIdentityId::mint(),
            display_name: "Seeded owner".to_owned(),
            status: KoruIdentityStatus::Active,
            terms: TermsAcceptance {
                version: "terms-v1".to_owned(),
                accepted_at_millis: now,
            },
            created_at_millis: now,
            updated_at_millis: now,
        };
        let auth = AuthIdentity {
            id: AuthIdentityId::mint(),
            koru_identity_id: identity.id,
            provider: AuthProvider::Google,
            issuer: ISSUER.to_owned(),
            subject: subject.to_owned(),
            verified_email: email.map(|value| VerifiedEmailSnapshot {
                normalized_email: value.to_owned(),
                captured_at_millis: now,
            }),
            created_at_millis: now,
            last_authenticated_at_millis: now,
        };
        let id = identity.id;
        let mut state = self.0.lock().unwrap();
        state.identities.push(identity);
        state.auth.push(auth);
        id
    }

    fn flow(&self, id: AuthFlowId) -> Option<TemporaryAuthFlow> {
        self.0.lock().unwrap().flows.get(&id).cloned()
    }
}

impl IdentityRepository for FakeRepository {
    fn find_koru_identity<'a>(
        &'a self,
        id: KoruIdentityId,
    ) -> PortFuture<'a, Result<Option<KoruIdentity>, RepositoryError>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .unwrap()
                .identities
                .iter()
                .find(|row| row.id == id)
                .cloned())
        })
    }

    fn find_auth_identity<'a>(
        &'a self,
        issuer: &'a str,
        subject: &'a str,
    ) -> PortFuture<'a, Result<Option<AuthIdentity>, RepositoryError>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .unwrap()
                .auth
                .iter()
                .find(|row| row.issuer == issuer && row.subject == subject)
                .cloned())
        })
    }

    fn list_auth_identities<'a>(
        &'a self,
        id: KoruIdentityId,
    ) -> PortFuture<'a, Result<Vec<AuthIdentity>, RepositoryError>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .unwrap()
                .auth
                .iter()
                .filter(|row| row.koru_identity_id == id)
                .cloned()
                .collect())
        })
    }

    fn create_identity_with_auth<'a>(
        &'a self,
        identity: KoruIdentity,
        auth_identity: AuthIdentity,
    ) -> PortFuture<'a, Result<(), RepositoryError>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if state.auth.iter().any(|row| {
                row.issuer == auth_identity.issuer && row.subject == auth_identity.subject
            }) {
                return Err(RepositoryError::Conflict);
            }
            state.identities.push(identity);
            state.auth.push(auth_identity);
            Ok(())
        })
    }

    fn update_identity_display_name<'a>(
        &'a self,
        id: KoruIdentityId,
        display_name: &'a str,
        updated_at_millis: UnixMillis,
    ) -> PortFuture<'a, Result<KoruIdentity, RepositoryError>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            let identity = state
                .identities
                .iter_mut()
                .find(|row| row.id == id)
                .ok_or(RepositoryError::NotFound)?;
            identity.display_name = display_name.to_owned();
            identity.updated_at_millis = updated_at_millis;
            Ok(identity.clone())
        })
    }

    fn link_auth_identity<'a>(
        &'a self,
        auth_identity: AuthIdentity,
    ) -> PortFuture<'a, Result<(), RepositoryError>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if state.auth.iter().any(|row| {
                row.issuer == auth_identity.issuer && row.subject == auth_identity.subject
            }) {
                return Err(RepositoryError::Conflict);
            }
            state.auth.push(auth_identity);
            Ok(())
        })
    }
}

impl IdentityCollisionRepository for FakeRepository {
    fn find_koru_identities_by_verified_email<'a>(
        &'a self,
        normalized_email: &'a str,
    ) -> PortFuture<'a, Result<Vec<KoruIdentityId>, RepositoryError>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .unwrap()
                .auth
                .iter()
                .filter_map(|row| {
                    row.verified_email
                        .as_ref()
                        .filter(|email| email.normalized_email == normalized_email)
                        .map(|_| row.koru_identity_id)
                })
                .collect())
        })
    }
}

impl AuthFlowRepository for FakeRepository {
    fn insert_flow<'a>(
        &'a self,
        flow: TemporaryAuthFlow,
    ) -> PortFuture<'a, Result<(), RepositoryError>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if state.flows.insert(flow.id, flow).is_some() {
                return Err(RepositoryError::Conflict);
            }
            Ok(())
        })
    }

    fn find_flow<'a>(
        &'a self,
        id: AuthFlowId,
    ) -> PortFuture<'a, Result<Option<TemporaryAuthFlow>, RepositoryError>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().flows.get(&id).cloned()) })
    }

    fn consume_flow<'a>(
        &'a self,
        id: AuthFlowId,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<TemporaryAuthFlow, RepositoryError>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            let flow = state.flows.get_mut(&id).ok_or(RepositoryError::NotFound)?;
            if flow.status != AuthFlowStatus::Pending || now_millis >= flow.expires_at_millis {
                return Err(RepositoryError::Conflict);
            }
            flow.status = AuthFlowStatus::Consumed;
            flow.consumed_at_millis = Some(now_millis);
            Ok(flow.clone())
        })
    }
}

impl IdentityResolutionRepository for FakeRepository {
    fn link_auth_identity_and_profile<'a>(
        &'a self,
        _auth_identity: AuthIdentity,
        _profile_id: ProfileId,
    ) -> PortFuture<'a, Result<AuthProfileLinkCommit, AuthProfileLinkError>> {
        Box::pin(async { Err(AuthProfileLinkError::Unavailable) })
    }

    fn record_authentication<'a>(
        &'a self,
        auth_identity_id: AuthIdentityId,
        authenticated_at_millis: UnixMillis,
    ) -> PortFuture<'a, Result<AuthIdentity, RepositoryError>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if state.fail_record_once {
                state.fail_record_once = false;
                return Err(RepositoryError::Unavailable);
            }
            let row = state
                .auth
                .iter_mut()
                .find(|row| row.id == auth_identity_id)
                .ok_or(RepositoryError::NotFound)?;
            row.last_authenticated_at_millis = row
                .last_authenticated_at_millis
                .max(authenticated_at_millis);
            Ok(row.clone())
        })
    }

    fn complete_collision_link<'a>(
        &'a self,
        flow_id: AuthFlowId,
        auth_identity: AuthIdentity,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<CollisionLinkCommit, CollisionLinkError>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if state.fail_atomic_link_once {
                state.fail_atomic_link_once = false;
                return Err(CollisionLinkError::Unavailable);
            }
            let flow = state
                .flows
                .get(&flow_id)
                .ok_or(CollisionLinkError::FlowNotFound)?;
            if flow.status != AuthFlowStatus::Pending {
                return Err(CollisionLinkError::FlowConsumed);
            }
            if now_millis >= flow.expires_at_millis {
                return Err(CollisionLinkError::FlowExpired);
            }
            let existing = state
                .auth
                .iter()
                .find(|row| {
                    row.issuer == auth_identity.issuer && row.subject == auth_identity.subject
                })
                .cloned();
            let commit = match existing {
                Some(row) if row.koru_identity_id == auth_identity.koru_identity_id => {
                    CollisionLinkCommit::AlreadyLinked(row)
                }
                Some(_) => return Err(CollisionLinkError::IdentityOwnedByAnother),
                None => {
                    state.auth.push(auth_identity.clone());
                    CollisionLinkCommit::Linked(auth_identity)
                }
            };
            let flow = state.flows.get_mut(&flow_id).unwrap();
            flow.status = AuthFlowStatus::Consumed;
            flow.consumed_at_millis = Some(now_millis);
            Ok(commit)
        })
    }
}

struct FakeClock(AtomicI64);

impl FakeClock {
    fn new(now: UnixMillis) -> Self {
        Self(AtomicI64::new(now))
    }

    fn set(&self, now: UnixMillis) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_millis(&self) -> UnixMillis {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct FakeVerifier;

impl GoogleIdentityVerifier for FakeVerifier {
    fn verify<'a>(
        &'a self,
        input: GoogleVerificationInput,
    ) -> PortFuture<'a, Result<VerifiedGoogleIdentity, ProviderVerificationError>> {
        Box::pin(async move {
            let code = String::from_utf8(input.authorization_code.expose_secret().to_vec())
                .map_err(|_| ProviderVerificationError::Rejected)?;
            if let Some(error) = provider_error(&code) {
                return Err(error);
            }
            if input.expected_pkce_challenge != "expected-s256" {
                return Err(ProviderVerificationError::PkceMismatch);
            }
            let (subject, email) = code.split_once('|').unwrap_or((&code, ""));
            Ok(VerifiedGoogleIdentity {
                canonical_issuer: ISSUER.to_owned(),
                subject: subject.to_owned(),
                verified_email: (!email.is_empty()).then(|| email.to_owned()),
                display_name_snapshot: Some("名字 🌿".to_owned()),
            })
        })
    }
}

fn provider_error(code: &str) -> Option<ProviderVerificationError> {
    Some(match code {
        "bad-issuer" => ProviderVerificationError::InvalidIssuer,
        "bad-audience" => ProviderVerificationError::InvalidAudience,
        "bad-signature" => ProviderVerificationError::InvalidSignature,
        "expired-token" => ProviderVerificationError::Expired,
        "bad-nonce" => ProviderVerificationError::NonceMismatch,
        "bad-pkce" => ProviderVerificationError::PkceMismatch,
        _ => return None,
    })
}

#[derive(Default)]
struct FakeEncryption;

impl TransientEncryption for FakeEncryption {
    fn seal(&self, purpose: &'static str, plaintext: &[u8]) -> Result<SealedValue, CryptoError> {
        if purpose != COLLISION_PURPOSE {
            return Err(CryptoError::InvalidInput);
        }
        Ok(SealedValue(
            plaintext.iter().map(|byte| byte ^ 0xaa).collect(),
        ))
    }

    fn open(&self, purpose: &'static str, sealed: &SealedValue) -> Result<Vec<u8>, CryptoError> {
        if purpose != COLLISION_PURPOSE {
            return Err(CryptoError::InvalidInput);
        }
        Ok(sealed.0.iter().map(|byte| byte ^ 0xaa).collect())
    }

    fn digest(&self, _: &'static str, value: &[u8]) -> Result<KeyedDigest, CryptoError> {
        Ok(KeyedDigest(value.iter().map(|byte| byte ^ 0x55).collect()))
    }
}

fn secret(value: &str) -> OpaqueSecret {
    OpaqueSecret(value.as_bytes().to_vec())
}

fn request(code: &str, flow_id: AuthFlowId, terms: &str) -> GoogleAuthenticationRequest {
    GoogleAuthenticationRequest {
        verification: GoogleVerificationInput {
            authorization_code: secret(code),
            expected_nonce: secret("nonce"),
            pkce_verifier: secret("0123456789abcdefghijklmnopqrstuvwxyzABCDEFG"),
            expected_pkce_challenge: "expected-s256".to_owned(),
        },
        terms_version_at_creation: terms.to_owned(),
        collision_flow: CollisionFlowContext {
            flow_id,
            state_digest: KeyedDigest(vec![1]),
            source_digest: KeyedDigest(vec![2]),
        },
    }
}

fn resolver<'a>(
    repo: &'a FakeRepository,
    verifier: &'a FakeVerifier,
    clock: &'a FakeClock,
    encryption: &'a FakeEncryption,
) -> IdentityResolver<'a, FakeRepository, FakeVerifier, FakeClock, FakeEncryption> {
    IdentityResolver::new(repo, verifier, clock, encryption)
}

fn returning(result: IdentityResolution) -> ResolvedIdentity {
    match result {
        IdentityResolution::Returning(value) => value,
        other => panic!("expected returning identity, got {other:?}"),
    }
}

fn collision_flow(result: IdentityResolution) -> AuthFlowId {
    match result {
        IdentityResolution::CollisionRequired {
            flow_id, prompt, ..
        } => {
            assert_eq!(prompt, AuthenticationPrompt::SelectAccount);
            flow_id
        }
        other => panic!("expected collision, got {other:?}"),
    }
}

#[test]
fn unseen_identity_pauses_and_prepares_without_writes_then_returning_auth_is_touched() {
    let repo = FakeRepository::default();
    let clock = FakeClock::new(1_000);
    let verifier = FakeVerifier;
    let encryption = FakeEncryption;
    let service = resolver(&repo, &verifier, &clock, &encryption);
    let first = pollster::block_on(service.authenticate_google(request(
        "subject-a|fresh@example.test",
        AuthFlowId::mint(),
        "terms-v1",
    )))
    .unwrap();
    let verified = match first {
        IdentityResolution::WouldCreate(value) => value,
        other => panic!("expected creation pause, got {other:?}"),
    };
    assert!(repo.0.lock().unwrap().identities.is_empty());
    assert!(repo.0.lock().unwrap().auth.is_empty());
    assert!(repo.0.lock().unwrap().flows.is_empty());
    let prepared = pollster::block_on(service.prepare_creation_resolution(
        verified,
        "terms-v1".into(),
        KoruIdentityId::mint(),
        AuthIdentityId::mint(),
        1_000,
        2_000,
        10_000,
        request("", AuthFlowId::mint(), "").collision_flow,
    ))
    .unwrap();
    assert_eq!(prepared.expected, CreationResolution::Unseen);
    assert!(repo.0.lock().unwrap().identities.is_empty());
    let identity = prepared.identity.unwrap();
    let auth = prepared.auth_identity.unwrap();
    assert_eq!(identity.terms.accepted_at_millis, 2_000);
    assert_eq!(auth.created_at_millis, 2_000);
    assert_eq!(auth.last_authenticated_at_millis, 2_000);
    assert_eq!(
        auth.verified_email.as_ref().unwrap().captured_at_millis,
        1_000
    );
    // Seed committed fixture rows through the repository; ordinary resolution
    // has no creation operation. The PG suite covers confirmation atomicity.
    pollster::block_on(repo.create_identity_with_auth(identity.clone(), auth.clone())).unwrap();
    let created = resolved(identity, auth, 1_000, 301_000);
    assert_eq!(created.identity.terms.version, "terms-v1");
    assert_eq!(created.identity.display_name, "名字 🌿");
    assert!(!format!("{:?}", created.identity).contains("名字"));

    clock.set(5_000);
    let returned = returning(
        pollster::block_on(service.authenticate_google(request(
            "subject-a|changed@example.test",
            AuthFlowId::mint(),
            "terms-v99",
        )))
        .unwrap(),
    );
    assert_eq!(returned.identity.id, created.identity.id);
    assert_eq!(returned.identity.terms.version, "terms-v1");
    assert_eq!(returned.auth_identity.last_authenticated_at_millis, 5_000);
    assert_eq!(
        repo.0.lock().unwrap().auth[0].last_authenticated_at_millis,
        5_000
    );

    repo.0.lock().unwrap().fail_record_once = true;
    clock.set(6_000);
    assert_eq!(
        pollster::block_on(service.authenticate_google(request(
            "subject-a|fresh@example.test",
            AuthFlowId::mint(),
            "",
        ))),
        Err(IdentityResolutionError::RepositoryUnavailable)
    );
}

#[test]
fn returning_google_name_replaces_only_the_old_placeholder() {
    for pinned in [false, true] {
        let repo = FakeRepository::default();
        let placeholder = repo.seed("placeholder", None, 10);
        repo.0
            .lock()
            .unwrap()
            .identities
            .iter_mut()
            .find(|row| row.id == placeholder)
            .unwrap()
            .display_name = "Koru user".into();
        let chosen = repo.seed("chosen", None, 10);
        let clock = FakeClock::new(1_000);
        let verifier = FakeVerifier;
        let encryption = FakeEncryption;
        let service = resolver(&repo, &verifier, &clock, &encryption);

        let authenticate = |subject: &str| {
            let input = request(subject, AuthFlowId::mint(), "terms-v1");
            if pinned {
                let verified = pollster::block_on(verifier.verify(input.verification)).unwrap();
                let auth = repo
                    .0
                    .lock()
                    .unwrap()
                    .auth
                    .iter()
                    .find(|row| row.subject == subject)
                    .unwrap()
                    .clone();
                pollster::block_on(service.authenticate_existing_verified_pinned(
                    verified.into(),
                    auth.koru_identity_id,
                    auth.id,
                ))
                .unwrap()
            } else {
                returning(pollster::block_on(service.authenticate_google(input)).unwrap())
            }
        };
        let updated = authenticate("placeholder");
        let retained = authenticate("chosen");

        assert_eq!(updated.identity.display_name, "名字 🌿");
        assert_eq!(retained.identity.id, chosen);
        assert_eq!(retained.identity.display_name, "Seeded owner");
    }
}

#[test]
fn confirmation_preparation_preserves_returning_proof_time_and_collision_policy() {
    let repo = FakeRepository::default();
    let target = repo.seed("existing", Some("same@example.test"), 10);
    let clock = FakeClock::new(2_000);
    let verifier = FakeVerifier;
    let encryption = FakeEncryption;
    let service = resolver(&repo, &verifier, &clock, &encryption);
    let candidate = |subject: &str| VerifiedExternalIdentity {
        provider: AuthProvider::Google,
        canonical_issuer: ISSUER.into(),
        subject: subject.into(),
        verified_email: Some(" SAME@example.test ".into()),
        display_name_snapshot: Some("Private name".into()),
    };
    let prepared = pollster::block_on(service.prepare_creation_resolution(
        candidate("existing"),
        "".into(),
        KoruIdentityId::mint(),
        AuthIdentityId::mint(),
        1_000,
        2_000,
        3_000,
        request("", AuthFlowId::mint(), "").collision_flow,
    ))
    .unwrap();
    assert!(
        matches!(prepared.expected, CreationResolution::Returning { koru_identity_id, .. } if koru_identity_id == target)
    );
    assert_eq!(
        prepared.auth_identity.unwrap().last_authenticated_at_millis,
        10
    );
    assert_eq!(
        repo.0.lock().unwrap().auth[0].last_authenticated_at_millis,
        10
    );
    let prepared = pollster::block_on(service.prepare_creation_resolution(
        candidate("new"),
        "terms-v1".into(),
        KoruIdentityId::mint(),
        AuthIdentityId::mint(),
        1_000,
        2_000,
        3_000,
        request("", AuthFlowId::mint(), "").collision_flow,
    ))
    .unwrap();
    assert_eq!(
        prepared.expected,
        CreationResolution::Collision {
            targets: vec![target]
        }
    );
    assert!(prepared.identity.is_none());
    assert!(prepared.auth_identity.is_none());
    assert_eq!(
        format!("{prepared:?}"),
        "PreparedCreationResolution(<redacted>)"
    );
    let flow = prepared.collision_flow.unwrap();
    assert_eq!(flow.expires_at_millis, 3_000);
    let bytes = encryption
        .open(COLLISION_PURPOSE, &flow.sealed_transient)
        .unwrap();
    let decoded = CollisionCandidate::decode(&bytes).unwrap();
    assert_eq!(decoded.targets, vec![target]);
    assert_eq!(
        decoded.verified.verified_email.as_deref(),
        Some("same@example.test")
    );
    assert!(repo.0.lock().unwrap().flows.is_empty());
    assert_eq!(repo.0.lock().unwrap().auth.len(), 1);
}

#[test]
fn matching_email_creates_only_an_encrypted_expiring_candidate() {
    let repo = FakeRepository::default();
    repo.seed("old", Some("same@example.test"), 10);
    let clock = FakeClock::new(1_000);
    let verifier = FakeVerifier;
    let encryption = FakeEncryption;
    let service = resolver(&repo, &verifier, &clock, &encryption);
    let flow_id = AuthFlowId::mint();
    let result = pollster::block_on(service.authenticate_google(request(
        "new|SAME@example.test",
        flow_id,
        "terms-v2",
    )))
    .unwrap();
    assert_eq!(collision_flow(result), flow_id);
    let state = repo.0.lock().unwrap();
    assert_eq!(state.identities.len(), 1);
    assert_eq!(state.auth.len(), 1, "email alone must never link");
    let flow = state.flows.get(&flow_id).unwrap();
    assert_eq!(flow.expires_at_millis, 1_000 + COLLISION_LIFETIME_MILLIS);
    assert!(
        !flow
            .sealed_transient
            .0
            .windows("same@example.test".len())
            .any(|bytes| bytes == b"same@example.test")
    );
}

#[test]
fn collision_link_is_atomic_retryable_single_use_and_requires_the_right_account() {
    let repo = FakeRepository::default();
    let target = repo.seed("old", Some("same@example.test"), 10);
    let wrong = repo.seed("wrong", None, 10);
    let clock = FakeClock::new(1_000);
    let verifier = FakeVerifier;
    let encryption = FakeEncryption;
    let service = resolver(&repo, &verifier, &clock, &encryption);
    let flow_id = collision_flow(
        pollster::block_on(service.authenticate_google(request(
            "new|same@example.test",
            AuthFlowId::mint(),
            "terms-v2",
        )))
        .unwrap(),
    );
    let target_proof = returning(
        pollster::block_on(service.authenticate_google(request(
            "old|same@example.test",
            AuthFlowId::mint(),
            "ignored",
        )))
        .unwrap(),
    )
    .recent_authentication;
    assert_eq!(target_proof.koru_identity_id(), target);
    let wrong_proof = returning(
        pollster::block_on(service.authenticate_google(request(
            "wrong",
            AuthFlowId::mint(),
            "ignored",
        )))
        .unwrap(),
    )
    .recent_authentication;
    assert_eq!(wrong_proof.koru_identity_id(), wrong);
    assert_eq!(
        pollster::block_on(service.complete_collision(flow_id, &wrong_proof)),
        Err(IdentityResolutionError::WrongAccount)
    );
    assert_eq!(repo.flow(flow_id).unwrap().status, AuthFlowStatus::Pending);
    assert!(
        !repo
            .0
            .lock()
            .unwrap()
            .auth
            .iter()
            .any(|row| row.subject == "new"),
        "wrong-account proof must not link the collision candidate"
    );

    repo.0.lock().unwrap().fail_atomic_link_once = true;
    assert_eq!(
        pollster::block_on(service.complete_collision(flow_id, &target_proof)),
        Err(IdentityResolutionError::RepositoryUnavailable)
    );
    let state = repo.0.lock().unwrap();
    assert_eq!(
        state.flows.get(&flow_id).unwrap().status,
        AuthFlowStatus::Pending
    );
    assert!(!state.auth.iter().any(|row| row.subject == "new"));
    drop(state);

    assert!(matches!(
        pollster::block_on(service.complete_collision(flow_id, &target_proof)),
        Ok(LinkResolution::Linked(_))
    ));
    assert_eq!(repo.flow(flow_id).unwrap().status, AuthFlowStatus::Consumed);
    assert_eq!(
        pollster::block_on(service.complete_collision(flow_id, &target_proof)),
        Err(IdentityResolutionError::FlowConsumed)
    );
}

#[test]
fn competing_collision_targets_cannot_both_claim_the_auth_identity() {
    let repo = Arc::new(FakeRepository::default());
    repo.seed("first", Some("same@example.test"), 10);
    repo.seed("second", Some("same@example.test"), 10);
    let clock = Arc::new(FakeClock::new(1_000));
    let verifier = Arc::new(FakeVerifier);
    let encryption = Arc::new(FakeEncryption);
    let service = resolver(&repo, &verifier, &clock, &encryption);
    let flow_id = collision_flow(
        pollster::block_on(service.authenticate_google(request(
            "new|same@example.test",
            AuthFlowId::mint(),
            "terms-v2",
        )))
        .unwrap(),
    );
    let first = returning(
        pollster::block_on(service.authenticate_google(request(
            "first|same@example.test",
            AuthFlowId::mint(),
            "ignored",
        )))
        .unwrap(),
    )
    .recent_authentication;
    let second = returning(
        pollster::block_on(service.authenticate_google(request(
            "second|same@example.test",
            AuthFlowId::mint(),
            "ignored",
        )))
        .unwrap(),
    )
    .recent_authentication;
    let results = thread::scope(|scope| {
        let one = scope.spawn(|| {
            pollster::block_on(
                resolver(&repo, &verifier, &clock, &encryption).complete_collision(flow_id, &first),
            )
        });
        let two = scope.spawn(|| {
            pollster::block_on(
                resolver(&repo, &verifier, &clock, &encryption)
                    .complete_collision(flow_id, &second),
            )
        });
        [one.join().unwrap(), two.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Err(IdentityResolutionError::FlowConsumed))
            .count(),
        1
    );
    let state = repo.0.lock().unwrap();
    assert_eq!(
        state.auth.iter().filter(|row| row.subject == "new").count(),
        1
    );
}

#[test]
fn expired_collision_and_expired_recent_authentication_are_rejected() {
    let repo = FakeRepository::default();
    repo.seed("old", Some("same@example.test"), 10);
    let clock = FakeClock::new(1_000);
    let verifier = FakeVerifier;
    let encryption = FakeEncryption;
    let service = resolver(&repo, &verifier, &clock, &encryption);
    let flow_id = collision_flow(
        pollster::block_on(service.authenticate_google(request(
            "new|same@example.test",
            AuthFlowId::mint(),
            "terms-v2",
        )))
        .unwrap(),
    );
    let proof = returning(
        pollster::block_on(service.authenticate_google(request(
            "old|same@example.test",
            AuthFlowId::mint(),
            "ignored",
        )))
        .unwrap(),
    )
    .recent_authentication;
    clock.set(1_000 + RECENT_AUTH_LIFETIME_MILLIS);
    assert_eq!(
        pollster::block_on(service.complete_collision(flow_id, &proof)),
        Err(IdentityResolutionError::RecentAuthenticationExpired)
    );
    clock.set(1_000 + COLLISION_LIFETIME_MILLIS - 1);
    let fresh = returning(
        pollster::block_on(service.authenticate_google(request(
            "old|same@example.test",
            AuthFlowId::mint(),
            "ignored",
        )))
        .unwrap(),
    )
    .recent_authentication;
    clock.set(1_000 + COLLISION_LIFETIME_MILLIS);
    assert_eq!(
        pollster::block_on(service.complete_collision(flow_id, &fresh)),
        Err(IdentityResolutionError::FlowExpired)
    );
}

#[test]
fn fake_provider_rejects_every_security_check_without_account_effects() {
    let repo = FakeRepository::default();
    let clock = FakeClock::new(1_000);
    let verifier = FakeVerifier;
    let encryption = FakeEncryption;
    let service = resolver(&repo, &verifier, &clock, &encryption);
    for (code, expected) in [
        ("bad-issuer", ProviderVerificationError::InvalidIssuer),
        ("bad-audience", ProviderVerificationError::InvalidAudience),
        ("bad-signature", ProviderVerificationError::InvalidSignature),
        ("expired-token", ProviderVerificationError::Expired),
        ("bad-nonce", ProviderVerificationError::NonceMismatch),
        ("bad-pkce", ProviderVerificationError::PkceMismatch),
    ] {
        assert_eq!(
            pollster::block_on(service.authenticate_google(request(
                code,
                AuthFlowId::mint(),
                "terms-v1",
            ))),
            Err(IdentityResolutionError::Provider(expected))
        );
    }
    let state = repo.0.lock().unwrap();
    assert!(state.identities.is_empty());
    assert!(state.auth.is_empty());
    assert!(state.flows.is_empty());
}

#[test]
fn explicit_link_requires_fresh_exact_authentication_and_never_uses_email() {
    let repo = FakeRepository::default();
    let target = repo.seed("old", None, 10);
    let clock = FakeClock::new(1_000);
    let verifier = FakeVerifier;
    let encryption = FakeEncryption;
    let service = resolver(&repo, &verifier, &clock, &encryption);
    let proof = returning(
        pollster::block_on(service.authenticate_google(request(
            "old",
            AuthFlowId::mint(),
            "ignored",
        )))
        .unwrap(),
    )
    .recent_authentication;
    let linked = pollster::block_on(service.link_google(
        request("new|same@example.test", AuthFlowId::mint(), "ignored").verification,
        &proof,
    ))
    .unwrap();
    assert!(matches!(linked, LinkResolution::Linked(_)));
    assert_eq!(
        repo.0
            .lock()
            .unwrap()
            .auth
            .iter()
            .find(|row| row.subject == "new")
            .unwrap()
            .koru_identity_id,
        target
    );
    clock.set(proof.expires_at_millis());
    assert_eq!(
        pollster::block_on(service.link_google(
            request("later", AuthFlowId::mint(), "ignored").verification,
            &proof,
        )),
        Err(IdentityResolutionError::RecentAuthenticationExpired)
    );
}

#[test]
fn concurrent_first_callbacks_both_pause_without_creating_any_identity() {
    let repo = Arc::new(FakeRepository::default());
    let clock = Arc::new(FakeClock::new(1_000));
    let verifier = Arc::new(FakeVerifier);
    let encryption = Arc::new(FakeEncryption);
    let results = thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    pollster::block_on(
                        resolver(&repo, &verifier, &clock, &encryption).authenticate_google(
                            request("same-subject", AuthFlowId::mint(), "terms-v1"),
                        ),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect::<Vec<_>>()
    });
    let subjects: Vec<_> = results
        .iter()
        .map(|result| match result {
            IdentityResolution::WouldCreate(candidate) => candidate.subject.clone(),
            other => panic!("unexpected resolution {other:?}"),
        })
        .collect();
    assert_eq!(subjects[0], subjects[1]);
    let state = repo.0.lock().unwrap();
    assert!(state.identities.is_empty());
    assert!(state.auth.is_empty());
    assert!(state.flows.is_empty());
}

#[test]
fn identity_and_error_debug_never_include_provider_pii_or_secrets() {
    let verified = VerifiedExternalIdentity {
        provider: AuthProvider::Google,
        canonical_issuer: "seed-issuer".to_owned(),
        subject: "seed-subject".to_owned(),
        verified_email: Some("seed-email".to_owned()),
        display_name_snapshot: Some("seed-name".to_owned()),
    };
    let rendered = format!(
        "{verified:?} {:?} {}",
        secret("seed-token"),
        IdentityResolutionError::Provider(ProviderVerificationError::InvalidSignature)
    );
    for forbidden in [
        "seed-issuer",
        "seed-subject",
        "seed-email",
        "seed-name",
        "seed-token",
    ] {
        assert!(!rendered.contains(forbidden));
    }
    assert_eq!(
        IdentityResolutionError::Provider(ProviderVerificationError::InvalidSignature).to_string(),
        "provider_verification_failed"
    );
}

#[test]
fn expiry_construction_fails_closed_at_clock_boundaries_without_writes() {
    assert_eq!(
        checked_expiry(
            i64::MAX - RECENT_AUTH_LIFETIME_MILLIS,
            RECENT_AUTH_LIFETIME_MILLIS
        ),
        Ok(i64::MAX)
    );
    assert_eq!(
        checked_expiry(
            i64::MAX - RECENT_AUTH_LIFETIME_MILLIS + 1,
            RECENT_AUTH_LIFETIME_MILLIS
        ),
        Err(IdentityResolutionError::ClockOutOfRange)
    );
    assert_eq!(
        checked_expiry(
            i64::MAX - COLLISION_LIFETIME_MILLIS,
            COLLISION_LIFETIME_MILLIS
        ),
        Ok(i64::MAX)
    );
    assert_eq!(
        checked_expiry(
            i64::MAX - COLLISION_LIFETIME_MILLIS + 1,
            COLLISION_LIFETIME_MILLIS
        ),
        Err(IdentityResolutionError::ClockOutOfRange)
    );

    let repo = FakeRepository::default();
    let returning_id = repo.seed("returning", None, 10);
    repo.seed("collision-target", Some("same@example.test"), 10);
    let clock = FakeClock::new(i64::MAX - RECENT_AUTH_LIFETIME_MILLIS + 1);
    let verifier = FakeVerifier;
    let encryption = FakeEncryption;
    let service = resolver(&repo, &verifier, &clock, &encryption);

    for code in ["returning"] {
        assert_eq!(
            pollster::block_on(service.authenticate_google(request(
                code,
                AuthFlowId::mint(),
                "terms-v1",
            ))),
            Err(IdentityResolutionError::ClockOutOfRange)
        );
    }
    let state = repo.0.lock().unwrap();
    assert_eq!(
        state.identities.len(),
        2,
        "new identity must not be written"
    );
    assert_eq!(state.auth.len(), 2, "new auth identity must not be written");
    assert_eq!(
        state
            .auth
            .iter()
            .find(|row| row.koru_identity_id == returning_id)
            .unwrap()
            .last_authenticated_at_millis,
        10,
        "returning timestamp must not be touched when expiry is invalid"
    );
    drop(state);

    clock.set(i64::MAX - COLLISION_LIFETIME_MILLIS + 1);
    assert_eq!(
        pollster::block_on(service.authenticate_google(request(
            "collision-new|same@example.test",
            AuthFlowId::mint(),
            "terms-v1",
        ))),
        Err(IdentityResolutionError::ClockOutOfRange)
    );
    assert!(repo.0.lock().unwrap().flows.is_empty());
    assert_eq!(
        IdentityResolutionError::ClockOutOfRange.to_string(),
        "account_clock_out_of_range"
    );
}
