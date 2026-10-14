// SPDX-License-Identifier: MPL-2.0

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use koru_account_core::workspace_authorization::Region;
use koru_account_core::{
    AccessTokenClaims, AccountSessionId, CryptoError, KoruIdentityId, OpaqueSecret, ProfileId,
    SignedToken, TokenSigner,
};
use koru_device::{DeviceId, DevicePublicKey, ReplicationEndpointId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::str::FromStr as _;

use crate::ACCESS_TOKEN_MILLIS;
use crate::{REGIONAL_CREDENTIAL_VERSION, RegionalCredentialPolicy, regional::CredentialUse};

pub const DEVICE_ATTESTATION_VERSION: u32 = 1;
pub const DEVICE_ATTESTATION_AUDIENCE: &str = "koru-device-enrollment-v1";
pub const DEVICE_ATTESTATION_MAX_MILLIS: i64 = 5 * 60 * 1_000;
/// Allow modest device clock drift at issuance, never beyond signed expiry.
const DEVICE_ATTESTATION_ISSUED_SKEW_MILLIS: i64 = 30_000;
const DEVICE_ATTESTATION_TYPE: &str = "koru-device-attestation+jwt";
const DEVICE_ATTESTATION_PURPOSE: &str = "koru:device-attestation:v1";
const MAX_ATTESTED_PROFILES: usize = 128;

#[derive(Clone)]
pub struct Ed25519KeyRing {
    active: String,
    keys: BTreeMap<String, SigningKey>,
    verification_only: BTreeMap<String, VerifyingKey>,
    regional: Option<RegionalCredentialPolicy>,
}

/// A separately constructed signing ring for one-purpose device attestations.
/// It intentionally has no access-token signing implementation, so callers
/// cannot accidentally issue one signed type with the other's configured key.
#[derive(Clone)]
pub struct DeviceAttestationKeyRing(Ed25519KeyRing);

impl DeviceAttestationKeyRing {
    pub fn with_regional_policy(
        mut self,
        policy: RegionalCredentialPolicy,
    ) -> Result<Self, CryptoError> {
        policy.validate()?;
        policy.issuance_region(
            policy.issuance_issuer(),
            policy.legacy_cutover_millis,
            &self.0.active,
            CredentialUse::Attestation,
        )?;
        self.0.regional = Some(policy);
        Ok(self)
    }

    pub fn with_verification_keys(
        mut self,
        keys: impl IntoIterator<Item = (String, VerifyingKey)>,
    ) -> Result<Self, CryptoError> {
        self.0 = self.0.with_verification_keys(keys)?;
        Ok(self)
    }

    pub fn regional_policy(&self) -> Option<&RegionalCredentialPolicy> {
        self.0.regional.as_ref()
    }
    pub fn new(
        active: String,
        keys: impl IntoIterator<Item = (String, SigningKey)>,
    ) -> Result<Self, CryptoError> {
        Ed25519KeyRing::new(active, keys).map(Self)
    }

    pub fn verification_keys(&self) -> BTreeMap<String, VerifyingKey> {
        self.0.verification_keys()
    }

    pub fn public_jwks(&self) -> Result<String, CryptoError> {
        self.0.public_jwks()
    }

    pub fn sign(
        &self,
        claims: &DeviceAttestationClaimsV1,
    ) -> Result<SignedDeviceAttestationV1, CryptoError> {
        validate_attestation_claims_for_signing(claims)?;
        let key = self
            .0
            .keys
            .get(&self.0.active)
            .ok_or(CryptoError::KeyUnavailable)?;
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&SigningHeader {
                alg: "EdDSA",
                typ: DEVICE_ATTESTATION_TYPE,
                kid: &self.0.active,
            })
            .map_err(|_| CryptoError::InvalidInput)?,
        );
        let mut wire = AttestationClaims::from(claims);
        if let Some(policy) = &self.0.regional {
            wire.region = policy.issuance_region(
                &claims.issuer,
                claims.issued_at_millis,
                &self.0.active,
                CredentialUse::Attestation,
            )?;
            if wire.region.is_some() {
                wire.ver = REGIONAL_CREDENTIAL_VERSION;
            }
        }
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&wire).map_err(|_| CryptoError::InvalidInput)?);
        let input = format!("{header}.{payload}");
        let signature = URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes());
        Ok(SignedDeviceAttestationV1 {
            compact: format!("{input}.{signature}"),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAttestationClaimsV1 {
    pub version: u32,
    pub issuer: String,
    pub audience: String,
    pub koru_identity_id: KoruIdentityId,
    pub device_id: DeviceId,
    pub signing_public_key: DevicePublicKey,
    pub endpoint_id: ReplicationEndpointId,
    pub authorized_profile_ids: Vec<ProfileId>,
    pub session_id: AccountSessionId,
    pub token_id: String,
    pub issued_at_millis: i64,
    pub expires_at_millis: i64,
}

impl std::fmt::Debug for DeviceAttestationClaimsV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceAttestationClaimsV1")
            .field("version", &self.version)
            .field("audience", &self.audience)
            .field("profile_count", &self.authorized_profile_ids.len())
            .field("issued_at_millis", &self.issued_at_millis)
            .field("expires_at_millis", &self.expires_at_millis)
            .field("claim_material", &"<redacted>")
            .finish()
    }
}

/// Device attestation evidence is deliberately nonsecret. It carries only
/// public ids and keys, and may be relayed to the peer that validates it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDeviceAttestationV1 {
    pub compact: String,
}

impl std::fmt::Debug for SignedDeviceAttestationV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SignedDeviceAttestationV1(<signed-evidence>)")
    }
}

impl SignedDeviceAttestationV1 {
    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.compact.as_bytes()).into()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedDeviceAttestationV1 {
    claims: DeviceAttestationClaimsV1,
    digest: [u8; 32],
}

impl std::fmt::Debug for VerifiedDeviceAttestationV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VerifiedDeviceAttestationV1(<verified-evidence>)")
    }
}

/// Cryptographically valid attestation material whose current lifetime has
/// not been admitted. It cannot be passed to pairing authorization APIs.
#[derive(Clone, PartialEq, Eq)]
pub struct InspectedDeviceAttestationV1 {
    claims: DeviceAttestationClaimsV1,
    digest: [u8; 32],
}

impl std::fmt::Debug for InspectedDeviceAttestationV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InspectedDeviceAttestationV1(<inspected-evidence>)")
    }
}

impl InspectedDeviceAttestationV1 {
    pub fn claims(&self) -> &DeviceAttestationClaimsV1 {
        &self.claims
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
}

impl VerifiedDeviceAttestationV1 {
    pub fn claims(&self) -> &DeviceAttestationClaimsV1 {
        &self.claims
    }

    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// Recheck cached evidence with the same time policy as initial verification.
    pub fn is_current_at(&self, now_millis: i64) -> bool {
        attestation_issued_by(self.claims.issued_at_millis, now_millis)
            && now_millis < self.claims.expires_at_millis
    }
}

fn attestation_issued_by(issued_at_millis: i64, now_millis: i64) -> bool {
    issued_at_millis <= now_millis.saturating_add(DEVICE_ATTESTATION_ISSUED_SKEW_MILLIS)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttestationClaims {
    ver: u32,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    region: Option<Region>,
    purpose: String,
    iss: String,
    aud: String,
    sub: String,
    did: String,
    dpk: String,
    eid: String,
    profiles: Vec<String>,
    sid: String,
    jti: String,
    iat: i64,
    exp: i64,
}

impl From<&DeviceAttestationClaimsV1> for AttestationClaims {
    fn from(value: &DeviceAttestationClaimsV1) -> Self {
        Self {
            ver: value.version,
            region: None,
            purpose: DEVICE_ATTESTATION_PURPOSE.to_owned(),
            iss: value.issuer.clone(),
            aud: value.audience.clone(),
            sub: value.koru_identity_id.to_string(),
            did: value.device_id.to_storage(),
            dpk: value.signing_public_key.to_storage(),
            eid: value.endpoint_id.to_storage(),
            profiles: {
                let mut profiles = value
                    .authorized_profile_ids
                    .iter()
                    .map(ProfileId::to_storage)
                    .collect::<Vec<_>>();
                profiles.sort();
                profiles
            },
            sid: value.session_id.to_string(),
            jti: value.token_id.clone(),
            iat: value.issued_at_millis / 1_000,
            exp: value.expires_at_millis / 1_000,
        }
    }
}
impl Ed25519KeyRing {
    pub fn new(
        active: String,
        keys: impl IntoIterator<Item = (String, SigningKey)>,
    ) -> Result<Self, CryptoError> {
        let mut collected = BTreeMap::new();
        for (id, key) in keys {
            if !valid_identifier(&id) || collected.insert(id, key).is_some() {
                return Err(CryptoError::InvalidInput);
            }
        }
        if !valid_identifier(&active) || !collected.contains_key(&active) {
            return Err(CryptoError::KeyUnavailable);
        }
        Ok(Self {
            active,
            keys: collected,
            verification_only: BTreeMap::new(),
            regional: None,
        })
    }
    pub fn with_regional_policy(
        mut self,
        policy: RegionalCredentialPolicy,
    ) -> Result<Self, CryptoError> {
        policy.validate()?;
        policy.issuance_region(
            policy.issuance_issuer(),
            policy.legacy_cutover_millis,
            &self.active,
            CredentialUse::Access,
        )?;
        self.regional = Some(policy);
        Ok(self)
    }

    /// Retired public keys can verify receipts but can never be selected to sign.
    pub fn with_verification_keys(
        mut self,
        keys: impl IntoIterator<Item = (String, VerifyingKey)>,
    ) -> Result<Self, CryptoError> {
        for (kid, key) in keys {
            if !valid_identifier(&kid)
                || self.keys.contains_key(&kid)
                || self.verification_only.insert(kid, key).is_some()
            {
                return Err(CryptoError::InvalidInput);
            }
        }
        Ok(self)
    }

    pub fn regional_policy(&self) -> Option<&RegionalCredentialPolicy> {
        self.regional.as_ref()
    }
    pub fn verification_keys(&self) -> BTreeMap<String, VerifyingKey> {
        self.keys
            .iter()
            .map(|(id, k)| (id.clone(), k.verifying_key()))
            .chain(
                self.verification_only
                    .iter()
                    .map(|(id, key)| (id.clone(), *key)),
            )
            .collect()
    }
}
#[derive(Serialize)]
struct SigningHeader<'a> {
    alg: &'static str,
    typ: &'static str,
    kid: &'a str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerificationHeader {
    alg: String,
    typ: String,
    kid: String,
}

pub(crate) struct VerifiedCompact {
    pub kid: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy)]
pub(crate) enum CompactVerificationError {
    Malformed,
    WrongType,
    InvalidHeader,
    UnknownKey,
    Signature,
}

impl From<CompactVerificationError> for JwtVerificationError {
    fn from(error: CompactVerificationError) -> Self {
        match error {
            CompactVerificationError::Malformed => Self::Malformed,
            CompactVerificationError::WrongType | CompactVerificationError::InvalidHeader => {
                Self::Claims
            }
            CompactVerificationError::UnknownKey => Self::UnknownKey,
            CompactVerificationError::Signature => Self::Signature,
        }
    }
}

/// One bounded parser for the existing signed credential families. Payload
/// bytes are returned only after verification with the explicitly supplied ring.
pub(crate) fn verify_compact(
    token: &[u8],
    keys: &BTreeMap<String, VerifyingKey>,
    expected_type: &str,
    max_payload: usize,
) -> Result<VerifiedCompact, CompactVerificationError> {
    use CompactVerificationError as Error;
    if token.len() > max_payload.saturating_add(2 * 1024) {
        return Err(Error::Malformed);
    }
    let token = std::str::from_utf8(token).map_err(|_| Error::Malformed)?;
    let mut segments = token.split('.');
    let header_segment = segments.next().ok_or(Error::Malformed)?;
    let payload_segment = segments.next().ok_or(Error::Malformed)?;
    let signature_segment = segments.next().ok_or(Error::Malformed)?;
    if segments.next().is_some()
        || header_segment.len() > 1024
        || payload_segment.len() > max_payload
        || signature_segment.len() > 128
    {
        return Err(Error::Malformed);
    }
    let header: VerificationHeader = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(header_segment)
            .map_err(|_| Error::Malformed)?,
    )
    .map_err(|_| Error::Malformed)?;
    if header.alg != "EdDSA" || !valid_identifier(&header.kid) {
        return Err(Error::InvalidHeader);
    }
    if header.typ != expected_type {
        return Err(Error::WrongType);
    }
    let key = keys.get(&header.kid).ok_or(Error::UnknownKey)?;
    let signature: [u8; 64] = URL_SAFE_NO_PAD
        .decode(signature_segment)
        .map_err(|_| Error::Malformed)?
        .try_into()
        .map_err(|_| Error::Malformed)?;
    key.verify(
        format!("{header_segment}.{payload_segment}").as_bytes(),
        &Signature::from_bytes(&signature),
    )
    .map_err(|_| Error::Signature)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload_segment)
        .map_err(|_| Error::Malformed)?;
    Ok(VerifiedCompact {
        kid: header.kid,
        payload,
    })
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    ver: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    region: Option<Region>,
    iss: String,
    aud: String,
    sub: String,
    sid: String,
    jti: String,
    iat: i64,
    exp: i64,
}
impl TokenSigner for Ed25519KeyRing {
    fn sign_access_token(&self, v: &AccessTokenClaims) -> Result<SignedToken, CryptoError> {
        if !valid_claim(&v.issuer, 512)
            || !valid_claim(&v.audience, 256)
            || !valid_claim(&v.token_id, 256)
            || v.expires_at_millis.checked_sub(v.issued_at_millis) != Some(ACCESS_TOKEN_MILLIS)
        {
            return Err(CryptoError::InvalidInput);
        }
        let region = self
            .regional
            .as_ref()
            .map(|policy| {
                policy.issuance_region(
                    &v.issuer,
                    v.issued_at_millis,
                    &self.active,
                    CredentialUse::Access,
                )
            })
            .transpose()?
            .flatten();
        let c = Claims {
            ver: region.map(|_| REGIONAL_CREDENTIAL_VERSION),
            region,
            iss: v.issuer.clone(),
            aud: v.audience.clone(),
            sub: v.subject.to_string(),
            sid: v.session_id.to_string(),
            jti: v.token_id.clone(),
            iat: v.issued_at_millis / 1000,
            exp: v.expires_at_millis / 1000,
        };
        self.sign_compact("JWT", &c)
    }
    fn public_jwks(&self) -> Result<String, CryptoError> {
        let keys:Vec<_>=self.verification_keys().iter().map(|(kid,k)|serde_json::json!({"kty":"OKP","crv":"Ed25519","use":"sig","alg":"EdDSA","kid":kid,"x":URL_SAFE_NO_PAD.encode(k.as_bytes())})).collect();
        serde_json::to_string(&serde_json::json!({"keys":keys}))
            .map_err(|_| CryptoError::InvalidInput)
    }
}

impl Ed25519KeyRing {
    /// Encoding mechanism only. Purpose-specific callers validate their complete
    /// claims before this boundary; no public arbitrary-purpose signer exists.
    pub(crate) fn sign_compact(
        &self,
        typ: &'static str,
        claims: &impl Serialize,
    ) -> Result<SignedToken, CryptoError> {
        let key = self
            .keys
            .get(&self.active)
            .ok_or(CryptoError::KeyUnavailable)?;
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&SigningHeader {
                alg: "EdDSA",
                typ,
                kid: &self.active,
            })
            .map_err(|_| CryptoError::InvalidInput)?,
        );
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(claims).map_err(|_| CryptoError::InvalidInput)?);
        let input = format!("{header}.{payload}");
        let signature = URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes());
        Ok(SignedToken {
            key_id: self.active.clone(),
            compact: OpaqueSecret(format!("{input}.{signature}").into_bytes()),
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JwtVerificationError {
    Malformed,
    UnknownKey,
    Signature,
    Claims,
}
pub fn verify_access_token(
    token: &[u8],
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    audience: &str,
    now: i64,
) -> Result<AccessTokenClaims, JwtVerificationError> {
    verify_access_token_with_policy(token, keys, issuer, audience, now, None)
}

pub fn verify_access_token_with_policy(
    token: &[u8],
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    audience: &str,
    now: i64,
    policy: Option<&RegionalCredentialPolicy>,
) -> Result<AccessTokenClaims, JwtVerificationError> {
    verify_access_token_detailed(token, keys, issuer, audience, now, policy, None).map_err(
        |error| match error {
            crate::TokenCapabilityVerificationError::Malformed => JwtVerificationError::Malformed,
            crate::TokenCapabilityVerificationError::UnknownKey => JwtVerificationError::UnknownKey,
            crate::TokenCapabilityVerificationError::Signature => JwtVerificationError::Signature,
            _ => JwtVerificationError::Claims,
        },
    )
}

/// Same native-access verification mechanism and rollout policy, with detailed
/// purpose-isolated observations for the Cloud issuer. Ordinary routes retain
/// their prior erased public mapping through the wrapper above.
pub fn verify_access_token_for_sync_issuance(
    token: &[u8],
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    audience: &str,
    now: i64,
    policy: Option<&RegionalCredentialPolicy>,
) -> Result<AccessTokenClaims, crate::TokenCapabilityVerificationError> {
    verify_access_token_detailed(token, keys, issuer, audience, now, policy, Some(Region::Us))
}

fn verify_access_token_detailed(
    token: &[u8],
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    audience: &str,
    now: i64,
    policy: Option<&RegionalCredentialPolicy>,
    issuer_region: Option<Region>,
) -> Result<AccessTokenClaims, crate::TokenCapabilityVerificationError> {
    use crate::TokenCapabilityVerificationError as Error;
    let verified = verify_compact(token, keys, "JWT", 10 * 1024)?;
    let c: Claims = serde_json::from_slice(&verified.payload).map_err(|_| Error::Malformed)?;
    let issued_at_millis = c.iat.checked_mul(1000).ok_or(Error::InvalidClaims)?;
    let expires_at_millis = c.exp.checked_mul(1000).ok_or(Error::InvalidClaims)?;
    let lifetime_seconds = c.exp.checked_sub(c.iat).ok_or(Error::InvalidClaims)?;
    if !valid_claim(&c.iss, 512)
        || !valid_claim(&c.aud, 256)
        || !valid_claim(&c.jti, 256)
        || !matches!(
            (c.ver, c.region),
            (None, None) | (Some(REGIONAL_CREDENTIAL_VERSION), Some(_))
        )
    {
        return Err(Error::InvalidClaims);
    }
    if c.aud != audience {
        return Err(Error::WrongAudience);
    }
    if issuer_region.is_some_and(|expected| c.region.is_some_and(|region| region != expected)) {
        return Err(Error::WrongRegion);
    }
    if !credential_issuer_admitted(
        policy,
        issuer,
        &c.iss,
        c.region,
        issued_at_millis,
        now,
        &verified.kid,
        CredentialUse::Access,
    ) {
        return Err(Error::WrongIssuer);
    }
    if expires_at_millis <= issued_at_millis || lifetime_seconds != ACCESS_TOKEN_MILLIS / 1000 {
        return Err(Error::InvalidClaims);
    }
    if expires_at_millis <= now {
        return Err(Error::CredentialExpired);
    }
    if issued_at_millis > now {
        return Err(Error::CredentialNotYetValid);
    }
    Ok(AccessTokenClaims {
        issuer: c.iss,
        audience: c.aud,
        subject: c.sub.parse().map_err(|_| Error::InvalidClaims)?,
        session_id: c.sid.parse().map_err(|_| Error::InvalidClaims)?,
        token_id: c.jti,
        issued_at_millis,
        expires_at_millis,
    })
}

pub fn verify_device_attestation(
    attestation: &SignedDeviceAttestationV1,
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    now_millis: i64,
) -> Result<VerifiedDeviceAttestationV1, JwtVerificationError> {
    let inspected = verify_device_attestation_inner(attestation, keys, issuer, now_millis, false)?;
    Ok(VerifiedDeviceAttestationV1 {
        claims: inspected.claims,
        digest: inspected.digest,
    })
}

/// Verifies every signed claim and bounded lifetime shape while allowing a
/// previously completed provisioning mutation to prove an exact retry after
/// expiry. Callers must still let their atomic repository distinguish an
/// exact completed retry from a pending/changed request.
pub fn inspect_device_attestation(
    attestation: &SignedDeviceAttestationV1,
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    now_millis: i64,
) -> Result<InspectedDeviceAttestationV1, JwtVerificationError> {
    verify_device_attestation_inner(attestation, keys, issuer, now_millis, true)
}

fn verify_device_attestation_inner(
    attestation: &SignedDeviceAttestationV1,
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    now_millis: i64,
    allow_expired: bool,
) -> Result<InspectedDeviceAttestationV1, JwtVerificationError> {
    verify_device_attestation_with_policy_inner(
        attestation,
        keys,
        issuer,
        now_millis,
        allow_expired,
        None,
    )
}

pub fn verify_device_attestation_with_policy(
    attestation: &SignedDeviceAttestationV1,
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    now_millis: i64,
    policy: Option<&RegionalCredentialPolicy>,
) -> Result<VerifiedDeviceAttestationV1, JwtVerificationError> {
    let inspected = verify_device_attestation_with_policy_inner(
        attestation,
        keys,
        issuer,
        now_millis,
        false,
        policy,
    )?;
    Ok(VerifiedDeviceAttestationV1 {
        claims: inspected.claims,
        digest: inspected.digest,
    })
}

/// Expired legacy evidence remains inspection-only; the caller must compare the
/// authenticated original session and complete persisted exact receipt.
pub fn inspect_device_attestation_with_policy(
    attestation: &SignedDeviceAttestationV1,
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    now_millis: i64,
    policy: Option<&RegionalCredentialPolicy>,
) -> Result<InspectedDeviceAttestationV1, JwtVerificationError> {
    verify_device_attestation_with_policy_inner(attestation, keys, issuer, now_millis, true, policy)
}

fn verify_device_attestation_with_policy_inner(
    attestation: &SignedDeviceAttestationV1,
    keys: &BTreeMap<String, VerifyingKey>,
    issuer: &str,
    now_millis: i64,
    allow_expired: bool,
    policy: Option<&RegionalCredentialPolicy>,
) -> Result<InspectedDeviceAttestationV1, JwtVerificationError> {
    let verified = verify_compact(
        attestation.compact.as_bytes(),
        keys,
        DEVICE_ATTESTATION_TYPE,
        22 * 1024,
    )?;
    let claims: AttestationClaims =
        serde_json::from_slice(&verified.payload).map_err(|_| JwtVerificationError::Malformed)?;
    let issued_at_millis = claims
        .iat
        .checked_mul(1_000)
        .ok_or(JwtVerificationError::Claims)?;
    let expires_at_millis = claims
        .exp
        .checked_mul(1_000)
        .ok_or(JwtVerificationError::Claims)?;
    let lifetime = expires_at_millis
        .checked_sub(issued_at_millis)
        .ok_or(JwtVerificationError::Claims)?;
    if !matches!(
        (claims.ver, claims.region),
        (DEVICE_ATTESTATION_VERSION, None) | (REGIONAL_CREDENTIAL_VERSION, Some(_))
    ) || claims.purpose != DEVICE_ATTESTATION_PURPOSE
        || !credential_issuer_admitted(
            policy,
            issuer,
            &claims.iss,
            claims.region,
            issued_at_millis,
            now_millis,
            &verified.kid,
            if allow_expired {
                CredentialUse::ReceiptInspection
            } else {
                CredentialUse::Attestation
            },
        )
        || claims.aud != DEVICE_ATTESTATION_AUDIENCE
        || !valid_claim(&claims.iss, 512)
        || !valid_claim(&claims.jti, 256)
        || !attestation_issued_by(issued_at_millis, now_millis)
        || (!allow_expired && expires_at_millis <= now_millis)
        || lifetime <= 0
        || lifetime > DEVICE_ATTESTATION_MAX_MILLIS
        || claims.profiles.is_empty()
        || claims.profiles.len() > MAX_ATTESTED_PROFILES
    {
        return Err(JwtVerificationError::Claims);
    }
    let mut authorized_profile_ids = Vec::with_capacity(claims.profiles.len());
    for profile in claims.profiles {
        let parsed = ProfileId::parse(&profile).ok_or(JwtVerificationError::Claims)?;
        if parsed.to_storage() != profile {
            return Err(JwtVerificationError::Claims);
        }
        if authorized_profile_ids.contains(&parsed) {
            return Err(JwtVerificationError::Claims);
        }
        authorized_profile_ids.push(parsed);
    }
    if !authorized_profile_ids
        .windows(2)
        .all(|pair| pair[0] < pair[1])
    {
        return Err(JwtVerificationError::Claims);
    }
    let verified_claims = DeviceAttestationClaimsV1 {
        version: claims.ver,
        issuer: claims.iss,
        audience: claims.aud,
        koru_identity_id: KoruIdentityId::from_str(&claims.sub)
            .map_err(|_| JwtVerificationError::Claims)?,
        device_id: DeviceId::parse(&claims.did).ok_or(JwtVerificationError::Claims)?,
        signing_public_key: DevicePublicKey::parse(&claims.dpk)
            .ok_or(JwtVerificationError::Claims)?,
        endpoint_id: ReplicationEndpointId::parse(&claims.eid)
            .ok_or(JwtVerificationError::Claims)?,
        authorized_profile_ids,
        session_id: AccountSessionId::from_str(&claims.sid)
            .map_err(|_| JwtVerificationError::Claims)?,
        token_id: claims.jti,
        issued_at_millis,
        expires_at_millis,
    };
    Ok(InspectedDeviceAttestationV1 {
        digest: attestation.digest(),
        claims: verified_claims,
    })
}

#[allow(clippy::too_many_arguments)]
fn credential_issuer_admitted(
    policy: Option<&RegionalCredentialPolicy>,
    configured_issuer: &str,
    signed_issuer: &str,
    region: Option<Region>,
    issued_at: i64,
    now: i64,
    kid: &str,
    usage: CredentialUse,
) -> bool {
    match policy {
        Some(policy) => {
            policy.validate().is_ok()
                && configured_issuer == policy.issuance_issuer()
                && policy.admits(signed_issuer, region, issued_at, now, kid, usage)
        }
        None => region.is_none() && signed_issuer == configured_issuer,
    }
}

#[cfg(test)]
mod tests;

fn validate_attestation_claims_for_signing(
    claims: &DeviceAttestationClaimsV1,
) -> Result<(), CryptoError> {
    let lifetime = claims
        .expires_at_millis
        .checked_sub(claims.issued_at_millis)
        .ok_or(CryptoError::InvalidInput)?;
    if claims.version != DEVICE_ATTESTATION_VERSION
        || claims.audience != DEVICE_ATTESTATION_AUDIENCE
        || !valid_claim(&claims.issuer, 512)
        || !valid_claim(&claims.token_id, 256)
        || claims.issued_at_millis % 1_000 != 0
        || claims.expires_at_millis % 1_000 != 0
        || lifetime <= 0
        || lifetime > DEVICE_ATTESTATION_MAX_MILLIS
        || claims.authorized_profile_ids.is_empty()
        || claims.authorized_profile_ids.len() > MAX_ATTESTED_PROFILES
    {
        return Err(CryptoError::InvalidInput);
    }
    let mut profiles = claims.authorized_profile_ids.clone();
    profiles.sort();
    profiles.dedup();
    if profiles.len() != claims.authorized_profile_ids.len() {
        return Err(CryptoError::InvalidInput);
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

// An absent legacy field is allowed; an explicitly null signed field is not.
fn present_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn valid_claim(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum
}
