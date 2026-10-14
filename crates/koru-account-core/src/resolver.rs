// SPDX-License-Identifier: MPL-2.0

use std::{fmt, str::FromStr};

use uuid::Uuid;

use crate::*;

const COLLISION_PURPOSE: &str = "account.identity-collision.v1";
const COLLISION_LIFETIME_MILLIS: i64 = 10 * 60 * 1_000;
const RECENT_AUTH_LIFETIME_MILLIS: i64 = 5 * 60 * 1_000;
const CANDIDATE_VERSION: u8 = 1;
const MAX_ISSUER_BYTES: usize = 256;
const MAX_SUBJECT_BYTES: usize = 255;
const MAX_EMAIL_BYTES: usize = 320;
const MAX_DISPLAY_NAME_BYTES: usize = 512;
const MAX_COLLISION_TARGETS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionFlowContext {
    pub flow_id: AuthFlowId,
    pub state_digest: KeyedDigest,
    pub source_digest: KeyedDigest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoogleAuthenticationRequest {
    pub verification: GoogleVerificationInput,
    /// The terms displayed for a new account. Existing accounts deliberately
    /// ignore this value and are never blocked by a newer configured version.
    pub terms_version_at_creation: String,
    pub collision_flow: CollisionFlowContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationPrompt {
    SelectAccount,
}

/// In-process evidence that an exact Auth Identity authenticated recently.
/// Fields are private so callers can relay but cannot assemble a proof from
/// account ids and a clock value.
#[derive(Clone, PartialEq, Eq)]
pub struct RecentAuthentication {
    koru_identity_id: KoruIdentityId,
    auth_identity_id: AuthIdentityId,
    authenticated_at_millis: UnixMillis,
    expires_at_millis: UnixMillis,
}

impl RecentAuthentication {
    pub fn koru_identity_id(&self) -> KoruIdentityId {
        self.koru_identity_id
    }

    pub fn auth_identity_id(&self) -> AuthIdentityId {
        self.auth_identity_id
    }

    pub fn authenticated_at_millis(&self) -> UnixMillis {
        self.authenticated_at_millis
    }

    pub fn expires_at_millis(&self) -> UnixMillis {
        self.expires_at_millis
    }
}

impl fmt::Debug for RecentAuthentication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecentAuthentication")
            .field("koru_identity_id", &self.koru_identity_id)
            .field("auth_identity_id", &self.auth_identity_id)
            .field("authenticated_at_millis", &self.authenticated_at_millis)
            .field("expires_at_millis", &self.expires_at_millis)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedIdentity {
    pub identity: KoruIdentity,
    pub auth_identity: AuthIdentity,
    pub recent_authentication: RecentAuthentication,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityResolution {
    WouldCreate(VerifiedExternalIdentity),
    Returning(ResolvedIdentity),
    CollisionRequired {
        flow_id: AuthFlowId,
        expires_at_millis: UnixMillis,
        provider: AuthProvider,
        prompt: AuthenticationPrompt,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkResolution {
    Linked(AuthIdentity),
    AlreadyLinked(AuthIdentity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityResolutionError {
    Provider(ProviderVerificationError),
    InvalidProviderIdentity,
    IdentityDisabled,
    RepositoryUnavailable,
    RepositoryConflict,
    CryptoFailure,
    FlowNotFound,
    FlowExpired,
    FlowConsumed,
    WrongAccount,
    RecentAuthenticationExpired,
    ClockOutOfRange,
}

impl fmt::Display for IdentityResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Provider(_) => "provider_verification_failed",
            Self::InvalidProviderIdentity => "invalid_provider_identity",
            Self::IdentityDisabled => "identity_disabled",
            Self::RepositoryUnavailable => "account_repository_unavailable",
            Self::RepositoryConflict => "account_repository_conflict",
            Self::CryptoFailure => "account_crypto_failure",
            Self::FlowNotFound => "auth_flow_not_found",
            Self::FlowExpired => "auth_flow_expired",
            Self::FlowConsumed => "auth_flow_consumed",
            Self::WrongAccount => "wrong_account",
            Self::RecentAuthenticationExpired => "recent_authentication_expired",
            Self::ClockOutOfRange => "account_clock_out_of_range",
        })
    }
}

impl std::error::Error for IdentityResolutionError {}

impl From<ProviderVerificationError> for IdentityResolutionError {
    fn from(value: ProviderVerificationError) -> Self {
        Self::Provider(value)
    }
}

pub struct IdentityResolver<'a, I, V, C, E> {
    identities: &'a I,
    verifier: &'a V,
    clock: &'a C,
    encryption: &'a E,
}

impl<'a, I, V, C, E> IdentityResolver<'a, I, V, C, E>
where
    I: IdentityResolutionRepository,
    V: GoogleIdentityVerifier,
    C: Clock,
    E: TransientEncryption,
{
    pub fn prepare_link_auth_identity(
        verified: VerifiedExternalIdentity,
        target_identity_id: KoruIdentityId,
        now_millis: UnixMillis,
    ) -> Result<AuthIdentity, IdentityResolutionError> {
        let verified = normalize_and_validate(verified)?;
        Ok(new_auth_identity(&verified, target_identity_id, now_millis))
    }

    pub fn new(identities: &'a I, verifier: &'a V, clock: &'a C, encryption: &'a E) -> Self {
        Self {
            identities,
            verifier,
            clock,
            encryption,
        }
    }

    pub async fn authenticate_google(
        &self,
        request: GoogleAuthenticationRequest,
    ) -> Result<IdentityResolution, IdentityResolutionError> {
        let verified = self.verifier.verify(request.verification).await?;
        self.resolve_verified(
            verified.into(),
            request.terms_version_at_creation,
            request.collision_flow,
        )
        .await
    }

    /// Provider-neutral resolution. Provider adapters finish signature and
    /// protocol verification before constructing this value.
    pub async fn resolve_verified(
        &self,
        verified: VerifiedExternalIdentity,
        _terms_version_at_creation: String,
        collision_flow: CollisionFlowContext,
    ) -> Result<IdentityResolution, IdentityResolutionError> {
        let now = self.clock.now_millis();
        let verified = normalize_and_validate(verified)?;

        if let Some(existing) = self
            .identities
            .find_auth_identity(&verified.canonical_issuer, &verified.subject)
            .await
            .map_err(map_repository_error)?
        {
            let resolved = self.resolved_existing(existing, now).await?;
            return self
                .adopt_verified_name_if_placeholder(resolved, &verified, now)
                .await
                .map(IdentityResolution::Returning);
        }

        if let Some(email) = verified.verified_email.as_deref() {
            let mut targets = self
                .identities
                .find_koru_identities_by_verified_email(email)
                .await
                .map_err(map_repository_error)?;
            targets.sort_unstable();
            targets.dedup();
            if !targets.is_empty() {
                if targets.len() > MAX_COLLISION_TARGETS {
                    return Err(IdentityResolutionError::RepositoryConflict);
                }
                let expires_at_millis = checked_expiry(now, COLLISION_LIFETIME_MILLIS)?;
                let provider = verified.provider;
                let prepared = self.prepare_collision_flow(
                    verified,
                    targets,
                    collision_flow.clone(),
                    now,
                    expires_at_millis,
                )?;
                self.identities
                    .insert_flow(prepared)
                    .await
                    .map_err(map_repository_error)?;
                return Ok(IdentityResolution::CollisionRequired {
                    flow_id: collision_flow.flow_id,
                    expires_at_millis,
                    provider,
                    prompt: AuthenticationPrompt::SelectAccount,
                });
            }
        }

        Ok(IdentityResolution::WouldCreate(verified))
    }

    /// Read-only preparation for the explicit confirmation transaction. The
    /// repository rechecks this classification before committing any effect.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_creation_resolution(
        &self,
        verified: VerifiedExternalIdentity,
        terms_version: String,
        proposed_identity_id: KoruIdentityId,
        proposed_auth_identity_id: AuthIdentityId,
        verified_at_millis: UnixMillis,
        confirmed_at_millis: UnixMillis,
        expires_at_millis: UnixMillis,
        collision_flow: CollisionFlowContext,
    ) -> Result<PreparedCreationResolution, IdentityResolutionError> {
        let verified = normalize_and_validate(verified)?;
        if verified_at_millis > confirmed_at_millis || confirmed_at_millis >= expires_at_millis {
            return Err(IdentityResolutionError::FlowExpired);
        }
        if let Some(auth_identity) = self
            .identities
            .find_auth_identity(&verified.canonical_issuer, &verified.subject)
            .await
            .map_err(map_repository_error)?
        {
            let identity = self
                .identities
                .find_koru_identity(auth_identity.koru_identity_id)
                .await
                .map_err(map_repository_error)?
                .ok_or(IdentityResolutionError::RepositoryConflict)?;
            if identity.status != KoruIdentityStatus::Active {
                return Err(IdentityResolutionError::IdentityDisabled);
            }
            return Ok(PreparedCreationResolution {
                expected: CreationResolution::Returning {
                    koru_identity_id: identity.id,
                    auth_identity_id: auth_identity.id,
                },
                identity: Some(identity),
                auth_identity: Some(auth_identity),
                collision_flow: None,
            });
        }
        if let Some(email) = verified.verified_email.as_deref() {
            let mut targets = self
                .identities
                .find_koru_identities_by_verified_email(email)
                .await
                .map_err(map_repository_error)?;
            targets.sort_unstable();
            targets.dedup();
            if !targets.is_empty() {
                let flow = self.prepare_collision_flow(
                    verified,
                    targets.clone(),
                    collision_flow,
                    confirmed_at_millis,
                    expires_at_millis.min(checked_expiry(
                        confirmed_at_millis,
                        COLLISION_LIFETIME_MILLIS,
                    )?),
                )?;
                return Ok(PreparedCreationResolution {
                    expected: CreationResolution::Collision { targets },
                    identity: None,
                    auth_identity: None,
                    collision_flow: Some(flow),
                });
            }
        }
        if terms_version.is_empty() || terms_version.len() > 128 {
            return Err(IdentityResolutionError::InvalidProviderIdentity);
        }
        let identity = KoruIdentity {
            id: proposed_identity_id,
            display_name: verified
                .display_name_snapshot
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("Koru user")
                .to_owned(),
            status: KoruIdentityStatus::Active,
            terms: TermsAcceptance {
                version: terms_version,
                accepted_at_millis: confirmed_at_millis,
            },
            created_at_millis: confirmed_at_millis,
            updated_at_millis: confirmed_at_millis,
        };
        let mut auth_identity = new_auth_identity(&verified, identity.id, verified_at_millis);
        auth_identity.id = proposed_auth_identity_id;
        auth_identity.created_at_millis = confirmed_at_millis;
        // Row metadata obeys last_authenticated >= created_at. Proof recency
        // remains the separately frozen verified_at, never this commit time.
        auth_identity.last_authenticated_at_millis = confirmed_at_millis;
        Ok(PreparedCreationResolution {
            expected: CreationResolution::Unseen,
            identity: Some(identity),
            auth_identity: Some(auth_identity),
            collision_flow: None,
        })
    }

    fn prepare_collision_flow(
        &self,
        verified: VerifiedExternalIdentity,
        targets: Vec<KoruIdentityId>,
        context: CollisionFlowContext,
        now: UnixMillis,
        expires_at_millis: UnixMillis,
    ) -> Result<TemporaryAuthFlow, IdentityResolutionError> {
        if targets.is_empty() || targets.len() > MAX_COLLISION_TARGETS {
            return Err(IdentityResolutionError::RepositoryConflict);
        }
        let candidate = CollisionCandidate { verified, targets };
        let mut plaintext = candidate.encode()?;
        let sealed = self.encryption.seal(COLLISION_PURPOSE, &plaintext);
        plaintext.fill(0);
        let sealed = sealed.map_err(|_| IdentityResolutionError::CryptoFailure)?;
        Ok(TemporaryAuthFlow {
            id: context.flow_id,
            kind: AuthFlowKind::VerifiedEmailCollision,
            status: AuthFlowStatus::Pending,
            state_digest: context.state_digest,
            sealed_transient: sealed,
            sealed_exchange_result: None,
            source_digest: context.source_digest,
            target_identity_id: (candidate.targets.len() == 1).then(|| candidate.targets[0]),
            created_at_millis: now,
            expires_at_millis,
            consumed_at_millis: None,
        })
    }

    /// Link another verified provider identity to the identity proven by a
    /// recent exact Auth Identity authentication.
    pub async fn link_google(
        &self,
        verification: GoogleVerificationInput,
        proof: &RecentAuthentication,
    ) -> Result<LinkResolution, IdentityResolutionError> {
        self.validate_recent(proof).await?;
        let verified = self.verifier.verify(verification).await?;
        self.link_verified(verified.into(), proof).await
    }

    pub async fn link_verified(
        &self,
        verified: VerifiedExternalIdentity,
        proof: &RecentAuthentication,
    ) -> Result<LinkResolution, IdentityResolutionError> {
        self.validate_recent(proof).await?;
        let verified = normalize_and_validate(verified)?;
        self.link_to_target(verified, proof.koru_identity_id).await
    }

    pub async fn link_verified_and_authorize_profile(
        &self,
        verified: VerifiedExternalIdentity,
        profile_id: ProfileId,
        proof: &RecentAuthentication,
    ) -> Result<LinkResolution, IdentityResolutionError> {
        self.validate_recent(proof).await?;
        let verified = normalize_and_validate(verified)?;
        let auth = new_auth_identity(&verified, proof.koru_identity_id, self.clock.now_millis());
        self.identities
            .link_auth_identity_and_profile(auth, profile_id)
            .await
            .map(|commit| match commit {
                AuthProfileLinkCommit::Linked(value) => LinkResolution::Linked(value),
                AuthProfileLinkCommit::AlreadyLinked(value) => LinkResolution::AlreadyLinked(value),
            })
            .map_err(|error| match error {
                AuthProfileLinkError::ProfileNotOwned
                | AuthProfileLinkError::IdentityOwnedByAnother => {
                    IdentityResolutionError::WrongAccount
                }
                AuthProfileLinkError::Unavailable => IdentityResolutionError::RepositoryUnavailable,
                AuthProfileLinkError::Corrupt => IdentityResolutionError::RepositoryConflict,
            })
    }

    /// Turns a freshly verified provider identity into recent-authentication
    /// evidence only when that exact issuer/subject is already attached to the
    /// expected Koru Identity. It never creates or links an identity.
    pub async fn authenticate_existing_verified(
        &self,
        verified: VerifiedExternalIdentity,
        expected_identity_id: KoruIdentityId,
    ) -> Result<ResolvedIdentity, IdentityResolutionError> {
        let verified = normalize_and_validate(verified)?;
        let existing = self
            .identities
            .find_auth_identity(&verified.canonical_issuer, &verified.subject)
            .await
            .map_err(map_repository_error)?
            .ok_or(IdentityResolutionError::WrongAccount)?;
        if existing.koru_identity_id != expected_identity_id {
            return Err(IdentityResolutionError::WrongAccount);
        }
        self.resolved_existing(existing, self.clock.now_millis())
            .await
    }

    pub async fn authenticate_existing_verified_any(
        &self,
        verified: VerifiedExternalIdentity,
    ) -> Result<ResolvedIdentity, IdentityResolutionError> {
        let verified = normalize_and_validate(verified)?;
        let existing = self
            .identities
            .find_auth_identity(&verified.canonical_issuer, &verified.subject)
            .await
            .map_err(map_repository_error)?
            .ok_or(IdentityResolutionError::WrongAccount)?;
        self.resolved_existing(existing, self.clock.now_millis())
            .await
    }

    /// Resume a receipt classified as returning without changing its owner or
    /// Auth Identity, and without falling back to creation if either vanished.
    pub async fn authenticate_existing_verified_pinned(
        &self,
        verified: VerifiedExternalIdentity,
        expected_identity_id: KoruIdentityId,
        expected_auth_identity_id: AuthIdentityId,
    ) -> Result<ResolvedIdentity, IdentityResolutionError> {
        let verified = normalize_and_validate(verified)?;
        let existing = self
            .identities
            .find_auth_identity(&verified.canonical_issuer, &verified.subject)
            .await
            .map_err(map_repository_error)?
            .ok_or(IdentityResolutionError::WrongAccount)?;
        if existing.id != expected_auth_identity_id
            || existing.koru_identity_id != expected_identity_id
        {
            return Err(IdentityResolutionError::WrongAccount);
        }
        let now = self.clock.now_millis();
        let resolved = self.resolved_existing(existing, now).await?;
        self.adopt_verified_name_if_placeholder(resolved, &verified, now)
            .await
    }

    /// Rehydrates sealed recent-authentication fields only after re-checking
    /// current identity ownership and expiry against repository state.
    pub async fn recent_authentication_from_sealed(
        &self,
        koru_identity_id: KoruIdentityId,
        auth_identity_id: AuthIdentityId,
        authenticated_at_millis: UnixMillis,
    ) -> Result<RecentAuthentication, IdentityResolutionError> {
        let proof = RecentAuthentication {
            koru_identity_id,
            auth_identity_id,
            authenticated_at_millis,
            expires_at_millis: checked_expiry(
                authenticated_at_millis,
                RECENT_AUTH_LIFETIME_MILLIS,
            )?,
        };
        self.validate_recent(&proof).await?;
        Ok(proof)
    }

    /// Complete an email-collision link only after a fresh authentication as
    /// an Auth Identity already attached to one of the encrypted candidates.
    /// A wrong-account attempt does not consume the candidate.
    pub async fn complete_collision(
        &self,
        flow_id: AuthFlowId,
        proof: &RecentAuthentication,
    ) -> Result<LinkResolution, IdentityResolutionError> {
        self.validate_recent(proof).await?;
        let now = self.clock.now_millis();
        let flow = self
            .identities
            .find_flow(flow_id)
            .await
            .map_err(map_repository_error)?
            .ok_or(IdentityResolutionError::FlowNotFound)?;
        if flow.kind != AuthFlowKind::VerifiedEmailCollision {
            return Err(IdentityResolutionError::FlowNotFound);
        }
        match flow.status {
            AuthFlowStatus::Consumed => return Err(IdentityResolutionError::FlowConsumed),
            AuthFlowStatus::Expired => return Err(IdentityResolutionError::FlowExpired),
            AuthFlowStatus::Pending => {}
        }
        if now >= flow.expires_at_millis {
            return Err(IdentityResolutionError::FlowExpired);
        }
        let mut plaintext = self
            .encryption
            .open(COLLISION_PURPOSE, &flow.sealed_transient)
            .map_err(|_| IdentityResolutionError::CryptoFailure)?;
        let candidate_result = CollisionCandidate::decode(&plaintext);
        plaintext.fill(0);
        let candidate = candidate_result?;
        if !candidate.targets.contains(&proof.koru_identity_id) {
            return Err(IdentityResolutionError::WrongAccount);
        }

        let auth_identity = new_auth_identity(&candidate.verified, proof.koru_identity_id, now);
        self.identities
            .complete_collision_link(flow_id, auth_identity, now)
            .await
            .map(|commit| match commit {
                CollisionLinkCommit::Linked(identity) => LinkResolution::Linked(identity),
                CollisionLinkCommit::AlreadyLinked(identity) => {
                    LinkResolution::AlreadyLinked(identity)
                }
            })
            .map_err(|error| match error {
                CollisionLinkError::FlowNotFound => IdentityResolutionError::FlowNotFound,
                CollisionLinkError::FlowExpired => IdentityResolutionError::FlowExpired,
                CollisionLinkError::FlowConsumed => IdentityResolutionError::FlowConsumed,
                CollisionLinkError::IdentityOwnedByAnother => IdentityResolutionError::WrongAccount,
                CollisionLinkError::Unavailable | CollisionLinkError::Corrupt => {
                    IdentityResolutionError::RepositoryUnavailable
                }
            })
    }

    async fn resolved_existing(
        &self,
        auth_identity: AuthIdentity,
        now: UnixMillis,
    ) -> Result<ResolvedIdentity, IdentityResolutionError> {
        let recent_expires_at_millis = checked_expiry(now, RECENT_AUTH_LIFETIME_MILLIS)?;
        let identity = self
            .identities
            .find_koru_identity(auth_identity.koru_identity_id)
            .await
            .map_err(map_repository_error)?
            .ok_or(IdentityResolutionError::RepositoryConflict)?;
        if identity.status != KoruIdentityStatus::Active {
            return Err(IdentityResolutionError::IdentityDisabled);
        }
        let auth_identity = self
            .identities
            .record_authentication(auth_identity.id, now)
            .await
            .map_err(|error| match error {
                RepositoryError::NotFound | RepositoryError::Conflict => {
                    IdentityResolutionError::RepositoryConflict
                }
                other => map_repository_error(other),
            })?;
        Ok(resolved(
            identity,
            auth_identity,
            now,
            recent_expires_at_millis,
        ))
    }

    /// A provider name seeds an identity only while it still has the explicit
    /// fallback minted before profile claims were available. A name chosen in
    /// Koru is authoritative and is never overwritten by a later sign-in.
    async fn adopt_verified_name_if_placeholder(
        &self,
        mut resolved: ResolvedIdentity,
        verified: &VerifiedExternalIdentity,
        now: UnixMillis,
    ) -> Result<ResolvedIdentity, IdentityResolutionError> {
        if resolved.identity.display_name == "Koru user"
            && let Some(name) = verified.display_name_snapshot.as_deref()
        {
            resolved.identity = self
                .identities
                .update_identity_display_name(resolved.identity.id, name, now)
                .await
                .map_err(map_repository_error)?;
        }
        Ok(resolved)
    }

    async fn validate_recent(
        &self,
        proof: &RecentAuthentication,
    ) -> Result<(), IdentityResolutionError> {
        let now = self.clock.now_millis();
        if now < proof.authenticated_at_millis || now >= proof.expires_at_millis {
            return Err(IdentityResolutionError::RecentAuthenticationExpired);
        }
        let linked = self
            .identities
            .list_auth_identities(proof.koru_identity_id)
            .await
            .map_err(map_repository_error)?
            .into_iter()
            .any(|identity| identity.id == proof.auth_identity_id);
        if !linked {
            return Err(IdentityResolutionError::WrongAccount);
        }
        let identity = self
            .identities
            .find_koru_identity(proof.koru_identity_id)
            .await
            .map_err(map_repository_error)?
            .ok_or(IdentityResolutionError::WrongAccount)?;
        if identity.status != KoruIdentityStatus::Active {
            return Err(IdentityResolutionError::IdentityDisabled);
        }
        Ok(())
    }

    async fn link_to_target(
        &self,
        verified: VerifiedExternalIdentity,
        target_identity_id: KoruIdentityId,
    ) -> Result<LinkResolution, IdentityResolutionError> {
        if let Some(existing) = self
            .identities
            .find_auth_identity(&verified.canonical_issuer, &verified.subject)
            .await
            .map_err(map_repository_error)?
        {
            return if existing.koru_identity_id == target_identity_id {
                Ok(LinkResolution::AlreadyLinked(existing))
            } else {
                Err(IdentityResolutionError::WrongAccount)
            };
        }
        let auth_identity =
            new_auth_identity(&verified, target_identity_id, self.clock.now_millis());
        match self
            .identities
            .link_auth_identity(auth_identity.clone())
            .await
        {
            Ok(()) => Ok(LinkResolution::Linked(auth_identity)),
            Err(RepositoryError::Conflict) => {
                let existing = self
                    .identities
                    .find_auth_identity(&verified.canonical_issuer, &verified.subject)
                    .await
                    .map_err(map_repository_error)?
                    .ok_or(IdentityResolutionError::RepositoryConflict)?;
                if existing.koru_identity_id == target_identity_id {
                    Ok(LinkResolution::AlreadyLinked(existing))
                } else {
                    Err(IdentityResolutionError::WrongAccount)
                }
            }
            Err(error) => Err(map_repository_error(error)),
        }
    }
}

fn resolved(
    identity: KoruIdentity,
    auth_identity: AuthIdentity,
    now: UnixMillis,
    expires_at_millis: UnixMillis,
) -> ResolvedIdentity {
    let recent_authentication = RecentAuthentication {
        koru_identity_id: identity.id,
        auth_identity_id: auth_identity.id,
        authenticated_at_millis: now,
        expires_at_millis,
    };
    ResolvedIdentity {
        identity,
        auth_identity,
        recent_authentication,
    }
}

fn checked_expiry(
    now: UnixMillis,
    lifetime_millis: i64,
) -> Result<UnixMillis, IdentityResolutionError> {
    now.checked_add(lifetime_millis)
        .ok_or(IdentityResolutionError::ClockOutOfRange)
}

fn new_auth_identity(
    verified: &VerifiedExternalIdentity,
    koru_identity_id: KoruIdentityId,
    now: UnixMillis,
) -> AuthIdentity {
    AuthIdentity {
        id: AuthIdentityId::mint(),
        koru_identity_id,
        provider: verified.provider,
        issuer: verified.canonical_issuer.clone(),
        subject: verified.subject.clone(),
        verified_email: verified.verified_email.as_ref().map(|normalized_email| {
            VerifiedEmailSnapshot {
                normalized_email: normalized_email.clone(),
                captured_at_millis: now,
            }
        }),
        created_at_millis: now,
        last_authenticated_at_millis: now,
    }
}

fn normalize_and_validate(
    mut verified: VerifiedExternalIdentity,
) -> Result<VerifiedExternalIdentity, IdentityResolutionError> {
    if verified.canonical_issuer.is_empty()
        || verified.canonical_issuer.len() > MAX_ISSUER_BYTES
        || verified.subject.is_empty()
        || verified.subject.len() > MAX_SUBJECT_BYTES
    {
        return Err(IdentityResolutionError::InvalidProviderIdentity);
    }
    verified.verified_email = verified
        .verified_email
        .take()
        .map(|email| email.trim().to_lowercase())
        .filter(|email| !email.is_empty());
    if verified
        .verified_email
        .as_ref()
        .is_some_and(|email| email.len() > MAX_EMAIL_BYTES)
        || verified.display_name_snapshot.as_ref().is_some_and(|name| {
            name.len() > MAX_DISPLAY_NAME_BYTES || name.chars().any(char::is_control)
        })
    {
        return Err(IdentityResolutionError::InvalidProviderIdentity);
    }
    Ok(verified)
}

fn map_repository_error(error: RepositoryError) -> IdentityResolutionError {
    match error {
        RepositoryError::Conflict => IdentityResolutionError::RepositoryConflict,
        RepositoryError::NotFound => IdentityResolutionError::FlowNotFound,
        RepositoryError::Unavailable | RepositoryError::Corrupt => {
            IdentityResolutionError::RepositoryUnavailable
        }
    }
}

struct CollisionCandidate {
    verified: VerifiedExternalIdentity,
    targets: Vec<KoruIdentityId>,
}

impl CollisionCandidate {
    fn encode(&self) -> Result<Vec<u8>, IdentityResolutionError> {
        let mut bytes = vec![CANDIDATE_VERSION, provider_tag(self.verified.provider)];
        put_string(&mut bytes, &self.verified.canonical_issuer)?;
        put_string(&mut bytes, &self.verified.subject)?;
        put_optional_string(&mut bytes, self.verified.verified_email.as_deref())?;
        put_optional_string(&mut bytes, self.verified.display_name_snapshot.as_deref())?;
        put_u32(&mut bytes, self.targets.len())?;
        for target in &self.targets {
            bytes.extend_from_slice(target.as_uuid().as_bytes());
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, IdentityResolutionError> {
        let mut reader = CandidateReader::new(bytes);
        if reader.byte()? != CANDIDATE_VERSION {
            return Err(IdentityResolutionError::CryptoFailure);
        }
        let provider = provider_from_tag(reader.byte()?)?;
        let canonical_issuer = reader.string(MAX_ISSUER_BYTES)?;
        let subject = reader.string(MAX_SUBJECT_BYTES)?;
        let verified_email = reader.optional_string(MAX_EMAIL_BYTES)?;
        let display_name_snapshot = reader.optional_string(MAX_DISPLAY_NAME_BYTES)?;
        let target_count = reader.u32()? as usize;
        if target_count == 0 || target_count > MAX_COLLISION_TARGETS {
            return Err(IdentityResolutionError::CryptoFailure);
        }
        let mut targets = Vec::with_capacity(target_count);
        for _ in 0..target_count {
            let raw: [u8; 16] = reader
                .take(16)?
                .try_into()
                .map_err(|_| IdentityResolutionError::CryptoFailure)?;
            let id = KoruIdentityId::from_str(&Uuid::from_bytes(raw).to_string())
                .map_err(|_| IdentityResolutionError::CryptoFailure)?;
            targets.push(id);
        }
        if !reader.is_finished() {
            return Err(IdentityResolutionError::CryptoFailure);
        }
        Ok(Self {
            verified: VerifiedExternalIdentity {
                provider,
                canonical_issuer,
                subject,
                verified_email,
                display_name_snapshot,
            },
            targets,
        })
    }
}

/// Stable wire tags inside a sealed collision candidate.
///
/// These bytes are written into `auth_flows.sealed_transient` and read back
/// out of it, so a tag is storage: renumbering one would make every candidate
/// sealed before the change decode as a different provider. Google is 1 and
/// stays 1; a new provider takes the next free number and never an existing
/// one. Adding a tag is additive -- an older candidate carrying 1 still
/// decodes -- which is why this is safe to extend and unsafe to reorder.
fn provider_tag(provider: AuthProvider) -> u8 {
    match provider {
        AuthProvider::Google => 1,
        AuthProvider::Email => 2,
        AuthProvider::Apple => 3,
    }
}

fn provider_from_tag(tag: u8) -> Result<AuthProvider, IdentityResolutionError> {
    match tag {
        1 => Ok(AuthProvider::Google),
        2 => Ok(AuthProvider::Email),
        3 => Ok(AuthProvider::Apple),
        _ => Err(IdentityResolutionError::CryptoFailure),
    }
}

fn put_u32(bytes: &mut Vec<u8>, value: usize) -> Result<(), IdentityResolutionError> {
    let value = u32::try_from(value).map_err(|_| IdentityResolutionError::CryptoFailure)?;
    bytes.extend_from_slice(&value.to_be_bytes());
    Ok(())
}

fn put_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), IdentityResolutionError> {
    put_u32(bytes, value.len())?;
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

fn put_optional_string(
    bytes: &mut Vec<u8>,
    value: Option<&str>,
) -> Result<(), IdentityResolutionError> {
    match value {
        Some(value) => {
            bytes.push(1);
            put_string(bytes, value)
        }
        None => {
            bytes.push(0);
            Ok(())
        }
    }
}

struct CandidateReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> CandidateReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], IdentityResolutionError> {
        let end = self
            .cursor
            .checked_add(len)
            .ok_or(IdentityResolutionError::CryptoFailure)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(IdentityResolutionError::CryptoFailure)?;
        self.cursor = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, IdentityResolutionError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, IdentityResolutionError> {
        let raw: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| IdentityResolutionError::CryptoFailure)?;
        Ok(u32::from_be_bytes(raw))
    }

    fn string(&mut self, max: usize) -> Result<String, IdentityResolutionError> {
        let len = self.u32()? as usize;
        if len > max {
            return Err(IdentityResolutionError::CryptoFailure);
        }
        String::from_utf8(self.take(len)?.to_vec())
            .map_err(|_| IdentityResolutionError::CryptoFailure)
    }

    fn optional_string(&mut self, max: usize) -> Result<Option<String>, IdentityResolutionError> {
        match self.byte()? {
            0 => Ok(None),
            1 => self.string(max).map(Some),
            _ => Err(IdentityResolutionError::CryptoFailure),
        }
    }

    fn is_finished(&self) -> bool {
        self.cursor == self.bytes.len()
    }
}

#[cfg(test)]
mod tests;
