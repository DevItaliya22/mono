// SPDX-License-Identifier: MPL-2.0

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use koru_account_core::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::str::FromStr;
use zeroize::Zeroize;

use crate::{
    ACCESS_TOKEN_MILLIS, AUTHENTICATION_ATTEMPT_RATE, CALLBACK_RATE, DEVICE_RATE, FLOW_MILLIS,
    LINK_RATE, REFRESH_ABSOLUTE_MILLIS, REFRESH_IDLE_MILLIS, REFRESH_RATE, RETRY_MILLIS,
    RateLimitSubject, SECRET_BYTES, START_RATE, SecurityError, SymbolicReturnTarget,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSessionTokens {
    pub session_id: AccountSessionId,
    pub access: SignedToken,
    pub refresh: OpaqueSecret,
    pub access_expires_at_millis: UnixMillis,
    pub idle_expires_at_millis: UnixMillis,
    pub absolute_expires_at_millis: UnixMillis,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSessionToken {
    pub session_id: AccountSessionId,
    pub session: OpaqueSecret,
    pub csrf: OpaqueSecret,
    pub expires_at_millis: UnixMillis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedNativeSession {
    pub session: AccountSession,
    pub tokens: NativeSessionTokens,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedWebSession {
    pub session: AccountSession,
    pub token: WebSessionToken,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowSecrets {
    pub flow_id: AuthFlowId,
    pub state: OpaqueSecret,
    pub nonce: OpaqueSecret,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OneUseGrant {
    pub flow_id: AuthFlowId,
    pub grant: OpaqueSecret,
}

#[derive(Clone, PartialEq, Eq)]
pub enum ExchangeGrantInspection {
    Pending(ExchangeGrantPayload),
    ExactRetry(SealedValue),
}

impl std::fmt::Debug for ExchangeGrantInspection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending(_) => f.write_str("ExchangeGrantInspection::Pending(<redacted>)"),
            Self::ExactRetry(_) => f.write_str("ExchangeGrantInspection::ExactRetry(<redacted>)"),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct FlowPayload {
    pub return_target: SymbolicReturnTarget,
    pub nonce: OpaqueSecret,
    pub pkce_artifact: OpaqueSecret,
    pub one_use_grant: OpaqueSecret,
    pub exchange: Option<ExchangeGrantPayload>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeExchangeGrantInput {
    pub registration: crate::DeviceRegistration,
    pub client_state: Vec<u8>,
    pub pkce_challenge: String,
}

impl Drop for NativeExchangeGrantInput {
    fn drop(&mut self) {
        self.client_state.zeroize();
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeExchangeBinding {
    pub registration: crate::DeviceRegistration,
    pub client_state_digest: KeyedDigest,
    pub pkce_challenge: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeGrantPayload {
    pub parent_flow_id: Option<AuthFlowId>,
    pub koru_identity_id: KoruIdentityId,
    pub auth_identity_id: AuthIdentityId,
    pub authenticated_at_millis: UnixMillis,
    pub native: Option<NativeExchangeBinding>,
    pub verified_candidate: Option<VerifiedExternalIdentity>,
    pub profile_id: Option<ProfileId>,
    pub erased_koru_identity_id: Option<KoruIdentityId>,
    #[serde(default)]
    pub initiating_session_id: Option<AccountSessionId>,
    #[serde(default)]
    pub expires_at_millis: UnixMillis,
}

impl std::fmt::Debug for NativeExchangeGrantInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeExchangeGrantInput(<redacted>)")
    }
}

impl std::fmt::Debug for NativeExchangeBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeExchangeBinding(<redacted>)")
    }
}

impl std::fmt::Debug for ExchangeGrantPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExchangeGrantPayload(<redacted>)")
    }
}

#[derive(Serialize, Deserialize)]
struct FlowEnvelope {
    return_target: SymbolicReturnTarget,
    nonce: Vec<u8>,
    pkce_artifact: Vec<u8>,
    one_use_grant: Vec<u8>,
    #[serde(default)]
    exchange: Option<ExchangeGrantPayload>,
}

impl From<FlowEnvelope> for FlowPayload {
    fn from(value: FlowEnvelope) -> Self {
        Self {
            return_target: value.return_target,
            nonce: OpaqueSecret(value.nonce),
            pkce_artifact: OpaqueSecret(value.pkce_artifact),
            one_use_grant: OpaqueSecret(value.one_use_grant),
            exchange: value.exchange,
        }
    }
}
impl std::fmt::Debug for FlowPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FlowPayload(<redacted>)")
    }
}

#[derive(Serialize, Deserialize)]
struct RetryEnvelope {
    key_id: String,
    access: Vec<u8>,
    refresh: Vec<u8>,
}

impl Drop for RetryEnvelope {
    fn drop(&mut self) {
        self.access.zeroize();
        self.refresh.zeroize();
    }
}

pub struct SessionSecurity<'a> {
    pub issuer: &'a str,
    pub audience: &'a str,
    pub clock: &'a dyn Clock,
    pub entropy: &'a dyn Entropy,
    pub sessions: &'a dyn SessionRepository,
    pub flows: &'a dyn AuthFlowRepository,
    pub crypto: &'a dyn TransientEncryption,
    pub signer: &'a dyn TokenSigner,
    pub limiter: &'a dyn RateLimiter,
}

impl SessionSecurity<'_> {
    pub async fn issue_device_ownership_challenge(
        &self,
        device: AccountDevice,
        parent_flow_id: Option<AuthFlowId>,
        source: &[u8],
    ) -> Result<DeviceOwnershipChallenge, SecurityError> {
        if device.registration.app_version.is_empty()
            || device.registration.app_version.len() > crate::account::MAX_APP_VERSION_BYTES
        {
            return Err(SecurityError::InvalidSecret);
        }
        device
            .descriptor
            .verify()
            .map_err(|_| SecurityError::InvalidSecret)?;
        self.check_rate(RateLimitSubject::DeviceOperation {
            identity: device.registration.koru_identity_id,
            device: device.device_id(),
        })
        .await?;
        let now = self.clock.now_millis();
        let mut nonce = [0; 32];
        self.entropy
            .fill(&mut nonce)
            .map_err(|_| SecurityError::Crypto)?;
        let challenge = DeviceOwnershipChallenge {
            flow_id: self.id()?,
            parent_flow_id,
            device,
            expires_at_millis: Self::expires(now, 5 * 60 * 1000)?,
            nonce,
        };
        let mut raw = serde_json::to_vec(&challenge).map_err(|_| SecurityError::Crypto)?;
        let sealed = self.crypto.seal("device-ownership-challenge", &raw);
        raw.zeroize();
        self.flows
            .insert_flow(TemporaryAuthFlow {
                id: challenge.flow_id,
                kind: AuthFlowKind::GoogleNative,
                status: AuthFlowStatus::Pending,
                state_digest: self
                    .digest("device-ownership-challenge", &challenge.signing_bytes())?,
                sealed_transient: sealed.map_err(|_| SecurityError::Crypto)?,
                sealed_exchange_result: None,
                source_digest: self.digest("flow-source", source)?,
                target_identity_id: Some(challenge.device.registration.koru_identity_id),
                created_at_millis: now,
                expires_at_millis: challenge.expires_at_millis,
                consumed_at_millis: None,
            })
            .await
            .map_err(|_| SecurityError::Repository)?;
        Ok(challenge)
    }

    pub async fn verify_device_ownership(
        &self,
        proof: &DeviceOwnershipProof,
        device: &AccountDevice,
        parent_flow_id: Option<AuthFlowId>,
    ) -> Result<VerifiedDeviceOwnership, SecurityError> {
        if proof.challenge.parent_flow_id != parent_flow_id
            || !proof.challenge.matches_registration(device)
            || !proof.verify_signature()
        {
            return Err(SecurityError::InvalidSecret);
        }
        let flow = self
            .flows
            .find_flow(proof.challenge.flow_id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        let state_digest = self.digest(
            "device-ownership-challenge",
            &proof.challenge.signing_bytes(),
        )?;
        if flow.kind != AuthFlowKind::GoogleNative
            || flow.target_identity_id != Some(device.registration.koru_identity_id)
            || flow.state_digest != state_digest
        {
            return Err(SecurityError::InvalidSecret);
        }
        if flow.status != AuthFlowStatus::Pending {
            return Err(SecurityError::Replayed);
        }
        if self.clock.now_millis() >= flow.expires_at_millis
            || flow.expires_at_millis != proof.challenge.expires_at_millis
        {
            return Err(SecurityError::Expired);
        }
        let mut raw = self
            .crypto
            .open("device-ownership-challenge", &flow.sealed_transient)
            .map_err(|_| SecurityError::Crypto)?;
        let stored = serde_json::from_slice::<DeviceOwnershipChallenge>(&raw);
        raw.zeroize();
        if stored.map_err(|_| SecurityError::Crypto)? != proof.challenge {
            return Err(SecurityError::InvalidSecret);
        }
        Ok(VerifiedDeviceOwnership {
            challenge: proof.challenge.clone(),
            state_digest,
        })
    }

    pub async fn validate_native_retry_binding(
        &self,
        flow_id: AuthFlowId,
        digest: &KeyedDigest,
    ) -> Result<(), SecurityError> {
        if self
            .flows
            .exchange_proof_digest(flow_id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .as_ref()
            != Some(digest)
        {
            return Err(SecurityError::Replayed);
        }
        Ok(())
    }
    fn secret(&self) -> Result<OpaqueSecret, SecurityError> {
        let mut b = vec![0; SECRET_BYTES];
        self.entropy
            .fill(&mut b)
            .map_err(|_| SecurityError::Crypto)?;
        Ok(OpaqueSecret(b))
    }

    fn url_safe_secret(&self) -> Result<OpaqueSecret, SecurityError> {
        let secret = self.secret()?;
        Ok(OpaqueSecret(
            URL_SAFE_NO_PAD.encode(secret.expose_secret()).into_bytes(),
        ))
    }
    fn token_id(&self) -> Result<String, SecurityError> {
        let secret = self.secret()?;
        Ok(URL_SAFE_NO_PAD.encode(secret.expose_secret()))
    }
    fn expires(now: i64, duration: i64) -> Result<i64, SecurityError> {
        now.checked_add(duration).ok_or(SecurityError::Crypto)
    }
    fn id<T: FromStr>(&self) -> Result<T, SecurityError> {
        let mut bytes = [0; 16];
        self.entropy
            .fill(&mut bytes)
            .map_err(|_| SecurityError::Crypto)?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        )
        .parse()
        .map_err(|_| SecurityError::Crypto)
    }
    fn digest(&self, p: &'static str, v: &[u8]) -> Result<KeyedDigest, SecurityError> {
        self.crypto.digest(p, v).map_err(|_| SecurityError::Crypto)
    }
    fn retry_result(&self, session: &AccountSession) -> Result<NativeSessionTokens, SecurityError> {
        if session.kind != SessionKind::Native || session.revoked_at_millis.is_some() {
            return Err(SecurityError::Replayed);
        }
        let sealed = session
            .refresh_family
            .as_ref()
            .and_then(|family| family.sealed_previous_result.as_ref())
            .ok_or(SecurityError::Replayed)?;
        let mut raw = self
            .crypto
            .open("refresh-retry-result", sealed)
            .map_err(|_| SecurityError::Crypto)?;
        if raw.len() > 12 * 1024 {
            raw.zeroize();
            return Err(SecurityError::Crypto);
        }
        let decoded = serde_json::from_slice::<RetryEnvelope>(&raw);
        raw.zeroize();
        let mut retry = decoded.map_err(|_| SecurityError::Crypto)?;
        if retry.key_id.is_empty()
            || retry.key_id.len() > 128
            || !retry
                .key_id
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || b"-_.".contains(&value))
            || retry.access.is_empty()
            || retry.access.len() > 8 * 1024
            || retry.refresh.len() != SECRET_BYTES
        {
            return Err(SecurityError::Crypto);
        }
        let key_id = std::mem::take(&mut retry.key_id);
        let access = std::mem::take(&mut retry.access);
        let refresh = std::mem::take(&mut retry.refresh);
        Ok(NativeSessionTokens {
            session_id: session.id,
            access: SignedToken {
                key_id,
                compact: OpaqueSecret(access),
            },
            refresh: OpaqueSecret(refresh),
            access_expires_at_millis: Self::expires(
                session.last_seen_at_millis,
                ACCESS_TOKEN_MILLIS,
            )?,
            idle_expires_at_millis: session.idle_expires_at_millis,
            absolute_expires_at_millis: session.absolute_expires_at_millis,
        })
    }
    fn access(
        &self,
        identity: KoruIdentityId,
        session: AccountSessionId,
        now: i64,
    ) -> Result<SignedToken, SecurityError> {
        let signed = self
            .signer
            .sign_access_token(&AccessTokenClaims {
                issuer: self.issuer.into(),
                audience: self.audience.into(),
                subject: identity,
                session_id: session,
                token_id: self.token_id()?,
                issued_at_millis: now,
                expires_at_millis: Self::expires(now, ACCESS_TOKEN_MILLIS)?,
            })
            .map_err(|_| SecurityError::Crypto)?;
        if signed.key_id.is_empty()
            || signed.key_id.len() > 128
            || signed.compact.expose_secret().is_empty()
            || signed.compact.expose_secret().len() > 8 * 1024
            || std::str::from_utf8(signed.compact.expose_secret()).is_err()
        {
            return Err(SecurityError::Crypto);
        }
        Ok(signed)
    }
    pub async fn issue_native(
        &self,
        identity: KoruIdentityId,
        auth: AuthIdentityId,
        device: DeviceId,
    ) -> Result<NativeSessionTokens, SecurityError> {
        let prepared = self.prepare_native(identity, auth, device)?;
        self.sessions
            .insert_session(prepared.session)
            .await
            .map_err(|_| SecurityError::Repository)?;
        Ok(prepared.tokens)
    }

    pub fn prepare_native(
        &self,
        identity: KoruIdentityId,
        auth: AuthIdentityId,
        device: DeviceId,
    ) -> Result<PreparedNativeSession, SecurityError> {
        self.prepare_native_at(identity, auth, device, self.clock.now_millis())
    }

    pub fn prepare_native_at(
        &self,
        identity: KoruIdentityId,
        auth: AuthIdentityId,
        device: DeviceId,
        authenticated_at_millis: UnixMillis,
    ) -> Result<PreparedNativeSession, SecurityError> {
        let now = self.clock.now_millis();
        self.validate_recent_authentication(now, authenticated_at_millis)?;
        let refresh = self.secret()?;
        let id = self.id()?;
        let digest = self.digest("refresh", refresh.expose_secret())?;
        let idle_expires = Self::expires(now, REFRESH_IDLE_MILLIS)?;
        let absolute_expires = Self::expires(now, REFRESH_ABSOLUTE_MILLIS)?;
        let access = self.access(identity, id, now)?;
        let session = AccountSession {
            id,
            koru_identity_id: identity,
            authenticated_by: auth,
            device_id: Some(device),
            kind: SessionKind::Native,
            refresh_family: Some(RefreshFamily {
                current_digest: digest,
                previous_digest: None,
                previous_retry_until_millis: None,
                previous_retry_key_digest: None,
                sealed_previous_result: None,
                rotation: 0,
            }),
            created_at_millis: now,
            last_seen_at_millis: now,
            recent_auth_at_millis: authenticated_at_millis,
            idle_expires_at_millis: idle_expires,
            absolute_expires_at_millis: absolute_expires,
            revoked_at_millis: None,
        };
        Ok(PreparedNativeSession {
            session,
            tokens: NativeSessionTokens {
                session_id: id,
                access,
                refresh,
                access_expires_at_millis: Self::expires(now, ACCESS_TOKEN_MILLIS)?,
                idle_expires_at_millis: idle_expires,
                absolute_expires_at_millis: absolute_expires,
            },
        })
    }
    pub async fn issue_web(
        &self,
        identity: KoruIdentityId,
        auth: AuthIdentityId,
    ) -> Result<WebSessionToken, SecurityError> {
        let prepared = self.prepare_web(identity, auth)?;
        self.sessions
            .insert_session(prepared.session)
            .await
            .map_err(|_| SecurityError::Repository)?;
        Ok(prepared.token)
    }

    pub fn prepare_web(
        &self,
        identity: KoruIdentityId,
        auth: AuthIdentityId,
    ) -> Result<PreparedWebSession, SecurityError> {
        self.prepare_web_at(identity, auth, self.clock.now_millis())
    }

    pub fn prepare_web_at(
        &self,
        identity: KoruIdentityId,
        auth: AuthIdentityId,
        authenticated_at_millis: UnixMillis,
    ) -> Result<PreparedWebSession, SecurityError> {
        let now = self.clock.now_millis();
        self.validate_recent_authentication(now, authenticated_at_millis)?;
        let session = self.secret()?;
        let csrf = self.secret()?;
        let id = self.id()?;
        let digest = self.digest("web-session", session.expose_secret())?;
        let idle_expires = Self::expires(now, REFRESH_IDLE_MILLIS)?;
        let absolute_expires = Self::expires(now, REFRESH_ABSOLUTE_MILLIS)?;
        let stored = AccountSession {
            id,
            koru_identity_id: identity,
            authenticated_by: auth,
            device_id: None,
            kind: SessionKind::Web,
            refresh_family: Some(RefreshFamily {
                current_digest: digest,
                previous_digest: None,
                previous_retry_until_millis: None,
                previous_retry_key_digest: None,
                sealed_previous_result: None,
                rotation: 0,
            }),
            created_at_millis: now,
            last_seen_at_millis: now,
            recent_auth_at_millis: authenticated_at_millis,
            idle_expires_at_millis: idle_expires,
            absolute_expires_at_millis: absolute_expires,
            revoked_at_millis: None,
        };
        Ok(PreparedWebSession {
            session: stored,
            token: WebSessionToken {
                session_id: id,
                session,
                csrf,
                expires_at_millis: idle_expires.min(absolute_expires),
            },
        })
    }

    fn validate_recent_authentication(
        &self,
        now_millis: UnixMillis,
        authenticated_at_millis: UnixMillis,
    ) -> Result<(), SecurityError> {
        if now_millis < authenticated_at_millis {
            return Err(SecurityError::Expired);
        }
        Ok(())
    }

    pub async fn validate_web_session(
        &self,
        id: AccountSessionId,
        presented: &OpaqueSecret,
    ) -> Result<AccountSession, SecurityError> {
        let session = self
            .sessions
            .find_session(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        let now = self.clock.now_millis();
        if session.kind != SessionKind::Web
            || session.revoked_at_millis.is_some()
            || now >= session.idle_expires_at_millis
            || now >= session.absolute_expires_at_millis
            || session.refresh_family.as_ref().map(|v| &v.current_digest)
                != Some(&self.digest("web-session", presented.expose_secret())?)
        {
            return Err(SecurityError::InvalidSecret);
        }
        Ok(session)
    }
    /// Observe current native credential ownership without rotating, revoking,
    /// extending validity, or touching activity. Previous secrets never qualify.
    /// The returned snapshot still needs account/Auth Identity validation; it
    /// confers no access token or recent-authentication proof.
    pub async fn inspect_native_current_session(
        &self,
        id: AccountSessionId,
        presented: &OpaqueSecret,
        device_id: DeviceId,
    ) -> Result<AccountSession, SecurityError> {
        if presented.expose_secret().len() != SECRET_BYTES {
            return Err(SecurityError::InvalidSecret);
        }
        let digest = self.digest("refresh", presented.expose_secret())?;
        let session = self
            .sessions
            .find_session(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        // Authenticate first, before any owner lookup or state-specific result.
        if !session.refresh_family.as_ref().is_some_and(|family| {
            crate::policy::constant_time_eq(&family.current_digest.0, &digest.0)
        }) {
            return Err(SecurityError::InvalidSecret);
        }
        let now = self.clock.now_millis();
        if session.kind != SessionKind::Native
            || session.device_id != Some(device_id)
            || session.revoked_at_millis.is_some()
            || now < session.created_at_millis
            || now >= session.idle_expires_at_millis
            || now >= session.absolute_expires_at_millis
        {
            return Err(SecurityError::InvalidSecret);
        }
        self.check_rate(RateLimitSubject::Refresh { session: id })
            .await?;
        Ok(session)
    }

    pub async fn refresh(
        &self,
        id: AccountSessionId,
        presented: &OpaqueSecret,
        idempotency: &OpaqueSecret,
    ) -> Result<NativeSessionTokens, SecurityError> {
        if idempotency.expose_secret().len() != 32 {
            return Err(SecurityError::InvalidSecret);
        }
        let now = self.clock.now_millis();
        let mut session = self
            .sessions
            .find_session(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        if session.kind != SessionKind::Native || session.revoked_at_millis.is_some() {
            return Err(SecurityError::InvalidSecret);
        }
        if now >= session.idle_expires_at_millis || now >= session.absolute_expires_at_millis {
            self.sessions
                .revoke_session(id, now)
                .await
                .map_err(|_| SecurityError::Repository)?;
            return Err(SecurityError::Expired);
        }
        let presented_digest = self.digest("refresh", presented.expose_secret())?;
        let retry_key = self.digest("refresh-retry-key", idempotency.expose_secret())?;
        let family = session
            .refresh_family
            .clone()
            .ok_or(SecurityError::InvalidSecret)?;
        if family.current_digest != presented_digest
            && family.previous_digest.as_ref() != Some(&presented_digest)
        {
            return Err(SecurityError::InvalidSecret);
        }
        self.check_rate(RateLimitSubject::Refresh { session: id })
            .await?;
        let refresh = self.secret()?;
        let access = self.access(session.koru_identity_id, id, now)?;
        let mut envelope = RetryEnvelope {
            key_id: access.key_id.clone(),
            access: access.compact.expose_secret().to_vec(),
            refresh: refresh.expose_secret().to_vec(),
        };
        let mut plaintext = serde_json::to_vec(&envelope).map_err(|_| SecurityError::Crypto)?;
        let sealed = self
            .crypto
            .seal("refresh-retry-result", &plaintext)
            .map_err(|_| SecurityError::Crypto);
        plaintext.zeroize();
        envelope.access.zeroize();
        envelope.refresh.zeroize();
        let sealed = sealed?;
        let retry_until = Self::expires(now, RETRY_MILLIS)?;
        let idle_expires = Self::expires(now, REFRESH_IDLE_MILLIS)?;
        let rotation = family
            .rotation
            .checked_add(1)
            .ok_or(SecurityError::Crypto)?;
        session.refresh_family = Some(RefreshFamily {
            current_digest: self.digest("refresh", refresh.expose_secret())?,
            previous_digest: Some(presented_digest.clone()),
            previous_retry_until_millis: Some(retry_until),
            previous_retry_key_digest: Some(retry_key.clone()),
            sealed_previous_result: Some(sealed),
            rotation,
        });
        session.last_seen_at_millis = now;
        session.idle_expires_at_millis = idle_expires.min(session.absolute_expires_at_millis);
        let outcome = self
            .sessions
            .rotate_refresh(session, presented_digest, retry_key, now)
            .await
            .map_err(|_| SecurityError::Repository)?;
        match outcome {
            RefreshRotationOutcome::Rotated(stored)
            | RefreshRotationOutcome::ExactRetry(stored) => self.retry_result(&stored),
            RefreshRotationOutcome::ReplayedRevoked => Err(SecurityError::Replayed),
            RefreshRotationOutcome::ExpiredRevoked => Err(SecurityError::Expired),
        }
    }
    pub async fn create_flow(
        &self,
        kind: AuthFlowKind,
        source: &[u8],
        target: Option<KoruIdentityId>,
        return_target: SymbolicReturnTarget,
        pkce_artifact: Vec<u8>,
    ) -> Result<FlowSecrets, SecurityError> {
        let (row, secrets) =
            self.prepare_flow(kind, source, target, return_target, pkce_artifact, None)?;
        self.flows
            .insert_flow(row)
            .await
            .map_err(|_| SecurityError::Repository)?;
        Ok(secrets)
    }

    /// Prepare a provider continuation for atomic insertion with its parent.
    pub fn prepare_flow(
        &self,
        kind: AuthFlowKind,
        source: &[u8],
        target: Option<KoruIdentityId>,
        return_target: SymbolicReturnTarget,
        pkce_artifact: Vec<u8>,
        reservation: Option<(AuthFlowId, OpaqueSecret, OpaqueSecret, UnixMillis)>,
    ) -> Result<(TemporaryAuthFlow, FlowSecrets), SecurityError> {
        let now = self.clock.now_millis();
        let (id, state, nonce, expires_at) = match reservation {
            Some(value) => value,
            None => (
                self.id()?,
                self.secret()?,
                self.url_safe_secret()?,
                Self::expires(now, FLOW_MILLIS)?,
            ),
        };
        if state.expose_secret().len() != SECRET_BYTES
            || nonce.expose_secret().is_empty()
            || expires_at <= now
            || expires_at > Self::expires(now, FLOW_MILLIS)?
        {
            return Err(SecurityError::InvalidSecret);
        }
        let grant = self.secret()?;
        let mut payload = FlowEnvelope {
            return_target,
            nonce: nonce.expose_secret().to_vec(),
            pkce_artifact,
            one_use_grant: grant.expose_secret().to_vec(),
            exchange: None,
        };
        let mut plaintext = serde_json::to_vec(&payload).map_err(|_| SecurityError::Crypto)?;
        let sealed = self
            .crypto
            .seal("auth-flow", &plaintext)
            .map_err(|_| SecurityError::Crypto);
        plaintext.zeroize();
        payload.nonce.zeroize();
        payload.pkce_artifact.zeroize();
        payload.one_use_grant.zeroize();
        let sealed = sealed?;
        let row = TemporaryAuthFlow {
            id,
            kind,
            status: AuthFlowStatus::Pending,
            state_digest: self.digest("flow-state", state.expose_secret())?,
            sealed_transient: sealed,
            sealed_exchange_result: None,
            source_digest: self.digest("flow-source", source)?,
            target_identity_id: target,
            created_at_millis: now,
            expires_at_millis: expires_at,
            consumed_at_millis: None,
        };
        Ok((
            row,
            FlowSecrets {
                flow_id: id,
                state,
                nonce,
            },
        ))
    }

    /// The artifact a flow was created with, and nothing else from it.
    ///
    /// **Every other reader here presents the flow's state secret first, and
    /// this one cannot**: an email flow's steps are bound by the code that was
    /// mailed and by the keyed digest of the address, because the page holds a
    /// flow id and never a state secret. So the narrowest possible opening: it
    /// takes a row the caller has already fetched, returns the artifact bytes
    /// alone, and never yields the nonce, the one-use grant or the exchange
    /// payload that sit beside it in the envelope.
    ///
    /// It is not an authorization of anything. The caller still has to prove
    /// whatever the step requires.
    pub fn flow_artifact(&self, flow: &TemporaryAuthFlow) -> Result<Vec<u8>, SecurityError> {
        let mut raw = self
            .crypto
            .open("auth-flow", &flow.sealed_transient)
            .map_err(|_| SecurityError::Crypto)?;
        let decoded = serde_json::from_slice::<FlowEnvelope>(&raw);
        raw.zeroize();
        let mut envelope = decoded.map_err(|_| SecurityError::Crypto)?;
        envelope.nonce.zeroize();
        envelope.one_use_grant.zeroize();
        Ok(envelope.pkce_artifact)
    }

    /// Read the original callback binding after consumption. This does not
    /// permit exchanging a provider code again; only a committed receipt may
    /// resume an already consumed callback.
    pub async fn inspect_callback_flow(
        &self,
        id: AuthFlowId,
        state: &OpaqueSecret,
    ) -> Result<(TemporaryAuthFlow, FlowPayload), SecurityError> {
        self.validate_flow(id, state).await?;
        let row = self
            .flows
            .find_flow(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        let mut raw = self
            .crypto
            .open("auth-flow", &row.sealed_transient)
            .map_err(|_| SecurityError::Crypto)?;
        let decoded = serde_json::from_slice::<FlowEnvelope>(&raw);
        raw.zeroize();
        Ok((
            row,
            decoded
                .map(FlowPayload::from)
                .map_err(|_| SecurityError::Crypto)?,
        ))
    }
    pub async fn consume_flow(
        &self,
        id: AuthFlowId,
        state: &OpaqueSecret,
    ) -> Result<FlowPayload, SecurityError> {
        self.validate_flow(id, state).await?;
        let now = self.clock.now_millis();
        let consumed = self
            .flows
            .consume_flow(id, now)
            .await
            .map_err(|e| match e {
                RepositoryError::Conflict => SecurityError::Replayed,
                _ => SecurityError::Repository,
            })?;
        let mut raw = self
            .crypto
            .open("auth-flow", &consumed.sealed_transient)
            .map_err(|_| SecurityError::Crypto)?;
        let decoded = serde_json::from_slice::<FlowEnvelope>(&raw);
        raw.zeroize();
        decoded
            .map(FlowPayload::from)
            .map_err(|_| SecurityError::Crypto)
    }

    pub async fn validate_flow(
        &self,
        id: AuthFlowId,
        state: &OpaqueSecret,
    ) -> Result<(), SecurityError> {
        let found = self
            .flows
            .find_flow(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        if found.state_digest != self.digest("flow-state", state.expose_secret())? {
            return Err(SecurityError::InvalidSecret);
        }
        if self.clock.now_millis() >= found.expires_at_millis {
            return Err(SecurityError::Expired);
        }
        Ok(())
    }

    pub async fn issue_grant(
        &self,
        kind: AuthFlowKind,
        target_identity_id: KoruIdentityId,
        return_target: SymbolicReturnTarget,
    ) -> Result<OneUseGrant, SecurityError> {
        self.issue_exchange_grant(
            kind,
            target_identity_id,
            None,
            self.clock.now_millis(),
            return_target,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
    }

    /// Seals the resolved account and native device binding behind a one-use
    /// grant. A callback can therefore finish without creating a session; only
    /// a successful exchange can recover the inputs needed for issuance.
    pub async fn issue_exchange_grant(
        &self,
        kind: AuthFlowKind,
        target_identity_id: KoruIdentityId,
        auth_identity_id: Option<AuthIdentityId>,
        authenticated_at_millis: UnixMillis,
        return_target: SymbolicReturnTarget,
        native: Option<NativeExchangeGrantInput>,
        verified_candidate: Option<VerifiedExternalIdentity>,
        profile_id: Option<ProfileId>,
        erased_koru_identity_id: Option<KoruIdentityId>,
        parent_flow_id: Option<AuthFlowId>,
        initiating_session_id: Option<AccountSessionId>,
    ) -> Result<OneUseGrant, SecurityError> {
        self.issue_reserved_exchange_grant(
            kind,
            target_identity_id,
            auth_identity_id,
            authenticated_at_millis,
            return_target,
            native,
            verified_candidate,
            profile_id,
            erased_koru_identity_id,
            parent_flow_id,
            initiating_session_id,
            None,
        )
        .await
    }

    /// Native token delivery reserves a random grant identity before resolution.
    /// Replaying that pinned proof therefore resumes the same exchange authority.
    pub async fn issue_reserved_exchange_grant(
        &self,
        kind: AuthFlowKind,
        target_identity_id: KoruIdentityId,
        auth_identity_id: Option<AuthIdentityId>,
        authenticated_at_millis: UnixMillis,
        return_target: SymbolicReturnTarget,
        native: Option<NativeExchangeGrantInput>,
        verified_candidate: Option<VerifiedExternalIdentity>,
        profile_id: Option<ProfileId>,
        erased_koru_identity_id: Option<KoruIdentityId>,
        parent_flow_id: Option<AuthFlowId>,
        initiating_session_id: Option<AccountSessionId>,
        reservation: Option<(AuthFlowId, OpaqueSecret, UnixMillis)>,
    ) -> Result<OneUseGrant, SecurityError> {
        let (flow, grant) = self.prepare_reserved_exchange_grant(
            kind,
            target_identity_id,
            auth_identity_id,
            authenticated_at_millis,
            return_target,
            native,
            verified_candidate,
            profile_id,
            erased_koru_identity_id,
            parent_flow_id,
            initiating_session_id,
            reservation,
        )?;
        self.flows
            .insert_flow(flow)
            .await
            .map_err(|_| SecurityError::Repository)?;
        Ok(grant)
    }

    /// Prepare encrypted exchange authority before a repository transaction.
    /// The caller must commit this row atomically with the decision it serves.
    pub fn prepare_reserved_exchange_grant(
        &self,
        kind: AuthFlowKind,
        target_identity_id: KoruIdentityId,
        auth_identity_id: Option<AuthIdentityId>,
        authenticated_at_millis: UnixMillis,
        return_target: SymbolicReturnTarget,
        mut native: Option<NativeExchangeGrantInput>,
        verified_candidate: Option<VerifiedExternalIdentity>,
        profile_id: Option<ProfileId>,
        erased_koru_identity_id: Option<KoruIdentityId>,
        parent_flow_id: Option<AuthFlowId>,
        initiating_session_id: Option<AccountSessionId>,
        reservation: Option<(AuthFlowId, OpaqueSecret, UnixMillis)>,
    ) -> Result<(TemporaryAuthFlow, OneUseGrant), SecurityError> {
        let now = self.clock.now_millis();
        let (id, grant, expires_at) = match reservation {
            Some(value) => value,
            None => {
                let grant = self.secret()?;
                (self.id()?, grant, Self::expires(now, FLOW_MILLIS)?)
            }
        };
        if expires_at <= now {
            return Err(SecurityError::Expired);
        }
        if expires_at > Self::expires(now, FLOW_MILLIS)?
            || grant.expose_secret().len() != SECRET_BYTES
        {
            return Err(SecurityError::InvalidSecret);
        }
        let native_binding = native
            .as_ref()
            .map(|value| {
                Ok(NativeExchangeBinding {
                    registration: value.registration.clone(),
                    client_state_digest: self.digest("native-client-state", &value.client_state)?,
                    pkce_challenge: value.pkce_challenge.clone(),
                })
            })
            .transpose()?;
        if let Some(value) = native.as_mut() {
            value.client_state.zeroize();
        }
        let mut payload = FlowEnvelope {
            return_target,
            nonce: Vec::new(),
            pkce_artifact: Vec::new(),
            one_use_grant: Vec::new(),
            exchange: auth_identity_id.map(|auth_identity_id| ExchangeGrantPayload {
                parent_flow_id,
                koru_identity_id: target_identity_id,
                auth_identity_id,
                authenticated_at_millis,
                native: native_binding,
                verified_candidate,
                profile_id,
                erased_koru_identity_id,
                initiating_session_id,
                expires_at_millis: expires_at,
            }),
        };
        let mut plaintext = serde_json::to_vec(&payload).map_err(|_| SecurityError::Crypto)?;
        let sealed = self
            .crypto
            .seal("one-use-grant-result", &plaintext)
            .map_err(|_| SecurityError::Crypto);
        plaintext.zeroize();
        payload.nonce.zeroize();
        payload.pkce_artifact.zeroize();
        payload.one_use_grant.zeroize();
        let sealed = sealed?;
        let flow = TemporaryAuthFlow {
            id,
            kind,
            status: AuthFlowStatus::Pending,
            state_digest: self.digest("one-use-grant", grant.expose_secret())?,
            sealed_transient: sealed,
            sealed_exchange_result: None,
            source_digest: self.digest("grant-owner", target_identity_id.to_string().as_bytes())?,
            target_identity_id: Some(target_identity_id),
            created_at_millis: now,
            expires_at_millis: expires_at,
            consumed_at_millis: None,
        };
        Ok((flow, OneUseGrant { flow_id: id, grant }))
    }

    pub async fn exchange_grant(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
    ) -> Result<FlowPayload, SecurityError> {
        let (_, payload) = self.inspect_grant(id, grant).await?;
        self.consume_inspected_grant(id).await?;
        Ok(payload)
    }

    pub async fn exchange_grant_for_kind(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        expected_kind: AuthFlowKind,
    ) -> Result<FlowPayload, SecurityError> {
        let (kind, payload) = self.inspect_grant(id, grant).await?;
        if kind != expected_kind {
            return Err(SecurityError::InvalidSecret);
        }
        self.consume_inspected_grant(id).await?;
        Ok(payload)
    }

    pub async fn validate_grant_for_kind(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        expected_kind: AuthFlowKind,
    ) -> Result<(), SecurityError> {
        let (kind, _) = self.inspect_grant(id, grant).await?;
        if kind != expected_kind {
            return Err(SecurityError::InvalidSecret);
        }
        Ok(())
    }

    /// Which flow kinds a caller expecting `expected` will accept.
    ///
    /// A grant is bound to the flow kind that produced it, and the web and native
    /// exchange routes each name the one they serve. Email and Apple sign-in
    /// issue grants of their own kinds through the same two routes, so
    /// `GoogleWeb` also admits `EmailWeb` and `AppleWeb`, and `GoogleNative`
    /// also admits `EmailNative` and `AppleNative` -- the shape of the grant and
    /// everything downstream of it is identical, and only the provider that
    /// produced it differs.
    ///
    /// Deliberately a closed pairing rather than "any web-ish kind": a link or
    /// rebind grant must never be redeemable at the sign-in exchange, which is
    /// what the strict equality was protecting and what this keeps.
    fn kind_admits(expected: AuthFlowKind, actual: AuthFlowKind) -> bool {
        expected == actual
            || matches!(
                (expected, actual),
                (
                    AuthFlowKind::GoogleWeb,
                    AuthFlowKind::EmailWeb | AuthFlowKind::AppleWeb
                ) | (
                    AuthFlowKind::GoogleNative,
                    AuthFlowKind::EmailNative | AuthFlowKind::AppleNative
                )
            )
    }

    pub async fn inspect_exchange_grant_for_kind(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        expected_kind: AuthFlowKind,
    ) -> Result<ExchangeGrantInspection, SecurityError> {
        let (kind, payload, exact_retry) = self.inspect_grant_for_exchange(id, grant).await?;
        if !Self::kind_admits(expected_kind, kind) {
            return Err(SecurityError::InvalidSecret);
        }
        if let Some(sealed) = exact_retry {
            return Ok(ExchangeGrantInspection::ExactRetry(sealed));
        }
        let exchange = payload.exchange.ok_or(SecurityError::InvalidSecret)?;
        Ok(match exact_retry {
            Some(sealed) => ExchangeGrantInspection::ExactRetry(sealed),
            None => ExchangeGrantInspection::Pending(exchange),
        })
    }

    pub async fn inspect_proof_grant_for_kind(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        expected_kind: AuthFlowKind,
    ) -> Result<ExchangeGrantPayload, SecurityError> {
        let found = self
            .flows
            .find_flow(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        if found.kind != expected_kind
            || found.state_digest != self.digest("one-use-grant", grant.expose_secret())?
        {
            return Err(SecurityError::InvalidSecret);
        }
        if found.status != AuthFlowStatus::Consumed
            && self.clock.now_millis() >= found.expires_at_millis
        {
            return Err(SecurityError::Expired);
        }
        if found.status == AuthFlowStatus::Expired {
            return Err(SecurityError::Replayed);
        }
        let mut raw = self
            .crypto
            .open("one-use-grant-result", &found.sealed_transient)
            .map_err(|_| SecurityError::Crypto)?;
        let decoded = serde_json::from_slice::<FlowEnvelope>(&raw);
        raw.zeroize();
        decoded
            .map(FlowPayload::from)
            .map_err(|_| SecurityError::Crypto)?
            .exchange
            .ok_or(SecurityError::InvalidSecret)
    }

    pub fn one_use_grant_digest(&self, grant: &OpaqueSecret) -> Result<KeyedDigest, SecurityError> {
        self.digest("one-use-grant", grant.expose_secret())
    }

    pub fn proof_mutation_digest(&self, binding: &[u8]) -> Result<KeyedDigest, SecurityError> {
        self.digest("proof-mutation-binding", binding)
    }

    pub async fn exchange_grant_for_kind_and_identity(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        expected_kind: AuthFlowKind,
        expected_identity: KoruIdentityId,
    ) -> Result<FlowPayload, SecurityError> {
        let (kind, payload) = self.inspect_grant(id, grant).await?;
        if kind != expected_kind
            || payload
                .exchange
                .as_ref()
                .is_none_or(|exchange| exchange.koru_identity_id != expected_identity)
        {
            return Err(SecurityError::InvalidSecret);
        }
        self.consume_inspected_grant(id).await?;
        Ok(payload)
    }

    async fn inspect_grant(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
    ) -> Result<(AuthFlowKind, FlowPayload), SecurityError> {
        let now = self.clock.now_millis();
        let found = self
            .flows
            .find_flow(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        if found.state_digest != self.digest("one-use-grant", grant.expose_secret())? {
            return Err(SecurityError::InvalidSecret);
        }
        if now >= found.expires_at_millis {
            return Err(SecurityError::Expired);
        }
        let mut raw = self
            .crypto
            .open("one-use-grant-result", &found.sealed_transient)
            .map_err(|_| SecurityError::Crypto)?;
        let decoded = serde_json::from_slice::<FlowEnvelope>(&raw);
        raw.zeroize();
        decoded
            .map(FlowPayload::from)
            .map(|payload| (found.kind, payload))
            .map_err(|_| SecurityError::Crypto)
    }

    async fn inspect_grant_for_exchange(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
    ) -> Result<(AuthFlowKind, FlowPayload, Option<SealedValue>), SecurityError> {
        let now = self.clock.now_millis();
        let found = self
            .flows
            .find_flow(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        if found.state_digest != self.digest("one-use-grant", grant.expose_secret())? {
            return Err(SecurityError::InvalidSecret);
        }
        if now >= found.expires_at_millis {
            return Err(SecurityError::Expired);
        }
        let exact_retry = match found.status {
            AuthFlowStatus::Consumed => Some(
                found
                    .sealed_exchange_result
                    .ok_or(SecurityError::Replayed)?,
            ),
            AuthFlowStatus::Pending => None,
            AuthFlowStatus::Expired => return Err(SecurityError::Replayed),
        };
        let mut raw = self
            .crypto
            .open("one-use-grant-result", &found.sealed_transient)
            .map_err(|_| SecurityError::Crypto)?;
        let decoded = serde_json::from_slice::<FlowEnvelope>(&raw);
        raw.zeroize();
        decoded
            .map(FlowPayload::from)
            .map(|payload| (found.kind, payload, exact_retry))
            .map_err(|_| SecurityError::Crypto)
    }

    async fn consume_inspected_grant(&self, id: AuthFlowId) -> Result<(), SecurityError> {
        self.flows
            .consume_flow(id, self.clock.now_millis())
            .await
            .map(|_| ())
            .map_err(|error| {
                if error == RepositoryError::Conflict {
                    SecurityError::Replayed
                } else {
                    SecurityError::Repository
                }
            })
    }

    pub async fn exchange_native_grant(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        client_state: &[u8],
        pkce_verifier: &[u8],
        source: &[u8],
        expected_parent_flow_id: AuthFlowId,
    ) -> Result<ExchangeGrantPayload, SecurityError> {
        let exchange = self
            .validate_native_grant(
                id,
                grant,
                client_state,
                pkce_verifier,
                source,
                expected_parent_flow_id,
            )
            .await?;
        let ExchangeGrantInspection::Pending(exchange) = exchange else {
            return Err(SecurityError::Replayed);
        };
        self.consume_inspected_grant(id).await?;
        Ok(exchange)
    }

    pub async fn validate_native_grant(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        client_state: &[u8],
        pkce_verifier: &[u8],
        source: &[u8],
        expected_parent_flow_id: AuthFlowId,
    ) -> Result<ExchangeGrantInspection, SecurityError> {
        let inspection = self
            .inspect_exchange_grant_for_kind(id, grant, AuthFlowKind::GoogleNative)
            .await?;
        let (exchange, exact_retry) = match inspection {
            ExchangeGrantInspection::Pending(exchange) => (exchange, None),
            ExchangeGrantInspection::ExactRetry(sealed) => {
                let (_, payload, _) = self.inspect_grant_for_exchange(id, grant).await?;
                (
                    payload.exchange.ok_or(SecurityError::InvalidSecret)?,
                    Some(sealed),
                )
            }
        };
        if exchange.parent_flow_id != Some(expected_parent_flow_id) {
            return Err(SecurityError::InvalidSecret);
        }
        let native = exchange
            .native
            .as_ref()
            .ok_or(SecurityError::InvalidSecret)?;
        if native.client_state_digest != self.digest("native-client-state", client_state)?
            || !valid_pkce_proof(pkce_verifier, &native.pkce_challenge)
        {
            return Err(SecurityError::InvalidSecret);
        }
        self.check_rate(RateLimitSubject::CallbackOrExchange { flow: id, source })
            .await?;
        self.check_rate(RateLimitSubject::DeviceOperation {
            identity: exchange.koru_identity_id,
            device: native.registration.descriptor.device_id,
        })
        .await?;
        Ok(match exact_retry {
            Some(sealed) => ExchangeGrantInspection::ExactRetry(sealed),
            None => ExchangeGrantInspection::Pending(exchange),
        })
    }

    pub async fn exchange_link_grant(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        expected_parent_flow_id: AuthFlowId,
        expected_identity: KoruIdentityId,
        expected_session_id: AccountSessionId,
    ) -> Result<ExchangeGrantPayload, SecurityError> {
        let exchange = self
            .validate_link_grant(
                id,
                grant,
                expected_parent_flow_id,
                expected_identity,
                expected_session_id,
            )
            .await?;
        self.consume_inspected_grant(id).await?;
        Ok(exchange)
    }

    pub async fn validate_link_grant(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        expected_parent_flow_id: AuthFlowId,
        expected_identity: KoruIdentityId,
        expected_session_id: AccountSessionId,
    ) -> Result<ExchangeGrantPayload, SecurityError> {
        let parent = self
            .flows
            .find_flow(expected_parent_flow_id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        if parent.kind != AuthFlowKind::LinkAuthIdentity {
            return Err(SecurityError::InvalidSecret);
        }
        let exchange = self
            .inspect_proof_grant_for_kind(id, grant, AuthFlowKind::LinkAuthIdentity)
            .await?;
        if exchange.parent_flow_id != Some(expected_parent_flow_id)
            || exchange.koru_identity_id != expected_identity
            || exchange.initiating_session_id != Some(expected_session_id)
        {
            return Err(SecurityError::InvalidSecret);
        }
        Ok(exchange)
    }

    pub async fn validate_pending_link_grant(
        &self,
        id: AuthFlowId,
        grant: &OpaqueSecret,
        expected_parent_flow_id: AuthFlowId,
        expected_identity: KoruIdentityId,
        expected_session_id: AccountSessionId,
    ) -> Result<ExchangeGrantPayload, SecurityError> {
        let exchange = self
            .validate_link_grant(
                id,
                grant,
                expected_parent_flow_id,
                expected_identity,
                expected_session_id,
            )
            .await?;
        let flow = self
            .flows
            .find_flow(id)
            .await
            .map_err(|_| SecurityError::Repository)?
            .ok_or(SecurityError::InvalidSecret)?;
        if flow.status != AuthFlowStatus::Pending {
            return Err(SecurityError::Replayed);
        }
        Ok(exchange)
    }
    pub async fn check_rate(
        &self,
        subject: RateLimitSubject<'_>,
    ) -> Result<RateLimitDecision, SecurityError> {
        let result = self.rate_decision(subject).await?;
        if !result.allowed {
            return Err(SecurityError::RateLimited);
        }
        Ok(result)
    }

    /// Preserve denial timing for HTTP callers that communicate a retry delay.
    /// A successful call is not admission: callers must inspect `allowed`.
    pub async fn rate_decision(
        &self,
        subject: RateLimitSubject<'_>,
    ) -> Result<RateLimitDecision, SecurityError> {
        fn field(output: &mut Vec<u8>, value: &[u8]) {
            output.extend_from_slice(&(value.len() as u64).to_be_bytes());
            output.extend_from_slice(value);
        }
        let (policy, purpose, mut raw_subject, owner) = match subject {
            RateLimitSubject::CloudSyncToken {
                identity,
                device,
                workspace,
            } => {
                let mut value = Vec::new();
                field(&mut value, identity.to_string().as_bytes());
                field(&mut value, device.to_string().as_bytes());
                field(&mut value, workspace.to_string().as_bytes());
                (
                    crate::CLOUD_SYNC_TOKEN_RATE,
                    "rate-cloud-sync-token",
                    value,
                    Some(identity),
                )
            }
            RateLimitSubject::Enrollment {
                identity,
                device,
                operation,
            } => {
                let mut value = Vec::new();
                field(&mut value, identity.to_string().as_bytes());
                field(&mut value, device.to_string().as_bytes());
                (
                    crate::ENROLLMENT_RATE,
                    operation.purpose(),
                    value,
                    Some(identity),
                )
            }
            RateLimitSubject::AuthStart { source } => {
                (START_RATE, "rate-auth-start", source.to_vec(), None)
            }
            // Each of these gets its own purpose string, so two budgets over
            // the same bytes land in different buckets rather than sharing one.
            RateLimitSubject::EmailChallengeSendSource { source } => (
                crate::EMAIL_CHALLENGE_SEND_SOURCE_RATE,
                "rate-email-challenge-send-source",
                source.to_vec(),
                None,
            ),
            RateLimitSubject::EmailChallengeSendAddress { address } => (
                crate::EMAIL_CHALLENGE_SEND_ADDRESS_RATE,
                "rate-email-challenge-send-address",
                address.0.clone(),
                None,
            ),
            RateLimitSubject::EmailChallengeVerify { source } => (
                crate::EMAIL_CHALLENGE_VERIFY_RATE,
                "rate-email-challenge-verify",
                source.to_vec(),
                None,
            ),
            RateLimitSubject::EmailPasswordSource { source } => (
                crate::EMAIL_PASSWORD_SOURCE_RATE,
                "rate-email-password-source",
                source.to_vec(),
                None,
            ),
            RateLimitSubject::EmailPasswordAddress { address } => (
                crate::EMAIL_PASSWORD_ADDRESS_RATE,
                "rate-email-password-address",
                address.0.clone(),
                None,
            ),
            RateLimitSubject::AuthenticationAttempt { source } => (
                AUTHENTICATION_ATTEMPT_RATE,
                "rate-authentication-attempt-source",
                source.to_vec(),
                None,
            ),
            RateLimitSubject::CallbackOrExchange { flow, source } => {
                let mut value = Vec::new();
                field(&mut value, flow.to_string().as_bytes());
                field(&mut value, source);
                (CALLBACK_RATE, "rate-callback-exchange", value, None)
            }
            RateLimitSubject::IdentityLink { flow, identity } => {
                if flow.is_none() && identity.is_none() {
                    return Err(SecurityError::InvalidSecret);
                }
                let mut value = Vec::new();
                if let Some(flow) = flow {
                    field(&mut value, flow.to_string().as_bytes());
                }
                if let Some(identity) = identity {
                    field(&mut value, identity.to_string().as_bytes());
                }
                (LINK_RATE, "rate-identity-link", value, identity)
            }
            RateLimitSubject::Refresh { session } => (
                REFRESH_RATE,
                "rate-refresh",
                session.to_string().into_bytes(),
                None,
            ),
            RateLimitSubject::DeviceOperation { identity, device } => {
                let mut value = Vec::new();
                field(&mut value, identity.to_string().as_bytes());
                field(&mut value, device.to_string().as_bytes());
                (DEVICE_RATE, "rate-device-operation", value, Some(identity))
            }
        };
        let digest = self.digest(purpose, &raw_subject);
        raw_subject.zeroize();
        let digest = digest?;
        let result = self
            .limiter
            .check(
                policy.scope,
                digest,
                owner,
                self.clock.now_millis(),
                policy.limit,
                policy.window_millis,
            )
            .await
            .map_err(|_| SecurityError::Repository)?;
        Ok(result)
    }
}

fn valid_pkce_proof(verifier: &[u8], expected_challenge: &str) -> bool {
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .iter()
            .all(|value| value.is_ascii_alphanumeric() || b"-._~".contains(value))
    {
        return false;
    }
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)) == expected_challenge
}
