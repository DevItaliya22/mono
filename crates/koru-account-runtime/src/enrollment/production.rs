// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::account::lifecycle::LocalProfileInventory;
use crate::ports::{Clock, Entropy};
use koru_account_client::{
    AccountAuthorization, AccountClient, AttestationVerificationError, ClientError,
};
use koru_account_core::{DeviceProfileState, ProfileId};
use koru_account_session::{SignedDeviceAttestationV1, VerifiedDeviceAttestationV1};
use koru_account_wire::{
    DeviceAttestationEvidenceV1, DeviceAttestationRequestV1, EnrollmentCandidateV1,
    EnrollmentCompletionReceiptV1, JwksResponseV1, ProvisionDeviceRequestV1, Version1,
};
use koru_device::{
    ApplyOutcome, DeviceDescriptorV1, DevicePublicKey, DeviceSigningKey, ReplicationEndpointId,
};
use koru_network_iroh::{
    EnrollmentHandler, EnrollmentHandlerError, EnrollmentHandlerFuture, EnrollmentSession,
    EnrollmentTransportError, PEER_PROTOCOL_VERSION, RuntimeHandle,
};
use koru_pairing::{
    ENROLLMENT_VERSION, EnrollmentFrameV1, EnrollmentJoinV1, EnrollmentTranscriptV1,
    PreparedEnrollmentV1, verify_enrollment_joiner_transport,
    verify_enrollment_responder_transport,
};

use super::{Completion, Discovery, EnrollmentAuthorization, Failure, Port};

mod discovery;
mod io;
mod ownership;
mod provisioning;
mod repository;
use io::CeremonySession;
use repository::{DiskInboundRepository, InboundRepository, ReplayRequest};

#[derive(Clone)]
pub struct ProductionAttempt {
    evidence: SignedDeviceAttestationV1,
    verified: VerifiedDeviceAttestationV1,
    joining_challenge: [u8; 32],
}

/// Blocking hosted calls and ceremonies owned exclusively by the enrollment worker.
pub struct ProductionPort {
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    client: AccountClient,
    issuer: String,
    identity_db: PathBuf,
    local: DeviceDescriptorV1,
    signing_key: Arc<DeviceSigningKey>,
    network: RuntimeHandle,
    runtime: tokio::runtime::Runtime,
    jwks: Option<JwksResponseV1>,
    local_profiles: Option<Arc<dyn LocalProfileInventory>>,
    cloud_sync: Option<Arc<dyn crate::account::cloud_sync::CloudSyncPort>>,
    authorized: Arc<Mutex<ownership::AuthorizationCache>>,
    last_load_authorization: Option<ownership::AuthorizedProfiles>,
    effect_authorization: Option<(ProfileId, EnrollmentAuthorization)>,
}

impl ProductionPort {
    pub fn new(
        client: AccountClient,
        issuer: String,
        identity_db: PathBuf,
        local: DeviceDescriptorV1,
        signing_secret: [u8; 32],
        network: RuntimeHandle,
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
    ) -> Result<Self, &'static str> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|_| "enrollment_runtime")?;
        Ok(Self {
            clock,
            entropy,
            client,
            issuer,
            identity_db,
            local,
            signing_key: Arc::new(DeviceSigningKey::from_secret_bytes(signing_secret)),
            network,
            runtime,
            jwks: None,
            local_profiles: None,
            cloud_sync: None,
            authorized: Arc::new(Mutex::new(ownership::AuthorizationCache::default())),
            last_load_authorization: None,
            effect_authorization: None,
        })
    }

    pub fn with_local_profiles(mut self, profiles: Arc<dyn LocalProfileInventory>) -> Self {
        self.local_profiles = Some(profiles);
        self
    }

    pub fn with_cloud_sync(
        mut self,
        port: Arc<dyn crate::account::cloud_sync::CloudSyncPort>,
    ) -> Self {
        self.cloud_sync = Some(port);
        self
    }

    /// The coordinator creates its fixed effect lanes once. Each lane owns a
    /// current-thread runtime and JWKS snapshot; authority publication is a
    /// short shared cache operation, never a lock spanning HTTP/Store/SQLite.
    pub fn fork_effects(&self) -> Result<Self, Failure> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|_| Failure::Unavailable)?;
        Ok(Self {
            clock: self.clock.clone(),
            entropy: self.entropy.clone(),
            client: self.client.clone(),
            issuer: self.issuer.clone(),
            identity_db: self.identity_db.clone(),
            local: self.local.clone(),
            signing_key: self.signing_key.clone(),
            network: self.network.clone(),
            runtime,
            jwks: self.jwks.clone(),
            local_profiles: self.local_profiles.clone(),
            cloud_sync: self.cloud_sync.clone(),
            authorized: self.authorized.clone(),
            last_load_authorization: None,
            effect_authorization: None,
        })
    }
    fn check_effect(
        &self,
        profile: ProfileId,
        authority: &EnrollmentAuthorization,
    ) -> Result<(), Failure> {
        self.authorized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .authorize_effect(authority, profile)?;
        // The cache guard is gone before SQLite is opened or read.
        let mut repository = crate::identity::SqliteTrustRepository::open(&self.identity_db)
            .map_err(|_| Failure::Unavailable)?;
        verify_effect_target(&mut repository, authority, profile)
    }

    fn check_provision_effect(
        &self,
        profile: ProfileId,
        authority: &EnrollmentAuthorization,
        completion: &PreparedEnrollmentV1,
    ) -> Result<(), Failure> {
        self.check_effect(profile, authority)?;
        let repository = crate::identity::SqliteTrustRepository::open(&self.identity_db)
            .map_err(|_| Failure::Persistence)?;
        if repository
            .completed_enrollment(profile, self.local.device_id)
            .map_err(|_| Failure::Persistence)?
            .is_some_and(|(current, _)| current == *completion)
        {
            Ok(())
        } else {
            Err(Failure::AccessRevoked)
        }
    }
}

/// Reuse the existing transaction's ready/owner/version/complete-set fence.
/// A pending successor or changed intent pauses work without retiring proof.
pub(super) fn verify_effect_target(
    repository: &mut crate::identity::SqliteTrustRepository,
    authority: &EnrollmentAuthorization,
    profile: ProfileId,
) -> Result<(), Failure> {
    let targets = repository
        .account_targets(&authority.namespace, authority.owner.account)
        .map_err(|_| Failure::Unavailable)?;
    if !targets.contains(&profile) {
        return Err(Failure::Unavailable);
    }
    repository
        .validate_account_targets(
            &authority.namespace,
            authority.owner,
            authority.record_version,
            &targets,
        )
        .map_err(|_| Failure::Unavailable)
}

impl Port for ProductionPort {
    type Scope = (String, koru_account_core::KoruIdentityId);
    type Credential = EnrollmentAuthorization;
    type Candidate = EnrollmentCandidateV1;
    type Attempt = ProductionAttempt;
    type Completion = PreparedEnrollmentV1;

    fn fork_effects(&self) -> Result<Self, Failure> {
        ProductionPort::fork_effects(self)
    }

    fn prepare_effect(
        &mut self,
        profile: ProfileId,
        authority: &EnrollmentAuthorization,
    ) -> Result<(), Failure> {
        self.effect_authorization = None;
        self.check_effect(profile, authority)?;
        self.effect_authorization = Some((profile, authority.clone()));
        Ok(())
    }

    fn scope(credential: &EnrollmentAuthorization) -> Self::Scope {
        (credential.namespace.clone(), credential.owner.account)
    }

    fn snapshot_account_id(credential: &EnrollmentAuthorization) -> Option<String> {
        Some(credential.owner.account.to_string())
    }

    fn authority_successor(
        previous: &EnrollmentAuthorization,
        next: &EnrollmentAuthorization,
    ) -> bool {
        previous.namespace == next.namespace
            && previous.owner.account == next.owner.account
            && next.record_version > previous.record_version
    }

    fn refresh_continuity(
        previous: &EnrollmentAuthorization,
        next: &EnrollmentAuthorization,
    ) -> bool {
        previous.namespace == next.namespace
            && previous.owner == next.owner
            && previous.session_id == next.session_id
            && next.record_version > previous.record_version
    }

    fn refresh_authority(
        &mut self,
        previous: &EnrollmentAuthorization,
        next: &EnrollmentAuthorization,
        expected: &BTreeSet<ProfileId>,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        if !Self::refresh_continuity(previous, next) {
            return Err(Failure::SessionExpired);
        }
        // Explicit account-worker continuity plus the captured predecessor
        // authorizes a revision rebase. Owner equality alone never does.
        let authorization = AccountAuthorization::Bearer(&next.access);
        let session = self.client.session(&authorization).map_err(map_client)?;
        let profiles = self
            .client
            .profiles(&authorization)
            .map_err(map_client)?
            .profiles;
        let inventory = self
            .local_profiles
            .as_ref()
            .ok_or(Failure::Unavailable)?
            .inventory()
            .map_err(|_| Failure::Unavailable)?;
        let authorized = ownership::AuthorizedProfiles::verify(
            next,
            self.local.device_id,
            &session,
            &profiles,
            &inventory,
        )?;
        let revoked = expected
            .iter()
            .copied()
            .filter(|profile| !authorized.allows(next, *profile))
            .collect();
        // Existing CAS validates current ready owner/version and the whole set
        // in one transaction; its value is unchanged by this acknowledgement.
        crate::identity::SqliteTrustRepository::open(&self.identity_db)
            .and_then(|mut repository| {
                repository.validate_account_targets(
                    &next.namespace,
                    next.owner,
                    next.record_version,
                    expected,
                )
            })
            .map_err(|_| Failure::Persistence)?;
        self.authorized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .publish(authorized)?;
        Ok(revoked)
    }

    fn restored_targets(
        &mut self,
        credential: &EnrollmentAuthorization,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        self.last_load_authorization = None;
        self.authorized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .begin(credential)?;
        let authorization = AccountAuthorization::Bearer(&credential.access);
        let session = self.client.session(&authorization).map_err(map_client)?;
        let profiles = self
            .client
            .profiles(&authorization)
            .map_err(map_client)?
            .profiles;
        let inventory = self
            .local_profiles
            .as_ref()
            .ok_or(Failure::Unavailable)?
            .inventory()
            .map_err(|_| Failure::Unavailable)?;
        let authorized = ownership::AuthorizedProfiles::verify(
            credential,
            self.local.device_id,
            &session,
            &profiles,
            &inventory,
        )?;
        let mut repository = crate::identity::SqliteTrustRepository::open(&self.identity_db)
            .map_err(|_| Failure::Persistence)?;
        let targets = authorized.claim_and_load(&mut repository)?;
        // Older builds removed completed targets after only the hosted write.
        // Recover that exact durable intent for missing Profiles; an explicit
        // local removal marker is never absence and must not be resurrected.
        let targets = authorized.repair_missing_completed(
            &mut repository,
            &inventory,
            self.local.device_id,
            targets,
        )?;
        self.authorized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .publish(authorized.clone())?;
        self.last_load_authorization = Some(authorized);
        Ok(targets)
    }

    fn replace_targets(
        &mut self,
        credential: &EnrollmentAuthorization,
        expected: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), Failure> {
        for &profile in next.difference(expected) {
            if !self.authorize_profile(profile, credential)? {
                return Err(Failure::AccessRevoked);
            }
        }
        crate::identity::SqliteTrustRepository::open(&self.identity_db)
            .and_then(|mut repository| {
                repository.replace_account_targets(
                    &credential.namespace,
                    credential.owner,
                    credential.record_version,
                    expected,
                    next,
                )
            })
            .map_err(|_| Failure::Persistence)
    }

    fn authorize_profile(
        &mut self,
        profile: ProfileId,
        credential: &EnrollmentAuthorization,
    ) -> Result<bool, Failure> {
        if let Some(authorized) = &self.last_load_authorization {
            return Ok(authorized.allows(credential, profile));
        }
        Ok(self
            .authorized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .allows(credential, profile))
    }

    fn same_authority(previous: &EnrollmentAuthorization, next: &EnrollmentAuthorization) -> bool {
        previous.owner == next.owner
            && previous.namespace == next.namespace
            && previous.record_version == next.record_version
            && previous.session_id == next.session_id
    }

    fn cloud_primary(
        &mut self,
        profile: ProfileId,
        credential: &EnrollmentAuthorization,
    ) -> Result<bool, Failure> {
        let Some(port) = &self.cloud_sync else {
            return Ok(false);
        };
        let state = port.load().map_err(|_| Failure::Unavailable)?.accepted();
        Ok(state
            .accounts
            .get(&credential.owner.account.to_string())
            .and_then(|account| account.profiles.get(&profile))
            .is_some_and(|profile| profile.enabled))
    }
    fn reconcile(
        &mut self,
        profile: ProfileId,
    ) -> Result<Option<Completion<Self::Completion>>, Failure> {
        let repository = crate::identity::SqliteTrustRepository::open(&self.identity_db)
            .map_err(|_| Failure::Persistence)?;
        repository
            .completed_enrollment(profile, self.local.device_id)
            .map(|value| {
                value.map(|(value, durable_revision)| Completion {
                    value,
                    durable_revision,
                })
            })
            .map_err(|_| Failure::Persistence)
    }

    fn completion_current(
        &mut self,
        profile: ProfileId,
        completion: &Self::Completion,
    ) -> Result<bool, Failure> {
        Ok(self
            .reconcile(profile)?
            .is_some_and(|current| current.value == *completion))
    }

    fn discover(
        &mut self,
        profile: ProfileId,
        credential: &EnrollmentAuthorization,
        cancel: &AtomicBool,
    ) -> Result<Discovery<Self::Candidate, Self::Attempt>, Failure> {
        self.discover_profiles(profile, credential, None, cancel)
    }

    fn refresh_discovery(
        &mut self,
        profile: ProfileId,
        credential: &EnrollmentAuthorization,
        previous: &ProductionAttempt,
        cancel: &AtomicBool,
    ) -> Result<Discovery<Self::Candidate, Self::Attempt>, Failure> {
        self.discover_profiles(profile, credential, Some(previous), cancel)
    }

    fn attempt_expired(&self, attempt: &ProductionAttempt) -> bool {
        self.clock
            .now_millis()
            .is_some_and(|now| now >= attempt.verified.claims().expires_at_millis)
    }

    fn enroll(
        &mut self,
        profile: ProfileId,
        candidate: &EnrollmentCandidateV1,
        attempt: &ProductionAttempt,
        cancel: &AtomicBool,
    ) -> Result<Completion<Self::Completion>, Failure> {
        let prepared = self.runtime.block_on(run_initiator(
            self.network.clone(),
            self.local.clone(),
            &self.signing_key,
            profile,
            candidate.clone(),
            attempt.clone(),
            cancel,
            self.clock.as_ref(),
        ))?;
        if cancel.load(Ordering::Acquire) {
            // The responder's commit is already proven even though this
            // installation has not committed yet. Resume the same peer.
            return Err(Failure::Uncertain);
        }
        let (_, authority) = self
            .effect_authorization
            .as_ref()
            .filter(|(captured, _)| *captured == profile)
            .ok_or(Failure::Uncertain)?;
        // Transport can outlive a vault transition. A proven remote commit
        // remains replayable, but cannot cross the current local ready fence.
        self.check_effect(profile, authority)
            .map_err(|_| Failure::Uncertain)?;
        let mut repository = crate::identity::SqliteTrustRepository::open(&self.identity_db)
            .map_err(|_| Failure::Persistence)?;
        let durable_revision = match repository.commit_enrollment(
            &prepared,
            &attempt.verified,
            self.local.device_id,
        ) {
            Ok(ApplyOutcome::Applied { effective_revision })
            | Ok(ApplyOutcome::AlreadyApplied { effective_revision }) => effective_revision,
            Ok(ApplyOutcome::Stale { .. }) => return Err(Failure::Persistence),
            Err(_) => return Err(Failure::Persistence),
        };
        Ok(Completion {
            value: prepared,
            durable_revision,
        })
    }

    fn provision(
        &mut self,
        profile: ProfileId,
        completion: &PreparedEnrollmentV1,
        credential: &EnrollmentAuthorization,
    ) -> Result<(), Failure> {
        self.check_effect(profile, credential)?;
        provisioning::provision(
            &self.client,
            self.local_profiles.as_deref().ok_or(Failure::Unavailable)?,
            profile,
            self.local.device_id,
            completion,
            credential,
            self.clock.now_millis().ok_or(Failure::Unavailable)?,
            || self.check_provision_effect(profile, credential, completion),
        )
    }

    fn discard(&mut self, _profile: ProfileId) {
        // Ceremonies are held in memory until commit. Established trust is never
        // represented as provisional material and is therefore not removed here.
    }
}

fn provision_completion(
    client: &AccountClient,
    profile: ProfileId,
    local: koru_device::DeviceId,
    completion: &PreparedEnrollmentV1,
    authorization: &AccountAuthorization<'_>,
) -> Result<(), Failure> {
    let receipt = completion
        .encode_receipt()
        .map_err(|_| Failure::BadAttestation)?;
    client
        .provision_device(
            profile,
            local,
            &ProvisionDeviceRequestV1 {
                version: Version1,
                attestation: DeviceAttestationEvidenceV1 {
                    compact: completion.attestation.compact.clone(),
                },
                completion_receipt: EnrollmentCompletionReceiptV1(receipt),
            },
            authorization,
        )
        .map(|_| ())
        .map_err(map_client)
}

async fn run_initiator(
    network: RuntimeHandle,
    local: DeviceDescriptorV1,
    signing_key: &DeviceSigningKey,
    profile: ProfileId,
    candidate: EnrollmentCandidateV1,
    attempt: ProductionAttempt,
    cancel: &AtomicBool,
    clock: &dyn Clock,
) -> Result<PreparedEnrollmentV1, Failure> {
    let (endpoint, _) = validate_attempt(&candidate, &attempt, clock)?;
    let mut session = cancellable(cancel, network.open_enrollment(endpoint))
        .await?
        .map_err(map_transport)?;
    run_initiator_on(
        &mut session,
        local,
        signing_key,
        profile,
        candidate,
        attempt,
        cancel,
        clock,
    )
    .await
}

async fn run_initiator_on(
    session: &mut impl CeremonySession,
    local: DeviceDescriptorV1,
    signing_key: &DeviceSigningKey,
    profile: ProfileId,
    candidate: EnrollmentCandidateV1,
    attempt: ProductionAttempt,
    cancel: &AtomicBool,
    clock: &dyn Clock,
) -> Result<PreparedEnrollmentV1, Failure> {
    let (endpoint, signing_key_expected) = validate_attempt(&candidate, &attempt, clock)?;
    cancellable(
        cancel,
        session.send_frame(&EnrollmentFrameV1::Begin {
            attestation: attempt.evidence.clone(),
            profile_id: profile,
            existing_device_id: candidate.device_id,
            joining_device: local.clone(),
            joining_challenge: attempt.joining_challenge,
        }),
    )
    .await?
    .map_err(map_transport)?;
    let transcript = match cancellable(cancel, session.receive_frame())
        .await?
        .map_err(map_transport)?
    {
        EnrollmentFrameV1::Challenge { transcript } => transcript,
        EnrollmentFrameV1::Complete { prepared } => {
            validate_completion(
                &prepared,
                &local,
                profile,
                &candidate,
                &attempt,
                session.remote_endpoint(),
                clock,
            )?;
            return Ok(*prepared);
        }
        EnrollmentFrameV1::Error { code } => return Err(map_remote(&code)),
        _ => return Err(Failure::BadAttestation),
    };
    validate_transcript(
        &transcript,
        &local,
        profile,
        &candidate,
        &attempt,
        endpoint,
        signing_key_expected,
        session.remote_endpoint(),
    )?;
    let join = EnrollmentJoinV1::sign(
        attempt.evidence.clone(),
        &attempt.verified,
        transcript.clone(),
        signing_key,
        clock.now_millis().unwrap_or(0),
    )
    .map_err(|_| Failure::BadAttestation)?;
    // Once Join may have been delivered, losing the reply cannot establish
    // whether the responder committed. Retain this exact attempt and peer.
    cancellable(
        cancel,
        session.send_frame(&EnrollmentFrameV1::Join {
            join: Box::new(join),
        }),
    )
    .await
    .map_err(|_| Failure::Uncertain)?
    .map_err(|_| Failure::Uncertain)?;
    let prepared = match cancellable(cancel, session.receive_frame())
        .await
        .map_err(|_| Failure::Uncertain)?
        .map_err(|_| Failure::Uncertain)?
    {
        EnrollmentFrameV1::Complete { prepared } => *prepared,
        EnrollmentFrameV1::Error { code } => {
            return Err(match map_remote(&code) {
                Failure::Offline | Failure::Unavailable => Failure::Uncertain,
                other => other,
            });
        }
        _ => return Err(Failure::BadAttestation),
    };
    if prepared.transcript != transcript {
        return Err(Failure::BadAttestation);
    }
    validate_completion(
        &prepared,
        &local,
        profile,
        &candidate,
        &attempt,
        session.remote_endpoint(),
        clock,
    )?;
    Ok(prepared)
}

/// Cheap checks also run before dialing an endpoint.
fn validate_attempt(
    candidate: &EnrollmentCandidateV1,
    attempt: &ProductionAttempt,
    clock: &dyn Clock,
) -> Result<(ReplicationEndpointId, DevicePublicKey), Failure> {
    if clock
        .now_millis()
        .is_none_or(|now| now >= attempt.verified.claims().expires_at_millis)
    {
        return Err(Failure::BadAttestation);
    }
    if candidate.enrollment_protocol_version != ENROLLMENT_VERSION
        || candidate.peer_protocol_version != PEER_PROTOCOL_VERSION
    {
        return Err(Failure::Unavailable);
    }
    let endpoint_bytes: [u8; 32] = candidate
        .endpoint_id
        .as_slice()
        .try_into()
        .map_err(|_| Failure::BadAttestation)?;
    let signing_bytes: [u8; 32] = candidate
        .signing_public_key
        .as_slice()
        .try_into()
        .map_err(|_| Failure::BadAttestation)?;
    Ok((
        ReplicationEndpointId::from_bytes(endpoint_bytes),
        DevicePublicKey::from_bytes(signing_bytes),
    ))
}

fn validate_transcript(
    transcript: &EnrollmentTranscriptV1,
    local: &DeviceDescriptorV1,
    profile: ProfileId,
    candidate: &EnrollmentCandidateV1,
    attempt: &ProductionAttempt,
    endpoint: ReplicationEndpointId,
    signing_key: DevicePublicKey,
    remote_endpoint: ReplicationEndpointId,
) -> Result<(), Failure> {
    if transcript.profile_id != profile
        || transcript.existing_device.device_id != candidate.device_id
        || transcript.existing_device.signing_public_key != signing_key
        || transcript.existing_device.endpoint_id != endpoint
        || &transcript.joining_device != local
        || transcript.joining_challenge != attempt.joining_challenge
        || transcript.verify_attestation(&attempt.verified).is_err()
        || verify_enrollment_joiner_transport(transcript, local.endpoint_id, remote_endpoint)
            .is_err()
    {
        return Err(Failure::BadAttestation);
    }
    Ok(())
}

fn validate_completion(
    prepared: &PreparedEnrollmentV1,
    local: &DeviceDescriptorV1,
    profile: ProfileId,
    candidate: &EnrollmentCandidateV1,
    attempt: &ProductionAttempt,
    remote_endpoint: ReplicationEndpointId,
    clock: &dyn Clock,
) -> Result<(), Failure> {
    // Recheck expiry after receiving either normal or replayed completion.
    let (endpoint, signing_key) = validate_attempt(candidate, attempt, clock)?;
    validate_transcript(
        &prepared.transcript,
        local,
        profile,
        candidate,
        attempt,
        endpoint,
        signing_key,
        remote_endpoint,
    )?;
    if prepared.attestation != attempt.evidence || prepared.verify(&attempt.verified).is_err() {
        return Err(Failure::BadAttestation);
    }
    Ok(())
}

async fn cancellable<F: Future>(cancel: &AtomicBool, future: F) -> Result<F::Output, Failure> {
    tokio::pin!(future);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Failure::Unavailable);
        }
        tokio::select! {
            output = &mut future => return Ok(output),
            _ = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
    }
}

fn map_client(error: ClientError) -> Failure {
    match error {
        ClientError::RateLimited { retry_after_millis } => {
            Failure::RateLimited(Duration::from_millis(retry_after_millis))
        }
        ClientError::Api { code, .. }
            if code.as_str() == koru_account_wire::ErrorCode::UNAUTHENTICATED =>
        {
            Failure::SessionExpired
        }
        ClientError::Api { code, .. }
            if code.as_str() == koru_account_wire::ErrorCode::PERMISSION_DENIED =>
        {
            Failure::AccessRevoked
        }
        ClientError::Transport(_) => Failure::Unavailable,
        _ => Failure::Unavailable,
    }
}

fn map_attestation(error: AttestationVerificationError) -> Failure {
    match error {
        AttestationVerificationError::RefreshFailed => Failure::Unavailable,
        AttestationVerificationError::InvalidKeySet
        | AttestationVerificationError::InvalidAttestation => Failure::BadAttestation,
    }
}

fn map_transport(error: EnrollmentTransportError) -> Failure {
    match error {
        EnrollmentTransportError::EndpointMismatch
        | EnrollmentTransportError::MalformedFrame
        | EnrollmentTransportError::OversizedFrame
        | EnrollmentTransportError::FrameLimit => Failure::BadAttestation,
        _ => Failure::Offline,
    }
}

fn map_remote(code: &str) -> Failure {
    match code {
        "profile_access_revoked" | "profile_not_authorized" => Failure::AccessRevoked,
        "session_revoked" => Failure::SessionExpired,
        "attestation_invalid" | "enrollment_invalid" => Failure::BadAttestation,
        "trust_persistence_failed" => Failure::Persistence,
        _ => Failure::Unavailable,
    }
}

pub struct ResponderSessionGate {
    eligibility: Mutex<ResponderEligibility>,
    commit: Mutex<()>,
}

#[derive(Default)]
struct ResponderEligibility {
    next_incarnation: u64,
    accounts: std::collections::BTreeMap<koru_account_core::KoruIdentityId, (String, u64)>,
}

#[derive(Clone, Copy)]
pub(crate) struct ResponderTicket {
    account: koru_account_core::KoruIdentityId,
    incarnation: u64,
}

impl ResponderSessionGate {
    pub fn new() -> Self {
        Self {
            eligibility: Mutex::new(ResponderEligibility::default()),
            commit: Mutex::new(()),
        }
    }

    pub fn enable_account(&self, account: koru_account_core::KoruIdentityId, session_id: String) {
        let mut eligibility = self
            .eligibility
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if eligibility
            .accounts
            .get(&account)
            .is_some_and(|(current, _)| *current == session_id)
        {
            return;
        }
        eligibility.next_incarnation = eligibility
            .next_incarnation
            .checked_add(1)
            .expect("responder incarnation exhausted");
        let incarnation = eligibility.next_incarnation;
        eligibility
            .accounts
            .insert(account, (session_id, incarnation));
    }

    pub fn disable_account(&self, account: koru_account_core::KoruIdentityId) {
        self.eligibility
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .accounts
            .remove(&account);
    }

    pub fn disable_account_and_wait(&self, account: koru_account_core::KoruIdentityId) {
        self.disable_account(account);
        drop(
            self.commit
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
    }

    pub fn disable_account_session(
        &self,
        account: koru_account_core::KoruIdentityId,
        session_id: &str,
    ) {
        let mut eligibility = self
            .eligibility
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if eligibility
            .accounts
            .get(&account)
            .is_some_and(|(current, _)| current == session_id)
        {
            eligibility.accounts.remove(&account);
        }
    }

    pub fn disable_account_session_and_wait(
        &self,
        account: koru_account_core::KoruIdentityId,
        session_id: &str,
    ) {
        self.disable_account_session(account, session_id);
        drop(
            self.commit
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
    }

    pub fn disable(&self) {
        self.eligibility
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .accounts
            .clear();
    }

    pub fn disable_and_wait(&self) {
        self.disable();
        drop(
            self.commit
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
    }

    pub(crate) fn ticket(
        &self,
        account: koru_account_core::KoruIdentityId,
    ) -> Result<ResponderTicket, EnrollmentHandlerError> {
        let eligibility = self
            .eligibility
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (_, incarnation) = eligibility
            .accounts
            .get(&account)
            .ok_or_else(|| refused("session_revoked"))?;
        Ok(ResponderTicket {
            account,
            incarnation: *incarnation,
        })
    }

    pub(crate) fn ensure(&self, ticket: ResponderTicket) -> Result<(), EnrollmentHandlerError> {
        if self
            .eligibility
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .accounts
            .get(&ticket.account)
            .is_some_and(|(_, incarnation)| *incarnation == ticket.incarnation)
        {
            Ok(())
        } else {
            Err(refused("session_revoked"))
        }
    }

    pub(crate) fn commit<T>(
        &self,
        epoch: ResponderTicket,
        apply: impl FnOnce() -> Result<T, EnrollmentHandlerError>,
    ) -> Result<T, EnrollmentHandlerError> {
        let _guard = self
            .commit
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.ensure(epoch)?;
        apply()
    }
}

impl Default for ResponderSessionGate {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ProductionResponder {
    client: AccountClient,
    issuer: String,
    local_profiles: Arc<dyn LocalProfileInventory>,
    clock: Arc<dyn Clock>,
    identity_db: PathBuf,
    local: DeviceDescriptorV1,
    signing_secret: [u8; 32],
    jwks: Arc<Mutex<Option<JwksResponseV1>>>,
    durable: Arc<dyn Fn(u64) + Send + Sync>,
    session: Arc<ResponderSessionGate>,
}

impl ProductionResponder {
    pub fn new(
        client: AccountClient,
        issuer: String,
        local_profiles: Arc<dyn LocalProfileInventory>,
        clock: Arc<dyn Clock>,
        identity_db: PathBuf,
        local: DeviceDescriptorV1,
        signing_secret: [u8; 32],
        session: Arc<ResponderSessionGate>,
        durable: impl Fn(u64) + Send + Sync + 'static,
    ) -> Self {
        Self {
            client,
            issuer,
            local_profiles,
            clock,
            identity_db,
            local,
            signing_secret,
            jwks: Arc::new(Mutex::new(None)),
            durable: Arc::new(durable),
            session,
        }
    }
}

impl EnrollmentHandler for ProductionResponder {
    fn handle<'a>(&'a self, session: &'a mut EnrollmentSession) -> EnrollmentHandlerFuture<'a> {
        Box::pin(async move { self.respond(session).await })
    }
}

impl ProductionResponder {
    async fn respond(&self, session: &mut EnrollmentSession) -> Result<(), EnrollmentHandlerError> {
        self.respond_on(session, &DiskInboundRepository(self.identity_db.clone()))
            .await
    }

    async fn respond_on(
        &self,
        session: &mut impl CeremonySession,
        repository: &impl InboundRepository,
    ) -> Result<(), EnrollmentHandlerError> {
        let (evidence, profile, joining, joining_challenge) = match session
            .receive_frame()
            .await
            .map_err(|_| refused("enrollment_transport"))?
        {
            EnrollmentFrameV1::Begin {
                attestation,
                profile_id,
                existing_device_id,
                joining_device,
                joining_challenge,
            } if existing_device_id == self.local.device_id => {
                (attestation, profile_id, joining_device, joining_challenge)
            }
            _ => return Err(refused("enrollment_invalid")),
        };
        if joining.verify().is_err() || joining.endpoint_id != session.remote_endpoint() {
            return Err(refused("attestation_invalid"));
        }
        let verified = verify_inbound(
            self.client.clone(),
            self.issuer.clone(),
            Arc::clone(&self.jwks),
            evidence.clone(),
            Arc::clone(&self.clock),
        )
        .await?;
        // Account eligibility comes from verified attestation authority, never
        // a different held account or an untrusted Begin field.
        let epoch = self.session.ticket(verified.claims().koru_identity_id)?;
        if verified.claims().device_id != joining.device_id
            || verified.claims().signing_public_key != joining.signing_public_key
            || verified.claims().endpoint_id != joining.endpoint_id
            || !verified.claims().authorized_profile_ids.contains(&profile)
        {
            return Err(refused("attestation_invalid"));
        }
        authorize_local_profile(
            Arc::clone(&self.local_profiles),
            profile,
            verified.claims().koru_identity_id,
        )
        .await?;
        self.session.ensure(epoch)?;

        if let Some(prepared) = repository
            .replay(ReplayRequest {
                evidence: evidence.clone(),
                jti: verified.claims().token_id.clone(),
                profile,
                local: self.local.clone(),
                joining: joining.clone(),
                joining_challenge,
            })
            .await?
        {
            self.session.ensure(epoch)?;
            session
                .send_frame(&EnrollmentFrameV1::Complete {
                    prepared: Box::new(prepared),
                })
                .await
                .map_err(|_| refused("enrollment_transport"))?;
            return Ok(());
        }

        let transcript = EnrollmentTranscriptV1::new(
            &verified,
            profile,
            self.local.clone(),
            joining,
            joining_challenge,
        )
        .map_err(|_| refused("enrollment_invalid"))?;
        verify_enrollment_responder_transport(
            &transcript,
            self.local.endpoint_id,
            session.remote_endpoint(),
        )
        .map_err(|_| refused("enrollment_invalid"))?;
        session
            .send_frame(&EnrollmentFrameV1::Challenge {
                transcript: transcript.clone(),
            })
            .await
            .map_err(|_| refused("enrollment_transport"))?;
        let join = match session
            .receive_frame()
            .await
            .map_err(|_| refused("enrollment_transport"))?
        {
            EnrollmentFrameV1::Join { join } if join.transcript == transcript => *join,
            _ => return Err(refused("enrollment_invalid")),
        };
        self.session.ensure(epoch)?;
        let prepared = join
            .complete(
                &verified,
                &DeviceSigningKey::from_secret_bytes(self.signing_secret),
            )
            .map_err(|_| refused("enrollment_invalid"))?;
        let (revision, applied) = repository
            .persist(
                prepared.clone(),
                verified,
                self.local.device_id,
                Arc::clone(&self.session),
                epoch,
            )
            .await?;
        if applied {
            (self.durable)(revision);
        }
        session
            .send_frame(&EnrollmentFrameV1::Complete {
                prepared: Box::new(prepared),
            })
            .await
            .map_err(|_| refused("enrollment_transport"))?;
        Ok(())
    }
}

async fn verify_inbound(
    client: AccountClient,
    issuer: String,
    jwks: Arc<Mutex<Option<JwksResponseV1>>>,
    evidence: SignedDeviceAttestationV1,
    clock: Arc<dyn Clock>,
) -> Result<VerifiedDeviceAttestationV1, EnrollmentHandlerError> {
    tokio::task::spawn_blocking(move || {
        let mut cached = jwks.lock().unwrap_or_else(|error| error.into_inner());
        if cached.is_none() {
            *cached = Some(
                client
                    .jwks()
                    .map_err(|_| refused("enrollment_unavailable"))?,
            );
        }
        client
            .verify_device_attestation(
                &evidence,
                &issuer,
                clock.now_millis().unwrap_or(0),
                cached.as_mut().expect("JWKS initialized above"),
            )
            .map_err(|error| match error {
                AttestationVerificationError::RefreshFailed => refused("enrollment_unavailable"),
                _ => refused("attestation_invalid"),
            })
    })
    .await
    .map_err(|_| refused("enrollment_unavailable"))?
}

async fn authorize_local_profile(
    local_profiles: Arc<dyn LocalProfileInventory>,
    profile: ProfileId,
    identity: koru_account_core::KoruIdentityId,
) -> Result<(), EnrollmentHandlerError> {
    tokio::task::spawn_blocking(move || {
        local_profiles
            .authorize(profile, identity)
            .map_err(|error| {
                refused(if error.code == "profile_not_authorized" {
                    "profile_not_authorized"
                } else {
                    "enrollment_unavailable"
                })
            })?
            .then_some(())
            .ok_or_else(|| refused("profile_not_authorized"))
    })
    .await
    .map_err(|_| refused("enrollment_unavailable"))?
}

async fn replay_completion(
    identity_db: PathBuf,
    request: ReplayRequest,
) -> Result<Option<PreparedEnrollmentV1>, EnrollmentHandlerError> {
    tokio::task::spawn_blocking(move || {
        let repository = crate::identity::SqliteTrustRepository::open(&identity_db)
            .map_err(|_| refused("trust_persistence_failed"))?;
        let value = repository
            .enrollment_by_jti(&request.jti)
            .map_err(|_| refused("trust_persistence_failed"))?;
        request.validate(value)
    })
    .await
    .map_err(|_| refused("trust_persistence_failed"))?
}

async fn persist_inbound(
    identity_db: PathBuf,
    prepared: PreparedEnrollmentV1,
    verified: VerifiedDeviceAttestationV1,
    local: koru_device::DeviceId,
    session: Arc<ResponderSessionGate>,
    epoch: ResponderTicket,
) -> Result<(u64, bool), EnrollmentHandlerError> {
    tokio::task::spawn_blocking(move || {
        session.commit(epoch, || {
            let mut repository = crate::identity::SqliteTrustRepository::open(&identity_db)
                .map_err(|_| refused("trust_persistence_failed"))?;
            match repository.commit_enrollment(&prepared, &verified, local) {
                Ok(ApplyOutcome::Applied { effective_revision }) => Ok((effective_revision, true)),
                Ok(ApplyOutcome::AlreadyApplied { effective_revision }) => {
                    Ok((effective_revision, false))
                }
                Ok(ApplyOutcome::Stale { .. }) => Err(refused("trust_persistence_failed")),
                Err(_) => Err(refused("trust_persistence_failed")),
            }
        })
    })
    .await
    .map_err(|_| refused("trust_persistence_failed"))?
}

fn refused(code: &'static str) -> EnrollmentHandlerError {
    EnrollmentHandlerError::refused(code)
}

#[cfg(test)]
mod tests;
