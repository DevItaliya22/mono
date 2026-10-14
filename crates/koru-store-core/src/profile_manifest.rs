// SPDX-License-Identifier: MPL-2.0

//! The small CRDT backing shared profile metadata.
//!
//! A profile is addressed by `ProfileId`; it is not an ordinary workspace
//! document and therefore needs no second `DocumentId`. The catalog row is a
//! disposable projection used for listing, while this Loro map is the
//! replicated authority.

use loro::{ExportMode, LoroDoc, LoroValue, ValueOrContainer};
#[cfg(test)]
use std::collections::BTreeSet;

use crate::errors::{Code, Error, Result};
use crate::ids::{ActionId, ProfileId, ReminderId};
use crate::model::{Profile, ProfileKind, Timestamp};
use crate::personal_attributes::{EntityRef, PersonalAttributeValue, PersonalAttributes};
#[cfg(test)]
use crate::profile_units::content::{
    C_PERSONAL_ATTRIBUTES, C_REMINDER_ACTIONS, C_REMINDER_RULES, C_REMINDERS,
};
#[cfg(test)]
use crate::reminders::ReminderReductionInput;
use crate::reminders::{OccurrenceAction, ReminderRule, ReminderSettings, StoredReminder};
use crate::text::Locale;
use crate::version::PROFILE_CRDT_SCHEMA_VERSION;

const C_META: &str = "meta";
const K_ID: &str = "profile_id";
const K_NAME: &str = "name";
const K_PURPOSE: &str = "purpose";
const K_AVATAR: &str = "avatar";
const K_ICON: &str = "icon";
const K_COLOR: &str = "color";
const K_ACCOUNT: &str = "account";
const K_LOCALE: &str = "locale";
const K_CREATED: &str = "created_at";
/// Persona or legacy holder compatibility marker.
///
/// New Profiles always write `normal`. The replicated field remains so a
/// legacy holder arriving from an all-data peer cannot surface as a persona.
const K_KIND: &str = "kind";
const K_SCHEMA: &str = "schema_version";

pub struct ProfileManifest {
    doc: LoroDoc,
}

impl ProfileManifest {
    /// The underlying document, for this crate's own tests — which need to
    /// plant shapes a future build might write.
    #[cfg(test)]
    pub(crate) fn doc(&self) -> &LoroDoc {
        &self.doc
    }

    pub fn empty() -> Self {
        Self {
            doc: LoroDoc::new(),
        }
    }

    pub fn create(profile: &Profile) -> Result<Self> {
        let doc = LoroDoc::new();
        let manifest = Self { doc };
        let meta = manifest.doc.get_map(C_META);
        meta.insert(K_ID, profile.id.to_storage().as_str())
            .map_err(loro_failed)?;
        meta.insert(K_NAME, profile.name.as_str())
            .map_err(loro_failed)?;
        set_optional(&meta, K_PURPOSE, profile.purpose.as_deref())?;
        set_optional(&meta, K_AVATAR, profile.avatar.as_deref())?;
        set_optional(&meta, K_ICON, profile.icon.as_deref())?;
        set_optional(&meta, K_COLOR, profile.color.as_deref())?;
        set_optional(&meta, K_ACCOUNT, profile.account.as_deref())?;
        meta.insert(K_LOCALE, profile.locale.as_str())
            .map_err(loro_failed)?;
        meta.insert(K_CREATED, profile.created_at.millis())
            .map_err(loro_failed)?;
        meta.insert(K_KIND, profile.kind.as_str())
            .map_err(loro_failed)?;
        meta.insert(K_SCHEMA, PROFILE_CRDT_SCHEMA_VERSION)
            .map_err(loro_failed)?;
        manifest.doc.commit();
        Ok(manifest)
    }

    /// Rename the profile — one map write, last-writer-wins, the same
    /// semantics as a workspace name: a merged name is gibberish in both
    /// languages, where last-writer-wins is what a name wants.
    pub fn set_name(&self, name: &str) -> Result<()> {
        let meta = self.doc.get_map(C_META);
        meta.insert(K_NAME, name).map_err(loro_failed)?;
        self.doc.commit();
        Ok(())
    }

    pub fn set_appearance(&self, icon: Option<&str>, color: Option<&str>) -> Result<()> {
        let meta = self.doc.get_map(C_META);
        set_optional(&meta, K_ICON, icon)?;
        set_optional(&meta, K_COLOR, color)?;
        self.doc.commit();
        Ok(())
    }

    /// Replace the existing stable `account` storage key with one already-
    /// validated `koru:<identity-uuid>` binding.
    pub fn set_account(&self, account: &str) -> Result<()> {
        let meta = self.doc.get_map(C_META);
        meta.insert(K_ACCOUNT, account).map_err(loro_failed)?;
        self.doc.commit();
        Ok(())
    }

    /// Remove the account binding while retaining the Profile and all of its
    /// local content. Used only after the hosted lifecycle proves the Profile
    /// is deleted and the person explicitly chooses to continue offline.
    pub fn clear_account(&self) -> Result<()> {
        let meta = self.doc.get_map(C_META);
        if meta.get(K_ACCOUNT).is_some() {
            meta.delete(K_ACCOUNT).map_err(loro_failed)?;
            self.doc.commit();
        }
        Ok(())
    }

    pub fn from_snapshot(bytes: &[u8]) -> Result<Self> {
        let doc = LoroDoc::new();
        doc.import(bytes).map_err(|error| {
            Error::new(Code::CorruptState, format!("profile snapshot: {error}"))
        })?;
        Ok(Self { doc })
    }

    pub fn set_peer_id(&self, peer: u64) -> Result<()> {
        self.doc
            .set_peer_id(peer)
            .map_err(|error| Error::new(Code::StorageFailed, format!("set_peer_id: {error}")))
    }

    pub fn export_snapshot(&self) -> Result<Vec<u8>> {
        self.doc
            .export(ExportMode::snapshot())
            .map_err(|error| Error::new(Code::StorageFailed, format!("profile snapshot: {error}")))
    }

    pub fn export_updates_since(&self, from: &loro::VersionVector) -> Result<Vec<u8>> {
        self.doc
            .export(ExportMode::updates(from))
            .map_err(|error| Error::new(Code::StorageFailed, format!("profile updates: {error}")))
    }

    pub fn version(&self) -> loro::VersionVector {
        self.doc.oplog_vv()
    }

    pub fn import_frames(&self, frames: &[Vec<u8>]) -> Result<()> {
        for (index, frame) in frames.iter().enumerate() {
            self.doc.import(frame).map_err(|error| {
                Error::new(
                    Code::CrdtRejected,
                    format!("profile frame {index}: {error}"),
                )
            })?;
        }
        Ok(())
    }

    // ------------------------------------------------- personal attributes

    /// Set one personal attribute on one entity. The value is validated, then
    /// written as a single atomic register — a concurrent write to the same
    /// key resolves to one whole value, never a splice of both.
    pub fn set_personal_attribute(
        &self,
        entity: &EntityRef,
        key: &str,
        value: &PersonalAttributeValue,
    ) -> Result<()> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .set_personal_attribute(entity, key, value)
    }

    /// Clear one personal attribute. Clearing deletes the key — absent is the
    /// stored spelling of "unset", never a stored null.
    ///
    /// Answers whether anything was actually removed, and is guarded on
    /// presence: a delete of an absent key is still an operation, and this
    /// call must not create the entity's map just to delete nothing from it.
    pub fn clear_personal_attribute(&self, entity: &EntityRef, key: &str) -> Result<bool> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .clear_personal_attribute(entity, key)
    }

    /// Every personal attribute one entity carries. A pure read: it creates
    /// no container, and an entity nothing was filed under answers an empty
    /// map. An entry whose stored shape this build does not recognise is
    /// skipped — absent, never an error.
    pub fn personal_attributes(&self, entity: &EntityRef) -> PersonalAttributes {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.personal_attributes(entity)
    }

    // ---------------------------------------------------------- reminders

    /// Create or replace one whole rule revision. The rule is one register so
    /// concurrent edits resolve to one complete value, never field-by-field.
    /// A deletion tombstone is independent and cannot be cleared here.
    pub fn put_reminder_rule(&self, rule: &ReminderRule) -> Result<()> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.put_reminder_rule(rule)
    }

    /// Record the grow-only delete fact. Repeating it is idempotent; no method
    /// removes this key, so a concurrent rule edit cannot resurrect the id.
    pub fn delete_reminder(&self, id: ReminderId) -> Result<()> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.delete_reminder(id)
    }

    /// Tombstone every live rule for one authoritatively removed entity.
    /// Callers must establish removal; projection `Missing` is intentionally
    /// insufficient because a cold or stale projection is not deletion truth.
    pub fn delete_reminders_for_target(&self, target: &EntityRef) -> Result<Vec<ReminderId>> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .delete_reminders_for_target(target)
    }

    /// Append one action below the reminder identity. An action id names one
    /// atomic value; retrying the same request writes the same map key.
    pub fn append_reminder_action(
        &self,
        reminder: ReminderId,
        action: &OccurrenceAction,
    ) -> Result<()> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .append_reminder_action(reminder, action)
    }

    // ------------------------------------------------- personal dictionary

    /// Add one word to the personal spelling dictionary for one language.
    /// Idempotent: adding a word twice stores it once, and two devices each
    /// adding their first word merge rather than one winning.
    pub fn add_dictionary_word(&self, language: &str, word: &str) -> Result<()> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .add_dictionary_word(language, word)
    }

    /// Hide every handled action with a grow-only clear fact. Raw actions stay
    /// append-only, so a delayed replica or retry cannot resurrect history;
    /// snoozes and unhandled occurrences remain visible exactly as stored.
    pub fn clear_handled_reminder_history(&self) -> Result<usize> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .clear_handled_reminder_history()
    }

    /// Hide handled actions belonging to reminders for exactly one entity.
    ///
    /// This uses the same grow-only facts as profile-wide clearing. Target
    /// scope chooses which existing action ids receive facts; it does not add
    /// a second history model or make a late replica able to resurrect one.
    pub fn clear_handled_reminder_history_for_target(&self, target: &EntityRef) -> Result<usize> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .clear_handled_reminder_history_for_target(target)
    }

    pub fn update_reminder_settings(
        &self,
        end_of_day: Option<Option<crate::reminders::LocalTime>>,
        default_tone: Option<crate::reminders::ToneId>,
        week_start: Option<crate::reminders::WeekStartPreference>,
    ) -> Result<bool> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.update_reminder_settings(
            end_of_day,
            default_tone,
            week_start,
        )
    }

    /// Remove one word. Absent is the stored spelling of "not in the
    /// dictionary", so removal deletes the key; answers whether anything was
    /// actually there, and a remove of an absent word creates nothing.
    pub fn remove_dictionary_word(&self, language: &str, word: &str) -> Result<bool> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .remove_dictionary_word(language, word)
    }

    pub fn reminder_settings(&self) -> ReminderSettings {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.reminder_settings()
    }

    pub fn reminder(&self, id: ReminderId) -> Option<StoredReminder> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.reminder(id)
    }

    /// Lookup for idempotency checks, including actions hidden by Clear all.
    pub fn raw_reminder_action(
        &self,
        reminder: ReminderId,
        action: ActionId,
    ) -> Option<OccurrenceAction> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }
            .raw_reminder_action(reminder, action)
    }

    /// Grow-only Clear all facts, separate from raw actions so remote imports
    /// can emit a history wake rather than misreporting an action append.
    #[cfg(test)]
    pub(crate) fn cleared_reminder_actions(&self) -> BTreeSet<(ReminderId, ActionId)> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.cleared_reminder_actions()
    }

    /// Every recognized reminder, ordered by its opaque identity. Unknown
    /// future values are skipped without touching their bytes.
    pub fn reminders(&self) -> Vec<StoredReminder> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.reminders()
    }

    /// Visible reminder records plus the content-free cleared handled facts
    /// needed for effective-state reduction. This is an in-process boundary;
    /// the additional facts never enter the public serialized record.
    #[cfg(test)]
    pub(crate) fn reminder_reduction_inputs(&self) -> Vec<ReminderReductionInput> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.reminder_reduction_inputs()
    }

    /// The whole dictionary: language tag → sorted words. A pure read; an
    /// empty dictionary answers an empty map, and an entry whose shape this
    /// build does not recognise is skipped rather than an error.
    pub fn dictionary(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        crate::profile_units::content::ProfileContent { doc: &self.doc }.dictionary()
    }

    pub fn profile(&self) -> Result<Profile> {
        let meta = self.doc.get_map(C_META);
        let id = string(&meta.get(K_ID)).and_then(|value| ProfileId::parse(&value));
        let name = string(&meta.get(K_NAME));
        let locale = string(&meta.get(K_LOCALE)).and_then(|value| Locale::parse(&value));
        let created_at = integer(&meta.get(K_CREATED)).map(Timestamp::from_millis);
        let avatar = optional_string(&meta.get(K_AVATAR));
        Ok(Profile {
            id: id.ok_or_else(|| Error::new(Code::CorruptState, "profile CRDT has no id"))?,
            name: name.ok_or_else(|| Error::new(Code::CorruptState, "profile CRDT has no name"))?,
            purpose: optional_string(&meta.get(K_PURPOSE)),
            avatar,
            icon: optional_string(&meta.get(K_ICON)),
            color: optional_string(&meta.get(K_COLOR)),
            account: optional_string(&meta.get(K_ACCOUNT)),
            locale: locale
                .ok_or_else(|| Error::new(Code::CorruptState, "profile CRDT has no locale"))?,
            created_at: created_at.ok_or_else(|| {
                Error::new(Code::CorruptState, "profile CRDT has no creation time")
            })?,
            // **Absent is a persona, and that is not a guess.** Every profile
            // written before schema 3 is one, and an unreadable token is read
            // conservatively for `ProfileKind::parse`'s reason: the kind that is
            // safe to show and safe to leave alone.
            kind: optional_string(&meta.get(K_KIND))
                .map(|value| ProfileKind::parse(&value))
                .unwrap_or_default(),
        })
    }
}

fn set_optional(map: &loro::LoroMap, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(value) => map.insert(key, value).map_err(loro_failed)?,
        None => map.insert(key, LoroValue::Null).map_err(loro_failed)?,
    }
    Ok(())
}

fn string(value: &Option<ValueOrContainer>) -> Option<String> {
    match value.as_ref()? {
        ValueOrContainer::Value(LoroValue::String(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn optional_string(value: &Option<ValueOrContainer>) -> Option<String> {
    string(value)
}

fn integer(value: &Option<ValueOrContainer>) -> Option<i64> {
    match value.as_ref()? {
        ValueOrContainer::Value(LoroValue::I64(value)) => Some(*value),
        _ => None,
    }
}

fn loro_failed(error: loro::LoroError) -> Error {
    Error::new(Code::StorageFailed, error.to_string())
}

#[cfg(test)]
mod dictionary_tests;
#[cfg(test)]
mod tests;
