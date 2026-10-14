// SPDX-License-Identifier: MPL-2.0

use std::{future::Future, pin::Pin};

use crate::*;

pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryError {
    Conflict,
    NotFound,
    Unavailable,
    Corrupt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    InvalidInput,
    KeyUnavailable,
    VerificationFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderVerificationError {
    Rejected,
    InvalidIssuer,
    InvalidAudience,
    InvalidSignature,
    Expired,
    NonceMismatch,
    PkceMismatch,
    Unavailable,
}

pub trait IdentityRepository: Send + Sync {
    fn find_koru_identity<'a>(
        &'a self,
        id: KoruIdentityId,
    ) -> PortFuture<'a, Result<Option<KoruIdentity>, RepositoryError>>;
    fn find_auth_identity<'a>(
        &'a self,
        issuer: &'a str,
        subject: &'a str,
    ) -> PortFuture<'a, Result<Option<AuthIdentity>, RepositoryError>>;
    fn list_auth_identities<'a>(
        &'a self,
        id: KoruIdentityId,
    ) -> PortFuture<'a, Result<Vec<AuthIdentity>, RepositoryError>>;
    fn create_identity_with_auth<'a>(
        &'a self,
        identity: KoruIdentity,
        auth_identity: AuthIdentity,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;
    fn update_identity_display_name<'a>(
        &'a self,
        _id: KoruIdentityId,
        _display_name: &'a str,
        _updated_at_millis: UnixMillis,
    ) -> PortFuture<'a, Result<KoruIdentity, RepositoryError>> {
        Box::pin(async { Err(RepositoryError::Unavailable) })
    }
    fn link_auth_identity<'a>(
        &'a self,
        auth_identity: AuthIdentity,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;
}

/// Additive lookup needed by identity resolution. Email remains non-unique
/// collision metadata and this port never resolves an account from email.
pub trait IdentityCollisionRepository: IdentityRepository {
    fn find_koru_identities_by_verified_email<'a>(
        &'a self,
        normalized_email: &'a str,
    ) -> PortFuture<'a, Result<Vec<KoruIdentityId>, RepositoryError>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollisionLinkCommit {
    Linked(AuthIdentity),
    AlreadyLinked(AuthIdentity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionLinkError {
    FlowNotFound,
    FlowExpired,
    FlowConsumed,
    IdentityOwnedByAnother,
    Unavailable,
    Corrupt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthProfileLinkCommit {
    Linked(AuthIdentity),
    AlreadyLinked(AuthIdentity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthProfileLinkError {
    ProfileNotOwned,
    IdentityOwnedByAnother,
    Unavailable,
    Corrupt,
}

/// Transaction boundary used by provider-neutral identity resolution.
///
/// Implementations must make `complete_collision_link` one transaction: lock
/// and consume the pending, unexpired flow and establish the globally unique
/// `(issuer, subject)` link, or commit neither operation. This is deliberately
/// a combined account/flow port; composing `consume_flow` and
/// `link_auth_identity` in the service cannot provide that guarantee.
pub trait IdentityResolutionRepository: IdentityCollisionRepository + AuthFlowRepository {
    /// Serialize confirm/cancel, recheck exact subject before email collision,
    /// and commit identity rows, bounded child flows and the sealed terminal result
    /// atomically. Terminal replay precedes resolution and retains the original
    /// deadline. Missing-row/email-predicate races must be excluded against all
    /// identity writers, including linking and erasure. No session, device or
    /// Profile is created here. A failed attempt leaves the authority pending.
    fn commit_account_creation<'a>(
        &'a self,
        _request: AccountCreationRequest,
    ) -> PortFuture<'a, Result<AccountCreationOutcome, AccountCreationError>> {
        Box::pin(async { Err(AccountCreationError::Unavailable) })
    }

    /// Durably records a successful exact-provider authentication. The update
    /// is monotonic under races (`last_authenticated_at` never moves backward)
    /// and returns the persisted row.
    fn record_authentication<'a>(
        &'a self,
        auth_identity_id: AuthIdentityId,
        authenticated_at_millis: UnixMillis,
    ) -> PortFuture<'a, Result<AuthIdentity, RepositoryError>>;

    fn complete_collision_link<'a>(
        &'a self,
        flow_id: AuthFlowId,
        auth_identity: AuthIdentity,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<CollisionLinkCommit, CollisionLinkError>>;

    /// Atomically establishes the exact provider identity and authorizes it
    /// for an already-owned Profile. Implementations must commit both or
    /// neither; this prevents a failed Profile precondition from leaving a
    /// partially linked Auth Identity.
    fn link_auth_identity_and_profile<'a>(
        &'a self,
        auth_identity: AuthIdentity,
        profile_id: ProfileId,
    ) -> PortFuture<'a, Result<AuthProfileLinkCommit, AuthProfileLinkError>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileAuthorizationRemovalRequest {
    pub koru_identity_id: KoruIdentityId,
    pub profile_id: ProfileId,
    pub auth_identity_id: AuthIdentityId,
    /// Identity which authenticated the live session authorizing this request.
    pub authenticated_by: AuthIdentityId,
    pub authenticated_at_millis: UnixMillis,
    pub confirm_profile_offline_only: bool,
    pub now_millis: UnixMillis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileAuthorizationRemovalOutcome {
    Removed { profile_offline_only: bool },
    AlreadyRemoved { profile_offline_only: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileAuthorizationRemovalError {
    PermissionDenied,
    LastAuthorizationNeedsConfirmation,
    LastAuthorizationNeedsFreshExactGoogleAuthentication,
    Unavailable,
    Corrupt,
}

pub trait ProfileRepository: Send + Sync {
    fn find_profile<'a>(
        &'a self,
        profile_id: ProfileId,
    ) -> PortFuture<'a, Result<Option<ProfileBinding>, RepositoryError>>;
    fn list_profiles<'a>(
        &'a self,
        identity_id: KoruIdentityId,
        authenticated_by: AuthIdentityId,
    ) -> PortFuture<'a, Result<Vec<ProfileBinding>, RepositoryError>>;
    fn list_profile_auth_identities<'a>(
        &'a self,
        _identity_id: KoruIdentityId,
        _profile_id: ProfileId,
    ) -> PortFuture<'a, Result<Vec<AuthIdentity>, RepositoryError>> {
        Box::pin(async { Err(RepositoryError::Unavailable) })
    }
    /// Remove only one Profile authorization edge. The Auth Identity remains
    /// attached to its Koru Identity and to every other authorized Profile.
    fn remove_profile_authorization<'a>(
        &'a self,
        _request: ProfileAuthorizationRemovalRequest,
    ) -> PortFuture<'a, Result<ProfileAuthorizationRemovalOutcome, ProfileAuthorizationRemovalError>>
    {
        Box::pin(async { Err(ProfileAuthorizationRemovalError::Unavailable) })
    }
    fn delete_profile<'a>(
        &'a self,
        _request: DeleteProfileRequest,
    ) -> PortFuture<'a, Result<ProfileLifecycleMutationOutcome, ProfileLifecycleMutationError>>
    {
        Box::pin(async { Err(ProfileLifecycleMutationError::Unavailable) })
    }
    fn restore_profile<'a>(
        &'a self,
        _request: RestoreProfileRequest,
    ) -> PortFuture<'a, Result<ProfileLifecycleMutationOutcome, ProfileLifecycleMutationError>>
    {
        Box::pin(async { Err(ProfileLifecycleMutationError::Unavailable) })
    }
    fn resolve_profile_lifecycles<'a>(
        &'a self,
        _identity_id: KoruIdentityId,
        _profile_ids: &'a [ProfileId],
    ) -> PortFuture<'a, Result<Vec<ProfileLifecycle>, RepositoryError>> {
        Box::pin(async { Err(RepositoryError::Unavailable) })
    }
    fn attach_profile<'a>(
        &'a self,
        profile: ProfileBinding,
        authorization: ProfileAuthBinding,
        device_id: Option<DeviceId>,
    ) -> PortFuture<'a, Result<ProfileAttachOutcome, RepositoryError>>;
    fn rebind_erased_profile<'a>(
        &'a self,
        request: ProfileRebindRequest,
    ) -> PortFuture<'a, Result<ProfileRebindOutcome, ProfileRebindError>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileAttachOutcome {
    Attached(ProfileBinding),
    AlreadyAttached(ProfileBinding),
    AuthorizationAdded(ProfileBinding),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileRebindRequest {
    pub profile: ProfileBinding,
    pub authorization: ProfileAuthBinding,
    pub erased_identity_id: KoruIdentityId,
    pub authenticated_at_millis: UnixMillis,
    pub now_millis: UnixMillis,
    pub explicit_rebind: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileRebindOutcome {
    Rebound(ProfileBinding),
    AlreadyRebound(ProfileBinding),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileRebindError {
    IntentRequired,
    AuthenticationInvalid,
    OldIdentityStillLive,
    ProfileOwnedByAnother,
    Unavailable,
    Corrupt,
}

pub trait DeviceRepository: Send + Sync {
    fn find_device<'a>(
        &'a self,
        identity_id: KoruIdentityId,
        id: DeviceId,
    ) -> PortFuture<'a, Result<Option<AccountDevice>, RepositoryError>>;
    fn register_device<'a>(
        &'a self,
        device: AccountDevice,
        proof: Option<VerifiedDeviceOwnership>,
    ) -> PortFuture<'a, Result<DeviceRegistrationOutcome, RepositoryError>>;
    fn list_devices<'a>(
        &'a self,
        id: KoruIdentityId,
    ) -> PortFuture<'a, Result<Vec<AccountDevice>, RepositoryError>>;
    fn list_devices_for_session<'a>(
        &'a self,
        identity_id: KoruIdentityId,
        authenticated_by: AuthIdentityId,
        session_id: AccountSessionId,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<AccountDevicePage, RepositoryError>>;
    fn set_profile_state<'a>(
        &'a self,
        binding: DeviceProfileBinding,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;
    fn revoke_device<'a>(
        &'a self,
        identity_id: KoruIdentityId,
        device_id: DeviceId,
        at_millis: UnixMillis,
    ) -> PortFuture<'a, Result<AccountDevice, RepositoryError>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceRegistrationOutcome {
    Registered(AccountDevice),
    AlreadyRegistered(AccountDevice),
}

#[derive(Clone, PartialEq, Eq)]
pub struct EnrollmentCandidateQuery {
    pub koru_identity_id: KoruIdentityId,
    pub authenticated_by: AuthIdentityId,
    pub session_id: AccountSessionId,
    pub now_millis: UnixMillis,
    pub profile_id: ProfileId,
    pub caller_device_id: DeviceId,
    pub enrollment_protocol_version: u32,
    pub peer_protocol_version: u32,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PrepareDeviceAttestation {
    pub koru_identity_id: KoruIdentityId,
    pub authenticated_by: AuthIdentityId,
    pub profile_id: ProfileId,
    pub device_id: DeviceId,
    pub session_id: AccountSessionId,
    pub token_id: String,
    pub now_millis: UnixMillis,
    pub expires_at_millis: UnixMillis,
}

#[derive(Clone, PartialEq, Eq)]
pub struct BindPendingDeviceAttestation {
    pub koru_identity_id: KoruIdentityId,
    pub authenticated_by: AuthIdentityId,
    pub profile_id: ProfileId,
    pub device_id: DeviceId,
    pub session_id: AccountSessionId,
    pub token_id: String,
    pub attestation_digest: [u8; 32],
    pub expires_at_millis: UnixMillis,
    pub now_millis: UnixMillis,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CompleteDeviceProvisioning {
    pub koru_identity_id: KoruIdentityId,
    pub authenticated_by: AuthIdentityId,
    pub profile_id: ProfileId,
    pub device_id: DeviceId,
    pub session_id: AccountSessionId,
    pub token_id: String,
    pub attestation_digest: [u8; 32],
    pub mutation_digest: [u8; 32],
    pub existing_device_id: DeviceId,
    pub existing_signing_public_key: [u8; 32],
    pub existing_endpoint_id: [u8; 32],
    pub now_millis: UnixMillis,
    /// False permits only an already committed exact retry; it can never
    /// transition pending metadata after attestation expiry.
    pub allow_new_completion: bool,
}

macro_rules! redacted_enrollment_debug {
    ($($type:ty),+ $(,)?) => {$ (
        impl std::fmt::Debug for $type {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(concat!(stringify!($type), "(<redacted>)"))
            }
        }
    )+ };
}

redacted_enrollment_debug!(
    EnrollmentCandidateQuery,
    PrepareDeviceAttestation,
    BindPendingDeviceAttestation,
    CompleteDeviceProvisioning,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvisioningOutcome {
    Provisioned,
    ExactRetry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnrollmentRepositoryError {
    PermissionDenied,
    NotFound,
    Conflict,
    Expired,
    Replayed,
    Unavailable,
    Corrupt,
}

/// Atomic hosted metadata boundary for enrollment discovery and provisioning.
/// These rows help devices find each other; they never replace signed trust.
pub trait EnrollmentRepository: Send + Sync {
    fn list_enrollment_candidates<'a>(
        &'a self,
        query: EnrollmentCandidateQuery,
    ) -> PortFuture<'a, Result<EnrollmentCandidatePage, EnrollmentRepositoryError>>;

    fn prepare_device_attestation<'a>(
        &'a self,
        request: PrepareDeviceAttestation,
    ) -> PortFuture<'a, Result<DeviceAttestationSubject, EnrollmentRepositoryError>>;

    fn bind_pending_device_attestation<'a>(
        &'a self,
        request: BindPendingDeviceAttestation,
    ) -> PortFuture<'a, Result<(), EnrollmentRepositoryError>>;

    fn complete_device_provisioning<'a>(
        &'a self,
        request: CompleteDeviceProvisioning,
    ) -> PortFuture<'a, Result<ProvisioningOutcome, EnrollmentRepositoryError>>;
}

pub trait SessionRepository: Send + Sync {
    fn find_session<'a>(
        &'a self,
        id: AccountSessionId,
    ) -> PortFuture<'a, Result<Option<AccountSession>, RepositoryError>>;
    fn insert_session<'a>(
        &'a self,
        session: AccountSession,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;
    fn rotate_refresh<'a>(
        &'a self,
        session: AccountSession,
        presented_digest: KeyedDigest,
        retry_key_digest: KeyedDigest,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<RefreshRotationOutcome, RepositoryError>>;
    fn revoke_session<'a>(
        &'a self,
        id: AccountSessionId,
        at_millis: UnixMillis,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct ExchangeCommitRequest {
    pub flow_id: AuthFlowId,
    pub grant_digest: KeyedDigest,
    pub session: AccountSession,
    pub device: Option<AccountDevice>,
    pub device_ownership: Option<VerifiedDeviceOwnership>,
    pub proof_commit_digest: Option<KeyedDigest>,
    pub sealed_result: SealedValue,
    pub now_millis: UnixMillis,
}

impl std::fmt::Debug for ExchangeCommitRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExchangeCommitRequest(<redacted>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeCommitOutcome {
    Committed,
    ExactRetry(SealedValue),
}

pub trait AccountExchangeRepository: Send + Sync {
    fn commit_exchange<'a>(
        &'a self,
        request: ExchangeCommitRequest,
    ) -> PortFuture<'a, Result<ExchangeCommitOutcome, RepositoryError>>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct LinkProofCommitRequest {
    pub proof_flow_id: AuthFlowId,
    pub proof_grant_digest: KeyedDigest,
    pub mutation_digest: KeyedDigest,
    pub parent_flow_id: AuthFlowId,
    pub auth_identity: AuthIdentity,
    pub authorized_by: AuthIdentityId,
    pub authenticated_at_millis: UnixMillis,
    pub profile_id: ProfileId,
    pub session_id: AccountSessionId,
    pub now_millis: UnixMillis,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LinkProofMutationBindingV1 {
    pub version: u8,
    pub parent_flow_id: AuthFlowId,
    pub koru_identity_id: KoruIdentityId,
    pub profile_id: ProfileId,
    pub issuer: String,
    pub subject: String,
    pub authorized_by: AuthIdentityId,
    pub authenticated_at_millis: UnixMillis,
    pub session_id: AccountSessionId,
}

impl std::fmt::Debug for LinkProofMutationBindingV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LinkProofMutationBindingV1(<redacted>)")
    }
}

impl std::fmt::Debug for LinkProofCommitRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LinkProofCommitRequest(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RebindProofCommitRequest {
    pub proof_flow_id: AuthFlowId,
    pub proof_grant_digest: KeyedDigest,
    pub mutation_digest: KeyedDigest,
    pub parent_flow_id: AuthFlowId,
    pub authenticated_by: AuthIdentityId,
    pub rebind: ProfileRebindRequest,
    pub now_millis: UnixMillis,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RebindProofMutationBindingV1 {
    pub version: u8,
    pub parent_flow_id: AuthFlowId,
    pub koru_identity_id: KoruIdentityId,
    pub profile_id: ProfileId,
    pub erased_identity_id: KoruIdentityId,
    pub authenticated_by: AuthIdentityId,
    pub explicit_rebind: bool,
    pub discovery_name_snapshot: Option<String>,
    pub locale_snapshot: Option<String>,
}

impl std::fmt::Debug for RebindProofMutationBindingV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RebindProofMutationBindingV1(<redacted>)")
    }
}

impl std::fmt::Debug for RebindProofCommitRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RebindProofCommitRequest(<redacted>)")
    }
}

pub trait AccountProofRepository: Send + Sync {
    fn commit_link_proof<'a>(
        &'a self,
        request: LinkProofCommitRequest,
    ) -> PortFuture<'a, Result<LinkProofCommitOutcome, LinkProofCommitError>>;

    fn commit_rebind_proof<'a>(
        &'a self,
        request: RebindProofCommitRequest,
    ) -> PortFuture<'a, Result<RebindProofCommitOutcome, RebindProofCommitError>>;

    fn cancel_link_proof<'a>(
        &'a self,
        request: LinkProofCancelRequest,
    ) -> PortFuture<'a, Result<LinkProofCancelOutcome, LinkProofCommitError>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAuthorizationCommitStatus {
    Added,
    AlreadyAuthorized,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkProofCommitOutcome {
    Committed {
        auth_identity: AuthIdentity,
        authorization: LinkAuthorizationCommitStatus,
    },
    ExactRetry(AuthIdentity),
}

#[derive(Clone, PartialEq, Eq)]
pub struct LinkProofCancelRequest {
    pub proof_flow_id: AuthFlowId,
    pub proof_grant_digest: KeyedDigest,
    pub parent_flow_id: AuthFlowId,
    pub koru_identity_id: KoruIdentityId,
    pub now_millis: UnixMillis,
}

impl std::fmt::Debug for LinkProofCancelRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LinkProofCancelRequest(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkProofCancelOutcome {
    Cancelled,
    ExactRetry,
    AlreadyConfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkProofCommitError {
    ProofInvalid,
    ProofExpired,
    ProofAlreadyUsed,
    ProfileNotOwned,
    IdentityOwnedByAnother,
    Unavailable,
    Corrupt,
}

#[derive(Clone, PartialEq, Eq)]
pub enum RebindProofCommitOutcome {
    Committed(ProfileBinding),
    ExactRetry(ProfileBinding),
}

impl std::fmt::Debug for RebindProofCommitOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Committed(_) => "RebindProofCommitOutcome::Committed(<redacted>)",
            Self::ExactRetry(_) => "RebindProofCommitOutcome::ExactRetry(<redacted>)",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebindProofCommitError {
    ProofInvalid,
    ProofExpired,
    ProofAlreadyUsed,
    IntentRequired,
    AuthenticationInvalid,
    OldIdentityStillLive,
    ProfileOwnedByAnother,
    Unavailable,
    Corrupt,
}

/// Atomic persistence decision for one refresh-family request. Adapters compare
/// both digests and perform any replay revocation in the same transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshRotationOutcome {
    Rotated(AccountSession),
    ExactRetry(AccountSession),
    ReplayedRevoked,
    ExpiredRevoked,
}

pub trait AuthFlowRepository: Send + Sync {
    fn exchange_proof_digest<'a>(
        &'a self,
        _id: AuthFlowId,
    ) -> PortFuture<'a, Result<Option<KeyedDigest>, RepositoryError>> {
        Box::pin(async { Ok(None) })
    }
    fn insert_flow<'a>(
        &'a self,
        flow: TemporaryAuthFlow,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;
    fn find_flow<'a>(
        &'a self,
        id: AuthFlowId,
    ) -> PortFuture<'a, Result<Option<TemporaryAuthFlow>, RepositoryError>>;
    /// Consume iff pending and unexpired. The state transition is atomic.
    fn consume_flow<'a>(
        &'a self,
        id: AuthFlowId,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<TemporaryAuthFlow, RepositoryError>>;
}

pub trait ServerErasureRepository: Send + Sync {
    fn preview<'a>(
        &'a self,
        id: KoruIdentityId,
    ) -> PortFuture<'a, Result<ServerErasurePreview, RepositoryError>>;
    fn erase<'a>(
        &'a self,
        id: KoruIdentityId,
        expected: ServerErasureCounts,
    ) -> PortFuture<'a, Result<ServerErasureOutcome, RepositoryError>>;
}

pub trait Clock: Send + Sync {
    fn now_millis(&self) -> UnixMillis;
}

pub trait Entropy: Send + Sync {
    fn fill(&self, output: &mut [u8]) -> Result<(), CryptoError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct GoogleVerificationInput {
    pub authorization_code: OpaqueSecret,
    pub expected_nonce: OpaqueSecret,
    pub pkce_verifier: OpaqueSecret,
    /// S256 challenge retained with the flow. The adapter verifies this before
    /// sending the verifier to the provider token endpoint.
    pub expected_pkce_challenge: String,
}

impl std::fmt::Debug for GoogleVerificationInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleVerificationInput")
            .field("authorization_code", &"<redacted>")
            .field("expected_nonce", &"<redacted>")
            .field("pkce_verifier", &"<redacted>")
            .field("expected_pkce_challenge", &"<redacted>")
            .finish()
    }
}

pub trait GoogleIdentityVerifier: Send + Sync {
    fn verify<'a>(
        &'a self,
        input: GoogleVerificationInput,
    ) -> PortFuture<'a, Result<VerifiedGoogleIdentity, ProviderVerificationError>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessTokenClaims {
    pub issuer: String,
    pub audience: String,
    pub subject: KoruIdentityId,
    pub session_id: AccountSessionId,
    pub token_id: String,
    pub issued_at_millis: UnixMillis,
    pub expires_at_millis: UnixMillis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedToken {
    pub key_id: String,
    pub compact: OpaqueSecret,
}

pub trait TokenSigner: Send + Sync {
    fn sign_access_token(&self, claims: &AccessTokenClaims) -> Result<SignedToken, CryptoError>;
    fn public_jwks(&self) -> Result<String, CryptoError>;
}

pub trait TransientEncryption: Send + Sync {
    fn seal(&self, purpose: &'static str, plaintext: &[u8]) -> Result<SealedValue, CryptoError>;
    fn open(&self, purpose: &'static str, sealed: &SealedValue) -> Result<Vec<u8>, CryptoError>;
    fn digest(&self, purpose: &'static str, value: &[u8]) -> Result<KeyedDigest, CryptoError>;
}

/// Turning an accepted password into a stored verifier, and back.
///
/// The policy that decides whether a password is allowed at all is domain and
/// lives in [`crate::password`]; this is only the cryptography, so the
/// algorithm and its cost parameters stay in the adapter and out of every
/// client that links this crate.
///
/// **Both methods are deliberately synchronous and deliberately expensive.**
/// Argon2id is memory-hard because that is what makes it worth using, so a
/// caller on an async runtime must not invoke either one on the reactor: the
/// hosted API wraps them in `spawn_blocking` under a bounded semaphore. Making
/// them `async` here would hide that decision inside the adapter, where the one
/// caller who must get it right cannot see it.
pub trait PasswordHashing: Send + Sync {
    /// Hash an accepted password into a verifier for storage.
    fn hash(&self, password: &AcceptedPassword) -> Result<PasswordVerifier, CryptoError>;

    /// Check a password against the verifier stored for an address.
    ///
    /// **`stored` is an `Option` on purpose, and that is the whole defence
    /// against a timing oracle.** Sign-in must cost the same whether or not an
    /// account exists at the address, or response time answers the question
    /// that the collision flow spends a screen and an email refusing to answer.
    /// Passing `None` spends the same work against a fixed internal verifier
    /// and returns [`PasswordCheck::Rejected`].
    ///
    /// The shape is what enforces it: a caller holding `Option<&Verifier>` has
    /// to go out of its way to short-circuit, where a separate
    /// `verify_absent()` would be a step somebody simply forgets on the error
    /// path. The adapter is not permitted to return early on `None`.
    fn verify(
        &self,
        password: &str,
        stored: Option<&PasswordVerifier>,
    ) -> Result<PasswordCheck, CryptoError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub remaining: u32,
    pub retry_after_millis: Option<u64>,
}

pub trait RateLimiter: Send + Sync {
    fn check<'a>(
        &'a self,
        scope: RateLimitScope,
        subject_digest: KeyedDigest,
        koru_identity_id: Option<KoruIdentityId>,
        now_millis: UnixMillis,
        limit: u32,
        window_millis: u64,
    ) -> PortFuture<'a, Result<RateLimitDecision, RepositoryError>>;
}

/// One stored email credential, as the repository returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailCredential {
    pub auth_identity_id: AuthIdentityId,
    pub koru_identity_id: KoruIdentityId,
    pub verifier: PasswordVerifier,
}

/// Everything one email sign-up writes, in one transaction.
///
/// The caller prepares every row; the repository's only judgement is whether
/// the flow is still there to consume and whether the address is still free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailSignupCommit {
    pub flow_id: AuthFlowId,
    pub state_digest: KeyedDigest,
    pub now_millis: UnixMillis,
    pub identity: KoruIdentity,
    pub auth_identity: AuthIdentity,
    pub address_digest: KeyedDigest,
    pub verifier: PasswordVerifier,
    /// The child flow that carries the one-use exchange grant, inserted in the
    /// same transaction as the rows above.
    ///
    /// The Google path inserts its `child_flows` inside `commit_account_creation`
    /// for the same reason: a grant that existed without its account, or an
    /// account whose grant was never written, is a person who has signed up and
    /// cannot get in.
    pub grant_flow: TemporaryAuthFlow,
    /// What an exact retry of this same request replays. Distinct from the
    /// grant: this is stored on the consumed sign-up flow, so a
    /// double-submitted final step returns the first attempt's answer.
    pub sealed_terminal_result: SealedValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmailSignupOutcome {
    /// Committed, or replayed from a request that already committed. The
    /// caller cannot tell the two apart, and must not: an exact retry is the
    /// same act, not a second one.
    Committed {
        sealed_terminal_result: SealedValue,
        replayed: bool,
    },
    /// Somebody else finished a sign-up at this address first. The address is
    /// not free, and the flow is untouched.
    AddressTaken,
}

/// Reading and writing email credentials and their challenges.
pub trait EmailRepository: Send + Sync {
    /// The credential at an address, if there is one.
    ///
    /// Returning `Option` rather than an error for "no account" is what lets
    /// the caller hand it straight to [`PasswordHashing::verify`], whose
    /// `None` arm spends the same work — so the absence of an account costs
    /// what its presence costs.
    fn credential_for_address<'a>(
        &'a self,
        address_digest: &'a KeyedDigest,
    ) -> PortFuture<'a, Result<Option<EmailCredential>, RepositoryError>>;

    /// Attach a challenge to a flow that already exists.
    fn insert_challenge<'a>(
        &'a self,
        flow_id: AuthFlowId,
        address_digest: &'a KeyedDigest,
        code_digest: &'a KeyedDigest,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;

    /// A challenge and the flow state it is judged with.
    fn challenge<'a>(
        &'a self,
        flow_id: AuthFlowId,
    ) -> PortFuture<'a, Result<Option<ChallengeState>, RepositoryError>>;

    /// Spend one attempt, atomically, returning the count after the increment.
    ///
    /// Atomic because two wrong guesses arriving together must cost two
    /// attempts and not one; a read-modify-write in the caller would let a
    /// burst of concurrent guesses share a single attempt.
    fn record_attempt<'a>(
        &'a self,
        flow_id: AuthFlowId,
    ) -> PortFuture<'a, Result<i32, RepositoryError>>;

    /// Record that another code went out, for the resend cooldown.
    fn record_send<'a>(
        &'a self,
        flow_id: AuthFlowId,
        code_digest: &'a KeyedDigest,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;

    /// Create the identity, the auth identity and the credential, and consume
    /// the flow — all of it or none of it.
    fn complete_email_signup<'a>(
        &'a self,
        commit: EmailSignupCommit,
    ) -> PortFuture<'a, Result<EmailSignupOutcome, AccountCreationError>>;

    /// Replace a stored verifier: a rehash at a raised cost, a password
    /// change, or the end of a reset.
    fn replace_verifier<'a>(
        &'a self,
        auth_identity_id: AuthIdentityId,
        verifier: &'a PasswordVerifier,
        now_millis: UnixMillis,
    ) -> PortFuture<'a, Result<(), RepositoryError>>;
}
