// SPDX-License-Identifier: MPL-2.0

//! Independently durable Profile content, addressed by owner and unit.

use super::*;
use crate::personal_attributes::{PersonalAttributeValue, PersonalAttributes};
use crate::ports::{ProfileUnitState, ReminderDue};
use crate::profile_units::{IdentityManifest, PersonalManifest, PrivateManifest, ProfileUnitRef};

impl<C: Catalog, B: Backend> Store<C, B> {
    pub(super) fn require_profile(&self, profile: ProfileId) -> Result<()> {
        match self.catalog.profile_catalog_state(profile)? {
            Some(ProfileCatalogState::Active) => Ok(()),
            Some(ProfileCatalogState::Removed) => Err(Error::bare(Code::ProfileRemoved)),
            None => Err(Error::bare(Code::ProfileNotFound)),
        }
    }

    fn load_private(&mut self, profile: ProfileId) -> Result<&PrivateManifest> {
        self.require_profile(profile)?;
        if !self.private_profiles.contains_key(&profile) {
            let state = self
                .catalog
                .profile_unit(ProfileUnitRef::Private { profile })?
                .ok_or_else(|| Error::bare(Code::ProfilePrivateUnavailable))?;
            let doc = PrivateManifest::from_snapshot(&state.snapshot)?;
            if doc.profile_id() != profile {
                return Err(Error::bare(Code::ProfileUnitScopeMismatch));
            }
            doc.set_peer_id(self.peer_id)?;
            self.private_profiles.insert(profile, doc);
        }
        Ok(&self.private_profiles[&profile])
    }

    fn personal_candidate(
        &self,
        owner: ProfileId,
        workspace: WorkspaceId,
    ) -> Result<PersonalManifest> {
        self.require_profile(owner)?;
        if self
            .catalog
            .profile_unit(ProfileUnitRef::Identity { profile: owner })?
            .is_none()
        {
            return Err(Error::bare(Code::VersionUnavailable));
        }
        if self.catalog.workspace(workspace)?.is_none() {
            return Err(Error::bare(Code::WorkspaceNotFound));
        }
        let unit = ProfileUnitRef::Personal { owner, workspace };
        let doc = match self.catalog.profile_unit(unit)? {
            Some(state) => PersonalManifest::from_snapshot(&state.snapshot)?,
            None => PersonalManifest::create(owner, workspace)?,
        };
        if doc.owner() != owner || doc.workspace() != workspace {
            return Err(Error::bare(Code::ProfileUnitScopeMismatch));
        }
        doc.set_peer_id(self.peer_id)?;
        Ok(doc)
    }

    fn personal_documents(&self, owner: ProfileId) -> Result<Vec<PersonalManifest>> {
        self.require_profile(owner)?;
        self.catalog
            .profile_units(owner)?
            .into_iter()
            .filter(|state| matches!(state.unit, ProfileUnitRef::Personal { .. }))
            .map(|state| {
                let doc = PersonalManifest::from_snapshot(&state.snapshot)?;
                if state.unit
                    != (ProfileUnitRef::Personal {
                        owner: doc.owner(),
                        workspace: doc.workspace(),
                    })
                {
                    return Err(Error::bare(Code::ProfileUnitScopeMismatch));
                }
                doc.set_peer_id(self.peer_id)?;
                Ok(doc)
            })
            .collect()
    }

    fn save_personal(&mut self, doc: &PersonalManifest) -> Result<()> {
        self.catalog.commit_profile_unit(
            ProfileUnitRef::Personal {
                owner: doc.owner(),
                workspace: doc.workspace(),
            },
            &doc.export_snapshot()?,
        )
    }

    fn locate_reminder(
        &self,
        owner: ProfileId,
        reminder: ReminderId,
    ) -> Result<(PersonalManifest, StoredReminder)> {
        let mut found = None;
        for doc in self.personal_documents(owner)? {
            if let Some(row) = doc.reminder(reminder) {
                if found.is_some() {
                    return Err(Error::bare(Code::ProfileUnitRequired));
                }
                found = Some((doc, row));
            }
        }
        found.ok_or_else(|| Error::bare(Code::ReminderNotFound))
    }

    pub fn personal_attributes(
        &mut self,
        profile: ProfileId,
        entity: &EntityRef,
    ) -> Result<PersonalAttributes> {
        self.require_profile(profile)?;
        let Some(state) = self.catalog.profile_unit(ProfileUnitRef::Personal {
            owner: profile,
            workspace: entity.workspace(),
        })?
        else {
            return Ok(PersonalAttributes::new());
        };
        let doc = PersonalManifest::from_snapshot(&state.snapshot)?;
        if doc.owner() != profile || doc.workspace() != entity.workspace() {
            return Err(Error::bare(Code::ProfileUnitScopeMismatch));
        }
        if doc.is_read_only() {
            return Err(Error::bare(Code::SchemaTooNew));
        }
        Ok(doc.personal_attributes(entity))
    }

    pub fn set_personal_attribute(
        &mut self,
        profile: ProfileId,
        entity: &EntityRef,
        key: &str,
        value: Option<&PersonalAttributeValue>,
    ) -> Result<PersonalAttributes> {
        let doc = self.personal_candidate(profile, entity.workspace())?;
        let changed = match value {
            Some(value) => {
                doc.set_personal_attribute(entity, key, value)?;
                true
            }
            None => doc.clear_personal_attribute(entity, key)?,
        };
        if changed {
            self.save_personal(&doc)?;
        }
        Ok(doc.personal_attributes(entity))
    }

    fn locate_reminder_in(
        &self,
        owner: ProfileId,
        workspace: WorkspaceId,
        reminder: ReminderId,
    ) -> Result<(PersonalManifest, StoredReminder)> {
        let doc = self.personal_candidate(owner, workspace)?;
        if doc.is_read_only() {
            return Err(Error::bare(Code::SchemaTooNew));
        }
        let row = doc
            .reminder(reminder)
            .ok_or_else(|| Error::bare(Code::ReminderNotFound))?;
        Ok((doc, row))
    }

    pub fn reminder_in(
        &mut self,
        profile: ProfileId,
        workspace: WorkspaceId,
        reminder: ReminderId,
    ) -> Result<StoredReminder> {
        self.locate_reminder_in(profile, workspace, reminder)
            .map(|(_, row)| row)
    }

    pub fn reminder(&mut self, profile: ProfileId, reminder: ReminderId) -> Result<StoredReminder> {
        self.locate_reminder(profile, reminder).map(|(_, row)| row)
    }

    pub fn reminders(&mut self, profile: ProfileId) -> Result<Vec<StoredReminder>> {
        Ok(self
            .personal_documents(profile)?
            .into_iter()
            .flat_map(|doc| doc.reminders())
            .collect())
    }

    /// A complete index is required before native delivery can interpret an
    /// empty result. The asynchronous adapter repairs stale units off-thread.
    pub fn reminder_reduction_inputs(
        &mut self,
        profile: ProfileId,
    ) -> Result<Vec<ReminderReductionInput>> {
        self.require_profile(profile)?;
        let mut out = Vec::new();
        let indexed_rows = self.catalog.profile_reminder_due(profile)?;
        for state in self.catalog.profile_units(profile)? {
            if !matches!(state.unit, ProfileUnitRef::Personal { .. }) {
                continue;
            }
            let doc = PersonalManifest::from_snapshot(&state.snapshot)?;
            if doc.is_read_only() {
                return Err(Error::bare(Code::SchemaTooNew));
            }
            let indexed = state
                .indexed_version
                .as_deref()
                .and_then(|v| loro::VersionVector::decode(v).ok());
            if indexed.as_ref() != Some(&doc.version()) {
                return Err(Error::bare(Code::ReminderIndexUpdating));
            }
            let inputs = doc.reminder_reduction_inputs();
            let ids: BTreeSet<_> = indexed_rows
                .iter()
                .filter(|row| row.workspace == doc.workspace())
                .map(|row| row.reminder)
                .collect();
            let actual: BTreeSet<_> = inputs.iter().map(|row| row.stored().rule.id).collect();
            if ids != actual {
                return Err(Error::bare(Code::ReminderIndexUpdating));
            }
            out.extend(inputs);
        }
        let mut seen = BTreeSet::new();
        if out.iter().any(|row| !seen.insert(row.stored().rule.id)) {
            return Err(Error::bare(Code::ProfileUnitRequired));
        }
        Ok(out)
    }

    /// Opaque candidates are taken under the Store lock; Loro decoding and
    /// reducer work run on the adapter's existing blocking worker.
    pub fn reminder_index_candidates(&self, profile: ProfileId) -> Result<Vec<ProfileUnitState>> {
        self.require_profile(profile)?;
        Ok(self
            .catalog
            .profile_units(profile)?
            .into_iter()
            .filter(|state| matches!(state.unit, ProfileUnitRef::Personal { .. }))
            .collect())
    }

    pub fn reminder_index_rows(&self, profile: ProfileId) -> Result<Vec<ReminderDue>> {
        self.require_profile(profile)?;
        self.catalog.profile_reminder_due(profile)
    }

    pub fn publish_reminder_index(
        &mut self,
        source: &ProfileUnitState,
        version: &[u8],
        rows: &[ReminderDue],
    ) -> Result<bool> {
        let ProfileUnitRef::Personal { owner, workspace } = source.unit else {
            return Err(Error::bare(Code::ProfileUnitScopeMismatch));
        };
        self.require_profile(owner)?;
        self.catalog
            .replace_profile_due_index(owner, workspace, &source.snapshot, version, rows)
    }

    pub fn reminders_for(
        &mut self,
        profile: ProfileId,
        target: &EntityRef,
    ) -> Result<Vec<StoredReminder>> {
        Ok(self
            .reminders(profile)?
            .into_iter()
            .filter(|row| row.rule.target == *target)
            .collect())
    }

    pub fn create_reminder(
        &mut self,
        profile: ProfileId,
        rule: &ReminderRule,
    ) -> Result<StoredReminder> {
        if rule.status == RuleStatus::Deleted {
            return Err(Error::new(
                Code::InvalidRequest,
                "deleted status requires the monotonic delete operation",
            ));
        }
        let doc = self.personal_candidate(profile, rule.target.workspace())?;
        for other in self.personal_documents(profile)? {
            if other.workspace() != doc.workspace() && other.reminder(rule.id).is_some() {
                return Err(Error::bare(Code::ProfileUnitRequired));
            }
        }
        if let Some(existing) = doc.reminder(rule.id) {
            return create_retry(&existing, rule).map(|_| existing);
        }
        doc.put_reminder_rule(rule)?;
        self.save_personal(&doc)?;
        self.record_reminder_change(profile, Some(rule.id), ReminderChange::Rule);
        doc.reminder(rule.id)
            .ok_or_else(|| Error::bare(Code::ReminderNotFound))
    }

    pub fn update_reminder(
        &mut self,
        profile: ProfileId,
        rule: &ReminderRule,
    ) -> Result<StoredReminder> {
        let (doc, _) = self.locate_reminder(profile, rule.id)?;
        self.update_reminder_in(profile, doc.workspace(), rule)
    }

    pub fn update_reminder_in(
        &mut self,
        profile: ProfileId,
        workspace: WorkspaceId,
        rule: &ReminderRule,
    ) -> Result<StoredReminder> {
        if rule.status == RuleStatus::Deleted {
            return Err(Error::new(
                Code::InvalidRequest,
                "deleted status requires the monotonic delete operation",
            ));
        }
        let (doc, existing) = self.locate_reminder_in(profile, workspace, rule.id)?;
        if doc.workspace() != rule.target.workspace() {
            return Err(Error::bare(Code::ProfileUnitScopeMismatch));
        }
        if !update_writes(&existing, rule)? {
            return Ok(existing);
        }
        doc.put_reminder_rule(rule)?;
        self.save_personal(&doc)?;
        self.record_reminder_change(profile, Some(rule.id), ReminderChange::Rule);
        doc.reminder(rule.id)
            .ok_or_else(|| Error::bare(Code::ReminderNotFound))
    }

    pub fn set_reminder_status_in(
        &mut self,
        profile: ProfileId,
        workspace: WorkspaceId,
        reminder: ReminderId,
        revision: RuleRevisionId,
        status: RuleStatus,
    ) -> Result<StoredReminder> {
        let mut rule = self.reminder_in(profile, workspace, reminder)?.rule;
        rule.revision = revision;
        rule.status = status;
        self.update_reminder_in(profile, workspace, &rule)
    }

    pub fn set_reminder_status(
        &mut self,
        profile: ProfileId,
        reminder: ReminderId,
        revision: RuleRevisionId,
        status: RuleStatus,
    ) -> Result<StoredReminder> {
        let mut rule = self.reminder(profile, reminder)?.rule;
        rule.revision = revision;
        rule.status = status;
        self.update_reminder(profile, &rule)
    }

    pub fn delete_reminder_rule(
        &mut self,
        profile: ProfileId,
        reminder: ReminderId,
    ) -> Result<StoredReminder> {
        let (doc, _) = self.locate_reminder(profile, reminder)?;
        self.delete_reminder_rule_in(profile, doc.workspace(), reminder)
    }

    pub fn delete_reminder_rule_in(
        &mut self,
        profile: ProfileId,
        workspace: WorkspaceId,
        reminder: ReminderId,
    ) -> Result<StoredReminder> {
        let (doc, existing) = self.locate_reminder_in(profile, workspace, reminder)?;
        if existing.deleted {
            return Ok(existing);
        }
        doc.delete_reminder(reminder)?;
        self.save_personal(&doc)?;
        self.record_reminder_change(profile, Some(reminder), ReminderChange::Rule);
        doc.reminder(reminder)
            .ok_or_else(|| Error::bare(Code::ReminderNotFound))
    }

    pub fn delete_reminders_for_target(
        &mut self,
        profile: ProfileId,
        target: &EntityRef,
    ) -> Result<Vec<ReminderId>> {
        let doc = self.personal_candidate(profile, target.workspace())?;
        let removed = doc.delete_reminders_for_target(target)?;
        if !removed.is_empty() {
            self.save_personal(&doc)?;
            for id in &removed {
                self.record_reminder_change(profile, Some(*id), ReminderChange::Rule);
            }
        }
        Ok(removed)
    }

    pub fn append_reminder_action(
        &mut self,
        profile: ProfileId,
        reminder: ReminderId,
        action: &OccurrenceAction,
    ) -> Result<StoredReminder> {
        let (doc, _) = self.locate_reminder(profile, reminder)?;
        self.append_reminder_action_in(profile, doc.workspace(), reminder, action)
    }

    pub fn append_reminder_action_in(
        &mut self,
        profile: ProfileId,
        workspace: WorkspaceId,
        reminder: ReminderId,
        action: &OccurrenceAction,
    ) -> Result<StoredReminder> {
        let (doc, existing) = self.locate_reminder_in(profile, workspace, reminder)?;
        if existing.deleted {
            return Err(Error::new(
                Code::IdempotencyConflict,
                "a deleted reminder cannot accept actions",
            ));
        }
        if let Some(prior) = doc.raw_reminder_action(reminder, action.id) {
            return action_retry(&prior, action).map(|_| existing);
        }
        doc.append_reminder_action(reminder, action)?;
        self.save_personal(&doc)?;
        self.record_reminder_change(profile, Some(reminder), ReminderChange::Action);
        doc.reminder(reminder)
            .ok_or_else(|| Error::bare(Code::ReminderNotFound))
    }

    pub fn clear_reminder_history(&mut self, profile: ProfileId) -> Result<usize> {
        let mut changed = Vec::new();
        let mut removed = 0;
        for doc in self.personal_documents(profile)? {
            let count = doc.clear_handled_reminder_history()?;
            if count > 0 {
                removed += count;
                changed.push((
                    ProfileUnitRef::Personal {
                        owner: profile,
                        workspace: doc.workspace(),
                    },
                    doc.export_snapshot()?,
                ));
            }
        }
        if !changed.is_empty() {
            self.catalog.commit_profile_units(&changed)?;
            self.record_reminder_change(profile, None, ReminderChange::History);
        }
        Ok(removed)
    }

    pub fn clear_reminder_history_for_target(
        &mut self,
        profile: ProfileId,
        target: &EntityRef,
    ) -> Result<usize> {
        let doc = self.personal_candidate(profile, target.workspace())?;
        let removed = doc.clear_handled_reminder_history_for_target(target)?;
        if removed > 0 {
            self.save_personal(&doc)?;
            self.record_reminder_change(profile, None, ReminderChange::History);
        }
        Ok(removed)
    }

    pub fn reminder_settings(&mut self, profile: ProfileId) -> Result<ReminderSettings> {
        let doc = self.load_private(profile)?;
        if doc.is_read_only() {
            return Err(Error::bare(Code::SchemaTooNew));
        }
        Ok(doc.reminder_settings())
    }

    pub fn update_reminder_settings(
        &mut self,
        profile: ProfileId,
        end_of_day: Option<Option<crate::reminders::LocalTime>>,
        default_tone: Option<crate::reminders::ToneId>,
        week_start: Option<crate::reminders::WeekStartPreference>,
    ) -> Result<ReminderSettings> {
        let snapshot = self.load_private(profile)?.export_snapshot()?;
        let doc = PrivateManifest::from_snapshot(&snapshot)?;
        doc.set_peer_id(self.peer_id)?;
        if doc.update_reminder_settings(end_of_day, default_tone, week_start)? {
            self.catalog.commit_profile_unit(
                ProfileUnitRef::Private { profile },
                &doc.export_snapshot()?,
            )?;
            self.record_reminder_change(profile, None, ReminderChange::Settings);
        }
        let result = doc.reminder_settings();
        self.private_profiles.insert(profile, doc);
        Ok(result)
    }

    pub fn personal_dictionary(
        &mut self,
        profile: ProfileId,
    ) -> Result<BTreeMap<String, Vec<String>>> {
        Ok(self.load_private(profile)?.dictionary())
    }

    pub fn set_dictionary_word(
        &mut self,
        profile: ProfileId,
        language: &str,
        word: &str,
        present: bool,
    ) -> Result<BTreeMap<String, Vec<String>>> {
        let snapshot = self.load_private(profile)?.export_snapshot()?;
        let doc = PrivateManifest::from_snapshot(&snapshot)?;
        doc.set_peer_id(self.peer_id)?;
        let changed = if present {
            doc.add_dictionary_word(language, word)?;
            true
        } else {
            doc.remove_dictionary_word(language, word)?
        };
        if changed {
            self.catalog.commit_profile_unit(
                ProfileUnitRef::Private { profile },
                &doc.export_snapshot()?,
            )?;
            self.record_dictionary_change(profile);
        }
        let result = doc.dictionary();
        self.private_profiles.insert(profile, doc);
        Ok(result)
    }

    pub fn clear_personal_dictionary(
        &mut self,
        profile: ProfileId,
    ) -> Result<BTreeMap<String, Vec<String>>> {
        let snapshot = self.load_private(profile)?.export_snapshot()?;
        let doc = PrivateManifest::from_snapshot(&snapshot)?;
        doc.set_peer_id(self.peer_id)?;
        if doc.clear_dictionary()? > 0 {
            self.catalog.commit_profile_unit(
                ProfileUnitRef::Private { profile },
                &doc.export_snapshot()?,
            )?;
            self.record_dictionary_change(profile);
        }
        let result = doc.dictionary();
        self.private_profiles.insert(profile, doc);
        Ok(result)
    }

    fn record_reminder_change(
        &mut self,
        profile: ProfileId,
        reminder: Option<ReminderId>,
        change: ReminderChange,
    ) {
        self.reminder_committed.insert(
            (profile, reminder, change),
            ReminderCommitted {
                profile,
                reminder,
                change,
            },
        );
    }

    fn record_dictionary_change(&mut self, profile: ProfileId) {
        self.dictionary_committed
            .insert(profile, DictionaryCommitted { profile });
    }

    fn record_identity_change(&mut self, profile: ProfileId, change: IdentityChange) {
        self.identity_committed
            .insert((profile, change), IdentityCommitted { profile, change });
    }

    pub fn profile_unit_states(&self, profile: ProfileId) -> Result<Vec<ProfileUnitState>> {
        self.require_profile(profile)?;
        self.catalog.profile_units(profile)
    }

    pub fn profile_unit_version(&mut self, unit: ProfileUnitRef) -> Result<loro::VersionVector> {
        self.require_profile(unit.profile())?;
        let Some(state) = self.catalog.profile_unit(unit)? else {
            return Ok(loro::VersionVector::new());
        };
        unit_version(unit, &state.snapshot)
    }

    /// The exact committed snapshot of one unit, never a synthesized empty unit
    /// or an enumeration/export of the other units belonging to its Profile.
    pub fn profile_unit_snapshot(&self, unit: ProfileUnitRef) -> Result<Vec<u8>> {
        self.require_profile(unit.profile())?;
        let state = self
            .catalog
            .profile_unit(unit)?
            .ok_or_else(|| Error::bare(Code::VersionUnavailable))?;
        unit_version(unit, &state.snapshot)?;
        Ok(state.snapshot)
    }

    pub fn profile_unit_updates_since(
        &mut self,
        unit: ProfileUnitRef,
        from: &loro::VersionVector,
    ) -> Result<Vec<u8>> {
        self.require_profile(unit.profile())?;
        let state = self
            .catalog
            .profile_unit(unit)?
            .ok_or_else(|| Error::bare(Code::VersionUnavailable))?;
        unit_version(unit, &state.snapshot)?;
        match unit {
            ProfileUnitRef::LegacyPrivate { .. } => {
                ProfileManifest::from_snapshot(&state.snapshot)?.export_updates_since(from)
            }
            ProfileUnitRef::Identity { .. } => {
                IdentityManifest::from_snapshot(&state.snapshot)?.export_updates_since(from)
            }
            ProfileUnitRef::Private { .. } => {
                PrivateManifest::from_snapshot(&state.snapshot)?.export_updates_since(from)
            }
            ProfileUnitRef::Personal { .. } => {
                PersonalManifest::from_snapshot(&state.snapshot)?.export_updates_since(from)
            }
        }
    }

    /// Compatibility for local callers that explicitly mean identity. Peer
    /// frames use apply_profile_unit_updates and cannot omit the discriminator.
    pub fn apply_profile_updates(&mut self, profile: ProfileId, updates: &[Vec<u8>]) -> Result<()> {
        self.apply_profile_unit_updates(ProfileUnitRef::Identity { profile }, updates)
    }

    pub fn apply_profile_unit_updates(
        &mut self,
        unit: ProfileUnitRef,
        updates: &[Vec<u8>],
    ) -> Result<()> {
        let profile = unit.profile();
        if self.catalog.profile_catalog_state(profile)? == Some(ProfileCatalogState::Removed) {
            return Err(Error::bare(Code::ProfileRemoved));
        }
        let before = self.catalog.profile_unit(unit)?;
        match unit {
            ProfileUnitRef::Identity { .. } => {
                // Parsed once and kept, because what it held before the import
                // is the only way to say *which* identity fact moved. The
                // Private and Personal branches below read their priors for the
                // same reason.
                let prior = before
                    .as_ref()
                    .map(|state| IdentityManifest::from_snapshot(&state.snapshot))
                    .transpose()?;
                let prior_identity = prior.as_ref().map(IdentityManifest::profile).transpose()?;
                let doc = match &prior {
                    Some(doc) => doc.staged_import(updates)?,
                    None => IdentityManifest::from_snapshot(&complete_unit_snapshot(updates)?)?,
                };
                if doc.unit() != unit {
                    return Err(Error::bare(Code::ProfileUnitScopeMismatch));
                }
                if doc.is_read_only() {
                    return Err(Error::bare(Code::SchemaTooNew));
                }
                let identity = doc.profile()?;
                doc.set_peer_id(self.peer_id)?;
                self.catalog
                    .commit_profile_identity(&identity, &doc.export_snapshot()?)?;
                self.open_profiles.insert(profile, doc);
                // Only against a unit this Store already held. A first import
                // is a profile arriving, not a profile changing, and saying
                // "renamed" about something nobody has seen yet would be a
                // lie; that case belongs to whatever announces a new profile.
                if let Some(prior_identity) = prior_identity {
                    if prior_identity.name != identity.name {
                        self.record_identity_change(profile, IdentityChange::Name);
                    }
                    if prior_identity.icon != identity.icon
                        || prior_identity.color != identity.color
                    {
                        self.record_identity_change(profile, IdentityChange::Appearance);
                    }
                }
            }
            ProfileUnitRef::Private { .. } => {
                self.require_profile(profile)?;
                let prior = before
                    .as_ref()
                    .map(|s| PrivateManifest::from_snapshot(&s.snapshot))
                    .transpose()?;
                let prior_settings = prior.as_ref().map(PrivateManifest::reminder_settings);
                let prior_dictionary = prior.as_ref().map(PrivateManifest::dictionary);
                let doc = match &before {
                    Some(state) => {
                        PrivateManifest::from_snapshot(&state.snapshot)?.staged_import(updates)?
                    }
                    None => PrivateManifest::from_snapshot(&complete_unit_snapshot(updates)?)?,
                };
                if doc.unit() != unit {
                    return Err(Error::bare(Code::ProfileUnitScopeMismatch));
                }
                if doc.is_read_only() {
                    return Err(Error::bare(Code::SchemaTooNew));
                }
                doc.set_peer_id(self.peer_id)?;
                let changed = prior_settings.as_ref() != Some(&doc.reminder_settings());
                self.catalog
                    .commit_profile_unit(unit, &doc.export_snapshot()?)?;
                self.private_profiles.insert(profile, doc);
                if changed {
                    self.record_reminder_change(profile, None, ReminderChange::Settings);
                }
                if prior_dictionary.unwrap_or_default()
                    != self.private_profiles[&profile].dictionary()
                {
                    self.record_dictionary_change(profile);
                }
            }
            ProfileUnitRef::Personal { workspace, .. } => {
                self.require_profile(profile)?;
                if self.catalog.workspace(workspace)?.is_none() {
                    return Err(Error::bare(Code::WorkspaceNotFound));
                }
                let prior = before
                    .as_ref()
                    .map(|s| PersonalManifest::from_snapshot(&s.snapshot))
                    .transpose()?;
                let doc = match &prior {
                    Some(doc) => doc.staged_import(updates)?,
                    None => PersonalManifest::from_snapshot(&complete_unit_snapshot(updates)?)?,
                };
                if doc.unit() != unit {
                    return Err(Error::bare(Code::ProfileUnitScopeMismatch));
                }
                if doc.is_read_only() {
                    return Err(Error::bare(Code::SchemaTooNew));
                }
                let old_rows = prior
                    .as_ref()
                    .map(|d| d.reminders())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|r| (r.rule.id, r))
                    .collect();
                let new_rows = doc
                    .reminders()
                    .into_iter()
                    .map(|r| (r.rule.id, r))
                    .collect();
                let old_actions = prior
                    .as_ref()
                    .map(|d| d.raw_reminder_actions())
                    .unwrap_or_default();
                let old_cleared = prior
                    .as_ref()
                    .map(|d| d.cleared_reminder_actions())
                    .unwrap_or_default();
                let changes = classify_reminder_changes(
                    &old_rows,
                    &new_rows,
                    &old_actions,
                    &doc.raw_reminder_actions(),
                    &old_cleared,
                    &doc.cleared_reminder_actions(),
                    &ReminderSettings::default(),
                    &ReminderSettings::default(),
                );
                self.catalog
                    .commit_profile_unit(unit, &doc.export_snapshot()?)?;
                for (reminder, change) in changes {
                    self.record_reminder_change(profile, reminder, change);
                }
            }
            ProfileUnitRef::LegacyPrivate { .. } => {
                self.require_profile(profile)?;
                let doc = loro::LoroDoc::new();
                if let Some(before) = before {
                    doc.import(&before.snapshot)
                        .map_err(|_| Error::bare(Code::CorruptState))?;
                }
                for bytes in updates {
                    if doc
                        .import(bytes)
                        .map_err(|_| Error::bare(Code::CrdtRejected))?
                        .pending
                        .is_some()
                    {
                        return Err(Error::bare(Code::CrdtRejected));
                    }
                }
                let bytes = doc
                    .export(loro::ExportMode::snapshot())
                    .map_err(|_| Error::bare(Code::StorageFailed))?;
                if ProfileManifest::from_snapshot(&bytes)?.profile()?.id != profile {
                    return Err(Error::bare(Code::ProfileUnitScopeMismatch));
                }
                // No legacy field projects or wakes the fresh authorities.
                self.catalog.commit_profile_unit(unit, &bytes)?;
            }
        }
        Ok(())
    }
}

fn complete_unit_snapshot(updates: &[Vec<u8>]) -> Result<Vec<u8>> {
    let doc = loro::LoroDoc::new();
    for bytes in updates {
        if doc
            .import(bytes)
            .map_err(|_| Error::bare(Code::CrdtRejected))?
            .pending
            .is_some()
        {
            return Err(Error::bare(Code::CrdtRejected));
        }
    }
    doc.export(loro::ExportMode::snapshot())
        .map_err(|_| Error::bare(Code::StorageFailed))
}

fn unit_version(unit: ProfileUnitRef, snapshot: &[u8]) -> Result<loro::VersionVector> {
    match unit {
        ProfileUnitRef::Identity { .. } => {
            let doc = IdentityManifest::from_snapshot(snapshot)?;
            if doc.unit() != unit {
                return Err(Error::bare(Code::ProfileUnitScopeMismatch));
            }
            Ok(doc.version())
        }
        ProfileUnitRef::Private { .. } => {
            let doc = PrivateManifest::from_snapshot(snapshot)?;
            if doc.unit() != unit {
                return Err(Error::bare(Code::ProfileUnitScopeMismatch));
            }
            Ok(doc.version())
        }
        ProfileUnitRef::Personal { .. } => {
            let doc = PersonalManifest::from_snapshot(snapshot)?;
            if doc.unit() != unit {
                return Err(Error::bare(Code::ProfileUnitScopeMismatch));
            }
            Ok(doc.version())
        }
        ProfileUnitRef::LegacyPrivate { profile } => {
            let doc = ProfileManifest::from_snapshot(snapshot)?;
            if doc.profile()?.id != profile {
                return Err(Error::bare(Code::ProfileUnitScopeMismatch));
            }
            Ok(doc.version())
        }
    }
}

/// Decode a bounded Profile-unit snapshot off the Store's answering thread.
pub fn read_personal_reminder_inputs(
    source: &ProfileUnitState,
) -> Result<Vec<ReminderReductionInput>> {
    let doc = PersonalManifest::from_snapshot(&source.snapshot)?;
    if doc.unit() != source.unit {
        return Err(Error::bare(Code::ProfileUnitScopeMismatch));
    }
    if doc.is_read_only() {
        return Err(Error::bare(Code::SchemaTooNew));
    }
    Ok(doc.reminder_reduction_inputs())
}

/// Decode a bounded Profile-unit snapshot off the Store's answering thread.
/// The index supplies lookup hints only; target/timezone-dependent reduction
/// remains with the existing reconciliation service.
pub fn build_reminder_index(source: &ProfileUnitState) -> Result<(Vec<u8>, Vec<ReminderDue>)> {
    let ProfileUnitRef::Personal { owner, workspace } = source.unit else {
        return Err(Error::bare(Code::ProfileUnitScopeMismatch));
    };
    let doc = PersonalManifest::from_snapshot(&source.snapshot)?;
    if doc.owner() != owner || doc.workspace() != workspace {
        return Err(Error::bare(Code::ProfileUnitScopeMismatch));
    }
    if doc.is_read_only() {
        return Err(Error::bare(Code::SchemaTooNew));
    }
    let rows = doc
        .reminder_reduction_inputs()
        .into_iter()
        .map(|row| {
            use crate::reminders::{EffectiveState, OccurrenceMoment, TargetState};
            let due_at_ms = match row.reduce(TargetState::Active, None).effective {
                EffectiveState::Scheduled {
                    moment: OccurrenceMoment::Zoned { value },
                    ..
                } => Some(value.instant_ms()),
                EffectiveState::Snoozed { until_ms, .. } => Some(until_ms),
                _ => None,
            };
            ReminderDue {
                owner,
                workspace,
                reminder: row.stored().rule.id,
                due_at_ms,
            }
        })
        .collect();
    Ok((doc.version().encode(), rows))
}
