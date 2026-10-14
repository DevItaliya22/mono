// SPDX-License-Identifier: MPL-2.0

//! A Connect click stays owned by its account form through admission and result.
use super::*;
use koru_workspace_host::account_ui::{CommandAdmission, CommandFailure, CommandSubmissionId};

#[cfg(test)]
mod tests;

pub(super) struct PendingConnect {
    pub profile: ProfileId,
    pub submission: CommandSubmissionId,
    profile_ids: Vec<String>,
    pub uncertain: bool,
    generation: Option<u64>,
    floor: u64,
    early: std::collections::BTreeMap<u64, AccountOperationResult>,
    pub awaiting_catalog: bool,
}

#[cfg(test)]
impl PendingConnect {
    pub(super) fn for_test(profile: ProfileId) -> Self {
        Self {
            profile,
            submission: CommandSubmissionId::next(),
            profile_ids: vec![profile.to_storage()],
            uncertain: false,
            generation: None,
            floor: 0,
            early: Default::default(),
            awaiting_catalog: false,
        }
    }
}

impl ProfilesController {
    pub(super) fn begin_inline_connect(&mut self, profile: ProfileId) {
        if let Some(pending) = self.inline_connect.as_mut() {
            if pending.profile == profile && pending.uncertain && self.pending.is_none() {
                pending.submission = CommandSubmissionId::next();
                pending.generation = None;
                pending.floor = self.account.generation;
                pending.early.clear();
                pending.uncertain = false;
                self.pending = Some(AccountCommand::SelectProfiles {
                    profile_ids: pending.profile_ids.clone(),
                });
                self.notice = None;
            }
            return;
        }
        if self.pending.is_some()
            || self.inline_connect.is_some()
            || !self
                .missing_account_profiles()
                .iter()
                .any(|row| row.profile_id == profile.to_storage())
        {
            return;
        }
        // SelectProfiles replaces the pending set. Keep previously selected,
        // still-authorized siblings while adding this row to the device.
        let mut profile_ids: Vec<_> = self
            .account
            .enrollments
            .iter()
            .filter(|row| {
                row.state != koru_agent_wire::EnrollmentState::Complete
                    && self
                        .account_profiles
                        .iter()
                        .any(|authorized| authorized.profile_id == row.profile_id)
            })
            .map(|row| row.profile_id.clone())
            .collect();
        profile_ids.push(profile.to_storage());
        profile_ids.sort();
        profile_ids.dedup();
        self.inline_connect = Some(PendingConnect {
            profile,
            submission: CommandSubmissionId::next(),
            profile_ids: profile_ids.clone(),
            uncertain: false,
            generation: None,
            floor: self.account.generation,
            early: Default::default(),
            awaiting_catalog: false,
        });
        self.notice = None;
        self.pending = Some(AccountCommand::SelectProfiles { profile_ids });
    }

    pub(super) fn inline_connect_failed(&mut self, failure: CommandFailure) -> bool {
        if self.inline_connect.as_ref().is_none_or(|pending| {
            pending.submission != failure.submission || pending.awaiting_catalog
        }) {
            return false;
        }
        if failure.uncertain {
            self.inline_connect.as_mut().unwrap().uncertain = true;
        } else {
            self.inline_connect = None;
        }
        self.notice =
            Some("Koru could not confirm the connection. Your local content is unchanged.".into());
        true
    }

    pub(super) fn inline_connect_admitted(&mut self, admission: CommandAdmission) -> bool {
        let Some(pending) = self.inline_connect.as_mut() else {
            return false;
        };
        if pending.submission != admission.submission
            || admission.owner_scope != self.account.active_scope
        {
            return false;
        }
        pending.generation = Some(admission.generation);
        let early = pending.early.remove(&admission.generation);
        pending.early.clear();
        if let Some(result) = early {
            self.inline_connect_result(admission.generation, &result);
        }
        true
    }

    pub(super) fn inline_connect_result_scoped(
        &mut self,
        scope: u64,
        generation: u64,
        result: &AccountOperationResult,
    ) -> bool {
        if !matches!(
            result,
            AccountOperationResult::ProfilesSelected { .. }
                | AccountOperationResult::Failed { .. }
                | AccountOperationResult::Cancelled
        ) {
            return false;
        }
        let active = self.account.active_scope;
        if scope != active && !self.account_forms.contains_key(&scope) {
            return false;
        }
        self.activate_scope(scope);
        let handled = self.inline_connect_result(generation, result);
        self.activate_scope(active);
        handled
    }

    fn inline_connect_result(&mut self, generation: u64, result: &AccountOperationResult) -> bool {
        let Some(pending) = self.inline_connect.as_mut() else {
            return false;
        };
        if pending.awaiting_catalog || generation <= pending.floor {
            return false;
        }
        if pending.generation.is_none() {
            if pending.early.len() < 8 {
                pending.early.insert(generation, result.clone());
            }
            return true;
        }
        if pending.generation != Some(generation) {
            return false;
        }
        match result {
            AccountOperationResult::ProfilesSelected { profiles }
                if profiles
                    .iter()
                    .any(|row| row.profile_id == pending.profile.to_storage()) =>
            {
                // SelectProfiles acknowledges durable local preparation. Wait
                // for the catalog projection, without opening another Profile.
                pending.awaiting_catalog = true;
                self.account.enrollments = profiles.clone();
                self.inline_connect_catalog_ready();
            }
            AccountOperationResult::ProfilesSelected { .. }
            | AccountOperationResult::Failed { .. }
            | AccountOperationResult::Cancelled => {
                self.inline_connect = None;
                self.notice =
                    Some("Koru could not connect this Profile. Try Connect again.".into());
            }
            _ => return false,
        }
        true
    }

    pub(super) fn inline_connect_catalog_ready(&mut self) {
        if let Some(pending) = &self.inline_connect
            && (pending.awaiting_catalog || pending.uncertain)
            && self.profiles.iter().any(|row| row.id == pending.profile)
        {
            self.selected = Some(pending.profile);
            self.inline_connect = None;
            self.notice = None;
        }
    }

    pub fn profile_connection_status(&self, profile: ProfileId) -> Option<&'static str> {
        // Cloud progress has its own subtitle; peer enrollment cannot block it.
        if self.cloud_sync_on(profile) {
            return None;
        }
        use koru_agent_wire::EnrollmentState;
        use koru_i18n::Key;
        let key = if let Some(pending) = &self.inline_connect
            && pending.profile == profile
            && !pending.awaiting_catalog
        {
            if pending.uncertain {
                Key::ProfileConnectionFailed
            } else {
                Key::ProfileChooserConnecting
            }
        } else {
            match self
                .account
                .enrollments
                .iter()
                .find(|row| row.profile_id == profile.to_storage())?
                .state
            {
                EnrollmentState::Waiting => Key::ProfileConnectionWaiting,
                EnrollmentState::Enrolling => Key::ProfileChooserConnecting,
                EnrollmentState::Failed => Key::ProfileConnectionFailed,
                EnrollmentState::Complete => Key::ProfileConnectionComplete,
                EnrollmentState::Idle => return None,
            }
        };
        Some(koru_i18n::t(crate::format::lang(), key))
    }
}
