// SPDX-License-Identifier: MPL-2.0

//! Agent-owned windows for Koru Voice.
//!
//! Settings has precedence over the passive HUD. The HUD receives only the
//! coordinator's content-free snapshot and content-free monitor geometry.

#[cfg(test)]
use std::cell::Cell;
use std::time::{Duration, Instant};

use koru_draw::text::TextEngine;
use vello::Scene;
use winit::dpi::PhysicalPosition;
use winit::window::{Icon, WindowAttributes};

const TERMINAL_LINGER: Duration = Duration::from_millis(2500);

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkCounts {
    pub window_requests: u64,
    pub draws: u64,
    pub redraw_requests: u64,
    pub deadline_expirations: u64,
    pub level_updates: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HudPlacement {
    position: PhysicalPosition<i32>,
}

pub(crate) struct VoiceWindowHost {
    surface: Option<koru_voice_ui::VoiceSurface>,
    hud: Option<koru_voice_ui::VoiceHud>,
    terminal_until: Option<Instant>,
    placement: Option<HudPlacement>,
    generation: u64,
    #[cfg(test)]
    counts: Cell<WorkCounts>,
}

impl Default for VoiceWindowHost {
    fn default() -> Self {
        Self {
            surface: None,
            hud: None,
            terminal_until: None,
            placement: None,
            generation: 1,
            #[cfg(test)]
            counts: Cell::new(WorkCounts::default()),
        }
    }
}

impl VoiceWindowHost {
    pub fn surface(&self) -> Option<&koru_voice_ui::VoiceSurface> {
        self.surface.as_ref()
    }
    pub fn surface_mut(&mut self) -> Option<&mut koru_voice_ui::VoiceSurface> {
        self.surface.as_mut()
    }
    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }
    pub fn has_hud(&self) -> bool {
        self.hud.is_some() && self.surface.is_none()
    }
    pub fn visible(&self) -> bool {
        self.has_surface() || self.has_hud()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn window(&self, icon: Option<Icon>) -> WindowAttributes {
        #[cfg(test)]
        {
            let mut counts = self.counts.get();
            counts.window_requests += 1;
            self.counts.set(counts);
        }
        if self.has_surface() {
            return winit::window::Window::default_attributes()
                .with_title("Koru Voice")
                .with_inner_size(winit::dpi::LogicalSize::new(
                    koru_voice_ui::WIDTH,
                    koru_voice_ui::HEIGHT,
                ))
                .with_resizable(false)
                .with_window_icon(icon);
        }
        let mut attributes = winit::window::Window::default_attributes()
            .with_title("Koru Voice")
            .with_inner_size(winit::dpi::LogicalSize::new(
                koru_voice_ui::HUD_WIDTH,
                koru_voice_ui::HUD_HEIGHT,
            ))
            .with_transparent(true)
            .with_resizable(false)
            .with_decorations(false)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
            .with_window_icon(icon);
        if let Some(placement) = self.placement {
            attributes = attributes.with_position(placement.position);
        }
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attributes = attributes.with_skip_taskbar(true);
        }
        attributes
    }

    pub fn window_activation(&self) -> koru_window::WindowActivation {
        if self.has_hud() {
            koru_window::WindowActivation::Passive
        } else {
            koru_window::WindowActivation::Focused
        }
    }

    pub fn draw(
        &self,
        scene: &mut Scene,
        engine: &mut TextEngine,
        width: f32,
        height: f32,
    ) -> bool {
        #[cfg(test)]
        {
            let mut counts = self.counts.get();
            counts.draws += 1;
            self.counts.set(counts);
        }
        if let Some(surface) = &self.surface {
            surface.draw(scene, engine, width, height);
            return true;
        }
        if let Some(hud) = &self.hud {
            hud.draw(scene, engine, width, height);
            return true;
        }
        false
    }

    pub fn hud_action_at(&self, x: f32, y: f32) -> Option<koru_voice_ui::HudAction> {
        self.hud.as_ref().and_then(|hud| hud.action_at(x, y))
    }

    pub fn refresh_hud_placement(&mut self) -> bool {
        false
    }

    #[cfg(test)]
    pub fn work_counts(&self) -> WorkCounts {
        self.counts.get()
    }

    pub fn open_surface(&mut self, surface: koru_voice_ui::VoiceSurface) {
        let changed_kind = !self.has_surface();
        self.surface = Some(surface);
        self.hud = None;
        self.terminal_until = None;
        if changed_kind {
            self.bump_generation();
        }
    }

    pub fn close_surface(&mut self) -> bool {
        let changed = self.surface.take().is_some();
        if changed {
            self.bump_generation();
        }
        changed
    }

    pub fn close_all(&mut self) -> bool {
        let surface = self.close_surface();
        self.hide_hud() | surface
    }

    pub fn set_surface_model(&mut self, model: koru_voice_ui::ViewModel) -> bool {
        self.surface
            .as_mut()
            .is_some_and(|surface| surface.set_model(model))
    }

    pub fn sync_hud(
        &mut self,
        snapshot: crate::dictation_coordinator::Snapshot,
        show_terminal_failure: bool,
        now: Instant,
    ) -> bool {
        if self.surface.is_some() {
            return self.hide_hud();
        }
        use crate::dictation_coordinator::Phase;
        let state = match snapshot.phase {
            Phase::Listening => {
                self.terminal_until = None;
                Some(koru_voice_ui::HudState::recording(snapshot.level.step()))
            }
            Phase::Finalising => {
                self.terminal_until = None;
                Some(koru_voice_ui::HudState::Finalising)
            }
            Phase::Idle if show_terminal_failure && snapshot.refusal.is_some() => {
                self.terminal_until = Some(now + TERMINAL_LINGER);
                Some(if terminal_is_failure(snapshot.refusal) {
                    koru_voice_ui::HudState::Failed
                } else {
                    koru_voice_ui::HudState::Refused
                })
            }
            Phase::Idle if self.terminal_until.is_some_and(|until| now < until) => {
                return false;
            }
            Phase::Idle => None,
        };
        let Some(state) = state else {
            return self.hide_hud();
        };
        let Some(placement) = snapshot.display.map(placement_for) else {
            return self.hide_hud();
        };
        if let Some(hud) = self.hud.as_mut() {
            let level_changed = matches!((hud.state(), state),
                (koru_voice_ui::HudState::Recording { level: old }, koru_voice_ui::HudState::Recording { level: new }) if old != new);
            let changed = hud.set_state(state);
            if level_changed {
                self.bump_level_updates();
            }
            let moved = self.placement != Some(placement);
            if moved {
                self.placement = Some(placement);
                self.bump_generation();
            }
            if changed || moved {
                self.bump_redraw_requests();
            }
            return changed || moved;
        }
        let lang =
            koru_i18n::Lang::for_system(koru_window::system_locale::system_locale().as_deref());
        self.placement = Some(placement);
        self.hud = Some(koru_voice_ui::VoiceHud::new(lang, state));
        self.bump_generation();
        self.bump_redraw_requests();
        true
    }

    pub fn maintenance_deadline(&self) -> Option<Instant> {
        self.terminal_until
    }

    pub fn maintain(&mut self, now: Instant) -> bool {
        if self.terminal_until.is_some_and(|until| now >= until) {
            self.terminal_until = None;
            self.bump_deadline_expirations();
            return self.hide_hud();
        }
        false
    }

    pub fn animating(&self) -> bool {
        false
    }

    fn hide_hud(&mut self) -> bool {
        let changed = self.hud.take().is_some();
        self.terminal_until = None;
        self.placement = None;
        if changed {
            self.bump_generation();
            self.bump_redraw_requests();
        }
        changed
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    fn bump_level_updates(&self) {
        #[cfg(test)]
        {
            let mut counts = self.counts.get();
            counts.level_updates += 1;
            self.counts.set(counts);
        }
    }
    fn bump_redraw_requests(&self) {
        #[cfg(test)]
        {
            let mut counts = self.counts.get();
            counts.redraw_requests += 1;
            self.counts.set(counts);
        }
    }
    fn bump_deadline_expirations(&self) {
        #[cfg(test)]
        {
            let mut counts = self.counts.get();
            counts.deadline_expirations += 1;
            self.counts.set(counts);
        }
    }
}

fn terminal_is_failure(refusal: Option<crate::dictation_coordinator::Refusal>) -> bool {
    matches!(
        refusal,
        Some(
            crate::dictation_coordinator::Refusal::Voice(_)
                | crate::dictation_coordinator::Refusal::DirectPartial
        )
    )
}

fn placement_for(display: crate::global_input::TargetDisplay) -> HudPlacement {
    let work = display.work_area;
    let scale = display.dpi.max(96) as f64 / 96.0;
    let width = (f64::from(koru_voice_ui::HUD_WIDTH) * scale).round() as i32;
    let height = (f64::from(koru_voice_ui::HUD_HEIGHT) * scale).round() as i32;
    let margin = (24.0 * scale).round() as i32;
    HudPlacement {
        position: PhysicalPosition::new(
            work.0 + (work.2 - work.0 - width) / 2,
            work.3 - height - margin,
        ),
    }
}

#[cfg(test)]
mod tests;
