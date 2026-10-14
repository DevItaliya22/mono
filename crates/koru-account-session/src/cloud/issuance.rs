// SPDX-License-Identifier: MPL-2.0

//! Plan 13 signing and expiry primitives, deliberately without hosted authority.

use koru_account_core::{CryptoError, SignedToken, workspace_authorization::Region};

use super::SyncCredentialClaimsV1;

pub const SYNC_CREDENTIAL_DEFAULT_SECONDS: i64 = 600;

/// Backdate a fresh credential's `iat` by this much.
///
/// The credential is minted on the issuer's clock and checked on the device's,
/// and `verify_time` allows the verifier **no** tolerance: a device whose clock
/// is a single millisecond behind the issuer refuses a perfectly good
/// credential as `credential_not_yet_valid`, on every dial, for as long as the
/// drift lasts. Truncating to whole seconds hides this about half the time,
/// which is worse than never hiding it.
///
/// The tolerance belongs here rather than in the verifier: the verifier's
/// exact behaviour is a frozen vector of `cloud-sync-admission-v1`, and a
/// credential that a device refuses is the issuer's problem to avoid.
/// `expires_at` moves with `issued_at`, so this spends 30 seconds of a
/// ten-minute credential and grants nothing beyond a signed expiry — the same
/// trade `DEVICE_ATTESTATION_ISSUED_SKEW_MILLIS` already makes for enrollment.
pub const SYNC_CREDENTIAL_ISSUED_BACKDATE_SECONDS: i64 = 30;

/// The `iat`, in whole seconds, to sign for a credential minted at `now_millis`.
/// Never negative: a zero-ish clock must not produce an unsignable credential.
pub fn sync_credential_issued_at(now_millis: i64) -> i64 {
    (now_millis / 1000)
        .saturating_sub(SYNC_CREDENTIAL_ISSUED_BACKDATE_SECONDS)
        .max(0)
}

/// MS2 supplies a finite boundary; this rung passes Unbounded explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntitlementBoundary {
    Unbounded,
    UntilSeconds(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncExpiryError {
    InvalidTime,
    InvalidDuration,
    NoRemainingLifetime,
}

/// Whole-second arithmetic, separate from the verifier's millis representation.
/// No wire denial or entitlement enforcement policy is introduced by this seam.
pub fn sync_credential_expiry(
    issued_at_seconds: i64,
    entitlement: EntitlementBoundary,
) -> Result<i64, SyncExpiryError> {
    expiry_with_duration(
        issued_at_seconds,
        SYNC_CREDENTIAL_DEFAULT_SECONDS,
        entitlement,
    )
}

fn expiry_with_duration(
    issued_at_seconds: i64,
    duration: i64,
    entitlement: EntitlementBoundary,
) -> Result<i64, SyncExpiryError> {
    if issued_at_seconds < 0 {
        return Err(SyncExpiryError::InvalidTime);
    }
    if !(1..=SYNC_CREDENTIAL_DEFAULT_SECONDS).contains(&duration) {
        return Err(SyncExpiryError::InvalidDuration);
    }
    let ordinary = issued_at_seconds
        .checked_add(duration)
        .ok_or(SyncExpiryError::InvalidTime)?;
    let expiry = match entitlement {
        EntitlementBoundary::Unbounded => ordinary,
        EntitlementBoundary::UntilSeconds(until) if until >= 0 => ordinary.min(until),
        EntitlementBoundary::UntilSeconds(_) => return Err(SyncExpiryError::InvalidTime),
    };
    if expiry <= issued_at_seconds {
        return Err(SyncExpiryError::NoRemainingLifetime);
    }
    Ok(expiry)
}

/// Purpose-specific signed result. Debug never exposes the bearer material.
pub struct SignedSyncCredential {
    token: SignedToken,
    expires_at_seconds: i64,
}

impl SignedSyncCredential {
    pub(crate) fn new(token: SignedToken, expires_at_seconds: i64) -> Self {
        Self {
            token,
            expires_at_seconds,
        }
    }
    pub fn compact(&self) -> &[u8] {
        self.token.compact.expose_secret()
    }
    pub fn key_id(&self) -> &str {
        &self.token.key_id
    }
    pub fn expires_at_seconds(&self) -> i64 {
        self.expires_at_seconds
    }
}

impl std::fmt::Debug for SignedSyncCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SignedSyncCredential(<redacted>)")
    }
}

pub(crate) fn validate_signing_claims(
    region: Region,
    issuer: &str,
    claims: &SyncCredentialClaimsV1,
) -> Result<(), CryptoError> {
    let lifetime = claims
        .exp
        .checked_sub(claims.iat)
        .ok_or(CryptoError::InvalidInput)?;
    if region != Region::Us
        || claims.region != region
        || issuer.is_empty()
        || issuer.len() > 512
        || issuer.chars().any(|c| c.is_whitespace() || c.is_control())
        || claims.iss != issuer
        || claims.ver != 1
        || claims.purpose != "koru:sync:device:v1"
        || claims.aud != "koru-sync"
        || claims.iat < 0
        || !(1..=SYNC_CREDENTIAL_DEFAULT_SECONDS).contains(&lifetime)
        || claims.iat.checked_mul(1_000).is_none()
        || claims.exp.checked_mul(1_000).is_none()
    {
        return Err(CryptoError::InvalidInput);
    }
    // Claims can be constructed directly in Rust. Apply the existing strict
    // endpoint/epoch decoder before signing too; do not tighten shared CloudId.
    let bytes = serde_json::to_vec(claims).map_err(|_| CryptoError::InvalidInput)?;
    serde_json::from_slice::<SyncCredentialClaimsV1>(&bytes)
        .map_err(|_| CryptoError::InvalidInput)?;
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod issued_at_tests {
    use super::{
        EntitlementBoundary, SYNC_CREDENTIAL_ISSUED_BACKDATE_SECONDS, sync_credential_expiry,
        sync_credential_issued_at,
    };

    /// The device is the verifier and its clock is its own. A credential minted
    /// on an issuer running ahead must still be valid on a device running
    /// behind, because `verify_time` gives the verifier no tolerance at all.
    #[test]
    fn a_device_running_behind_the_issuer_still_accepts_a_fresh_credential() {
        let issuer_now = 1_800_000_000_000;
        let iat = sync_credential_issued_at(issuer_now);
        for behind_millis in [
            1,
            999,
            29_000,
            SYNC_CREDENTIAL_ISSUED_BACKDATE_SECONDS * 1000,
        ] {
            let device_now = issuer_now - behind_millis;
            assert!(
                iat * 1000 <= device_now,
                "a device {behind_millis} ms behind refuses a credential minted now"
            );
        }
    }

    /// Backdating spends lifetime, it never adds any: the signed expiry moves
    /// with the signed issue time, so the credential is not valid for longer.
    #[test]
    fn backdating_shortens_the_window_and_never_extends_it() {
        let issuer_now = 1_800_000_000_000;
        let iat = sync_credential_issued_at(issuer_now);
        let exp = sync_credential_expiry(iat, EntitlementBoundary::Unbounded).unwrap();
        let untouched =
            sync_credential_expiry(issuer_now / 1000, EntitlementBoundary::Unbounded).unwrap();
        assert_eq!(untouched - exp, SYNC_CREDENTIAL_ISSUED_BACKDATE_SECONDS);
        assert!(exp * 1000 < issuer_now + super::SYNC_CREDENTIAL_DEFAULT_SECONDS * 1000);
    }

    /// A clock at the epoch must not produce a negative issue time, which
    /// `expiry_with_duration` refuses outright.
    #[test]
    fn a_clock_at_the_epoch_still_issues() {
        assert_eq!(sync_credential_issued_at(0), 0);
        assert_eq!(sync_credential_issued_at(1_000), 0);
        assert!(
            sync_credential_expiry(sync_credential_issued_at(0), EntitlementBoundary::Unbounded)
                .is_ok()
        );
    }
}
