// SPDX-License-Identifier: MPL-2.0

//! Reusable, presentation-only Voice surface.
//!
//! The host owns settings receipts, hotkeys, targets, workers and consent.
//! This crate receives only content-free facts and returns typed intentions.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_i18n::{Key as I18nKey, Lang, t};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

mod hud;
pub use hud::{HUD_HEIGHT, HUD_WIDTH, HudAction, HudState, VoiceHud};

pub const WIDTH: f32 = 520.0;
pub const HEIGHT: f32 = 430.0;

fn shortcut_key_name(physical: PhysicalKey) -> Option<&'static str> {
    Some(match physical {
        PhysicalKey::Code(KeyCode::KeyA) => "A",
        PhysicalKey::Code(KeyCode::KeyB) => "B",
        PhysicalKey::Code(KeyCode::KeyC) => "C",
        PhysicalKey::Code(KeyCode::KeyD) => "D",
        PhysicalKey::Code(KeyCode::KeyE) => "E",
        PhysicalKey::Code(KeyCode::KeyF) => "F",
        PhysicalKey::Code(KeyCode::KeyG) => "G",
        PhysicalKey::Code(KeyCode::KeyH) => "H",
        PhysicalKey::Code(KeyCode::KeyI) => "I",
        PhysicalKey::Code(KeyCode::KeyJ) => "J",
        PhysicalKey::Code(KeyCode::KeyK) => "K",
        PhysicalKey::Code(KeyCode::KeyL) => "L",
        PhysicalKey::Code(KeyCode::KeyM) => "M",
        PhysicalKey::Code(KeyCode::KeyN) => "N",
        PhysicalKey::Code(KeyCode::KeyO) => "O",
        PhysicalKey::Code(KeyCode::KeyP) => "P",
        PhysicalKey::Code(KeyCode::KeyQ) => "Q",
        PhysicalKey::Code(KeyCode::KeyR) => "R",
        PhysicalKey::Code(KeyCode::KeyS) => "S",
        PhysicalKey::Code(KeyCode::KeyT) => "T",
        PhysicalKey::Code(KeyCode::KeyU) => "U",
        PhysicalKey::Code(KeyCode::KeyV) => "V",
        PhysicalKey::Code(KeyCode::KeyW) => "W",
        PhysicalKey::Code(KeyCode::KeyX) => "X",
        PhysicalKey::Code(KeyCode::KeyY) => "Y",
        PhysicalKey::Code(KeyCode::KeyZ) => "Z",
        PhysicalKey::Code(KeyCode::Digit0) => "0",
        PhysicalKey::Code(KeyCode::Digit1) => "1",
        PhysicalKey::Code(KeyCode::Digit2) => "2",
        PhysicalKey::Code(KeyCode::Digit3) => "3",
        PhysicalKey::Code(KeyCode::Digit4) => "4",
        PhysicalKey::Code(KeyCode::Digit5) => "5",
        PhysicalKey::Code(KeyCode::Digit6) => "6",
        PhysicalKey::Code(KeyCode::Digit7) => "7",
        PhysicalKey::Code(KeyCode::Digit8) => "8",
        PhysicalKey::Code(KeyCode::Digit9) => "9",
        PhysicalKey::Code(KeyCode::Space) => "Space",
        PhysicalKey::Code(KeyCode::BracketLeft) => "[",
        PhysicalKey::Code(KeyCode::BracketRight) => "]",
        PhysicalKey::Code(KeyCode::Backslash) => "\\",
        PhysicalKey::Code(KeyCode::Slash) => "/",
        _ => return None,
    })
}

#[cfg(windows)]
fn shortcut_modifiers(mods: ModifiersState) -> Option<&'static str> {
    Some(if mods == ModifiersState::CONTROL | ModifiersState::SHIFT {
        "Ctrl+Shift"
    } else if mods == ModifiersState::CONTROL | ModifiersState::ALT {
        "Ctrl+Alt"
    } else if mods == ModifiersState::CONTROL | ModifiersState::SUPER {
        "Ctrl+Win"
    } else if mods == ModifiersState::SHIFT | ModifiersState::SUPER {
        "Shift+Win"
    } else {
        return None;
    })
}

#[cfg(target_os = "macos")]
fn shortcut_modifiers(mods: ModifiersState) -> Option<&'static str> {
    Some(if mods == ModifiersState::CONTROL | ModifiersState::SHIFT {
        "Ctrl+Shift"
    } else if mods == ModifiersState::CONTROL | ModifiersState::ALT {
        "Ctrl+Option"
    } else if mods == ModifiersState::SHIFT | ModifiersState::ALT {
        "Shift+Option"
    } else {
        return None;
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn shortcut_modifiers(mods: ModifiersState) -> Option<&'static str> {
    Some(if mods == ModifiersState::CONTROL | ModifiersState::SHIFT {
        "Ctrl+Shift"
    } else if mods == ModifiersState::CONTROL | ModifiersState::ALT {
        "Ctrl+Alt"
    } else {
        return None;
    })
}

fn picker_text(lang: Lang) -> &'static str {
    #[cfg(target_os = "macos")]
    return t(lang, I18nKey::VoiceGlobalPickerMac);
    #[cfg(not(target_os = "macos"))]
    t(lang, I18nKey::VoiceGlobalPicker)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saved {
    NeverEnabled,
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    Enable,
    Disable,
    Startup,
    Shortcut,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Preparing,
    Listening,
    Finalising,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    ShortcutTaken,
    ProtectedTarget,
    TargetChanged,
    TargetUnavailable,
    ConsentRequired,
    WorkerUnavailable,
    ModelUnavailable,
    Disconnected,
    TimedOut,
    Refused,
    SettingRefused,
    UnsavedRuntime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewModel {
    pub saved: Saved,
    pub effective_enabled: bool,
    pub start_on_login: bool,
    pub start_on_login_available: bool,
    pub shortcut: Option<String>,
    pub requested_shortcut: Option<String>,
    pub pending: Option<Pending>,
    pub phase: Phase,
    pub problem: Option<Problem>,
}

impl Default for ViewModel {
    fn default() -> Self {
        Self {
            saved: Saved::NeverEnabled,
            effective_enabled: false,
            start_on_login: false,
            start_on_login_available: true,
            shortcut: None,
            requested_shortcut: None,
            pending: None,
            phase: Phase::Idle,
            problem: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Enable,
    Disable,
    SetStartOnLogin(bool),
    PickShortcut,
    SetShortcut(String),
    Start,
    Cancel,
    Retry,
    Close,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputOutcome {
    pub changed: bool,
    pub action: Option<Action>,
}

impl InputOutcome {
    fn changed() -> Self {
        Self {
            changed: true,
            action: None,
        }
    }

    fn action(action: Option<Action>) -> Self {
        Self {
            changed: action.is_some(),
            action,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Enable,
    Startup,
    Shortcut,
    Start,
    Retry,
}

#[derive(Clone, Copy)]
struct ControlRect {
    control: Control,
    rect: Rect,
}

struct Layout {
    controls: Vec<ControlRect>,
    status_y: f32,
}

fn layout(model: &ViewModel, width: f32, picking: bool) -> Layout {
    let has_status = picking
        || model.problem.is_some()
        || model.shortcut.is_none()
        || model.phase != Phase::Idle;
    let top = if model.saved == Saved::NeverEnabled {
        168.0
    } else {
        124.0
    } + if has_status { 48.0 } else { 0.0 };
    let mut controls = vec![Control::Enable];
    if model.saved != Saved::NeverEnabled && model.start_on_login_available {
        controls.push(Control::Startup);
    }
    controls.extend([Control::Shortcut, Control::Start]);
    if model.problem.is_some() && model.problem != Some(Problem::ShortcutTaken) {
        controls.push(Control::Retry);
    }
    Layout {
        controls: controls
            .into_iter()
            .enumerate()
            .map(|(index, control)| ControlRect {
                control,
                rect: Rect::new(42.0, top + index as f32 * 48.0, width - 84.0, 42.0),
            })
            .collect(),
        status_y: if model.saved == Saved::NeverEnabled {
            142.0
        } else {
            96.0
        },
    }
}

fn phase_text(lang: Lang, phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Idle => None,
        Phase::Preparing => Some(t(lang, I18nKey::VoiceStarting)),
        Phase::Listening => Some(t(lang, I18nKey::VoiceListening)),
        Phase::Finalising => Some(t(lang, I18nKey::VoiceFinishing)),
    }
}

fn problem_text(lang: Lang, problem: Problem) -> &'static str {
    match problem {
        Problem::ShortcutTaken => t(lang, I18nKey::VoiceGlobalShortcutTaken),
        Problem::ProtectedTarget => t(lang, I18nKey::VoiceGlobalProtected),
        Problem::TargetChanged => t(lang, I18nKey::VoiceGlobalTargetChanged),
        Problem::TargetUnavailable => t(lang, I18nKey::VoiceGlobalTargetUnavailable),
        Problem::ConsentRequired => t(lang, I18nKey::VoiceErrorConsent),
        Problem::WorkerUnavailable => t(lang, I18nKey::VoiceErrorUnavailable),
        Problem::ModelUnavailable => t(lang, I18nKey::VoiceErrorModelMissing),
        Problem::Disconnected => t(lang, I18nKey::VoiceGlobalDisconnected),
        Problem::TimedOut => t(lang, I18nKey::VoiceGlobalTimedOut),
        Problem::Refused => t(lang, I18nKey::VoiceGlobalRefused),
        Problem::SettingRefused => t(lang, I18nKey::VoiceGlobalSettingRefused),
        Problem::UnsavedRuntime => t(lang, I18nKey::VoiceGlobalUnsavedRuntime),
    }
}

pub struct VoiceSurface {
    lang: Lang,
    model: ViewModel,
    focus: Control,
    hover: Option<Control>,
    pressed: Option<Control>,
    picking: bool,
}

impl VoiceSurface {
    pub fn new(lang: Lang, model: ViewModel) -> Self {
        Self {
            lang,
            model,
            focus: Control::Enable,
            hover: None,
            pressed: None,
            picking: false,
        }
    }

    pub fn model(&self) -> &ViewModel {
        &self.model
    }

    pub fn set_model(&mut self, model: ViewModel) -> bool {
        if self.model == model {
            return false;
        }
        self.model = model;
        let controls = self.controls();
        if !controls.contains(&self.focus) {
            self.focus = controls.first().copied().unwrap_or(Control::Enable);
        }
        true
    }

    pub fn picking(&self) -> bool {
        self.picking
    }

    fn controls(&self) -> Vec<Control> {
        layout(&self.model, WIDTH, self.picking)
            .controls
            .into_iter()
            .map(|control| control.control)
            .collect()
    }

    fn move_focus(&mut self, backwards: bool) {
        let controls = self.controls();
        if controls.is_empty() {
            return;
        }
        let current = controls
            .iter()
            .position(|control| *control == self.focus)
            .unwrap_or(0);
        let next = if backwards {
            current.checked_sub(1).unwrap_or(controls.len() - 1)
        } else {
            (current + 1) % controls.len()
        };
        self.focus = controls[next];
    }

    pub fn key(&mut self, key: &Key, mods: ModifiersState) -> InputOutcome {
        if self.picking {
            if matches!(key, Key::Named(NamedKey::Escape)) {
                self.picking = false;
                return InputOutcome::changed();
            }
            return InputOutcome::default();
        }
        match key {
            Key::Named(NamedKey::Escape) => InputOutcome::action(Some(Action::Close)),
            Key::Named(NamedKey::Tab) => {
                self.move_focus(mods.shift_key());
                InputOutcome::changed()
            }
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft) => {
                self.move_focus(true);
                InputOutcome::changed()
            }
            Key::Named(NamedKey::ArrowDown | NamedKey::ArrowRight) => {
                self.move_focus(false);
                InputOutcome::changed()
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                InputOutcome::action(self.activate(self.focus))
            }
            _ => InputOutcome::default(),
        }
    }

    /// Capture a picker chord from the physical key, not the layout-produced
    /// character. On Windows Ctrl+Alt may be interpreted as AltGr, so the
    /// logical key can be unidentified or carry a different character even
    /// though the key Agent must register is unambiguous.
    pub fn shortcut_key(&mut self, physical: PhysicalKey, mods: ModifiersState) -> InputOutcome {
        if !self.picking {
            return InputOutcome::default();
        }
        let Some(modifiers) = shortcut_modifiers(mods) else {
            return InputOutcome::default();
        };
        let Some(name) = shortcut_key_name(physical) else {
            return InputOutcome::default();
        };
        self.picking = false;
        InputOutcome::action(Some(Action::SetShortcut(format!("{modifiers}+{name}"))))
    }

    pub fn pointer_moved(&mut self, x: f32, y: f32) -> bool {
        let next = layout(&self.model, WIDTH, self.picking)
            .controls
            .into_iter()
            .find(|control| control.rect.contains(x, y))
            .map(|control| control.control);
        let changed = self.hover != next;
        self.hover = next;
        changed
    }

    pub fn pointer_left(&mut self) -> bool {
        self.hover.take().is_some() | self.pressed.take().is_some()
    }

    pub fn hovering_action(&self) -> bool {
        self.hover
            .is_some_and(|control| self.action_for(control).is_some())
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) -> bool {
        let hit = layout(&self.model, WIDTH, self.picking)
            .controls
            .into_iter()
            .find(|control| control.rect.contains(x, y))
            .map(|control| control.control);
        let Some(hit) = hit else {
            return false;
        };
        self.focus = hit;
        self.pressed = self.action_for(hit).is_some().then_some(hit);
        true
    }

    pub fn mouse_up(&mut self) -> Option<Action> {
        let pressed = self.pressed.take()?;
        (self.hover == Some(pressed))
            .then(|| self.activate(pressed))
            .flatten()
    }

    pub fn click(&mut self, x: f32, y: f32) -> Option<Action> {
        if !self.pointer_moved(x, y) && self.hover.is_none() {
            return None;
        }
        self.mouse_down(x, y);
        self.mouse_up()
    }

    fn action_for(&self, control: Control) -> Option<Action> {
        if control == Control::Start && self.model.phase != Phase::Idle {
            return Some(Action::Cancel);
        }
        if self.model.pending.is_some() {
            return None;
        }
        match control {
            Control::Enable if self.model.effective_enabled => Some(Action::Disable),
            Control::Enable => Some(Action::Enable),
            Control::Startup => Some(Action::SetStartOnLogin(!self.model.start_on_login)),
            Control::Shortcut => Some(Action::PickShortcut),
            Control::Start if self.model.phase == Phase::Idle && self.model.effective_enabled => {
                Some(Action::Start)
            }
            Control::Retry
                if self.model.problem.is_some()
                    && self.model.problem != Some(Problem::ShortcutTaken) =>
            {
                Some(Action::Retry)
            }
            _ => None,
        }
    }

    fn activate(&mut self, control: Control) -> Option<Action> {
        let action = self.action_for(control)?;
        if action == Action::PickShortcut {
            self.picking = true;
        }
        Some(action)
    }

    pub fn draw(&self, canvas: &mut Scene, engine: &mut TextEngine, w: f32, h: f32) {
        let palette = theme::p();
        scene::fill(
            canvas,
            Affine::IDENTITY,
            palette.bg,
            &scene::rect(0.0, 0.0, w, h),
        );
        // The card is the top-level window's content, not another floating
        // surface inside it. The native window already supplies elevation;
        // drawing a second panel shadow here made an unintended inner box.
        let card = scene::rrect(18.0, 18.0, w - 36.0, h - 36.0, 12.0);
        scene::fill(canvas, Affine::IDENTITY, palette.panel_bg, &card);
        scene::stroke(
            canvas,
            Affine::IDENTITY,
            palette.panel_border,
            theme::BORDER_W,
            &card,
        );
        let title = engine.simple_layout(
            t(self.lang, I18nKey::VoiceGlobalTitle),
            theme::role::subtitle(),
        );
        scene::draw_layout_first_line(
            canvas,
            Affine::IDENTITY,
            &title,
            42.0,
            42.0,
            Some(palette.text),
        );

        if self.model.saved == Saved::NeverEnabled {
            let notice = engine.simple_layout_wrapped(
                t(
                    self.lang,
                    if self.model.start_on_login_available {
                        I18nKey::VoiceGlobalNotice
                    } else {
                        I18nKey::VoiceGlobalNoticeNoStartup
                    },
                ),
                theme::role::body(),
                w - 84.0,
            );
            scene::draw_layout(
                canvas,
                Affine::IDENTITY,
                &notice,
                42.0,
                70.0,
                Some(palette.text_dim),
            );
        }

        let geometry = layout(&self.model, w, self.picking);
        let status = if self.picking {
            Some(picker_text(self.lang))
        } else {
            self.model
                .problem
                .map(|problem| problem_text(self.lang, problem))
                .or_else(|| {
                    self.model
                        .shortcut
                        .is_none()
                        .then(|| t(self.lang, I18nKey::VoiceGlobalNoShortcut))
                })
                .or_else(|| phase_text(self.lang, self.model.phase))
        };
        if let Some(status) = status {
            let status = engine.simple_layout_wrapped(status, theme::role::body(), w - 84.0);
            scene::draw_layout(
                canvas,
                Affine::IDENTITY,
                &status,
                42.0,
                geometry.status_y,
                Some(if self.model.problem.is_some() {
                    palette.caret
                } else {
                    palette.text_dim
                }),
            );
        }

        for item in geometry.controls {
            let enabled = self.action_for(item.control).is_some();
            if self.pressed == Some(item.control) {
                scene::fill(
                    canvas,
                    Affine::IDENTITY,
                    palette.row_selected,
                    &scene::rrect(item.rect.x, item.rect.y, item.rect.w, item.rect.h, 8.0),
                );
            } else if self.hover == Some(item.control) {
                scene::fill(
                    canvas,
                    Affine::IDENTITY,
                    palette.hover_tint,
                    &scene::rrect(item.rect.x, item.rect.y, item.rect.w, item.rect.h, 8.0),
                );
            }
            if self.focus == item.control {
                scene::stroke(
                    canvas,
                    Affine::IDENTITY,
                    palette.focus_ring,
                    1.5,
                    &scene::rrect(item.rect.x, item.rect.y, item.rect.w, item.rect.h, 8.0),
                );
            }

            let mut label = match item.control {
                Control::Enable
                    if self.model.effective_enabled
                        || self.model.pending == Some(Pending::Disable) =>
                {
                    t(self.lang, I18nKey::VoiceGlobalDisable).to_owned()
                }
                Control::Enable => t(self.lang, I18nKey::VoiceGlobalEnable).to_owned(),
                Control::Startup => t(self.lang, I18nKey::VoiceGlobalStartup).to_owned(),
                Control::Shortcut => {
                    let shortcut = self
                        .model
                        .requested_shortcut
                        .as_deref()
                        .or(self.model.shortcut.as_deref())
                        .unwrap_or("—");
                    if self.picking {
                        format!("{}: …", t(self.lang, I18nKey::VoiceGlobalShortcut))
                    } else {
                        format!("{}: {shortcut}", t(self.lang, I18nKey::VoiceGlobalShortcut))
                    }
                }
                Control::Start if self.model.phase == Phase::Idle => {
                    t(self.lang, I18nKey::VoiceGlobalStart).to_owned()
                }
                Control::Start => t(self.lang, I18nKey::VoiceGlobalCancel).to_owned(),
                Control::Retry => t(self.lang, I18nKey::VoiceGlobalRetry).to_owned(),
            };
            if self.model.pending.is_some() && item.control != Control::Retry {
                label.push('…');
            }
            let text = engine.simple_layout(&label, theme::role::row());
            let color = if enabled {
                palette.text
            } else {
                palette.text_dim
            };
            let text_x = if item.control == Control::Startup {
                item.rect.x + 38.0
            } else {
                item.rect.x + 12.0
            };
            scene::draw_layout_first_line(
                canvas,
                Affine::IDENTITY,
                &text,
                text_x,
                item.rect.y + 11.0,
                Some(color),
            );
            if item.control == Control::Startup {
                let box_color = if self.model.start_on_login {
                    palette.checkbox_done
                } else {
                    palette.checkbox
                };
                scene::fill(
                    canvas,
                    Affine::IDENTITY,
                    box_color,
                    &scene::rrect(item.rect.x + 12.0, item.rect.y + 12.0, 18.0, 18.0, 4.0),
                );
                if self.model.start_on_login {
                    scene::fill(
                        canvas,
                        Affine::IDENTITY,
                        palette.checkbox_mark,
                        &scene::rrect(item.rect.x + 17.0, item.rect.y + 17.0, 8.0, 8.0, 2.0),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
