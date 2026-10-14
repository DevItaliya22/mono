// SPDX-License-Identifier: MPL-2.0

//! The compact Voice status widget: state, geometry, hit-testing and drawing.
//!
//! It is content-free by type: bounded level and lifecycle only, never a
//! transcript or target identity.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::theme::Brush;
use koru_draw::{scene, theme};
use koru_i18n::{Key, Lang, t};
use parley::Layout;
#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;
use vello::Scene;
use vello::kurbo::Affine;

pub const HUD_WIDTH: f32 = 240.0;
pub const HUD_HEIGHT: f32 = 44.0;
const CELLS: u8 = 9;
const CELL_W: f32 = 3.0;
const CELL_GAP: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudState {
    Recording { level: u8 },
    Finalising,
    Refused,
    Failed,
}

impl HudState {
    pub fn recording(level: u8) -> Self {
        Self::Recording {
            level: level.min(8),
        }
    }

    fn key(self) -> Key {
        match self {
            Self::Recording { .. } => Key::VoiceListening,
            Self::Finalising => Key::VoiceFinishing,
            Self::Refused => Key::VoiceHudRefused,
            Self::Failed => Key::VoiceHudFailed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudAction {
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    meter: Rect,
    stop: Rect,
}

fn geometry(width: f32, height: f32) -> Geometry {
    let meter_w = f32::from(CELLS) * CELL_W + f32::from(CELLS - 1) * CELL_GAP;
    Geometry {
        meter: Rect::new(10.0, (height - 10.0) * 0.5, meter_w, 10.0),
        stop: Rect::new(width - 28.0, 6.0, 22.0, height - 12.0),
    }
}

pub struct VoiceHud {
    lang: Lang,
    state: HudState,
    label: RefCell<Option<(Key, Layout<Brush>)>>,
    #[cfg(test)]
    layout_builds: Cell<u64>,
}

impl VoiceHud {
    pub fn new(lang: Lang, state: HudState) -> Self {
        Self {
            lang,
            state,
            label: RefCell::new(None),
            #[cfg(test)]
            layout_builds: Cell::new(0),
        }
    }

    pub fn state(&self) -> HudState {
        self.state
    }

    pub fn set_state(&mut self, state: HudState) -> bool {
        if self.state == state {
            return false;
        }
        if self.state.key() != state.key() {
            self.label.get_mut().take();
        }
        self.state = state;
        true
    }

    pub fn action_at(&self, x: f32, y: f32) -> Option<HudAction> {
        matches!(
            self.state,
            HudState::Recording { .. } | HudState::Finalising
        )
        .then(|| geometry(HUD_WIDTH, HUD_HEIGHT).stop.contains(x, y))
        .filter(|inside| *inside)
        .map(|_| HudAction::Finish)
    }

    pub fn draw(&self, canvas: &mut Scene, engine: &mut TextEngine, width: f32, height: f32) {
        let p = theme::p();
        let g = geometry(width, height);
        // This is the window edge, not an inset panel: half a logical pixel
        // keeps the one-pixel stroke centred inside the native rounded region.
        scene::floating_panel(
            canvas,
            Affine::IDENTITY,
            0.5,
            0.5,
            width - 1.0,
            height - 1.0,
            &scene::Panel::tooltip(theme::PAGE_RADIUS)
                .fill(p.panel_bg)
                .border(p.panel_border),
        );

        let level = match self.state {
            HudState::Recording { level } => level,
            _ => 0,
        };
        for index in 0..CELLS {
            let x = g.meter.x + f32::from(index) * (CELL_W + CELL_GAP);
            scene::fill(
                canvas,
                Affine::IDENTITY,
                if index < level {
                    p.accent
                } else {
                    p.scrollbar_thumb
                },
                &scene::rrect(x, g.meter.y, CELL_W, g.meter.h, CELL_W * 0.5),
            );
        }

        let key = self.state.key();
        if self
            .label
            .borrow()
            .as_ref()
            .is_none_or(|(cached, _)| *cached != key)
        {
            let layout = engine.simple_layout(t(self.lang, key), theme::role::ui());
            *self.label.borrow_mut() = Some((key, layout));
            #[cfg(test)]
            self.layout_builds.set(self.layout_builds.get() + 1);
        }
        let label = self.label.borrow();
        let (_, label) = label.as_ref().expect("just cached");
        let color = if matches!(self.state, HudState::Refused | HudState::Failed) {
            p.caret
        } else {
            p.text
        };
        scene::draw_layout(
            canvas,
            Affine::IDENTITY,
            label,
            g.meter.x + g.meter.w + 9.0,
            (height - label.height()) * 0.5,
            Some(color),
        );

        if matches!(
            self.state,
            HudState::Recording { .. } | HudState::Finalising
        ) {
            scene::fill(
                canvas,
                Affine::IDENTITY,
                p.panel_border,
                &scene::rect(g.stop.x - 1.0, 10.0, 1.0, height - 20.0),
            );
            scene::fill(
                canvas,
                Affine::IDENTITY,
                p.text,
                &scene::rrect(g.stop.x + 7.0, (height - 8.0) * 0.5, 8.0, 8.0, 1.5),
            );
        }
    }

    #[cfg(test)]
    fn layout_builds(&self) -> u64 {
        self.layout_builds.get()
    }
}

#[cfg(test)]
mod tests;
