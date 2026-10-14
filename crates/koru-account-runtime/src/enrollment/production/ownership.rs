// SPDX-License-Identifier: MPL-2.0

//! Authenticated ownership evidence for persisted enrollment intent.
use std::collections::BTreeSet;

use koru_account_core::{DeviceId, ProfileId};
use koru_account_wire::{ProfileSummaryV1, SessionSummaryV1};
use koru_store_wire::ProfileLifecycleInventory;

use crate::enrollment::{EnrollmentAuthorization, Failure};
use crate::identity::{AccountOwner, DirectoryState, SqliteTrustRepository};

#[derive(Clone)]
pub(super) struct AuthorizedProfiles {
    namespace: String,
    owner: AccountOwner,
    record_version: i64,
    session_id: String,
    profiles: BTreeSet<ProfileId>,
}

/// Nonsecret authorization publication, ordered by the directory's monotonic
/// ready revision. A failed newer load retains its revision fence as well.
#[derive(Default)]
pub(super) struct AuthorizationCache {
    entries: std::collections::BTreeMap<
        (String, koru_account_core::KoruIdentityId),
        CachedAuthorization,
    >,
}
struct CachedAuthorization {
    version: i64,
    owner: AccountOwner,
    session_id: String,
    value: Option<AuthorizedProfiles>,
}
impl AuthorizationCache {
    pub fn authorize_effect(
        &self,
        credential: &EnrollmentAuthorization,
        profile: ProfileId,
    ) -> Result<(), Failure> {
        let value = self
            .entries
            .get(&(credential.namespace.clone(), credential.owner.account))
            .and_then(|entry| entry.value.as_ref())
            .ok_or(Failure::Unavailable)?;
        if value.owner != credential.owner
            || value.record_version != credential.record_version
            || value.session_id != credential.session_id
        {
            return Err(Failure::Unavailable);
        }
        if value.profiles.contains(&profile) {
            Ok(())
        } else {
            Err(Failure::AccessRevoked)
        }
    }
    pub fn begin(&mut self, credential: &EnrollmentAuthorization) -> Result<(), Failure> {
        let key = (credential.namespace.clone(), credential.owner.account);
        if let Some(current) = self.entries.get(&key) {
            if current.version > credential.record_version
                || (current.version == credential.record_version
                    && (current.owner != credential.owner
                        || current.session_id != credential.session_id))
            {
                return Err(Failure::Persistence);
            }
            if current.version == credential.record_version {
                // A concurrent selection revalidates this same authority. Its
                // outstanding hosted read must not manufacture a revocation
                // for an already-running member by briefly emptying the cache.
                return Ok(());
            }
        }
        self.entries.insert(
            key,
            CachedAuthorization {
                version: credential.record_version,
                owner: credential.owner,
                session_id: credential.session_id.clone(),
                value: None,
            },
        );
        Ok(())
    }

    pub fn publish(&mut self, value: AuthorizedProfiles) -> Result<(), Failure> {
        let key = (value.namespace.clone(), value.owner.account);
        if self.entries.get(&key).is_some_and(|current| {
            current.version > value.record_version
                || (current.version == value.record_version
                    && (current.owner != value.owner || current.session_id != value.session_id))
        }) {
            return Err(Failure::Persistence);
        }
        self.entries.insert(
            key,
            CachedAuthorization {
                version: value.record_version,
                owner: value.owner,
                session_id: value.session_id.clone(),
                value: Some(value),
            },
        );
        Ok(())
    }

    pub fn allows(&self, credential: &EnrollmentAuthorization, profile: ProfileId) -> bool {
        self.entries
            .get(&(credential.namespace.clone(), credential.owner.account))
            .and_then(|entry| entry.value.as_ref())
            .is_some_and(|value| value.allows(credential, profile))
    }
}

impl AuthorizedProfiles {
    pub fn repair_missing_completed(
        &self,
        repository: &mut SqliteTrustRepository,
        inventory: &ProfileLifecycleInventory,
        local: DeviceId,
        previous: BTreeSet<ProfileId>,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        let mut next = previous.clone();
        for profile in &self.profiles {
            if !inventory
                .profiles
                .iter()
                .any(|row| row.profile_id == *profile)
                && repository
                    .completed_enrollment(*profile, local)
                    .map_err(|_| Failure::Persistence)?
                    .is_some()
            {
                next.insert(*profile);
            }
        }
        if next != previous {
            repository
                .replace_account_targets(
                    &self.namespace,
                    self.owner,
                    self.record_version,
                    &previous,
                    &next,
                )
                .map_err(|_| Failure::Persistence)?;
        }
        Ok(next)
    }

    pub fn verify(
        credential: &EnrollmentAuthorization,
        device: DeviceId,
        session: &SessionSummaryV1,
        profiles: &[ProfileSummaryV1],
        inventory: &ProfileLifecycleInventory,
    ) -> Result<Self, Failure> {
        if session.session_id.to_string() != credential.session_id
            || session.koru_identity_id != credential.owner.account
            || session.auth_identity_id != credential.owner.auth_identity
            || session.device_id != Some(device)
        {
            return Err(Failure::SessionExpired);
        }
        if profiles
            .iter()
            .any(|profile| profile.koru_identity_id != credential.owner.account)
        {
            return Err(Failure::AccessRevoked);
        }
        let binding = format!("koru:{}", credential.owner.account);
        let profiles = profiles
            .iter()
            .filter(|profile| {
                profile.koru_identity_id == credential.owner.account
                    && profile
                        .auth_identities
                        .iter()
                        .any(|identity| identity.auth_identity_id == credential.owner.auth_identity)
                    && inventory
                        .profiles
                        .iter()
                        .filter(|local| local.profile_id == profile.profile_id)
                        .all(|local| {
                            local.account_binding.is_empty() || local.account_binding == binding
                        })
            })
            .map(|profile| profile.profile_id)
            .collect();
        Ok(Self {
            namespace: credential.namespace.clone(),
            owner: credential.owner,
            record_version: credential.record_version,
            session_id: credential.session_id.clone(),
            profiles,
        })
    }

    pub fn allows(&self, credential: &EnrollmentAuthorization, profile: ProfileId) -> bool {
        self.owner == credential.owner
            && self.namespace == credential.namespace
            && self.record_version == credential.record_version
            && self.session_id == credential.session_id
            && self.profiles.contains(&profile)
    }

    pub fn claim_and_load(
        &self,
        repository: &mut SqliteTrustRepository,
    ) -> Result<BTreeSet<ProfileId>, Failure> {
        let directory = repository
            .account_directory(&self.namespace)
            .map_err(|_| Failure::Persistence)?;
        if !directory.records.iter().any(|record| {
            record.owner == self.owner
                && record.version == self.record_version
                && record.state == DirectoryState::Ready
        }) {
            return Err(Failure::Persistence);
        }
        if let Some(profile) = repository
            .enrollment_target()
            .map_err(|_| Failure::Persistence)?
            && self.profiles.contains(&profile)
        {
            repository
                .claim_unattributed_target(
                    &self.namespace,
                    self.owner,
                    self.record_version,
                    profile,
                )
                .map_err(|_| Failure::Persistence)?;
        }
        repository
            .account_targets(&self.namespace, self.owner.account)
            .map_err(|_| Failure::Persistence)
    }
}

#[cfg(test)]
mod tests;
