// SPDX-License-Identifier: MPL-2.0

//! US account placement, before provider work and independent of transport.
//!
//! A deployment selection is provisional client placement, never evidence of
//! an account's persisted region. The narrow repository read establishes that
//! fact. Neither operation enables Cloud or creates a local region record.

use crate::{KoruIdentityId, PortFuture, RepositoryError, workspace_authorization::Region};

/// Only active accounts have an authoritative home-region result. Missing and
/// disabled identities both return None; database errors must not become US.
pub trait AccountHomeRegionRepository: Send + Sync {
    fn account_home_region(
        &self,
        identity: KoruIdentityId,
    ) -> PortFuture<'_, Result<Option<Region>, RepositoryError>>;
}

/// Validated deployment admission. Keep fields private: accepting a future
/// Region variant in another subsystem must not enable account PII there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountDeployment {
    region: Region,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountRegionError {
    RegionUnavailable,
    Unauthenticated,
    ServerUnavailable,
}

impl AccountRegionError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::RegionUnavailable => "region_unavailable",
            Self::Unauthenticated => "unauthenticated",
            Self::ServerUnavailable => "server_unavailable",
        }
    }
}

impl AccountDeployment {
    /// Called by trusted host configuration before sign-in, provider token
    /// handling or automatic session restore. No IO and no persisted choice.
    pub fn prepare(region: Region) -> Result<Self, AccountRegionError> {
        match region {
            Region::Us => Ok(Self { region }),
            Region::Eu => Err(AccountRegionError::RegionUnavailable),
        }
    }

    pub const fn region(self) -> Region {
        self.region
    }

    /// Caller supplies the identity from its current authenticated session,
    /// never from an unauthenticated request parameter or Profile binding.
    pub async fn account_home_region(
        self,
        repository: &(impl AccountHomeRegionRepository + ?Sized),
        identity: KoruIdentityId,
    ) -> Result<Region, AccountRegionError> {
        let region = repository
            .account_home_region(identity)
            .await
            .map_err(|_| AccountRegionError::ServerUnavailable)?
            .ok_or(AccountRegionError::Unauthenticated)?;
        if region != self.region {
            return Err(AccountRegionError::RegionUnavailable);
        }
        Ok(region)
    }
}

#[cfg(test)]
mod tests;
