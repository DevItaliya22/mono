// SPDX-License-Identifier: MPL-2.0

//! Explicit, bounded bridge between legacy and regional signed credentials.
//! This policy contains no secrets and never infers residency from a request.
use std::collections::BTreeSet;

use koru_account_core::{CryptoError, workspace_authorization::Region};
use serde::{Deserialize, Serialize};

use crate::{ACCESS_TOKEN_MILLIS, DEVICE_ATTESTATION_MAX_MILLIS, REFRESH_ABSOLUTE_MILLIS};

pub const REGIONAL_CREDENTIAL_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialIssuance {
    Legacy,
    Regional,
}

/// Trusted deployment/build configuration, never accepted from a credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionalCredentialPolicy {
    pub region: Region,
    pub regional_issuer: String,
    pub legacy_issuer: String,
    pub legacy_cutover_millis: i64,
    pub issuance: CredentialIssuance,
    pub legacy_access_kids: BTreeSet<String>,
    pub legacy_attestation_kids: BTreeSet<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum CredentialUse {
    Access,
    Attestation,
    ReceiptInspection,
}

impl RegionalCredentialPolicy {
    pub fn parse(value: &str) -> Result<Self, CryptoError> {
        if value.len() > 8 * 1024 {
            return Err(CryptoError::InvalidInput);
        }
        let policy: Self = serde_json::from_str(value).map_err(|_| CryptoError::InvalidInput)?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), CryptoError> {
        let valid_issuer = |value: &str| {
            !value.is_empty()
                && value.len() <= 512
                && !value.chars().any(|c| c.is_control() || c.is_whitespace())
        };
        // A policy that still issues under the legacy issuer promises that a
        // credential minted now will be admitted, and `admits` grants that only
        // while `issued_at <= legacy_cutover_millis`. A zero cutover therefore
        // admits nothing it issues: every attestation and access token is born
        // already refused, on every device carrying the policy, forever. That
        // surfaces far from here — enrollment reports `attestation_invalid`
        // against a hosted attestation the server signed correctly seconds
        // earlier — so refuse the configuration at the point it is read.
        // No clock is needed: zero is broken whatever the time.
        if self.region != Region::Us
            || !valid_issuer(&self.regional_issuer)
            || !valid_issuer(&self.legacy_issuer)
            || self.regional_issuer == self.legacy_issuer
            || self.legacy_cutover_millis < 0
            || (matches!(self.issuance, CredentialIssuance::Legacy)
                && self.legacy_cutover_millis == 0)
            || self
                .legacy_cutover_millis
                .checked_add(REFRESH_ABSOLUTE_MILLIS)
                .is_none()
            || self.legacy_access_kids.len() > 16
            || self.legacy_attestation_kids.len() > 16
            || !self
                .legacy_access_kids
                .is_disjoint(&self.legacy_attestation_kids)
            || self
                .legacy_access_kids
                .iter()
                .chain(&self.legacy_attestation_kids)
                .any(|kid| {
                    kid.is_empty()
                        || kid.len() > 128
                        || !kid
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                })
        {
            return Err(CryptoError::InvalidInput);
        }
        Ok(())
    }

    pub fn issuance_issuer(&self) -> &str {
        match self.issuance {
            CredentialIssuance::Legacy => &self.legacy_issuer,
            CredentialIssuance::Regional => &self.regional_issuer,
        }
    }

    pub(crate) fn issuance_region(
        &self,
        issuer: &str,
        issued_at: i64,
        kid: &str,
        usage: CredentialUse,
    ) -> Result<Option<Region>, CryptoError> {
        if issuer != self.issuance_issuer() {
            return Err(CryptoError::InvalidInput);
        }
        let legacy_kids = self.legacy_kids(usage);
        match self.issuance {
            CredentialIssuance::Legacy
                if issued_at <= self.legacy_cutover_millis && legacy_kids.contains(kid) =>
            {
                Ok(None)
            }
            CredentialIssuance::Regional if !self.is_legacy_kid(kid) => Ok(Some(self.region)),
            _ => Err(CryptoError::InvalidInput),
        }
    }

    fn legacy_kids(&self, usage: CredentialUse) -> &BTreeSet<String> {
        match usage {
            CredentialUse::Access => &self.legacy_access_kids,
            CredentialUse::Attestation | CredentialUse::ReceiptInspection => {
                &self.legacy_attestation_kids
            }
        }
    }

    fn is_legacy_kid(&self, kid: &str) -> bool {
        self.legacy_access_kids.contains(kid) || self.legacy_attestation_kids.contains(kid)
    }

    pub(crate) fn admits(
        &self,
        issuer: &str,
        region: Option<Region>,
        issued_at: i64,
        now: i64,
        kid: &str,
        usage: CredentialUse,
    ) -> bool {
        if region == Some(self.region) && issuer == self.regional_issuer {
            return !self.is_legacy_kid(kid);
        }
        let lifetime = match usage {
            CredentialUse::Access => ACCESS_TOKEN_MILLIS,
            CredentialUse::Attestation => DEVICE_ATTESTATION_MAX_MILLIS,
            CredentialUse::ReceiptInspection => REFRESH_ABSOLUTE_MILLIS,
        };
        region.is_none()
            && issuer == self.legacy_issuer
            && issued_at <= self.legacy_cutover_millis
            && self.legacy_kids(usage).contains(kid)
            && self
                .legacy_cutover_millis
                .checked_add(lifetime)
                .is_some_and(|until| now < until)
    }
}

#[cfg(test)]
mod tests;
