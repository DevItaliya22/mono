// SPDX-License-Identifier: MPL-2.0

//! Reminder preferences and delivery health inside the dedicated Settings tab.
//!
//! This is presentation only. The controller owns Store, Agent and shell
//! effects; this surface receives a clone of its plain state and emits typed
//! intents. In particular, drawing never inspects the OS or talks to the Agent.

use super::*;
use crate::reminder_settings::{
    DeliveryWarning, HealthState, Intent, ReminderPreferences, SettingsState, StartupState, TONES,
    authorization_label, tone_label, warning_label,
};
use koru_agent_wire::NotificationAuthorization;
use koru_store_core::ids::ProfileId;
use koru_store_core::reminders::{LocalTime, WeekStartPreference};
use koru_ui::{
    ActionButton, ActionStyle, Card, CardRow, Dropdown, DropdownOption, InputStyle, Menu,
    Segmented, TextInput, Toggle,
};
use std::collections::VecDeque;
use winit::keyboard::NamedKey;

const INPUT_H: f32 = 32.0;
const INPUT_W: f32 = 96.0;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Presentation {
    profile: Option<ProfileId>,
    generation: u64,
    settings: SettingsState,
    startup: StartupState,
    health: HealthState,
    authorization: NotificationAuthorization,
    warnings: Vec<DeliveryWarning>,
    delivery_desired: Option<bool>,
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            profile: None,
            generation: 0,
            settings: SettingsState::Unavailable,
            startup: StartupState::Unknown,
            health: HealthState::Unknown,
            authorization: NotificationAuthorization::Unavailable,
            warnings: Vec::new(),
            delivery_desired: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    EndOfDay,
    ClearEndOfDay,
    WeekStart(WeekStartPreference),
    /// The default-tone field, which opens the list of tones.
    ToneMenu,
    /// The play button beside it.
    PreviewTone,
    RetrySettings,
    Delivery,
    AllowNotifications,
    OpenNotificationSettings,
    RefreshNotificationStatus,
    RepairDelivery,
    BannerOpenNotificationSettings,
    SendTest,
    StartupEnable,
    StartupDecline,
}

#[derive(Clone, Copy, Debug)]
struct Target {
    control: Control,
    rect: Rect,
    enabled: bool,
}

pub(super) struct ReminderSurface {
    state: Presentation,
    /// The device choice survives a disconnected Agent reading. Connectivity
    /// is not permission to change what the person selected.
    last_enabled: Option<bool>,
    time: TextInput,
    time_invalid: bool,
    focus: Option<Control>,
    hot: Option<Control>,
    targets: Vec<Target>,
    intents: VecDeque<Intent>,
    top: f32,
    bottom: f32,
    /// The column the last layout used, and the cards it produced.
    column: Rect,
    cards: Vec<CardSpec>,
    lang: koru_i18n::Lang,
    /// The tone list, while it is open, and where it may sit.
    menu: Option<Menu<DropdownOption>>,
    view: (f32, f32),
    /// What *System default* resolves to on this machine, sampled by the host
    /// when the page opens — never on a frame.
    system_week_start: Option<koru_workspace_host::environment::FirstDayOfWeek>,
}

impl ReminderSurface {
    pub(super) fn new(engine: &mut TextEngine) -> Self {
        Self {
            state: Presentation::default(),
            last_enabled: None,
            time: TextInput::new(engine, theme::role::row()),
            time_invalid: false,
            focus: None,
            hot: None,
            targets: Vec::new(),
            intents: VecDeque::new(),
            top: 0.0,
            bottom: 0.0,
            column: Rect::new(0.0, 0.0, 0.0, 0.0),
            cards: Vec::new(),
            lang: koru_i18n::Lang::default(),
            menu: None,
            view: (f32::MAX, f32::MAX),
            system_week_start: None,
        }
    }

    pub(super) fn set_model(
        &mut self,
        engine: &mut TextEngine,
        model: &ReminderPreferences,
    ) -> bool {
        let identity = (model.profile(), model.generation());
        let profile_changed = identity != (self.state.profile, self.state.generation);
        let settings_disabled = !matches!(
            model.settings,
            SettingsState::Ready(_) | SettingsState::Saving(_)
        );
        let disabled_focus = settings_disabled && self.focus.is_some_and(profile_control);
        if profile_changed || disabled_focus {
            self.time.blur(engine);
            self.time_invalid = false;
            self.focus = None;
            if self.hot.is_some_and(profile_control) {
                self.hot = None;
            }
        }
        if profile_changed {
            self.time.set_text(engine, "");
            self.hot = None;
            self.targets.clear();
            self.intents.retain(|intent| {
                !matches!(
                    intent,
                    Intent::SetEndOfDay(_)
                        | Intent::SetDefaultTone(_)
                        | Intent::SetWeekStart(_)
                        | Intent::RetrySettings
                )
            });
        }
        let next = Presentation {
            profile: identity.0,
            generation: identity.1,
            settings: model.settings.clone(),
            startup: model.startup,
            health: model.health.clone(),
            authorization: model.authorization(),
            warnings: model.warnings().collect(),
            delivery_desired: model.desired_delivery_enabled(),
        };
        if let HealthState::Ready(diagnostics) = &next.health {
            self.last_enabled = Some(diagnostics.enabled);
        }
        if profile_changed || self.focus != Some(Control::EndOfDay) {
            let shown = settings(&next.settings)
                .and_then(|value| value.end_of_day)
                .map(|value| value.to_storage()[..5].to_owned())
                .unwrap_or_default();
            if self.time.raw_text() != shown {
                self.time.set_text(engine, &shown);
            }
            self.time_invalid = false;
        }
        let changed = self.state != next;
        self.state = next;
        changed
    }

    pub(super) fn take_intent(&mut self) -> Option<Intent> {
        self.intents.pop_front()
    }

    /// Give the end-of-day field keyboard focus and return its content-space
    /// geometry so the one settings scroll authority can reveal it.
    pub(super) fn focus_end_of_day(&mut self, engine: &mut TextEngine) -> Option<Rect> {
        let rect = self.enabled_rect(Control::EndOfDay)?;
        self.hot = None;
        self.focus = Some(Control::EndOfDay);
        self.time.release();
        self.time.move_to_end(engine);
        Some(rect)
    }

    #[cfg(test)]
    pub(super) fn end_of_day_rect(&self) -> Option<Rect> {
        self.enabled_rect(Control::EndOfDay)
    }

    #[cfg(test)]
    pub(super) fn end_of_day_focused(&self) -> bool {
        self.focus == Some(Control::EndOfDay)
    }

    /// Lay the Reminders page's cards out from `top`, in the column the shared
    /// Settings surface gives this host. Answers the bottom of the last card.
    ///
    /// The page is built from the same shared components as every other
    /// Settings page — `KoruCard`, `KoruCardRow`, `KoruSegmented`,
    /// `KoruDropdown`, `KoruToggle`, the action button — so it reads, and
    /// inspects, as the same surface. What stays here is only what belongs to
    /// reminders: which rows exist in which state, and what a press means.
    pub(super) fn layout(
        &mut self,
        engine: &mut TextEngine,
        column: Rect,
        top: f32,
        lang: koru_i18n::Lang,
    ) -> f32 {
        self.top = top;
        self.column = column;
        self.lang = lang;
        self.targets.clear();
        self.cards.clear();
        let settings_enabled = matches!(
            self.state.settings,
            SettingsState::Ready(_) | SettingsState::Saving(_)
        );
        let mut page = Page::new(column, top);

        // At most one prominent, actionable failure, first thing on the page.
        // There is deliberately no health score.
        if let Some((message, action)) = self.banner(lang) {
            page.card(None, None);
            let parts = action
                .map(|action| vec![self.button_part(engine, action, lang, true)])
                .unwrap_or_default();
            page.row(engine, &mut self.targets, &message, None, false, parts);
        }

        if matches!(
            self.state.startup,
            StartupState::NeedsExplanation | StartupState::Failed { .. }
        ) {
            page.card(
                Some(koru_i18n::t(lang, koru_i18n::Key::ReminderStartupTitle)),
                None,
            );
            let parts = vec![
                self.button_part(engine, Control::StartupDecline, lang, true),
                self.button_part(engine, Control::StartupEnable, lang, true),
            ];
            page.row(
                engine,
                &mut self.targets,
                koru_i18n::t(lang, koru_i18n::Key::ReminderStartupBody),
                None,
                false,
                parts,
            );
        }

        page.card(
            Some(koru_i18n::t(lang, koru_i18n::Key::SettingsSchedule)),
            None,
        );
        // End-of-day time: every minute remains expressible; empty is a real
        // value rather than an invented profile default.
        let mut parts = vec![Part::control(
            Control::EndOfDay,
            INPUT_W,
            INPUT_H,
            settings_enabled,
        )];
        if settings(&self.state.settings).is_some_and(|value| value.end_of_day.is_some()) {
            let mut clear = self.button_part(engine, Control::ClearEndOfDay, lang, true);
            clear.enabled = settings_enabled;
            parts.push(clear);
        }
        page.row(
            engine,
            &mut self.targets,
            koru_i18n::t(lang, koru_i18n::Key::ReminderEndOfDay),
            Some(koru_i18n::t(lang, koru_i18n::Key::ReminderEndOfDayHint)),
            self.time_invalid,
            parts,
        );

        let week_owned: Vec<String> = WEEK_STARTS
            .iter()
            .map(|week_start| self.week_label(lang, *week_start))
            .collect();
        let week_labels: Vec<&str> = week_owned.iter().map(String::as_str).collect();
        let week = Segmented::layout(engine, &week_labels, 0.0, 0.0);
        page.row(
            engine,
            &mut self.targets,
            koru_i18n::t(lang, koru_i18n::Key::ReminderWeekStart),
            None,
            false,
            vec![Part::segmented(
                week,
                WEEK_STARTS
                    .iter()
                    .map(|week_start| Control::WeekStart(*week_start))
                    .collect(),
                settings_enabled,
            )],
        );

        if matches!(self.state.settings, SettingsState::Failed) {
            let retry = self.button_part(engine, Control::RetrySettings, lang, true);
            page.row(
                engine,
                &mut self.targets,
                koru_i18n::t(lang, koru_i18n::Key::SettingsReminderLoadFailed),
                None,
                true,
                vec![retry],
            );
        }

        // Current-device delivery. The aside makes its local scope explicit and
        // the view never derives a new choice from connectivity.
        page.card(
            Some(koru_i18n::t(
                lang,
                koru_i18n::Key::ReminderNotificationsOnThisDevice,
            )),
            Some(koru_i18n::t(
                lang,
                koru_i18n::Key::ReminderDeliveryThisDeviceOnly,
            )),
        );
        let delivery = if self.last_enabled.is_some() {
            vec![Part::control(
                Control::Delivery,
                Toggle::new(false).width(),
                Toggle::H,
                true,
            )]
        } else {
            Vec::new()
        };
        let status = self.delivery_status(lang);
        page.row(
            engine,
            &mut self.targets,
            koru_i18n::t(lang, koru_i18n::Key::ReminderDeliveryThisDevice),
            Some(status),
            false,
            delivery,
        );
        let tone_options = self.tone_options(lang);
        let tone_w = Dropdown::width_for(engine, &tone_options);
        let play = koru_ui::icon_button::Metrics::STANDARD;
        let mut tone_controls = Vec::new();
        // Preview has a Windows adapter only. Do not offer a silent no-op on
        // other platforms; selecting a reminder tone remains available.
        if cfg!(windows) {
            tone_controls.push(Part::control(
                Control::PreviewTone,
                play.control_width(),
                play.control_height(),
                settings_enabled,
            ));
        }
        tone_controls.push(Part::control(
            Control::ToneMenu,
            tone_w,
            Dropdown::H,
            settings_enabled,
        ));
        page.row(
            engine,
            &mut self.targets,
            koru_i18n::t(lang, koru_i18n::Key::ReminderDefaultTone),
            None,
            false,
            tone_controls,
        );

        let test = self.button_part(engine, Control::SendTest, lang, true);
        page.row(
            engine,
            &mut self.targets,
            koru_i18n::t(lang, koru_i18n::Key::SettingsNotificationTest),
            Some(koru_i18n::t(
                lang,
                koru_i18n::Key::SettingsNotificationTestHint,
            )),
            false,
            vec![test],
        );

        // Permission is the operating system's, not Koru's, so it is its own
        // section — and only *Allow* may ask for it.
        page.card(
            Some(koru_i18n::t(lang, koru_i18n::Key::ReminderPermissionTitle)),
            None,
        );
        let mut permission = Vec::new();
        if self.state.authorization == NotificationAuthorization::NotDetermined {
            permission.push(self.button_part(engine, Control::AllowNotifications, lang, true));
        }
        permission.push(self.button_part(engine, Control::OpenNotificationSettings, lang, true));
        permission.push(self.button_part(engine, Control::RefreshNotificationStatus, lang, true));
        page.row(
            engine,
            &mut self.targets,
            koru_i18n::t(lang, koru_i18n::Key::ReminderNotificationStatus),
            Some(authorization_label(lang, self.state.authorization)),
            false,
            permission,
        );

        page.card(
            Some(koru_i18n::t(lang, koru_i18n::Key::ReminderHealthTitle)),
            None,
        );
        for (key, value) in self.health_rows(lang) {
            let w = engine.simple_layout(&value, theme::role::row()).width();
            page.row(
                engine,
                &mut self.targets,
                koru_i18n::t(lang, key),
                None,
                false,
                vec![Part::value(value, w)],
            );
        }

        let (cards, bottom) = page.finish();
        self.cards = cards;
        self.bottom = bottom;
        self.bottom
    }

    /// A row's action button, measured in the shared action-button geometry.
    fn button_part(
        &self,
        engine: &mut TextEngine,
        control: Control,
        lang: koru_i18n::Lang,
        enabled: bool,
    ) -> Part {
        let w = ActionButton {
            label: button_label(lang, control),
            style: button_style(control),
            trailing_icon: None,
        }
        .width(engine);
        Part::control(control, w, ActionButton::H, enabled)
    }

    /// The tone choices, the one in force checked.
    fn tone_options(&self, lang: koru_i18n::Lang) -> Vec<DropdownOption> {
        let active = settings(&self.state.settings).map(|value| value.default_tone);
        TONES
            .iter()
            .map(|tone| DropdownOption::new(tone_label(lang, *tone), active == Some(*tone)))
            .collect()
    }

    /// What delivery on this device is doing, said under its switch: off,
    /// on and working, on but needing the banner's fix — or that the device
    /// could not say.
    fn delivery_status(&self, lang: koru_i18n::Lang) -> &'static str {
        let key = match self.last_enabled {
            None => koru_i18n::Key::ReminderAuthorizationUnavailable,
            Some(false) => koru_i18n::Key::ReminderDeliveryOff,
            Some(true) if self.banner(lang).is_some() => {
                koru_i18n::Key::ReminderDeliveryNeedsFixing
            }
            Some(true) => koru_i18n::Key::ReminderDeliveryOn,
        };
        koru_i18n::t(lang, key)
    }

    fn banner(&self, lang: koru_i18n::Lang) -> Option<(String, Option<Control>)> {
        let warning = self.state.warnings.first().copied();
        if let Some(warning) = warning {
            let action = match warning {
                DeliveryWarning::NotificationsNotAllowed => {
                    Some(Control::BannerOpenNotificationSettings)
                }
                DeliveryWarning::NativeDeliveryUnavailable => Some(Control::RepairDelivery),
                _ => None,
            };
            return Some((warning_label(lang, warning).to_owned(), action));
        }
        let failure = match &self.state.health {
            HealthState::Ready(value) => value.failure_category.as_deref(),
            _ => None,
        }?;
        let action = if failure == "native_permission" {
            Control::BannerOpenNotificationSettings
        } else {
            Control::RepairDelivery
        };
        Some((
            koru_i18n::t(lang, koru_i18n::Key::ReminderHealthNeedsRepair).to_owned(),
            Some(action),
        ))
    }

    pub(super) fn press(&mut self, engine: &mut TextEngine, x: f32, y: f32) -> bool {
        if self.menu.is_some() {
            return self.press_menu(x, y);
        }
        let Some(target) = self
            .targets
            .iter()
            .find(|target| target.enabled && target.rect.contains(x, y))
            .copied()
        else {
            if self.focus == Some(Control::EndOfDay) {
                self.time.blur(engine);
            }
            self.focus = None;
            return false;
        };
        self.focus = Some(target.control);
        if target.control == Control::EndOfDay {
            self.time.click(engine, target.rect, x, y);
            return true;
        }
        self.activate(target.control, engine);
        true
    }

    pub(super) fn move_pointer(&mut self, engine: &mut TextEngine, x: f32, y: f32) -> bool {
        if let Some(menu) = self.menu.as_mut() {
            let hover = menu.item_at(self.view, x, y).or(menu.hover);
            let changed = hover != menu.hover;
            menu.hover = hover;
            return changed;
        }
        let next = self
            .targets
            .iter()
            .find(|target| target.enabled && target.rect.contains(x, y))
            .map(|target| target.control);
        let changed = next != self.hot;
        self.hot = next;
        if let Some(rect) = self.rect(Control::EndOfDay) {
            self.time.drag(engine, rect, x);
        }
        changed
    }

    pub(super) fn release(&mut self) -> bool {
        let dragging = self.time.is_dragging();
        self.time.release();
        dragging
    }

    pub(super) fn double_click(&mut self, engine: &mut TextEngine, x: f32, y: f32) -> bool {
        let Some(rect) = self.enabled_rect(Control::EndOfDay) else {
            return false;
        };
        if self.time.double_click(engine, rect, x, y) {
            self.focus = Some(Control::EndOfDay);
            true
        } else {
            false
        }
    }

    pub(super) fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> bool {
        if let Some(menu) = self.menu.as_mut() {
            let enabled = vec![true; menu.items().len()];
            match key {
                Key::Named(NamedKey::ArrowDown) => menu.step(&enabled, 1),
                Key::Named(NamedKey::ArrowUp) => menu.step(&enabled, -1),
                Key::Named(NamedKey::Escape) => self.menu = None,
                Key::Named(NamedKey::Enter | NamedKey::Space) => {
                    if let Some(item) = self.menu.take().and_then(|menu| menu.hover) {
                        self.choose_tone(item);
                    }
                }
                _ => return false,
            }
            return true;
        }
        if matches!(key, Key::Named(NamedKey::Tab)) && !mods.control_key() && !mods.alt_key() {
            self.step_focus(mods.shift_key());
            return true;
        }
        if self.focus == Some(Control::EndOfDay) {
            match key {
                Key::Named(NamedKey::Enter) => {
                    self.commit_time();
                    return true;
                }
                Key::Named(NamedKey::Escape) => {
                    self.restore_time(engine);
                    self.time.blur(engine);
                    self.focus = None;
                    return true;
                }
                _ => return self.time.key(engine, key, text, mods) == koru_ui::Typed::Handled,
            }
        }
        if matches!(key, Key::Named(NamedKey::Enter | NamedKey::Space)) {
            if let Some(control) = self.focus {
                self.activate(control, engine);
                return true;
            }
        }
        false
    }

    fn step_focus(&mut self, backwards: bool) {
        let enabled: Vec<_> = self
            .targets
            .iter()
            .filter(|target| target.enabled)
            .map(|target| target.control)
            .collect();
        if enabled.is_empty() {
            self.focus = None;
            return;
        }
        let at = self
            .focus
            .and_then(|focus| enabled.iter().position(|control| *control == focus));
        let next = match (at, backwards) {
            (None, false) => 0,
            (None, true) => enabled.len() - 1,
            (Some(0), true) => enabled.len() - 1,
            (Some(i), true) => i - 1,
            (Some(i), false) => (i + 1) % enabled.len(),
        };
        self.focus = Some(enabled[next]);
    }

    fn activate(&mut self, control: Control, engine: &mut TextEngine) {
        let intent = match control {
            Control::EndOfDay => return,
            Control::ClearEndOfDay => {
                self.time.set_text(engine, "");
                self.time_invalid = false;
                Intent::SetEndOfDay(None)
            }
            Control::ToneMenu => {
                let lang = self.lang;
                self.open_tone_menu(engine, lang);
                return;
            }
            Control::PreviewTone => {
                let Some(tone) = settings(&self.state.settings).map(|value| value.default_tone)
                else {
                    return;
                };
                Intent::PreviewTone(tone)
            }
            Control::WeekStart(week_start) => Intent::SetWeekStart(week_start),
            Control::RetrySettings => Intent::RetrySettings,
            Control::Delivery => {
                let enabled = !self
                    .state
                    .delivery_desired
                    .or(self.last_enabled)
                    .unwrap_or(false);
                self.state.delivery_desired = Some(enabled);
                Intent::SetDeliveryEnabled(enabled)
            }
            Control::AllowNotifications => Intent::AllowNotifications,
            Control::OpenNotificationSettings => Intent::OpenNotificationSettings,
            Control::RefreshNotificationStatus => Intent::RefreshNotificationStatus,
            Control::RepairDelivery => Intent::RepairDelivery,
            Control::BannerOpenNotificationSettings => Intent::OpenNotificationSettings,
            Control::SendTest => Intent::SendTest,
            Control::StartupEnable => Intent::AnswerStartup(true),
            Control::StartupDecline => Intent::AnswerStartup(false),
        };
        self.intents.push_back(intent);
    }

    fn commit_time(&mut self) {
        let authored = self.time.value();
        if authored.is_empty() {
            self.time_invalid = false;
            self.intents.push_back(Intent::SetEndOfDay(None));
            return;
        }
        let storage = if authored.len() == 5 {
            format!("{authored}:00")
        } else {
            authored
        };
        match LocalTime::parse(&storage) {
            Some(value) => {
                self.time_invalid = false;
                self.intents.push_back(Intent::SetEndOfDay(Some(value)));
            }
            None => self.time_invalid = true,
        }
    }

    fn restore_time(&mut self, engine: &mut TextEngine) {
        let shown = settings(&self.state.settings)
            .and_then(|value| value.end_of_day)
            .map(|value| value.to_storage()[..5].to_owned())
            .unwrap_or_default();
        self.time.set_text(engine, &shown);
        self.time_invalid = false;
    }

    fn rect(&self, control: Control) -> Option<Rect> {
        self.targets
            .iter()
            .find(|target| target.control == control)
            .map(|target| target.rect)
    }

    fn enabled_rect(&self, control: Control) -> Option<Rect> {
        self.targets
            .iter()
            .find(|target| target.control == control && target.enabled)
            .map(|target| target.rect)
    }

    pub(super) fn hot(&self) -> bool {
        self.hot.is_some() || self.menu.is_some()
    }

    /// Where an open tone list may sit: the right edge of the page area and
    /// the bottom of what is visible, both in the page's content coordinates.
    pub(super) fn set_view(&mut self, right: f32, visible_bottom: f32) {
        self.view = (right, visible_bottom);
    }

    /// The machine's first day of the week, as the host read it.
    pub(super) fn set_system_week_start(
        &mut self,
        day: Option<koru_workspace_host::environment::FirstDayOfWeek>,
    ) {
        self.system_week_start = day;
    }

    /// A week-start choice's words. *System default* says which day it
    /// means here, so a person can tell whether choosing Monday changes
    /// anything.
    fn week_label(&self, lang: koru_i18n::Lang, week_start: WeekStartPreference) -> String {
        use koru_workspace_host::environment::FirstDayOfWeek;
        let base = week_start_label(lang, week_start);
        if week_start != WeekStartPreference::System {
            return base.to_owned();
        }
        let day = match self.system_week_start {
            Some(FirstDayOfWeek::Monday) => week_start_label(lang, WeekStartPreference::Monday),
            Some(FirstDayOfWeek::Sunday) => week_start_label(lang, WeekStartPreference::Sunday),
            // Saturday has no choice of its own; the system value still needs a
            // English stands until the i18n pass (Q-V75).
            Some(FirstDayOfWeek::Saturday) => koru_i18n::t(lang, koru_i18n::Key::SettingsSaturday),
            None => return base.to_owned(),
        };
        format!("{base} ({day})")
    }

    fn open_tone_menu(&mut self, engine: &mut TextEngine, lang: koru_i18n::Lang) {
        let Some(field) = self.enabled_rect(Control::ToneMenu) else {
            return;
        };
        self.menu = Some(Dropdown::menu(engine, field, self.tone_options(lang)));
    }

    /// The open tone list's answer to a press: a row chooses it, anything else
    /// closes the list. Either way the press is the list's.
    fn press_menu(&mut self, x: f32, y: f32) -> bool {
        let Some(menu) = self.menu.take() else {
            return false;
        };
        if let Some(item) = menu.item_at(self.view, x, y) {
            self.choose_tone(item);
        }
        true
    }

    fn choose_tone(&mut self, item: usize) {
        if let Some(tone) = TONES.get(item) {
            self.intents.push_back(Intent::SetDefaultTone(*tone));
        }
    }

    pub(super) fn draw(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        _column: Rect,
        lang: koru_i18n::Lang,
    ) {
        let cards = std::mem::take(&mut self.cards);
        for card in &cards {
            let dividers: Vec<f32> = card.rows.iter().skip(1).map(|row| row.rect.y).collect();
            let _card = Card {
                title: card.title.as_deref(),
                aside: card.aside.as_deref(),
            }
            .draw(scene, root, engine, card.rect, &dividers, 1.0);
            for row in &card.rows {
                let _row = CardRow {
                    label: &row.label,
                    hint: row.hint.as_deref(),
                    link: None,
                    chip: None,
                }
                .draw(scene, root, engine, row.rect, row.text_w, 1.0);
                for part in &row.parts {
                    self.draw_part(scene, root, engine, part, lang);
                }
            }
        }
        self.cards = cards;
        if let Some(menu) = self.menu.as_ref() {
            let enabled = vec![true; menu.items().len()];
            menu.draw(scene, root, engine, self.view, &enabled);
        }
    }

    fn draw_part(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        part: &Placed,
        lang: koru_i18n::Lang,
    ) {
        let alpha = if part.enabled { 1.0 } else { DIMMED };
        match &part.kind {
            PartKind::Segmented { control, controls } => {
                let owned: Vec<String> = controls
                    .iter()
                    .map(|control| match control {
                        Control::WeekStart(week_start) => self.week_label(lang, *week_start),
                        _ => String::new(),
                    })
                    .collect();
                let labels: Vec<&str> = owned.iter().map(String::as_str).collect();
                let active = settings(&self.state.settings).map(|value| value.week_start);
                let selected = controls
                    .iter()
                    .position(|control| Some(*control) == active.map(Control::WeekStart));
                let hovered = controls
                    .iter()
                    .position(|control| Some(*control) == self.hot);
                control.draw(scene, root, engine, &labels, selected, hovered, alpha);
            }
            PartKind::Value(value) => {
                let layout = engine.simple_layout(value, theme::role::row());
                koru_ui::text_paragraph::draw_line(
                    scene,
                    root,
                    engine,
                    value,
                    part.rect.x + part.rect.w - layout.width(),
                    part.rect.y + (part.rect.h - scene::first_line_height(&layout)) * 0.5,
                    theme::role::row(),
                    "theme::role::row()",
                    theme::p().text,
                );
            }
            PartKind::Control(control) => {
                let hovered = self.hot == Some(*control);
                let focused = self.focus == Some(*control);
                match control {
                    Control::EndOfDay => {
                        let style = if self.time_invalid {
                            InputStyle::inline().invalid()
                        } else {
                            InputStyle::inline()
                        };
                        self.time.draw(
                            scene,
                            root,
                            engine,
                            part.rect,
                            style,
                            Some(koru_i18n::t(lang, koru_i18n::Key::ReminderEndOfDayUnset)),
                            focused,
                        );
                    }
                    Control::ToneMenu => {
                        let active = settings(&self.state.settings)
                            .map(|value| tone_label(lang, value.default_tone))
                            .unwrap_or_default();
                        Dropdown::new(active)
                            .open(self.menu.is_some())
                            .enabled(part.enabled)
                            .draw(scene, root, engine, part.rect, hovered, 1.0);
                    }
                    Control::PreviewTone => {
                        koru_ui::icon_button::IconButton::new(koru_draw::icons::PLAY, part.rect)
                            .with_state(koru_ui::icon_button::State {
                                hovered,
                                active: false,
                                disabled: !part.enabled,
                            })
                            .draw(scene, root, engine);
                    }
                    Control::Delivery => {
                        // What the device last confirmed, not what was just
                        // asked for: a disconnected Agent must not read as a
                        // change of choice.
                        Toggle::new(self.last_enabled.unwrap_or(false))
                            .draw(scene, root, engine, part.rect, false, hovered, focused);
                    }
                    _ => {
                        ActionButton {
                            label: button_label(lang, *control),
                            style: button_style(*control),
                            trailing_icon: None,
                        }
                        .draw_focused(scene, root, engine, part.rect, hovered, focused);
                    }
                }
            }
        }
    }

    fn health_rows(&self, lang: koru_i18n::Lang) -> [(koru_i18n::Key, String); 3] {
        let unknown =
            koru_i18n::t(lang, koru_i18n::Key::ReminderAuthorizationUnavailable).to_owned();
        let device = self.last_enabled.map_or_else(
            || unknown.clone(),
            |enabled| {
                koru_i18n::t(
                    lang,
                    if enabled {
                        koru_i18n::Key::ReminderDeliveryEnabled
                    } else {
                        koru_i18n::Key::ReminderDeliveryDisabled
                    },
                )
                .to_owned()
            },
        );
        let (next, sync) = match &self.state.health {
            HealthState::Ready(value) => (
                value
                    .next_deadline_ms
                    .map(|ms| crate::format::absolute_local(lang, ms))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        koru_i18n::t(lang, koru_i18n::Key::ReminderHealthNoneScheduled).to_owned()
                    }),
                value
                    .last_successful_sync_ms
                    .map(|ms| crate::format::absolute_local(lang, ms))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        koru_i18n::t(lang, koru_i18n::Key::ReminderHealthNeverSynced).to_owned()
                    }),
            ),
            _ => (unknown.clone(), unknown),
        };
        [
            (koru_i18n::Key::ReminderHealthDevice, device),
            (koru_i18n::Key::ReminderHealthNext, next),
            (koru_i18n::Key::ReminderHealthLastSync, sync),
        ]
    }
}

// ----------------------------------------------------------------- the page

/// The two card titles this page adds. English, not keyed, for the reason the
/// other Settings pages give (`koru-settings-ui/src/global.rs`, Q-V75).

/// The read-only alpha the shared surface draws with.
const DIMMED: f32 = 0.45;

const WEEK_STARTS: [WeekStartPreference; 3] = [
    WeekStartPreference::System,
    WeekStartPreference::Monday,
    WeekStartPreference::Sunday,
];

/// Between two controls in one row.
const PART_GAP: f32 = 8.0;
/// A row's controls take at most this share of it inline; more than that and
/// they go under the row's text, so a long translation never squeezes the name.
const INLINE_SHARE: f32 = 0.55;

/// What a row carries at its right end, before it is placed.
struct Part {
    kind: PartKind,
    w: f32,
    h: f32,
    enabled: bool,
}

#[derive(Clone)]
enum PartKind {
    Control(Control),
    /// A segmented control; one target per segment.
    Segmented {
        control: Segmented,
        controls: Vec<Control>,
    },
    /// A value read out on the right, pressable by nobody.
    Value(String),
}

impl Part {
    fn control(control: Control, w: f32, h: f32, enabled: bool) -> Self {
        Self {
            kind: PartKind::Control(control),
            w,
            h,
            enabled,
        }
    }

    fn segmented(control: Segmented, controls: Vec<Control>, enabled: bool) -> Self {
        Self {
            w: control.bounds.w,
            h: control.bounds.h,
            kind: PartKind::Segmented { control, controls },
            enabled,
        }
    }

    fn value(value: String, w: f32) -> Self {
        Self {
            kind: PartKind::Value(value),
            w,
            h: theme::role::row().metrics.line_height,
            enabled: true,
        }
    }
}

/// A part where it landed.
struct Placed {
    kind: PartKind,
    rect: Rect,
    enabled: bool,
}

struct RowSpec {
    rect: Rect,
    label: String,
    hint: Option<String>,
    text_w: f32,
    parts: Vec<Placed>,
}

pub(super) struct CardSpec {
    rect: Rect,
    title: Option<String>,
    aside: Option<String>,
    rows: Vec<RowSpec>,
}

/// The cards being laid out.
struct Page {
    column: Rect,
    y: f32,
    cards: Vec<CardSpec>,
    open: Option<(Option<String>, Option<String>, f32, Vec<RowSpec>)>,
}

impl Page {
    fn new(column: Rect, top: f32) -> Self {
        Self {
            column,
            y: top,
            cards: Vec::new(),
            open: None,
        }
    }

    fn card(&mut self, title: Option<&str>, aside: Option<&str>) {
        self.close();
        let header = Card { title, aside }.header_h();
        self.open = Some((
            title.map(str::to_owned),
            aside.map(str::to_owned),
            self.y,
            Vec::new(),
        ));
        self.y += header;
    }

    fn close(&mut self) {
        let Some((title, aside, top, rows)) = self.open.take() else {
            return;
        };
        if rows.is_empty() {
            self.y = top;
            return;
        }
        self.cards.push(CardSpec {
            rect: Rect::new(self.column.x, top, self.column.w, self.y - top),
            title,
            aside,
            rows,
        });
        self.y += Card::GAP;
    }

    /// One row. Its parts sit at the right end in a line when they fit the
    /// row's share; otherwise they go under its text, wrapping, so nothing is
    /// ever squeezed out of the column.
    fn row(
        &mut self,
        engine: &mut TextEngine,
        targets: &mut Vec<Target>,
        label: &str,
        hint: Option<&str>,
        _alert: bool,
        parts: Vec<Part>,
    ) {
        let w = self.column.w;
        let inline_w = parts.iter().map(|p| p.w).sum::<f32>()
            + PART_GAP * parts.len().saturating_sub(1) as f32;
        let inline_h = parts.iter().map(|p| p.h).fold(0.0, f32::max);
        let inline = inline_w <= w * INLINE_SHARE;
        let row = CardRow {
            label,
            hint,
            link: None,
            chip: None,
        };
        let (text_w, h) = if inline {
            let text_w = CardRow::text_w(w, inline_w);
            (
                text_w,
                row.height(engine, text_w, inline_h).max(CardRow::MIN_H),
            )
        } else {
            let text_w = CardRow::text_w(w, 0.0);
            let text = row.height(engine, text_w, 0.0) - CardRow::PAD_Y * 2.0;
            let lines = wrap(&parts, text_w);
            let stacked: f32 = lines.iter().map(|line| line.1).sum::<f32>()
                + PART_GAP * lines.len().saturating_sub(1) as f32;
            (text_w, text + PART_GAP + stacked + CardRow::PAD_Y * 2.0)
        };
        let rect = Rect::new(self.column.x, self.y, w, h);
        let mut placed = Vec::with_capacity(parts.len());
        if inline {
            let slot = CardRow::control_slot(rect, inline_w, inline_h);
            let mut x = slot.x;
            for part in parts {
                let at = Rect::new(x, slot.y + (slot.h - part.h) * 0.5, part.w, part.h);
                x += part.w + PART_GAP;
                placed.push(place(part, at, targets));
            }
        } else {
            let lines = wrap(&parts, text_w);
            let mut y = rect.y + rect.h
                - CardRow::PAD_Y
                - lines.iter().map(|line| line.1).sum::<f32>()
                - PART_GAP * lines.len().saturating_sub(1) as f32;
            let mut parts = parts.into_iter();
            for (count, line_h) in lines {
                let mut x = rect.x + CardRow::PAD_X;
                for part in parts.by_ref().take(count) {
                    let at = Rect::new(x, y + (line_h - part.h) * 0.5, part.w, part.h);
                    x += part.w + PART_GAP;
                    placed.push(place(part, at, targets));
                }
                y += line_h + PART_GAP;
            }
        }
        if let Some((_, _, _, rows)) = self.open.as_mut() {
            rows.push(RowSpec {
                rect,
                label: label.to_owned(),
                hint: hint.map(str::to_owned),
                text_w,
                parts: placed,
            });
        }
        self.y += h;
    }

    fn finish(mut self) -> (Vec<CardSpec>, f32) {
        self.close();
        let bottom = self
            .cards
            .last()
            .map_or(self.y, |card| card.rect.y + card.rect.h);
        (self.cards, bottom)
    }
}

/// Lines of parts that fit `width`: how many on each, and the line's height.
fn wrap(parts: &[Part], width: f32) -> Vec<(usize, f32)> {
    let mut lines: Vec<(usize, f32)> = Vec::new();
    let mut used = 0.0;
    for part in parts {
        match lines.last_mut() {
            Some((count, h)) if used + PART_GAP + part.w <= width => {
                *count += 1;
                *h = h.max(part.h);
                used += PART_GAP + part.w;
            }
            _ => {
                lines.push((1, part.h));
                used = part.w;
            }
        }
    }
    lines
}

/// Register a placed part's targets and keep it for drawing.
fn place(part: Part, rect: Rect, targets: &mut Vec<Target>) -> Placed {
    match &part.kind {
        PartKind::Control(control) => targets.push(Target {
            control: *control,
            rect,
            enabled: part.enabled,
        }),
        PartKind::Segmented { .. } => {}
        PartKind::Value(_) => {}
    }
    let kind = match part.kind {
        PartKind::Segmented { control, controls } => {
            // Re-laid at its final position: the segments are its geometry.
            let moved = Segmented {
                bounds: rect,
                segments: control
                    .segments
                    .iter()
                    .map(|s| Rect::new(s.x + rect.x, s.y + rect.y, s.w, s.h))
                    .collect(),
            };
            for (segment, control) in moved.segments.iter().zip(&controls) {
                targets.push(Target {
                    control: *control,
                    rect: *segment,
                    enabled: part.enabled,
                });
            }
            PartKind::Segmented {
                control: moved,
                controls,
            }
        }
        other => other,
    };
    Placed {
        kind,
        rect,
        enabled: part.enabled,
    }
}

fn button_label(lang: koru_i18n::Lang, control: Control) -> &'static str {
    let key = match control {
        Control::ClearEndOfDay => koru_i18n::Key::ReminderEndOfDayClear,
        Control::RetrySettings => koru_i18n::Key::ReminderRetry,
        Control::AllowNotifications => koru_i18n::Key::ReminderAllowNotifications,
        Control::OpenNotificationSettings | Control::BannerOpenNotificationSettings => {
            koru_i18n::Key::ReminderOpenNotificationSettings
        }
        Control::RefreshNotificationStatus => koru_i18n::Key::ReminderRefreshStatus,
        Control::RepairDelivery => koru_i18n::Key::ReminderRepairDelivery,
        Control::SendTest => koru_i18n::Key::ReminderTestNotification,
        Control::StartupEnable => koru_i18n::Key::ReminderStartupEnable,
        Control::StartupDecline => koru_i18n::Key::ReminderStartupDecline,
        _ => return "",
    };
    koru_i18n::t(lang, key)
}

/// The one action a situation asks for is primary; everything else is the
/// quiet alternative beside it.
fn button_style(control: Control) -> ActionStyle {
    if matches!(
        control,
        Control::AllowNotifications
            | Control::StartupEnable
            | Control::RepairDelivery
            | Control::BannerOpenNotificationSettings
    ) {
        ActionStyle::Primary
    } else {
        ActionStyle::Secondary
    }
}

fn settings(state: &SettingsState) -> Option<&koru_store_core::reminders::ReminderSettings> {
    match state {
        SettingsState::Ready(value) | SettingsState::Saving(value) => Some(value),
        _ => None,
    }
}

fn profile_control(control: Control) -> bool {
    matches!(
        control,
        Control::EndOfDay
            | Control::ClearEndOfDay
            | Control::WeekStart(_)
            | Control::ToneMenu
            | Control::PreviewTone
            | Control::RetrySettings
    )
}

fn week_start_label(lang: koru_i18n::Lang, week_start: WeekStartPreference) -> &'static str {
    let key = match week_start {
        WeekStartPreference::System => koru_i18n::Key::ReminderWeekStartSystem,
        WeekStartPreference::Monday => koru_i18n::Key::ReminderWeekStartMonday,
        WeekStartPreference::Sunday => koru_i18n::Key::ReminderWeekStartSunday,
    };
    koru_i18n::t(lang, key)
}

#[cfg(test)]
mod tests;
