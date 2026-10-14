// SPDX-License-Identifier: MPL-2.0

//! Milestone-one account orchestration.
//!
//! This module composes provider verification, identity resolution, hosted
//! discovery metadata, and session issuance. It never accepts Koru content or
//! provider credentials after the provider-neutral verification boundary.

use koru_account_core::*;
use serde::{Deserialize, Serialize};

use crate::{
    NativeSessionTokens, RateLimitSubject, SecurityError, SessionSecurity, WebSessionToken,
};

const MAX_PROFILE_NAME_BYTES: usize = 512;
const MAX_LOCALE_BYTES: usize = 64;
pub(crate) const MAX_APP_VERSION_BYTES: usize = 128;

pub trait AccountIntegrationRepository:
    IdentityResolutionRepository
    + ProfileRepository
    + DeviceRepository
    + SessionRepository
    + AccountExchangeRepository
    + AccountProofRepository
{
}
impl<T> AccountIntegrationRepository for T where
    T: IdentityResolutionRepository
        + ProfileRepository
        + DeviceRepository
        + SessionRepository
        + AccountExchangeRepository
        + AccountProofRepository
{
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRegistration {
    pub descriptor: DeviceDescriptorV1,
    pub platform: DevicePlatform,
    pub app_version: String,
    #[serde(default)]
    pub enrollment_protocol_version: Option<u32>,
    #[serde(default)]
    pub peer_protocol_version: Option<u32>,
    #[serde(default)]
    pub ownership_proof: Option<DeviceOwnershipProof>,
}

impl DeviceRegistration {
    pub fn account_device(&self, identity: KoruIdentityId, now: UnixMillis) -> AccountDevice {
        AccountDevice {
            descriptor: self.descriptor.clone(),
            registration: AccountDeviceRegistration {
                koru_identity_id: identity,
                platform: self.platform,
                app_version: self.app_version.clone(),
                enrollment_protocol_version: self.enrollment_protocol_version,
                peer_protocol_version: self.peer_protocol_version,
                registered_at_millis: now,
                last_seen_at_millis: now,
                revoked_at_millis: None,
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NativeAuthentication {
    /// Creation requires the hosted, client-bound confirmation ceremony.
    WouldCreate,
    Returning {
        resolved: ResolvedIdentity,
        device: DeviceRegistrationOutcome,
        tokens: NativeSessionTokens,
    },
    CollisionRequired {
        flow_id: AuthFlowId,
        expires_at_millis: UnixMillis,
        provider: AuthProvider,
        prompt: AuthenticationPrompt,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum WebAuthentication {
    /// Creation requires the hosted, client-bound confirmation ceremony.
    WouldCreate,
    Returning {
        resolved: ResolvedIdentity,
        token: WebSessionToken,
    },
    CollisionRequired {
        flow_id: AuthFlowId,
        expires_at_millis: UnixMillis,
        provider: AuthProvider,
        prompt: AuthenticationPrompt,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountIntegrationError {
    Identity(IdentityResolutionError),
    Session(SecurityError),
    InvalidAuthentication,
    InvalidDescriptor,
    InvalidMetadata,
    Conflict,
    RepositoryUnavailable,
    Corrupt,
    Rebind(ProfileRebindError),
}

impl From<IdentityResolutionError> for AccountIntegrationError {
    fn from(value: IdentityResolutionError) -> Self {
        Self::Identity(value)
    }
}
impl From<SecurityError> for AccountIntegrationError {
    fn from(value: SecurityError) -> Self {
        Self::Session(value)
    }
}
impl From<ProfileRebindError> for AccountIntegrationError {
    fn from(value: ProfileRebindError) -> Self {
        Self::Rebind(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatusStage {
    NeedsProfile,
    Ready,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatusFailureCode {
    Unauthenticated,
    IdentityDisabled,
    RepositoryUnavailable,
    Corrupt,
}

/// Deliberately contains only bounded counts and coarse state. It cannot leak
/// ids, email addresses, provider subjects, Profile names, or device labels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountStatusSnapshot {
    pub stage: AccountStatusStage,
    pub auth_identity_count: u32,
    pub authorized_profile_count: u32,
    pub device_count: u32,
    pub active_device_count: u32,
    pub failure: Option<AccountStatusFailureCode>,
}

pub struct AccountCoordinator<'a, I, V, C, E> {
    repository: &'a I,
    resolver: IdentityResolver<'a, I, V, C, E>,
    sessions: &'a SessionSecurity<'a>,
    clock: &'a C,
}

impl<'a, I, V, C, E> AccountCoordinator<'a, I, V, C, E>
where
    I: AccountIntegrationRepository,
    V: GoogleIdentityVerifier,
    C: Clock,
    E: TransientEncryption,
{
    pub fn new(
        repository: &'a I,
        verifier: &'a V,
        clock: &'a C,
        encryption: &'a E,
        sessions: &'a SessionSecurity<'a>,
    ) -> Self {
        Self {
            repository,
            resolver: IdentityResolver::new(repository, verifier, clock, encryption),
            sessions,
            clock,
        }
    }

    pub async fn authenticate_native(
        &self,
        request: GoogleAuthenticationRequest,
        registration: DeviceRegistration,
    ) -> Result<NativeAuthentication, AccountIntegrationError> {
        validate_registration(&registration)?;
        let resolved = self.resolver.authenticate_google(request).await?;
        let resolved = match resolved {
            IdentityResolution::CollisionRequired {
                flow_id,
                expires_at_millis,
                provider,
                prompt,
            } => {
                return Ok(NativeAuthentication::CollisionRequired {
                    flow_id,
                    expires_at_millis,
                    provider,
                    prompt,
                });
            }
            IdentityResolution::WouldCreate(_) => return Ok(NativeAuthentication::WouldCreate),
            IdentityResolution::Returning(value) => value,
        };
        let device = self
            .register_resolved_device(&resolved, registration)
            .await?;
        let tokens = self
            .sessions
            .issue_native(
                resolved.identity.id,
                resolved.auth_identity.id,
                device_value(&device).device_id(),
            )
            .await?;
        Ok(NativeAuthentication::Returning {
            resolved,
            device,
            tokens,
        })
    }

    pub async fn authenticate_web(
        &self,
        request: GoogleAuthenticationRequest,
    ) -> Result<WebAuthentication, AccountIntegrationError> {
        let resolved = self.resolver.authenticate_google(request).await?;
        let resolved = match resolved {
            IdentityResolution::CollisionRequired {
                flow_id,
                expires_at_millis,
                provider,
                prompt,
            } => {
                return Ok(WebAuthentication::CollisionRequired {
                    flow_id,
                    expires_at_millis,
                    provider,
                    prompt,
                });
            }
            IdentityResolution::WouldCreate(_) => return Ok(WebAuthentication::WouldCreate),
            IdentityResolution::Returning(value) => value,
        };
        let token = self
            .sessions
            .issue_web(resolved.identity.id, resolved.auth_identity.id)
            .await?;
        Ok(WebAuthentication::Returning { resolved, token })
    }

    pub async fn link_google(
        &self,
        verification: GoogleVerificationInput,
        proof: &RecentAuthentication,
    ) -> Result<LinkResolution, AccountIntegrationError> {
        Ok(self.resolver.link_google(verification, proof).await?)
    }

    pub async fn complete_collision_link(
        &self,
        flow_id: AuthFlowId,
        proof: &RecentAuthentication,
    ) -> Result<LinkResolution, AccountIntegrationError> {
        Ok(self.resolver.complete_collision(flow_id, proof).await?)
    }

    pub async fn attach_profile(
        &self,
        profile_id: ProfileId,
        discovery_name_snapshot: Option<String>,
        locale_snapshot: Option<String>,
        device_id: Option<DeviceId>,
        proof: &RecentAuthentication,
    ) -> Result<ProfileAttachOutcome, AccountIntegrationError> {
        validate_profile_metadata(
            discovery_name_snapshot.as_deref(),
            locale_snapshot.as_deref(),
        )?;
        self.validate_recent(proof).await?;
        let now = self.clock.now_millis();
        self.repository
            .attach_profile(
                ProfileBinding {
                    profile_id,
                    koru_identity_id: proof.koru_identity_id(),
                    discovery_name_snapshot,
                    locale_snapshot,
                    lifecycle_state: ProfileLifecycleState::Active,
                    lifecycle_revision: ProfileLifecycleRevision::INITIAL,
                    lifecycle_operation_id: None,
                    deleted_at_millis: None,
                    created_at_millis: now,
                    updated_at_millis: now,
                },
                ProfileAuthBinding {
                    profile_id,
                    auth_identity_id: proof.auth_identity_id(),
                    created_at_millis: now,
                },
                device_id,
            )
            .await
            .map_err(map_repository)
    }

    pub async fn create_profile(
        &self,
        profile_id: ProfileId,
        discovery_name_snapshot: Option<String>,
        locale_snapshot: Option<String>,
        session_id: AccountSessionId,
    ) -> Result<ProfileAttachOutcome, AccountIntegrationError> {
        validate_profile_metadata(
            discovery_name_snapshot.as_deref(),
            locale_snapshot.as_deref(),
        )?;
        let session = self.validate_session(session_id).await?;
        let now = self.clock.now_millis();
        self.repository
            .attach_profile(
                ProfileBinding {
                    profile_id,
                    koru_identity_id: session.koru_identity_id,
                    discovery_name_snapshot,
                    locale_snapshot,
                    lifecycle_state: ProfileLifecycleState::Active,
                    lifecycle_revision: ProfileLifecycleRevision::INITIAL,
                    lifecycle_operation_id: None,
                    deleted_at_millis: None,
                    created_at_millis: now,
                    updated_at_millis: now,
                },
                ProfileAuthBinding {
                    profile_id,
                    auth_identity_id: session.authenticated_by,
                    created_at_millis: now,
                },
                session.device_id,
            )
            .await
            .map_err(map_repository)
    }

    pub async fn list_profiles(
        &self,
        proof: &RecentAuthentication,
    ) -> Result<Vec<ProfileBinding>, AccountIntegrationError> {
        self.validate_recent(proof).await?;
        self.repository
            .list_profiles(proof.koru_identity_id(), proof.auth_identity_id())
            .await
            .map_err(map_repository)
    }

    pub async fn list_profiles_for_session(
        &self,
        session_id: AccountSessionId,
    ) -> Result<Vec<ProfileBinding>, AccountIntegrationError> {
        let session = self.validate_session(session_id).await?;
        self.repository
            .list_profiles(session.koru_identity_id, session.authenticated_by)
            .await
            .map_err(map_repository)
    }

    pub async fn rebind_erased_profile(
        &self,
        erased_identity_id: KoruIdentityId,
        profile_id: ProfileId,
        discovery_name_snapshot: Option<String>,
        locale_snapshot: Option<String>,
        explicit_rebind: bool,
        proof: &RecentAuthentication,
    ) -> Result<ProfileRebindOutcome, AccountIntegrationError> {
        validate_profile_metadata(
            discovery_name_snapshot.as_deref(),
            locale_snapshot.as_deref(),
        )?;
        let auth = self.validate_recent(proof).await?;
        if auth.provider != AuthProvider::Google {
            return Err(AccountIntegrationError::InvalidAuthentication);
        }
        let now = self.clock.now_millis();
        Ok(self
            .repository
            .rebind_erased_profile(ProfileRebindRequest {
                profile: ProfileBinding {
                    profile_id,
                    koru_identity_id: proof.koru_identity_id(),
                    discovery_name_snapshot,
                    locale_snapshot,
                    lifecycle_state: ProfileLifecycleState::Active,
                    lifecycle_revision: ProfileLifecycleRevision::INITIAL,
                    lifecycle_operation_id: None,
                    deleted_at_millis: None,
                    created_at_millis: now,
                    updated_at_millis: now,
                },
                authorization: ProfileAuthBinding {
                    profile_id,
                    auth_identity_id: proof.auth_identity_id(),
                    created_at_millis: now,
                },
                erased_identity_id,
                authenticated_at_millis: proof.authenticated_at_millis(),
                now_millis: now,
                explicit_rebind,
            })
            .await?)
    }

    pub async fn prepare_rebind_erased_profile(
        &self,
        erased_identity_id: KoruIdentityId,
        profile_id: ProfileId,
        discovery_name_snapshot: Option<String>,
        locale_snapshot: Option<String>,
        explicit_rebind: bool,
        proof: &RecentAuthentication,
    ) -> Result<ProfileRebindRequest, AccountIntegrationError> {
        validate_profile_metadata(
            discovery_name_snapshot.as_deref(),
            locale_snapshot.as_deref(),
        )?;
        let auth = self.validate_recent(proof).await?;
        if auth.provider != AuthProvider::Google {
            return Err(AccountIntegrationError::InvalidAuthentication);
        }
        let now = self.clock.now_millis();
        Ok(ProfileRebindRequest {
            profile: ProfileBinding {
                profile_id,
                koru_identity_id: proof.koru_identity_id(),
                discovery_name_snapshot,
                locale_snapshot,
                lifecycle_state: ProfileLifecycleState::Active,
                lifecycle_revision: ProfileLifecycleRevision::INITIAL,
                lifecycle_operation_id: None,
                deleted_at_millis: None,
                created_at_millis: now,
                updated_at_millis: now,
            },
            authorization: ProfileAuthBinding {
                profile_id,
                auth_identity_id: proof.auth_identity_id(),
                created_at_millis: now,
            },
            erased_identity_id,
            authenticated_at_millis: proof.authenticated_at_millis(),
            now_millis: now,
            explicit_rebind,
        })
    }

    pub fn prepare_rebind_request_from_grant(
        &self,
        erased_identity_id: KoruIdentityId,
        profile_id: ProfileId,
        discovery_name_snapshot: Option<String>,
        locale_snapshot: Option<String>,
        explicit_rebind: bool,
        koru_identity_id: KoruIdentityId,
        auth_identity_id: AuthIdentityId,
        authenticated_at_millis: UnixMillis,
    ) -> Result<ProfileRebindRequest, AccountIntegrationError> {
        validate_profile_metadata(
            discovery_name_snapshot.as_deref(),
            locale_snapshot.as_deref(),
        )?;
        let now = self.clock.now_millis();
        Ok(ProfileRebindRequest {
            profile: ProfileBinding {
                profile_id,
                koru_identity_id,
                discovery_name_snapshot,
                locale_snapshot,
                lifecycle_state: ProfileLifecycleState::Active,
                lifecycle_revision: ProfileLifecycleRevision::INITIAL,
                lifecycle_operation_id: None,
                deleted_at_millis: None,
                created_at_millis: now,
                updated_at_millis: now,
            },
            authorization: ProfileAuthBinding {
                profile_id,
                auth_identity_id,
                created_at_millis: now,
            },
            erased_identity_id,
            authenticated_at_millis,
            now_millis: now,
            explicit_rebind,
        })
    }

    pub async fn register_device(
        &self,
        registration: DeviceRegistration,
        proof: &RecentAuthentication,
    ) -> Result<DeviceRegistrationOutcome, AccountIntegrationError> {
        validate_registration(&registration)?;
        self.validate_recent(proof).await?;
        self.register_device_for(proof.koru_identity_id(), registration)
            .await
    }

    /// Completes registration after `SessionSecurity::exchange_native_grant`
    /// has already admitted the same identity/device operation. Keeping this
    /// narrow seam avoids charging one native exchange twice while preserving
    /// descriptor validation and fresh exact-account proof checks.
    pub async fn register_device_after_exchange_rate_check(
        &self,
        registration: DeviceRegistration,
        proof: &RecentAuthentication,
    ) -> Result<DeviceRegistrationOutcome, AccountIntegrationError> {
        validate_registration(&registration)?;
        self.validate_recent(proof).await?;
        self.persist_device(proof.koru_identity_id(), registration)
            .await
    }

    pub async fn register_device_for_session(
        &self,
        registration: DeviceRegistration,
        session_id: AccountSessionId,
    ) -> Result<DeviceRegistrationOutcome, AccountIntegrationError> {
        validate_registration(&registration)?;
        let session = self.validate_session(session_id).await?;
        self.register_device_for(session.koru_identity_id, registration)
            .await
    }

    pub async fn revoke_device(
        &self,
        device_id: DeviceId,
        proof: &RecentAuthentication,
    ) -> Result<AccountDevice, AccountIntegrationError> {
        self.validate_recent(proof).await?;
        self.sessions
            .check_rate(RateLimitSubject::DeviceOperation {
                identity: proof.koru_identity_id(),
                device: device_id,
            })
            .await?;
        self.repository
            .revoke_device(proof.koru_identity_id(), device_id, self.clock.now_millis())
            .await
            .map_err(map_repository)
    }

    pub async fn revoke_device_for_session(
        &self,
        device_id: DeviceId,
        session_id: AccountSessionId,
    ) -> Result<AccountDevice, AccountIntegrationError> {
        let session = self.validate_session(session_id).await?;
        self.sessions
            .check_rate(RateLimitSubject::DeviceOperation {
                identity: session.koru_identity_id,
                device: device_id,
            })
            .await?;
        self.repository
            .revoke_device(session.koru_identity_id, device_id, self.clock.now_millis())
            .await
            .map_err(map_repository)
    }

    pub async fn list_devices_for_session(
        &self,
        session_id: AccountSessionId,
    ) -> Result<AccountDevicePage, AccountIntegrationError> {
        let session = self.validate_session(session_id).await?;
        self.repository
            .list_devices_for_session(
                session.koru_identity_id,
                session.authenticated_by,
                session.id,
                self.clock.now_millis(),
            )
            .await
            .map_err(map_repository)
    }

    pub async fn status(&self, proof: &RecentAuthentication) -> AccountStatusSnapshot {
        match self.status_inner(proof).await {
            Ok(snapshot) => snapshot,
            Err(failure) => AccountStatusSnapshot {
                stage: AccountStatusStage::Unavailable,
                auth_identity_count: 0,
                authorized_profile_count: 0,
                device_count: 0,
                active_device_count: 0,
                failure: Some(failure),
            },
        }
    }

    pub async fn status_for_session(&self, session_id: AccountSessionId) -> AccountStatusSnapshot {
        let session = match self.validate_session(session_id).await {
            Ok(session) => session,
            Err(error) => return failed_status(error),
        };
        match self
            .status_counts(session.koru_identity_id, session.authenticated_by)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(failure) => failure_status(failure),
        }
    }

    /// Validates a durable session and converts its recorded authentication
    /// timestamp into the same unforgeable proof used by recent-auth flows.
    /// Callers still fail closed when that timestamp is outside the five-minute
    /// recent-authentication window.
    pub async fn recent_authentication_for_session(
        &self,
        session_id: AccountSessionId,
    ) -> Result<RecentAuthentication, AccountIntegrationError> {
        let session = self.validate_session(session_id).await?;
        Ok(self
            .resolver
            .recent_authentication_from_sealed(
                session.koru_identity_id,
                session.authenticated_by,
                session.recent_auth_at_millis,
            )
            .await?)
    }

    pub async fn session_by_id(
        &self,
        session_id: AccountSessionId,
    ) -> Result<AccountSession, AccountIntegrationError> {
        self.validate_session(session_id).await
    }

    pub async fn recent_authentication_from_grant(
        &self,
        koru_identity_id: KoruIdentityId,
        auth_identity_id: AuthIdentityId,
        authenticated_at_millis: UnixMillis,
    ) -> Result<RecentAuthentication, AccountIntegrationError> {
        Ok(self
            .resolver
            .recent_authentication_from_sealed(
                koru_identity_id,
                auth_identity_id,
                authenticated_at_millis,
            )
            .await?)
    }

    async fn status_inner(
        &self,
        proof: &RecentAuthentication,
    ) -> Result<AccountStatusSnapshot, AccountStatusFailureCode> {
        self.validate_recent(proof)
            .await
            .map_err(|error| match error {
                AccountIntegrationError::Corrupt => AccountStatusFailureCode::Corrupt,
                AccountIntegrationError::RepositoryUnavailable => {
                    AccountStatusFailureCode::RepositoryUnavailable
                }
                AccountIntegrationError::Identity(IdentityResolutionError::IdentityDisabled) => {
                    AccountStatusFailureCode::IdentityDisabled
                }
                _ => AccountStatusFailureCode::Unauthenticated,
            })?;
        self.status_counts(proof.koru_identity_id(), proof.auth_identity_id())
            .await
    }

    async fn status_counts(
        &self,
        identity_id: KoruIdentityId,
        authenticated_by: AuthIdentityId,
    ) -> Result<AccountStatusSnapshot, AccountStatusFailureCode> {
        let auth = self
            .repository
            .list_auth_identities(identity_id)
            .await
            .map_err(map_status_repository)?;
        let profiles = self
            .repository
            .list_profiles(identity_id, authenticated_by)
            .await
            .map_err(map_status_repository)?;
        let devices = self
            .repository
            .list_devices(identity_id)
            .await
            .map_err(map_status_repository)?;
        Ok(AccountStatusSnapshot {
            stage: if profiles.is_empty() {
                AccountStatusStage::NeedsProfile
            } else {
                AccountStatusStage::Ready
            },
            auth_identity_count: bounded_count(auth.len())?,
            authorized_profile_count: bounded_count(profiles.len())?,
            device_count: bounded_count(devices.len())?,
            active_device_count: bounded_count(
                devices
                    .iter()
                    .filter(|d| d.registration.revoked_at_millis.is_none())
                    .count(),
            )?,
            failure: None,
        })
    }

    async fn validate_session(
        &self,
        session_id: AccountSessionId,
    ) -> Result<AccountSession, AccountIntegrationError> {
        let session = self
            .repository
            .find_session(session_id)
            .await
            .map_err(map_repository)?
            .ok_or(AccountIntegrationError::InvalidAuthentication)?;
        self.validate_session_snapshot(session).await
    }

    /// Read-only validation of a session snapshot already loaded and
    /// authenticated by the session-security boundary. This avoids a second
    /// session read; callers must not treat an arbitrary supplied row as proof.
    pub async fn validate_session_snapshot(
        &self,
        session: AccountSession,
    ) -> Result<AccountSession, AccountIntegrationError> {
        let now = self.clock.now_millis();
        if session.revoked_at_millis.is_some()
            || now < session.created_at_millis
            || now >= session.idle_expires_at_millis
            || now >= session.absolute_expires_at_millis
        {
            return Err(AccountIntegrationError::InvalidAuthentication);
        }
        let identity = self
            .repository
            .find_koru_identity(session.koru_identity_id)
            .await
            .map_err(map_repository)?
            .ok_or(AccountIntegrationError::InvalidAuthentication)?;
        if identity.status != KoruIdentityStatus::Active {
            return Err(AccountIntegrationError::Identity(
                IdentityResolutionError::IdentityDisabled,
            ));
        }
        let owns_auth = self
            .repository
            .list_auth_identities(session.koru_identity_id)
            .await
            .map_err(map_repository)?
            .iter()
            .any(|auth| auth.id == session.authenticated_by);
        if !owns_auth {
            return Err(AccountIntegrationError::InvalidAuthentication);
        }
        Ok(session)
    }

    async fn validate_recent(
        &self,
        proof: &RecentAuthentication,
    ) -> Result<AuthIdentity, AccountIntegrationError> {
        let now = self.clock.now_millis();
        if now < proof.authenticated_at_millis() || now >= proof.expires_at_millis() {
            return Err(AccountIntegrationError::InvalidAuthentication);
        }
        let identity = self
            .repository
            .find_koru_identity(proof.koru_identity_id())
            .await
            .map_err(map_repository)?
            .ok_or(AccountIntegrationError::InvalidAuthentication)?;
        if identity.status != KoruIdentityStatus::Active {
            return Err(AccountIntegrationError::Identity(
                IdentityResolutionError::IdentityDisabled,
            ));
        }
        self.repository
            .list_auth_identities(proof.koru_identity_id())
            .await
            .map_err(map_repository)?
            .into_iter()
            .find(|auth| {
                auth.id == proof.auth_identity_id()
                    && auth.last_authenticated_at_millis >= proof.authenticated_at_millis()
            })
            .ok_or(AccountIntegrationError::InvalidAuthentication)
    }

    async fn register_resolved_device(
        &self,
        resolved: &ResolvedIdentity,
        registration: DeviceRegistration,
    ) -> Result<DeviceRegistrationOutcome, AccountIntegrationError> {
        self.register_device_for(resolved.identity.id, registration)
            .await
    }

    async fn register_device_for(
        &self,
        identity: KoruIdentityId,
        registration: DeviceRegistration,
    ) -> Result<DeviceRegistrationOutcome, AccountIntegrationError> {
        self.sessions
            .check_rate(RateLimitSubject::DeviceOperation {
                identity,
                device: registration.descriptor.device_id,
            })
            .await?;
        self.persist_device(identity, registration).await
    }

    async fn persist_device(
        &self,
        identity: KoruIdentityId,
        registration: DeviceRegistration,
    ) -> Result<DeviceRegistrationOutcome, AccountIntegrationError> {
        let now = self.clock.now_millis();
        let device = registration.account_device(identity, now);
        let proof = match &registration.ownership_proof {
            Some(proof) => Some(
                self.sessions
                    .verify_device_ownership(proof, &device, None)
                    .await?,
            ),
            None => None,
        };
        self.repository
            .register_device(device, proof)
            .await
            .map_err(map_repository)
    }
}

fn device_value(outcome: &DeviceRegistrationOutcome) -> &AccountDevice {
    match outcome {
        DeviceRegistrationOutcome::Registered(value)
        | DeviceRegistrationOutcome::AlreadyRegistered(value) => value,
    }
}

fn validate_registration(value: &DeviceRegistration) -> Result<(), AccountIntegrationError> {
    if value.app_version.is_empty() || value.app_version.len() > MAX_APP_VERSION_BYTES {
        return Err(AccountIntegrationError::InvalidMetadata);
    }
    value
        .descriptor
        .verify()
        .map_err(|_| AccountIntegrationError::InvalidDescriptor)
}

fn validate_profile_metadata(
    name: Option<&str>,
    locale: Option<&str>,
) -> Result<(), AccountIntegrationError> {
    if name.is_some_and(|v| v.len() > MAX_PROFILE_NAME_BYTES)
        || locale.is_some_and(|v| v.is_empty() || v.len() > MAX_LOCALE_BYTES)
    {
        return Err(AccountIntegrationError::InvalidMetadata);
    }
    Ok(())
}

fn map_repository(error: RepositoryError) -> AccountIntegrationError {
    match error {
        RepositoryError::Conflict | RepositoryError::NotFound => AccountIntegrationError::Conflict,
        RepositoryError::Unavailable => AccountIntegrationError::RepositoryUnavailable,
        RepositoryError::Corrupt => AccountIntegrationError::Corrupt,
    }
}

fn map_status_repository(error: RepositoryError) -> AccountStatusFailureCode {
    match error {
        RepositoryError::Corrupt => AccountStatusFailureCode::Corrupt,
        RepositoryError::Conflict | RepositoryError::NotFound | RepositoryError::Unavailable => {
            AccountStatusFailureCode::RepositoryUnavailable
        }
    }
}

fn bounded_count(value: usize) -> Result<u32, AccountStatusFailureCode> {
    u32::try_from(value).map_err(|_| AccountStatusFailureCode::Corrupt)
}

fn failed_status(error: AccountIntegrationError) -> AccountStatusSnapshot {
    let failure = match error {
        AccountIntegrationError::Corrupt => AccountStatusFailureCode::Corrupt,
        AccountIntegrationError::RepositoryUnavailable => {
            AccountStatusFailureCode::RepositoryUnavailable
        }
        AccountIntegrationError::Identity(IdentityResolutionError::IdentityDisabled) => {
            AccountStatusFailureCode::IdentityDisabled
        }
        _ => AccountStatusFailureCode::Unauthenticated,
    };
    failure_status(failure)
}

fn failure_status(failure: AccountStatusFailureCode) -> AccountStatusSnapshot {
    AccountStatusSnapshot {
        stage: AccountStatusStage::Unavailable,
        auth_identity_count: 0,
        authorized_profile_count: 0,
        device_count: 0,
        active_device_count: 0,
        failure: Some(failure),
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests;
