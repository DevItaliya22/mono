// SPDX-License-Identifier: MPL-2.0

//! Design-neutral state for reminder preferences and delivery prerequisites.
//!
//! The final reminder surfaces are deliberately not chosen here.  This module
//! owns the values and effects those surfaces consume: synchronized profile
//! settings travel through the Store, notification controls travel through the
//! Agent, and start-on-login consent travels to the shell's sole
//! `DesktopPreferences` writer.  A renderer can therefore be changed without
//! creating a second persistence or native-notification path.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc;

use koru_agent_wire::{
    NotificationAuthorization, NotificationControlResult, NotificationFailure,
    NotificationOperation, NotificationTone, ReminderDiagnostics,
};
use koru_store_core::ids::ProfileId;
use koru_store_core::reminders::{LocalTime, ReminderSettings, ToneId, WeekStartPreference};
use koru_store_wire::reminders::SettingsPatch;
use koru_workspace_host::{
    ReminderStartupRequest, ReminderStartupRequestId, ReminderStartupResult,
};

/// The machine-local startup facts the shell owns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupFacts {
    pub start_on_login: bool,
    pub reminder_explained: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsState {
    Unavailable,
    Loading,
    Ready(ReminderSettings),
    Saving(ReminderSettings),
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupState {
    Unknown,
    NotNeeded,
    NeedsExplanation,
    Saving {
        request_id: ReminderStartupRequestId,
        accepted: bool,
    },
    Declined,
    Failed {
        accepted: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HealthState {
    Unknown,
    Ready(ReminderDiagnostics),
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DeliveryWarning {
    AgentUnavailable,
    NotificationsNotAllowed,
    NativeDeliveryUnavailable,
    CustomToneUnavailable,
    StartOnLoginDeclined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HealthAction {
    Refresh,
    Repair,
    OpenNotificationSettings,
    EnableDelivery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HealthPresentation {
    pub footer: String,
    pub banner: Option<String>,
    pub action_label: Option<String>,
    pub action: Option<HealthAction>,
}

/// Design-independent intents the eventual Settings controls emit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    SetEndOfDay(Option<LocalTime>),
    SetDefaultTone(ToneId),
    SetWeekStart(WeekStartPreference),
    RetrySettings,
    RefreshNotificationStatus,
    AllowNotifications,
    OpenNotificationSettings,
    SetDeliveryEnabled(bool),
    RepairDelivery,
    SendTest,
    AnswerStartup(bool),
    /// Play a tone once, here, without scheduling anything.
    PreviewTone(ToneId),
}

/// Work crosses one of the three already-owned boundaries.  It is a value so
/// tests can prove a gesture does not accidentally write reminder data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    LoadSettings {
        profile: ProfileId,
        generation: u64,
    },
    PatchSettings {
        profile: ProfileId,
        generation: u64,
        patch: SettingsPatch,
    },
    Notification {
        id: u64,
        operation: NotificationOperation,
    },
    ReminderStatus {
        id: u64,
    },
    SaveStartupConsent(ReminderStartupRequest),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NotificationPurpose {
    Inspect,
    Authorization,
    OpenSettings,
    SetDeliveryEnabled { enabled: bool },
    Repair,
    Test { requested: ToneId, fallback: bool },
}

/// One Settings tab's state.  Answers are generation-stamped so a late result
/// from profile A cannot overwrite profile B.
#[derive(Clone, Debug)]
pub(crate) struct ReminderPreferences {
    profile: Option<ProfileId>,
    generation: u64,
    pub settings: SettingsState,
    /// Changes made after the current Store write began. They are folded into
    /// one follow-up patch when that write answers.
    pending_settings: SettingsPatch,
    /// Fields currently being written, kept separately so recovery never
    /// overwrites an unrelated authoritative field.
    in_flight_settings: SettingsPatch,
    /// Authored fields retained across a failed write/reload.
    settings_retry: Option<SettingsPatch>,
    pub startup: StartupState,
    pub health: HealthState,
    startup_facts: StartupFacts,
    has_reminder: bool,
    authorization: NotificationAuthorization,
    delivery_supported: bool,
    custom_sound: bool,
    agent_connected: bool,
    next_notification: u64,
    next_health: u64,
    latest_health: u64,
    pending_notifications: BTreeMap<u64, NotificationPurpose>,
    /// The latest local delivery intent. It is cleared only by correlated
    /// diagnostics confirming it, or by a final failed native operation.
    delivery_desired: Option<bool>,
    warnings: BTreeSet<DeliveryWarning>,
}

impl Default for ReminderPreferences {
    fn default() -> Self {
        Self {
            profile: None,
            generation: 0,
            settings: SettingsState::Unavailable,
            pending_settings: SettingsPatch::default(),
            in_flight_settings: SettingsPatch::default(),
            settings_retry: None,
            startup: StartupState::Unknown,
            health: HealthState::Unknown,
            startup_facts: StartupFacts::default(),
            has_reminder: false,
            authorization: NotificationAuthorization::Unavailable,
            delivery_supported: false,
            custom_sound: false,
            agent_connected: false,
            next_notification: 1,
            next_health: 1,
            latest_health: 0,
            pending_notifications: BTreeMap::new(),
            delivery_desired: None,
            warnings: BTreeSet::new(),
        }
    }
}

impl ReminderPreferences {
    pub fn profile(&self) -> Option<ProfileId> {
        self.profile
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn desired_delivery_enabled(&self) -> Option<bool> {
        self.delivery_desired
    }

    pub fn select_profile(&mut self, profile: ProfileId) -> Vec<Effect> {
        if self.profile == Some(profile) {
            return Vec::new();
        }
        self.profile = Some(profile);
        self.generation = self.generation.wrapping_add(1);
        self.settings = SettingsState::Loading;
        self.pending_settings = SettingsPatch::default();
        self.in_flight_settings = SettingsPatch::default();
        self.settings_retry = None;
        self.pending_notifications.clear();
        self.health = HealthState::Unknown;
        let mut effects = vec![Effect::LoadSettings {
            profile,
            generation: self.generation,
        }];
        effects.push(
            self.notification_effect(NotificationPurpose::Inspect, NotificationOperation::Inspect),
        );
        effects.push(self.health_effect());
        effects
    }

    #[cfg(test)]
    pub fn settings_loaded(
        &mut self,
        profile: ProfileId,
        generation: u64,
        result: Result<ReminderSettings, ()>,
    ) -> bool {
        self.settings_loaded_with_follow_up(profile, generation, result)
            .0
    }

    fn settings_loaded_with_follow_up(
        &mut self,
        profile: ProfileId,
        generation: u64,
        result: Result<ReminderSettings, ()>,
    ) -> (bool, Vec<Effect>) {
        if self.profile != Some(profile) || self.generation != generation {
            return (false, Vec::new());
        }
        let mut effects = Vec::new();
        match result {
            Ok(authoritative) => {
                if let Some(patch) = self.settings_retry.take() {
                    let desired = apply_settings_patch(&authoritative, &patch);
                    if desired == authoritative {
                        self.settings = SettingsState::Ready(authoritative);
                    } else {
                        self.settings = SettingsState::Saving(desired);
                        self.in_flight_settings = patch.clone();
                        effects.push(Effect::PatchSettings {
                            profile,
                            generation,
                            patch,
                        });
                    }
                } else if matches!(&self.settings, SettingsState::Saving(_)) {
                    // A remote refresh can finish while local authored fields
                    // are still queued or in flight. Fold those fields over
                    // the authoritative snapshot so the refresh incorporates
                    // the peer's independent changes without flashing back or
                    // discarding the local intent.
                    let mut authored = self.in_flight_settings.clone();
                    merge_settings_patch(&mut authored, self.pending_settings.clone());
                    self.settings =
                        SettingsState::Saving(apply_settings_patch(&authoritative, &authored));
                } else {
                    self.settings = SettingsState::Ready(authoritative);
                }
            }
            Err(()) => self.settings = SettingsState::Failed,
        }
        self.refresh_warnings();
        (true, effects)
    }

    pub fn set_end_of_day(&mut self, value: Option<LocalTime>) -> Vec<Effect> {
        self.patch(SettingsPatch {
            end_of_day: Some(value),
            default_tone: None,
            week_start: None,
        })
    }

    pub fn set_default_tone(&mut self, value: ToneId) -> Vec<Effect> {
        self.patch(SettingsPatch {
            end_of_day: None,
            default_tone: Some(value),
            week_start: None,
        })
    }

    pub fn set_week_start(&mut self, value: WeekStartPreference) -> Vec<Effect> {
        self.patch(SettingsPatch {
            end_of_day: None,
            default_tone: None,
            week_start: Some(value),
        })
    }

    fn patch(&mut self, patch: SettingsPatch) -> Vec<Effect> {
        let Some(profile) = self.profile else {
            return Vec::new();
        };
        match &self.settings {
            SettingsState::Ready(current) => {
                let next = apply_settings_patch(current, &patch);
                if &next == current {
                    return Vec::new();
                }
                self.pending_settings = SettingsPatch::default();
                self.in_flight_settings = patch.clone();
                self.settings = SettingsState::Saving(next);
                self.refresh_warnings();
                vec![Effect::PatchSettings {
                    profile,
                    generation: self.generation,
                    patch,
                }]
            }
            SettingsState::Saving(current) => {
                let next = apply_settings_patch(current, &patch);
                if &next == current {
                    return Vec::new();
                }
                merge_settings_patch(&mut self.pending_settings, patch);
                self.settings = SettingsState::Saving(next);
                self.refresh_warnings();
                Vec::new()
            }
            SettingsState::Unavailable | SettingsState::Loading | SettingsState::Failed => {
                Vec::new()
            }
        }
    }

    #[cfg(test)]
    pub fn settings_saved(
        &mut self,
        profile: ProfileId,
        generation: u64,
        result: Result<ReminderSettings, ()>,
    ) -> bool {
        self.settings_saved_with_follow_up(profile, generation, result)
            .0
    }

    fn settings_saved_with_follow_up(
        &mut self,
        profile: ProfileId,
        generation: u64,
        result: Result<ReminderSettings, ()>,
    ) -> (bool, Vec<Effect>) {
        if self.profile != Some(profile) || self.generation != generation {
            return (false, Vec::new());
        }
        match result {
            Ok(authoritative) => {
                let patch = std::mem::take(&mut self.pending_settings);
                if patch == SettingsPatch::default() {
                    self.in_flight_settings = SettingsPatch::default();
                    self.settings_retry = None;
                    self.settings = SettingsState::Ready(authoritative);
                    self.refresh_warnings();
                    (true, Vec::new())
                } else {
                    let next = apply_settings_patch(&authoritative, &patch);
                    if next == authoritative {
                        self.in_flight_settings = SettingsPatch::default();
                        self.settings_retry = None;
                        self.settings = SettingsState::Ready(authoritative);
                        self.refresh_warnings();
                        return (true, Vec::new());
                    }
                    self.in_flight_settings = patch.clone();
                    self.settings = SettingsState::Saving(next);
                    self.refresh_warnings();
                    (
                        true,
                        vec![Effect::PatchSettings {
                            profile,
                            generation,
                            patch,
                        }],
                    )
                }
            }
            Err(()) => {
                let mut retry = std::mem::take(&mut self.in_flight_settings);
                merge_settings_patch(&mut retry, std::mem::take(&mut self.pending_settings));
                self.settings_retry = Some(retry);
                self.settings = SettingsState::Failed;
                self.refresh_warnings();
                (true, Vec::new())
            }
        }
    }

    pub fn retry_settings(&mut self) -> Vec<Effect> {
        let (Some(profile), SettingsState::Failed) = (self.profile, &self.settings) else {
            return Vec::new();
        };
        self.settings = SettingsState::Loading;
        vec![Effect::LoadSettings {
            profile,
            generation: self.generation,
        }]
    }

    /// Update the shell-owned startup reading and whether a synchronized
    /// reminder exists.  Merely observing it never changes the reminder.
    pub fn observe_reminders(&mut self, any: bool, facts: StartupFacts) -> bool {
        let before = self.startup;
        self.has_reminder = any;
        self.startup_facts = facts;
        if !matches!(self.startup, StartupState::Saving { .. }) {
            self.startup = if !any || facts.start_on_login {
                StartupState::NotNeeded
            } else if !facts.reminder_explained {
                StartupState::NeedsExplanation
            } else {
                StartupState::Declined
            };
        }
        self.refresh_warnings();
        self.startup != before
    }

    /// Answer the one-time explanation.  Accept and decline both go back to
    /// the shell: the former configures the OS, while the latter only records
    /// that the explanation was answered.  Neither touches reminder activity.
    pub fn answer_startup(&mut self, accepted: bool) -> Vec<Effect> {
        if !matches!(
            self.startup,
            StartupState::NeedsExplanation | StartupState::Failed { .. }
        ) {
            return Vec::new();
        }
        let request = ReminderStartupRequest::new(accepted);
        self.startup = StartupState::Saving {
            request_id: request.id,
            accepted,
        };
        vec![Effect::SaveStartupConsent(request)]
    }

    pub fn startup_saved(
        &mut self,
        request_id: ReminderStartupRequestId,
        result: Result<StartupFacts, StartupFacts>,
    ) -> bool {
        let accepted = match self.startup {
            StartupState::Saving {
                request_id: pending,
                accepted,
            } if pending == request_id => accepted,
            _ => return false,
        };
        match result {
            Ok(facts) => {
                self.startup_facts = facts;
                self.startup = if accepted && facts.start_on_login {
                    StartupState::NotNeeded
                } else {
                    StartupState::Declined
                };
            }
            Err(facts) => {
                // Persistence may have failed after the OS change and its
                // rollback may also have failed. Keep warnings tied to the
                // actual state reported by the shell, never the hoped-for one.
                self.startup_facts = facts;
                self.startup = StartupState::Failed { accepted };
            }
        }
        self.refresh_warnings();
        true
    }

    pub fn set_agent_connected(&mut self, connected: bool) -> bool {
        if self.agent_connected == connected {
            return false;
        }
        self.agent_connected = connected;
        if !connected {
            self.health = HealthState::Unavailable;
        }
        self.refresh_warnings();
        true
    }

    /// Explicit status refresh. Authorization is not requested here: Q-P5-1
    /// permits only the dedicated Allow notifications intent to prompt.
    pub fn refresh_notification_status(&mut self) -> Vec<Effect> {
        vec![
            self.notification_effect(NotificationPurpose::Inspect, NotificationOperation::Inspect),
            self.health_effect(),
        ]
    }

    /// The dedicated user gesture which may show the operating-system prompt.
    pub fn allow_notifications(&mut self) -> Vec<Effect> {
        vec![self.notification_effect(
            NotificationPurpose::Authorization,
            NotificationOperation::RequestAuthorization,
        )]
    }

    pub fn open_notification_settings(&mut self) -> Vec<Effect> {
        vec![self.notification_effect(
            NotificationPurpose::OpenSettings,
            NotificationOperation::OpenSettings,
        )]
    }

    pub fn set_delivery_enabled(&mut self, enabled: bool) -> Vec<Effect> {
        self.delivery_desired = Some(enabled);
        if self
            .pending_notifications
            .values()
            .any(|purpose| matches!(purpose, NotificationPurpose::SetDeliveryEnabled { .. }))
        {
            return Vec::new();
        }
        vec![self.notification_effect(
            NotificationPurpose::SetDeliveryEnabled { enabled },
            NotificationOperation::SetDeliveryEnabled { enabled },
        )]
    }

    pub fn repair_delivery(&mut self) -> Vec<Effect> {
        vec![self.notification_effect(NotificationPurpose::Repair, NotificationOperation::Repair)]
    }

    pub fn send_test(&mut self, lang: koru_i18n::Lang) -> Vec<Effect> {
        let requested = match &self.settings {
            SettingsState::Ready(settings) | SettingsState::Saving(settings) => {
                settings.default_tone
            }
            _ => ToneId::System,
        };
        let fallback = requested != ToneId::System && !self.custom_sound;
        let tone = if fallback { ToneId::System } else { requested };
        self.refresh_warnings();
        vec![self.notification_effect(
            NotificationPurpose::Test {
                requested,
                fallback,
            },
            NotificationOperation::SendTest {
                tone: notification_tone(tone),
                title: koru_i18n::t(lang, koru_i18n::Key::ReminderTestTitle).to_owned(),
                body: Some(koru_i18n::t(lang, koru_i18n::Key::ReminderTestBody).to_owned()),
            },
        )]
    }

    pub fn notification_not_queued(&mut self, id: u64) -> bool {
        let removed = self.pending_notifications.remove(&id).is_some();
        self.set_agent_connected(false);
        removed
    }

    /// Apply the typed Agent response.  An absent bundled asset retries once
    /// with system sound without rewriting the synchronized preference.
    pub fn notification_result(
        &mut self,
        id: u64,
        result: NotificationControlResult,
        lang: koru_i18n::Lang,
    ) -> Vec<Effect> {
        let Some(purpose) = self.pending_notifications.remove(&id) else {
            return Vec::new();
        };
        let succeeded = matches!(result, NotificationControlResult::Done);
        match result {
            NotificationControlResult::Capability {
                authorization,
                actions,
                stable_replacement,
                withdrawal,
                custom_sound,
                activation_while_stopped,
            } => {
                self.authorization = authorization;
                self.custom_sound = custom_sound;
                self.delivery_supported =
                    actions && stable_replacement && withdrawal && activation_while_stopped;
            }
            NotificationControlResult::Authorization { authorization } => {
                self.authorization = authorization;
            }
            NotificationControlResult::Failed {
                code: NotificationFailure::AssetMissing,
            } if matches!(
                purpose,
                NotificationPurpose::Test {
                    requested,
                    fallback: false
                } if requested != ToneId::System
            ) =>
            {
                self.custom_sound = false;
                self.refresh_warnings();
                return vec![self.notification_effect(
                    NotificationPurpose::Test {
                        requested: ToneId::System,
                        fallback: true,
                    },
                    NotificationOperation::SendTest {
                        tone: NotificationTone::System,
                        title: koru_i18n::t(lang, koru_i18n::Key::ReminderTestTitle).to_owned(),
                        body: Some(koru_i18n::t(lang, koru_i18n::Key::ReminderTestBody).to_owned()),
                    },
                )];
            }
            NotificationControlResult::Failed {
                code: NotificationFailure::PermissionDenied,
            } => self.authorization = NotificationAuthorization::Denied,
            NotificationControlResult::Failed { .. } => self.delivery_supported = false,
            NotificationControlResult::Done => {}
        }
        if let NotificationPurpose::SetDeliveryEnabled { enabled } = purpose {
            if self
                .delivery_desired
                .is_some_and(|desired| desired != enabled)
            {
                let desired = self.delivery_desired.expect("checked above");
                self.refresh_warnings();
                return vec![self.notification_effect(
                    NotificationPurpose::SetDeliveryEnabled { enabled: desired },
                    NotificationOperation::SetDeliveryEnabled { enabled: desired },
                )];
            }
            if !succeeded {
                self.delivery_desired = None;
            }
        }
        self.refresh_warnings();
        if matches!(
            purpose,
            NotificationPurpose::Authorization
                | NotificationPurpose::OpenSettings
                | NotificationPurpose::SetDeliveryEnabled { .. }
                | NotificationPurpose::Repair
                | NotificationPurpose::Test { .. }
        ) {
            vec![self.health_effect()]
        } else {
            Vec::new()
        }
    }

    fn notification_effect(
        &mut self,
        purpose: NotificationPurpose,
        operation: NotificationOperation,
    ) -> Effect {
        let id = self.next_notification;
        self.next_notification = self.next_notification.wrapping_add(1).max(1);
        self.pending_notifications.insert(id, purpose);
        Effect::Notification { id, operation }
    }

    fn health_effect(&mut self) -> Effect {
        let id = self.next_health;
        self.next_health = self.next_health.wrapping_add(1).max(1);
        self.latest_health = id;
        Effect::ReminderStatus { id }
    }

    pub fn health_result(&mut self, id: u64, result: Result<ReminderDiagnostics, ()>) -> bool {
        if id != self.latest_health {
            return false;
        }
        let was_connected = self.agent_connected;
        let was_desired = self.delivery_desired;
        let next = match result {
            Ok(diagnostics) => {
                self.agent_connected = true;
                if self.delivery_desired == Some(diagnostics.enabled) {
                    self.delivery_desired = None;
                }
                HealthState::Ready(diagnostics)
            }
            Err(()) => {
                self.agent_connected = false;
                HealthState::Unavailable
            }
        };
        let changed = self.health != next
            || was_connected != self.agent_connected
            || was_desired != self.delivery_desired;
        self.health = next;
        self.refresh_warnings();
        changed
    }

    pub fn authorization(&self) -> NotificationAuthorization {
        self.authorization
    }

    pub fn warnings(&self) -> impl Iterator<Item = DeliveryWarning> + '_ {
        self.warnings.iter().copied()
    }

    fn refresh_warnings(&mut self) {
        self.warnings.clear();
        if !self.agent_connected {
            self.warnings.insert(DeliveryWarning::AgentUnavailable);
        }
        if self.authorization == NotificationAuthorization::Denied {
            self.warnings
                .insert(DeliveryWarning::NotificationsNotAllowed);
        }
        if self.agent_connected && !self.delivery_supported {
            self.warnings
                .insert(DeliveryWarning::NativeDeliveryUnavailable);
        }
        let custom_selected = matches!(
            &self.settings,
            SettingsState::Ready(ReminderSettings {
                default_tone: tone,
                ..
            }) | SettingsState::Saving(ReminderSettings {
                default_tone: tone,
                ..
            }) if *tone != ToneId::System
        );
        if custom_selected && !self.custom_sound {
            self.warnings.insert(DeliveryWarning::CustomToneUnavailable);
        }
        if self.has_reminder
            && self.startup_facts.reminder_explained
            && !self.startup_facts.start_on_login
        {
            self.warnings.insert(DeliveryWarning::StartOnLoginDeclined);
        }
    }
}

fn notification_agent_connected(result: &NotificationControlResult) -> bool {
    !matches!(
        result,
        NotificationControlResult::Failed {
            code: NotificationFailure::Unavailable
        }
    )
}

fn apply_settings_patch(current: &ReminderSettings, patch: &SettingsPatch) -> ReminderSettings {
    let mut next = current.clone();
    if let Some(end_of_day) = patch.end_of_day {
        next.end_of_day = end_of_day;
    }
    if let Some(default_tone) = patch.default_tone {
        next.default_tone = default_tone;
    }
    if let Some(week_start) = patch.week_start {
        next.week_start = week_start;
    }
    next
}

fn merge_settings_patch(target: &mut SettingsPatch, next: SettingsPatch) {
    if next.end_of_day.is_some() {
        target.end_of_day = next.end_of_day;
    }
    if next.default_tone.is_some() {
        target.default_tone = next.default_tone;
    }
    if next.week_start.is_some() {
        target.week_start = next.week_start;
    }
}

pub(crate) const TONES: [ToneId; 11] = [
    ToneId::System,
    ToneId::KoruDefault,
    ToneId::Soft,
    ToneId::Chime,
    ToneId::Pulse,
    ToneId::Bright,
    ToneId::Calm,
    ToneId::Drop,
    ToneId::Knock,
    ToneId::Signal,
    ToneId::Urgent,
];

pub(crate) fn tone_label(lang: koru_i18n::Lang, tone: ToneId) -> &'static str {
    let key = match tone {
        ToneId::System => koru_i18n::Key::ReminderToneSystem,
        ToneId::KoruDefault => koru_i18n::Key::ReminderToneKoruDefault,
        ToneId::Soft => koru_i18n::Key::ReminderToneSoft,
        ToneId::Chime => koru_i18n::Key::ReminderToneChime,
        ToneId::Pulse => koru_i18n::Key::ReminderTonePulse,
        ToneId::Bright => koru_i18n::Key::ReminderToneBright,
        ToneId::Calm => koru_i18n::Key::ReminderToneCalm,
        ToneId::Drop => koru_i18n::Key::ReminderToneDrop,
        ToneId::Knock => koru_i18n::Key::ReminderToneKnock,
        ToneId::Signal => koru_i18n::Key::ReminderToneSignal,
        ToneId::Urgent => koru_i18n::Key::ReminderToneUrgent,
    };
    koru_i18n::t(lang, key)
}

pub(crate) fn authorization_label(
    lang: koru_i18n::Lang,
    authorization: NotificationAuthorization,
) -> &'static str {
    let key = match authorization {
        NotificationAuthorization::Allowed => koru_i18n::Key::ReminderAuthorizationAllowed,
        NotificationAuthorization::NotDetermined => {
            koru_i18n::Key::ReminderAuthorizationNotDetermined
        }
        NotificationAuthorization::Denied => koru_i18n::Key::ReminderAuthorizationDenied,
        NotificationAuthorization::Unavailable => koru_i18n::Key::ReminderAuthorizationUnavailable,
    };
    koru_i18n::t(lang, key)
}

pub(crate) fn warning_label(lang: koru_i18n::Lang, warning: DeliveryWarning) -> &'static str {
    let key = match warning {
        DeliveryWarning::AgentUnavailable => koru_i18n::Key::ReminderWarningAgentUnavailable,
        DeliveryWarning::NotificationsNotAllowed => {
            koru_i18n::Key::ReminderWarningNotificationsNotAllowed
        }
        DeliveryWarning::NativeDeliveryUnavailable => {
            koru_i18n::Key::ReminderWarningDeliveryUnavailable
        }
        DeliveryWarning::CustomToneUnavailable => koru_i18n::Key::ReminderWarningToneUnavailable,
        DeliveryWarning::StartOnLoginDeclined => koru_i18n::Key::ReminderWarningStartOnLogin,
    };
    koru_i18n::t(lang, key)
}

fn notification_tone(tone: ToneId) -> NotificationTone {
    match tone {
        ToneId::System => NotificationTone::System,
        ToneId::KoruDefault => NotificationTone::KoruDefault,
        ToneId::Soft => NotificationTone::Soft,
        ToneId::Chime => NotificationTone::Chime,
        ToneId::Pulse => NotificationTone::Pulse,
        ToneId::Bright => NotificationTone::Bright,
        ToneId::Calm => NotificationTone::Calm,
        ToneId::Drop => NotificationTone::Drop,
        ToneId::Knock => NotificationTone::Knock,
        ToneId::Signal => NotificationTone::Signal,
        ToneId::Urgent => NotificationTone::Urgent,
    }
}

enum StoreAnswer {
    Loaded {
        profile: ProfileId,
        generation: u64,
        result: Result<(ReminderSettings, bool), ()>,
    },
    Saved {
        profile: ProfileId,
        generation: u64,
        result: Result<ReminderSettings, ()>,
    },
}

/// Runs the pure state against the existing Store, Agent, and shell seams.
/// It owns no clock and wakes only for a result or explicit intent.
pub(crate) struct Controller {
    pub model: ReminderPreferences,
    client: Option<koru_api_client::Client>,
    effects: VecDeque<Effect>,
    store_in_flight: bool,
    store_tx: mpsc::Sender<StoreAnswer>,
    store_rx: mpsc::Receiver<StoreAnswer>,
    /// Agent correlation id -> state-local effect id.
    agent_requests: BTreeMap<u64, u64>,
    health_requests: BTreeMap<u64, u64>,
    startup_requests: VecDeque<ReminderStartupRequest>,
    startup_facts: StartupFacts,
}

impl Default for Controller {
    fn default() -> Self {
        let (store_tx, store_rx) = mpsc::channel();
        Self {
            model: ReminderPreferences::default(),
            client: None,
            effects: VecDeque::new(),
            store_in_flight: false,
            store_tx,
            store_rx,
            agent_requests: BTreeMap::new(),
            health_requests: BTreeMap::new(),
            startup_requests: VecDeque::new(),
            startup_facts: StartupFacts::default(),
        }
    }
}

impl Controller {
    pub fn health_presentation(&self, lang: koru_i18n::Lang) -> HealthPresentation {
        let footer = match &self.model.health {
            HealthState::Unknown => koru_i18n::t(lang, koru_i18n::Key::ReminderUpdating),
            HealthState::Unavailable => {
                koru_i18n::t(lang, koru_i18n::Key::ReminderErrorUnavailable)
            }
            HealthState::Ready(_) => koru_i18n::t(lang, koru_i18n::Key::ReminderUpToDate),
        }
        .to_owned();
        let issue = if self
            .model
            .warnings()
            .any(|warning| warning == DeliveryWarning::NotificationsNotAllowed)
        {
            Some((
                warning_label(lang, DeliveryWarning::NotificationsNotAllowed).to_owned(),
                koru_i18n::t(lang, koru_i18n::Key::ReminderOpenNotificationSettings).to_owned(),
                HealthAction::OpenNotificationSettings,
            ))
        } else if let HealthState::Ready(diagnostics) = &self.model.health
            && !diagnostics.enabled
        {
            Some((
                koru_i18n::t(lang, koru_i18n::Key::ReminderDeliveryOffThisDevice).to_owned(),
                koru_i18n::t(lang, koru_i18n::Key::ReminderEnableDelivery).to_owned(),
                HealthAction::EnableDelivery,
            ))
        } else if let HealthState::Ready(diagnostics) = &self.model.health
            && diagnostics.failure_category.is_some()
        {
            Some((
                warning_label(lang, DeliveryWarning::NativeDeliveryUnavailable).to_owned(),
                koru_i18n::t(lang, koru_i18n::Key::ReminderRetry).to_owned(),
                HealthAction::Repair,
            ))
        } else if let Some(warning) = self.model.warnings().next() {
            Some((
                warning_label(lang, warning).to_owned(),
                koru_i18n::t(lang, koru_i18n::Key::ReminderRetry).to_owned(),
                HealthAction::Refresh,
            ))
        } else {
            None
        };
        let (banner, action_label, action) = issue.map_or((None, None, None), |value| {
            (Some(value.0), Some(value.1), Some(value.2))
        });
        HealthPresentation {
            footer,
            banner,
            action_label,
            action,
        }
    }

    pub fn apply(&mut self, intent: Intent, lang: koru_i18n::Lang) {
        match intent {
            Intent::SetEndOfDay(value) => self.set_end_of_day(value),
            Intent::SetDefaultTone(value) => self.set_default_tone(value),
            Intent::SetWeekStart(value) => self.set_week_start(value),
            Intent::RetrySettings => self.retry_settings(),
            Intent::RefreshNotificationStatus => self.refresh_notification_status(),
            Intent::AllowNotifications => self.allow_notifications(),
            Intent::OpenNotificationSettings => self.open_notification_settings(),
            Intent::SetDeliveryEnabled(enabled) => self.set_delivery_enabled(enabled),
            Intent::RepairDelivery => self.repair_delivery(),
            Intent::SendTest => self.send_test(lang),
            Intent::AnswerStartup(accepted) => self.answer_startup(accepted),
            Intent::PreviewTone(tone) => preview_tone(tone),
        }
    }

    pub fn set_client(&mut self, client: Option<koru_api_client::Client>) {
        self.client = client;
    }

    pub fn select_profile(&mut self, profile: ProfileId) {
        if self.model.profile() == Some(profile) {
            return;
        }
        self.effects.extend(self.model.select_profile(profile));
    }

    /// Reload the selected Profile after the shared reminder event tail sees a
    /// remote private-settings mutation. Store work remains serialized with a
    /// local save, and the model preserves any authored fields still in flight.
    pub fn refresh_selected_settings(&mut self) {
        let Some(profile) = self.model.profile() else {
            return;
        };
        let generation = self.model.generation();
        if self.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::LoadSettings {
                    profile: queued_profile,
                    generation: queued_generation,
                } if *queued_profile == profile && *queued_generation == generation
            )
        }) {
            return;
        }
        self.effects.push_back(Effect::LoadSettings {
            profile,
            generation,
        });
    }

    /// Device-local delivered/unhandled identities for the selected profile.
    /// The Agent response contains no authored content or target; the global
    /// Store projection joins these identities to visible rows.
    pub fn attention(&self) -> Vec<koru_editor::reminders::GlobalAttention> {
        let Some(profile) = self.model.profile() else {
            return Vec::new();
        };
        let HealthState::Ready(diagnostics) = &self.model.health else {
            return Vec::new();
        };
        diagnostics
            .attention
            .iter()
            .filter(|identity| ProfileId::parse(&identity.profile) == Some(profile))
            .map(|identity| koru_editor::reminders::GlobalAttention {
                reminder_id: identity.reminder.clone(),
                occurrence: identity.occurrence.clone(),
                fired_at_ms: identity.fired_at_ms,
            })
            .collect()
    }

    pub fn set_pushed_diagnostics(&mut self, diagnostics: ReminderDiagnostics) -> bool {
        // A push is the Agent's newer whole snapshot. Invalidate every
        // explicit status generation already in flight before maintenance can
        // apply a response that the reader queued just before this push.
        self.model.latest_health = self.model.next_health;
        self.model.next_health = self.model.next_health.wrapping_add(1).max(1);
        let next = HealthState::Ready(diagnostics);
        let changed = self.model.health != next || !self.model.agent_connected;
        self.model.health = next;
        self.model.agent_connected = true;
        self.model.refresh_warnings();
        changed
    }

    pub fn set_startup_facts(&mut self, facts: StartupFacts) -> bool {
        self.startup_facts = facts;
        self.model.observe_reminders(self.model.has_reminder, facts)
    }

    /// Piece 6 feeds this from its authoritative profile reconciliation after
    /// a durable create/remote change. No timer or second event subscription
    /// is introduced here.
    pub fn observe_reminders(&mut self, any: bool) -> bool {
        self.model.observe_reminders(any, self.startup_facts)
    }

    pub fn finish_startup(&mut self, result: ReminderStartupResult) -> bool {
        let facts = result.actual;
        let actual = StartupFacts {
            start_on_login: facts.start_on_login,
            reminder_explained: facts.explained,
        };
        let outcome = if result.error.is_none() {
            Ok(actual)
        } else {
            Err(actual)
        };
        let applied = self.model.startup_saved(result.id, outcome);
        if applied {
            self.startup_facts = actual;
        }
        applied
    }

    pub fn answer_startup(&mut self, accepted: bool) {
        self.effects.extend(self.model.answer_startup(accepted));
    }

    pub fn set_end_of_day(&mut self, value: Option<LocalTime>) {
        self.effects.extend(self.model.set_end_of_day(value));
    }

    pub fn set_default_tone(&mut self, value: ToneId) {
        self.effects.extend(self.model.set_default_tone(value));
    }

    pub fn set_week_start(&mut self, value: WeekStartPreference) {
        self.effects.extend(self.model.set_week_start(value));
    }

    pub fn retry_settings(&mut self) {
        self.effects.extend(self.model.retry_settings());
    }

    pub fn refresh_notification_status(&mut self) {
        self.effects
            .extend(self.model.refresh_notification_status());
    }

    pub fn allow_notifications(&mut self) {
        self.effects.extend(self.model.allow_notifications());
    }

    pub fn open_notification_settings(&mut self) {
        self.effects.extend(self.model.open_notification_settings());
    }

    pub fn set_delivery_enabled(&mut self, enabled: bool) {
        self.effects
            .extend(self.model.set_delivery_enabled(enabled));
    }

    pub fn repair_delivery(&mut self) {
        self.effects.extend(self.model.repair_delivery());
    }

    pub fn send_test(&mut self, lang: koru_i18n::Lang) {
        self.effects.extend(self.model.send_test(lang));
    }

    pub fn take_startup_request(&mut self) -> Option<ReminderStartupRequest> {
        self.startup_requests.pop_front()
    }

    pub fn maintain(&mut self, agent: &crate::agent::AgentLink) -> bool {
        let mut changed = false;
        while let Ok(answer) = self.store_rx.try_recv() {
            self.store_in_flight = false;
            changed |= match answer {
                StoreAnswer::Loaded {
                    profile,
                    generation,
                    result,
                } => match result {
                    Ok((settings, any)) => {
                        let (settings_changed, follow_up) = self
                            .model
                            .settings_loaded_with_follow_up(profile, generation, Ok(settings));
                        self.effects.extend(follow_up);
                        if settings_changed {
                            self.model.observe_reminders(any, self.startup_facts);
                        }
                        settings_changed
                    }
                    Err(()) => {
                        let (settings_changed, follow_up) = self
                            .model
                            .settings_loaded_with_follow_up(profile, generation, Err(()));
                        self.effects.extend(follow_up);
                        settings_changed
                    }
                },
                StoreAnswer::Saved {
                    profile,
                    generation,
                    result,
                } => {
                    let (settings_changed, follow_up) = self
                        .model
                        .settings_saved_with_follow_up(profile, generation, result);
                    self.effects.extend(follow_up);
                    settings_changed
                }
            };
        }

        let completed: Vec<_> = self
            .agent_requests
            .iter()
            .filter_map(|(agent_id, local_id)| {
                agent
                    .take_notification_control_for(*agent_id)
                    .map(|result| (*agent_id, *local_id, result))
            })
            .collect();
        for (agent_id, local_id, result) in completed {
            self.agent_requests.remove(&agent_id);
            self.model
                .set_agent_connected(notification_agent_connected(&result));
            let follow_up = self
                .model
                .notification_result(local_id, result, crate::format::lang());
            self.effects.extend(follow_up);
            changed = true;
        }

        let completed_health: Vec<_> = self
            .health_requests
            .iter()
            .filter_map(|(agent_id, local_id)| {
                agent
                    .take_reminder_status_for(*agent_id)
                    .map(|result| (*agent_id, *local_id, result))
            })
            .collect();
        for (agent_id, local_id, result) in completed_health {
            self.health_requests.remove(&agent_id);
            changed |= self.model.health_result(local_id, result);
        }

        loop {
            let Some(effect) = self.effects.front().cloned() else {
                break;
            };
            match effect {
                Effect::LoadSettings {
                    profile,
                    generation,
                } => {
                    if self.store_in_flight {
                        break;
                    }
                    let Some(client) = self.client.clone() else {
                        break;
                    };
                    self.effects.pop_front();
                    let tx = self.store_tx.clone();
                    let wake = agent.clone();
                    match std::thread::Builder::new()
                        .name("koru-reminder-settings".into())
                        .spawn(move || {
                            let result = client
                                .store()
                                .reminder_reconciliation(profile)
                                .map(|snapshot| {
                                    let any = snapshot.reminders.iter().any(|row| {
                                        !row.stored.deleted
                                            && row.stored.rule.status
                                                != koru_store_core::reminders::RuleStatus::Deleted
                                    });
                                    (snapshot.settings, any)
                                })
                                .map_err(|_| ());
                            let _ = tx.send(StoreAnswer::Loaded {
                                profile,
                                generation,
                                result,
                            });
                            wake.wake_loop();
                        }) {
                        Ok(_) => self.store_in_flight = true,
                        Err(_) => {
                            let (settings_changed, follow_up) = self
                                .model
                                .settings_loaded_with_follow_up(profile, generation, Err(()));
                            self.effects.extend(follow_up);
                            changed |= settings_changed;
                        }
                    }
                }
                Effect::PatchSettings {
                    profile,
                    generation,
                    patch,
                } => {
                    if self.store_in_flight {
                        break;
                    }
                    let Some(client) = self.client.clone() else {
                        break;
                    };
                    self.effects.pop_front();
                    let tx = self.store_tx.clone();
                    let wake = agent.clone();
                    match std::thread::Builder::new()
                        .name("koru-reminder-settings".into())
                        .spawn(move || {
                            let result = client
                                .store()
                                .update_reminder_settings(profile, patch)
                                .map_err(|_| ());
                            let _ = tx.send(StoreAnswer::Saved {
                                profile,
                                generation,
                                result,
                            });
                            wake.wake_loop();
                        }) {
                        Ok(_) => self.store_in_flight = true,
                        Err(_) => {
                            let (settings_changed, follow_up) = self
                                .model
                                .settings_saved_with_follow_up(profile, generation, Err(()));
                            self.effects.extend(follow_up);
                            changed |= settings_changed;
                        }
                    }
                }
                Effect::Notification { id, operation } => {
                    self.effects.pop_front();
                    let agent_id = agent.notification_control(operation);
                    match agent_id {
                        Some(agent_id) => {
                            self.agent_requests.insert(agent_id, id);
                            self.model.set_agent_connected(true);
                        }
                        None => changed |= self.model.notification_not_queued(id),
                    }
                }
                Effect::ReminderStatus { id } => {
                    self.effects.pop_front();
                    match agent.reminder_status() {
                        Some(agent_id) => {
                            self.health_requests.insert(agent_id, id);
                            self.model.set_agent_connected(true);
                        }
                        None => changed |= self.model.health_result(id, Err(())),
                    }
                }
                Effect::SaveStartupConsent(request) => {
                    self.effects.pop_front();
                    self.startup_requests.push_back(request);
                    changed = true;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests;

/// Play `tone` once on this machine. Nothing is scheduled and nothing is
/// written; a test build plays nothing, because the suite must not make noise
/// on whoever runs it.
fn preview_tone(tone: ToneId) {
    use koru_native_notifications::BundledTone;
    if cfg!(test) {
        return;
    }
    let bundled = match tone {
        ToneId::System => None,
        ToneId::KoruDefault => Some(BundledTone::KoruDefault),
        ToneId::Soft => Some(BundledTone::Soft),
        ToneId::Chime => Some(BundledTone::Chime),
        ToneId::Pulse => Some(BundledTone::Pulse),
        ToneId::Bright => Some(BundledTone::Bright),
        ToneId::Calm => Some(BundledTone::Calm),
        ToneId::Drop => Some(BundledTone::Drop),
        ToneId::Knock => Some(BundledTone::Knock),
        ToneId::Signal => Some(BundledTone::Signal),
        ToneId::Urgent => Some(BundledTone::Urgent),
    };
    koru_native_notifications::preview(bundled);
}
