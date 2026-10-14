// SPDX-License-Identifier: MPL-2.0

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    AccountSessionId, AuthFlowId, AuthIdentityId, DeviceDescriptorV1, DeviceId, KeyedDigest,
    KoruIdentityId, ProfileId, ProfileLifecycleMutationId, ProfileLifecycleRevision,
    ProfileLifecycleState, SealedValue,
};

pub type UnixMillis = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KoruIdentityStatus {
    Active,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermsAcceptance {
    pub version: String,
    pub accepted_at_millis: UnixMillis,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KoruIdentity {
    pub id: KoruIdentityId,
    /// The person who owns this Koru Identity. Profile names describe personas
    /// such as "Steven (Work)" and never stand in for this value.
    pub display_name: String,
    pub status: KoruIdentityStatus,
    pub terms: TermsAcceptance,
    pub created_at_millis: UnixMillis,
    pub updated_at_millis: UnixMillis,
}

impl fmt::Debug for KoruIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KoruIdentity")
            .field("id", &self.id)
            .field("display_name_present", &!self.display_name.is_empty())
            .field("status", &self.status)
            .field("terms", &self.terms)
            .field("created_at_millis", &self.created_at_millis)
            .field("updated_at_millis", &self.updated_at_millis)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuthProvider {
    Google,
    // Koru's own email credential: `issuer` is the fixed `KORU_EMAIL_ISSUER`
    // and `subject` is an opaque id, never the address -- which lives in
    // `verified_email` as the same non-key collision metadata a provider's
    // does. See `crate::email` for the whole of it.
    //
    // Deliberately `//` and not `///`. A doc comment on one variant makes
    // schemars emit it as its own subschema with a description, which turns
    // the published contract's `"enum": ["google", "email"]` into a `oneOf`
    // of an enum and a const -- equivalent, uglier, and a needless diff for
    // every consumer of koru-account-wire.schema.json.
    Email,
    // Sign in with Apple: `issuer` is exactly `https://appleid.apple.com` and
    // `subject` is Apple's team-scoped `sub`.
    Apple,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedEmailSnapshot {
    /// Discovery/collision metadata only. It is never an identity key.
    pub normalized_email: String,
    pub captured_at_millis: UnixMillis,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthIdentity {
    pub id: AuthIdentityId,
    pub koru_identity_id: KoruIdentityId,
    pub provider: AuthProvider,
    /// Exact canonical issuer. Together with `subject`, this is the external key.
    pub issuer: String,
    pub subject: String,
    pub verified_email: Option<VerifiedEmailSnapshot>,
    pub created_at_millis: UnixMillis,
    pub last_authenticated_at_millis: UnixMillis,
}

impl fmt::Debug for AuthIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthIdentity")
            .field("id", &self.id)
            .field("koru_identity_id", &self.koru_identity_id)
            .field("provider", &self.provider)
            .field("issuer", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("verified_email", &self.verified_email.is_some())
            .field("created_at_millis", &self.created_at_millis)
            .field(
                "last_authenticated_at_millis",
                &self.last_authenticated_at_millis,
            )
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedGoogleIdentity {
    pub canonical_issuer: String,
    pub subject: String,
    pub verified_email: Option<String>,
    pub display_name_snapshot: Option<String>,
}

impl fmt::Debug for VerifiedGoogleIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedGoogleIdentity")
            .field("canonical_issuer", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("verified_email", &self.verified_email.is_some())
            .field(
                "display_name_snapshot",
                &self.display_name_snapshot.is_some(),
            )
            .finish()
    }
}

/// Provider-neutral result after the provider adapter has completed protocol
/// and cryptographic verification. Raw provider credentials never cross this
/// boundary.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedExternalIdentity {
    pub provider: AuthProvider,
    pub canonical_issuer: String,
    pub subject: String,
    pub verified_email: Option<String>,
    pub display_name_snapshot: Option<String>,
}

impl fmt::Debug for VerifiedExternalIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedExternalIdentity")
            .field("provider", &self.provider)
            .field("canonical_issuer", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("verified_email", &self.verified_email.is_some())
            .field(
                "display_name_snapshot",
                &self.display_name_snapshot.is_some(),
            )
            .finish()
    }
}

impl From<VerifiedGoogleIdentity> for VerifiedExternalIdentity {
    fn from(value: VerifiedGoogleIdentity) -> Self {
        Self {
            provider: AuthProvider::Google,
            canonical_issuer: value.canonical_issuer,
            subject: value.subject,
            verified_email: value.verified_email,
            display_name_snapshot: value.display_name_snapshot,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBinding {
    pub profile_id: ProfileId,
    pub koru_identity_id: KoruIdentityId,
    /// Non-canonical discovery metadata. Local CRDT state remains authoritative.
    pub discovery_name_snapshot: Option<String>,
    pub locale_snapshot: Option<String>,
    pub lifecycle_state: ProfileLifecycleState,
    pub lifecycle_revision: ProfileLifecycleRevision,
    pub lifecycle_operation_id: Option<ProfileLifecycleMutationId>,
    pub deleted_at_millis: Option<UnixMillis>,
    pub created_at_millis: UnixMillis,
    pub updated_at_millis: UnixMillis,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileAuthBinding {
    pub profile_id: ProfileId,
    pub auth_identity_id: AuthIdentityId,
    pub created_at_millis: UnixMillis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    Windows,
    Macos,
    Ios,
    Android,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDevice {
    pub descriptor: DeviceDescriptorV1,
    pub registration: AccountDeviceRegistration,
}

/// Account-scoped observations of a physical device. The descriptor is immutable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDeviceRegistration {
    pub koru_identity_id: KoruIdentityId,
    pub platform: DevicePlatform,
    pub app_version: String,
    /// Enrollment wire generation spoken by this build. `None` identifies a
    /// pre-enrollment build and is never guessed by the hosted service.
    pub enrollment_protocol_version: Option<u32>,
    /// Replication peer protocol spoken after enrollment. Kept distinct from
    /// the ceremony protocol because either may evolve independently.
    pub peer_protocol_version: Option<u32>,
    pub registered_at_millis: UnixMillis,
    pub last_seen_at_millis: UnixMillis,
    pub revoked_at_millis: Option<UnixMillis>,
}

impl AccountDevice {
    pub fn device_id(&self) -> DeviceId {
        self.descriptor.device_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeviceProfileState {
    Authorized,
    PendingProvisioning,
    Provisioned,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceProfileBinding {
    pub device_id: DeviceId,
    pub profile_id: ProfileId,
    /// Discovery state only; replicated peer trust remains authoritative.
    pub state: DeviceProfileState,
    pub created_at_millis: UnixMillis,
    pub updated_at_millis: UnixMillis,
}

/// One account device together with only the Profile discovery states visible
/// to the Auth Identity that authenticated the current session.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountDeviceWithProfiles {
    pub device: AccountDevice,
    pub profile_states: Vec<DeviceProfileStateSummary>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DeviceProfileStateSummary {
    pub profile_id: ProfileId,
    pub state: DeviceProfileState,
}

impl std::fmt::Debug for DeviceProfileStateSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceProfileStateSummary")
            .field("state", &self.state)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountDevicePage {
    pub devices: Vec<AccountDeviceWithProfiles>,
    pub truncated: bool,
}

impl std::fmt::Debug for AccountDeviceWithProfiles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountDeviceWithProfiles")
            .field("profile_count", &self.profile_states.len())
            .field("discovery_metadata", &"<redacted>")
            .finish()
    }
}

/// Public, bounded discovery material for a device that may answer an
/// enrollment ceremony. It contains no Profile or provider metadata.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentCandidate {
    pub descriptor: DeviceDescriptorV1,
    pub platform: DevicePlatform,
    pub app_version: String,
    pub enrollment_protocol_version: u32,
    pub peer_protocol_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnrollmentCandidatePage {
    pub candidates: Vec<EnrollmentCandidate>,
    pub truncated: bool,
}

impl std::fmt::Debug for EnrollmentCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnrollmentCandidate")
            .field("platform", &self.platform)
            .field(
                "enrollment_protocol_version",
                &self.enrollment_protocol_version,
            )
            .field("peer_protocol_version", &self.peer_protocol_version)
            .field("discovery_material", &"<redacted>")
            .finish()
    }
}

/// The exact server-side subject frozen before an attestation is signed.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceAttestationSubject {
    pub descriptor: DeviceDescriptorV1,
    pub authorized_profile_ids: Vec<ProfileId>,
}

impl std::fmt::Debug for DeviceAttestationSubject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DeviceAttestationSubject(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Native,
    Web,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshFamily {
    pub current_digest: KeyedDigest,
    pub previous_digest: Option<KeyedDigest>,
    pub previous_retry_until_millis: Option<UnixMillis>,
    /// Digest of the idempotency key that may replay the immediately previous
    /// refresh request. It is meaningful only with `sealed_previous_result`.
    pub previous_retry_key_digest: Option<KeyedDigest>,
    /// Encrypted access/refresh result returned for that one exact retry.
    pub sealed_previous_result: Option<SealedValue>,
    pub rotation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSession {
    pub id: AccountSessionId,
    pub koru_identity_id: KoruIdentityId,
    pub authenticated_by: AuthIdentityId,
    pub device_id: Option<DeviceId>,
    pub kind: SessionKind,
    pub refresh_family: Option<RefreshFamily>,
    pub created_at_millis: UnixMillis,
    pub last_seen_at_millis: UnixMillis,
    pub recent_auth_at_millis: UnixMillis,
    pub idle_expires_at_millis: UnixMillis,
    pub absolute_expires_at_millis: UnixMillis,
    pub revoked_at_millis: Option<UnixMillis>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthFlowKind {
    GoogleWeb,
    GoogleNative,
    // The six email kinds. Ordinary comments, not doc comments: see the note
    // on `AuthProvider::Email` for why a documented variant would reshape the
    // published wire contract.
    //
    // EmailWeb/EmailNative are sign-up and sign-in, mirroring the Google pair.
    // EmailGoogleLink is the sealed state between "Google came back and we
    // recognised the address" and "the Koru password was accepted", which is
    // what keeps one person to one account when they arrive from that side.
    EmailWeb,
    EmailNative,
    EmailPasswordReset,
    EmailPasswordAdd,
    EmailAddressChange,
    EmailGoogleLink,
    LinkAuthIdentity,
    VerifiedEmailCollision,
    ProfileRebind,
    // Sign in with Apple's website and desktop redirect flows, mirroring the
    // Google pair.
    AppleWeb,
    AppleNative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthFlowStatus {
    Pending,
    Consumed,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryAuthFlow {
    pub id: AuthFlowId,
    pub kind: AuthFlowKind,
    pub status: AuthFlowStatus,
    pub state_digest: KeyedDigest,
    pub sealed_transient: SealedValue,
    pub sealed_exchange_result: Option<SealedValue>,
    pub source_digest: KeyedDigest,
    pub target_identity_id: Option<KoruIdentityId>,
    pub created_at_millis: UnixMillis,
    pub expires_at_millis: UnixMillis,
    pub consumed_at_millis: Option<UnixMillis>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitScope {
    AuthStart,
    AuthenticationAttempt,
    CallbackOrExchange,
    IdentityLink,
    Refresh,
    DeviceOperation,
    // Email sign-in's four. A bucket is (scope, subject_digest), so a source
    // budget and an address budget are separate scopes rather than one scope
    // with two policies -- which would work, since RatePolicy carries its
    // scope as a field and DeviceOperation already has two, but would rest on
    // a source digest and an address digest never colliding in one namespace.
    EmailChallengeSend,
    EmailChallengeVerify,
    EmailPasswordSourceAttempt,
    EmailPasswordAddressAttempt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitBucket {
    pub scope: RateLimitScope,
    pub subject_digest: KeyedDigest,
    /// Owner for authenticated identity/device limits. Pre-auth source and
    /// flow limits remain unowned and expire independently of erasure.
    pub koru_identity_id: Option<KoruIdentityId>,
    pub window_started_at_millis: UnixMillis,
    pub attempts: u32,
    pub expires_at_millis: UnixMillis,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerErasureCounts {
    pub koru_identities: u64,
    pub auth_identities: u64,
    pub profile_bindings: u64,
    pub profile_auth_bindings: u64,
    pub devices: u64,
    pub device_profile_bindings: u64,
    pub sessions: u64,
    pub flows: u64,
    pub rate_buckets: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerErasurePreview {
    pub koru_identity_id: KoruIdentityId,
    pub counts: ServerErasureCounts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerErasureOutcome {
    pub koru_identity_id: KoruIdentityId,
    pub removed: ServerErasureCounts,
    pub completed_at_millis: UnixMillis,
}
