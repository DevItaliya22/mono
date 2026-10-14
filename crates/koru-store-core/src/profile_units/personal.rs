// SPDX-License-Identifier: MPL-2.0

use super::content::ProfileContent;
use super::*;
use crate::ids::{ActionId, ReminderId};
use crate::personal_attributes::{EntityRef, PersonalAttributeValue, PersonalAttributes};
use crate::reminders::{OccurrenceAction, ReminderReductionInput, ReminderRule, StoredReminder};
use std::collections::{BTreeMap, BTreeSet};

pub struct PersonalManifest {
    pub(super) inner: UnitDoc,
}
impl PersonalManifest {
    pub fn create(owner: ProfileId, workspace: WorkspaceId) -> Result<Self> {
        Ok(Self {
            inner: UnitDoc::fresh(ProfileUnitRef::Personal { owner, workspace })?,
        })
    }
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            inner: UnitDoc::load(bytes, |owner, workspace| {
                Ok(ProfileUnitRef::Personal {
                    owner,
                    workspace: workspace.ok_or_else(scope_mismatch)?,
                })
            })?,
        })
    }
    pub fn owner(&self) -> ProfileId {
        self.inner.unit.profile()
    }
    pub fn workspace(&self) -> WorkspaceId {
        self.inner
            .unit
            .workspace()
            .expect("personal address has workspace")
    }
    fn check_target(&self, target: &EntityRef) -> Result<()> {
        if target.workspace() == self.workspace() {
            Ok(())
        } else {
            Err(scope_mismatch())
        }
    }
    unit_methods!();
    fn content(&self) -> ProfileContent<'_> {
        ProfileContent {
            doc: &self.inner.doc,
        }
    }

    pub fn set_personal_attribute(
        &self,
        entity: &EntityRef,
        key: &str,
        value: &PersonalAttributeValue,
    ) -> Result<()> {
        self.inner.require_writable()?;
        self.check_target(entity)?;
        self.content().set_personal_attribute(entity, key, value)
    }

    pub fn clear_personal_attribute(&self, entity: &EntityRef, key: &str) -> Result<bool> {
        self.inner.require_writable()?;
        self.check_target(entity)?;
        self.content().clear_personal_attribute(entity, key)
    }

    pub fn personal_attributes(&self, entity: &EntityRef) -> PersonalAttributes {
        if self.check_target(entity).is_err() || self.is_read_only() {
            return Default::default();
        }
        self.content().personal_attributes(entity)
    }

    pub fn put_reminder_rule(&self, rule: &ReminderRule) -> Result<()> {
        self.inner.require_writable()?;
        self.check_target(&rule.target)?;
        self.content().put_reminder_rule(rule)
    }

    pub fn delete_reminder(&self, id: ReminderId) -> Result<()> {
        self.inner.require_writable()?;
        self.content().delete_reminder(id)
    }

    pub fn delete_reminders_for_target(&self, target: &EntityRef) -> Result<Vec<ReminderId>> {
        self.inner.require_writable()?;
        self.check_target(target)?;
        self.content().delete_reminders_for_target(target)
    }

    pub fn append_reminder_action(
        &self,
        reminder: ReminderId,
        action: &OccurrenceAction,
    ) -> Result<()> {
        self.inner.require_writable()?;
        self.content().append_reminder_action(reminder, action)
    }

    pub fn clear_handled_reminder_history(&self) -> Result<usize> {
        self.inner.require_writable()?;
        self.content().clear_handled_reminder_history()
    }

    pub fn clear_handled_reminder_history_for_target(&self, target: &EntityRef) -> Result<usize> {
        self.inner.require_writable()?;
        self.check_target(target)?;
        self.content()
            .clear_handled_reminder_history_for_target(target)
    }

    pub fn reminder(&self, id: ReminderId) -> Option<StoredReminder> {
        if self.is_read_only() {
            return None;
        }
        self.content().reminder(id)
    }

    pub fn raw_reminder_action(
        &self,
        reminder: ReminderId,
        action: ActionId,
    ) -> Option<OccurrenceAction> {
        if self.is_read_only() {
            return None;
        }
        self.content().raw_reminder_action(reminder, action)
    }

    pub(crate) fn raw_reminder_actions(
        &self,
    ) -> BTreeMap<(ReminderId, ActionId), OccurrenceAction> {
        if self.is_read_only() {
            return BTreeMap::new();
        }
        self.content().raw_reminder_actions()
    }

    pub(crate) fn cleared_reminder_actions(&self) -> BTreeSet<(ReminderId, ActionId)> {
        if self.is_read_only() {
            return BTreeSet::new();
        }
        self.content().cleared_reminder_actions()
    }

    pub fn reminders(&self) -> Vec<StoredReminder> {
        if self.is_read_only() {
            return Vec::new();
        }
        self.content().reminders()
    }

    pub(crate) fn reminder_reduction_inputs(&self) -> Vec<ReminderReductionInput> {
        if self.is_read_only() {
            return Vec::new();
        }
        self.content().reminder_reduction_inputs()
    }
}
