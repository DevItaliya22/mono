// SPDX-License-Identifier: MPL-2.0

//! Journal schema 2 has no account column. Never infer the owner of an old
//! destructive intent from a Profile binding that may already have changed.
use super::*;
use koru_account_core::ProfileLifecycleMutationId;

#[derive(Default)]
pub(super) struct Ownership {
    intents: HashMap<ProfileId, (ProfileLifecycleMutationId, KoruIdentityId)>,
    rebound: HashMap<ProfileId, KoruIdentityId>,
}
impl Ownership {
    pub fn validate(
        &self,
        profile: ProfileId,
        account: KoruIdentityId,
        existing: Option<&lifecycle::CleanupRecord>,
    ) -> Result<(), ServiceError> {
        if self
            .rebound
            .get(&profile)
            .is_some_and(|owner| *owner != account)
        {
            return Err(ServiceError::terminal("profile_lifecycle_owner_changed"));
        }
        if let Some(record) = existing {
            match self.intents.get(&profile) {
                Some((operation, owner)) if *operation == record.operation && *owner == account => {
                }
                Some((operation, _)) if *operation == record.operation => {
                    return Err(ServiceError::terminal("profile_lifecycle_owner_changed"));
                }
                _ => return Err(ServiceError::terminal("profile_lifecycle_owner_unproven")),
            }
        }
        Ok(())
    }

    pub fn remember(&mut self, record: &lifecycle::CleanupRecord, account: KoruIdentityId) {
        self.intents
            .insert(record.profile, (record.operation, account));
    }

    /// Called only after a validated hosted rebind response. Keep this fence
    /// even if journal retirement fails: local Store binding may still name A.
    pub fn proven_rebind(&mut self, profile: ProfileId, account: KoruIdentityId) {
        self.rebound.insert(profile, account);
        self.intents.remove(&profile);
    }
}

#[cfg(test)]
mod tests;
