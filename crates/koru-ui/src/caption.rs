// SPDX-License-Identifier: MPL-2.0

//! Shared window controls for an application-drawn caption.
//!
//! Geometry and hit testing come from the same [`controls`] answer, as required
//! by `docs/features/design/DESIGN.md`. The widget reports neutral actions; a host maps them to
//! `koru_window::WindowRequest` without making this crate depend on a window,
//! node, workspace, or document. Platform drag, resize, snap, hairline and
//! maximize mechanics remain in `koru-window` (ADR-25).

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{icons, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

/// Host-neutral meaning of one caption control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    WindowMenu,
    Minimise,
    ToggleMaximise,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Control {
    pub action: Action,
    pub rect: Rect,
}

/// The ordinary three direct window controls for a small custom-caption
/// window. Full workspace windows also reserve an overflow menu; focused
/// surfaces such as onboarding do not need one.
pub fn direct_controls(caption: Rect) -> [Control; 3] {
    let width = theme::CAPTION_BTN;
    let right = caption.x + caption.w;
    let at = |n: f32| Rect::new(right - width * n, caption.y, width, caption.h);
    [
        Control {
            action: Action::Minimise,
            rect: at(3.0),
        },
        Control {
            action: Action::ToggleMaximise,
            rect: at(2.0),
        },
        Control {
            action: Action::Close,
            rect: at(1.0),
        },
    ]
}

pub fn direct_hit(caption: Rect, x: f32, y: f32) -> Option<Action> {
    if !caption.contains(x, y) {
        return None;
    }
    direct_controls(caption)
        .into_iter()
        .find(|control| control.rect.contains(x, y))
        .map(|control| control.action)
}

/// The three direct window controls as a [`KoruButtonGroup`](crate::button_group)
/// of [`KoruIconButton`](crate::icon_button)s: the minimise and maximise
/// controls in the default tone, Close in the danger tone. This is the one
/// composition every custom-caption window draws, so the inspector shows the
/// cluster rather than three loose glyphs.
fn direct_group(caption: Rect, state: State) -> crate::button_group::ButtonGroup<Action> {
    crate::button_group::ButtonGroup::new(
        direct_controls(caption)
            .into_iter()
            .map(|control| {
                crate::icon_button::IconButton::new(
                    direct_glyph(control.action, state.maximized),
                    control.rect,
                )
                .on_click(control.action)
                .tone(if control.action == Action::Close {
                    crate::icon_button::Tone::Danger
                } else {
                    crate::icon_button::Tone::Default
                })
                .with_state(crate::icon_button::State {
                    hovered: state.hovered == Some(control.action),
                    active: false,
                    disabled: false,
                })
            })
            .collect::<Vec<_>>(),
    )
}

pub fn draw_direct_controls(
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    caption: Rect,
    state: State,
) {
    direct_group(caption, state).draw(canvas, root, engine);
}

/// The four controls, left to right, with Close owning the far corner.
pub fn controls(caption: Rect) -> [Control; 4] {
    let width = theme::CAPTION_BTN;
    let right = caption.x + caption.w;
    let at = |n: f32| Rect::new(right - width * n, caption.y, width, caption.h);
    [
        Control {
            action: Action::WindowMenu,
            rect: at(4.0),
        },
        Control {
            action: Action::Minimise,
            rect: at(3.0),
        },
        Control {
            action: Action::ToggleMaximise,
            rect: at(2.0),
        },
        Control {
            action: Action::Close,
            rect: at(1.0),
        },
    ]
}

/// Width reserved by the controls, clamped to a narrow caption.
pub fn reserved_width(caption: Rect) -> f32 {
    (theme::CAPTION_BTN * controls(caption).len() as f32).min(caption.w.max(0.0))
}

pub fn hit(caption: Rect, x: f32, y: f32) -> Option<Action> {
    if !caption.contains(x, y) {
        return None;
    }
    controls(caption)
        .into_iter()
        .find(|control| control.rect.contains(x, y))
        .map(|control| control.action)
}

/// The overflow control is static in a container and may morph into Close in
/// the standalone document while its menu opens -- or, in a window that has
/// no menu, it is the pin: Always on Top, drawn active while the window is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MenuVisual {
    #[default]
    Static,
    Morph {
        progress: f32,
        open: bool,
    },
    Pin {
        pinned: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub hovered: Option<Action>,
    pub maximized: bool,
    pub menu: MenuVisual,
}

pub fn draw(
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    caption: Rect,
    state: State,
) {
    for control in controls(caption) {
        if control.action == Action::WindowMenu {
            draw_menu(canvas, root, engine, control.rect, state);
        }
    }
    direct_group(caption, state).draw(canvas, root, engine);
}

fn direct_glyph(action: Action, maximized: bool) -> char {
    match action {
        Action::Minimise => icons::WINDOW_MINIMISE,
        Action::ToggleMaximise if maximized => icons::WINDOW_RESTORE,
        Action::ToggleMaximise => icons::SQUARE,
        Action::Close => icons::WINDOW_CLOSE,
        Action::WindowMenu => unreachable!("the menu has its own painter"),
    }
}

fn draw_menu(canvas: &mut Scene, root: Affine, engine: &mut TextEngine, rect: Rect, state: State) {
    let hovered = state.hovered == Some(Action::WindowMenu);
    let (progress, open) = match state.menu {
        MenuVisual::Morph { progress, open } => (progress, open),
        MenuVisual::Static | MenuVisual::Pin { .. } => {
            let (glyph, active) = match state.menu {
                MenuVisual::Pin { pinned } => (icons::PUSH_PIN, pinned),
                _ => (icons::DOTS_THREE, false),
            };
            crate::icon_button::draw(
                canvas,
                root,
                engine,
                rect,
                glyph,
                crate::icon_button::State {
                    hovered,
                    active,
                    disabled: false,
                },
            );
            return;
        }
    };

    let progress = progress.clamp(0.0, 1.0);
    let palette = theme::p();
    if hovered {
        let chip = crate::icon_button::hover_chip(rect);
        scene::fill(
            canvas,
            root,
            if open {
                palette.close_hover
            } else {
                palette.hover_tint
            },
            &scene::rrect(chip.x, chip.y, chip.w, chip.h, 6.0),
        );
    }
    let colour = if hovered && open {
        palette.close_hover_fg
    } else {
        palette.text_dim
    };
    let (cx, cy) = rect.centre();
    let spun = root
        * Affine::translate((cx as f64, cy as f64))
        * Affine::rotate((progress * std::f32::consts::FRAC_PI_2) as f64)
        * Affine::translate((-(cx as f64), -(cy as f64)));
    if progress < 1.0 {
        scene::dots_three(canvas, spun, rect, colour.with_alpha(1.0 - progress));
    }
    if progress > 0.0 {
        let layout = engine.icon_layout(icons::WINDOW_CLOSE, theme::CAPTION_ICON);
        scene::draw_layout(
            canvas,
            spun,
            &layout,
            rect.x + (rect.w - layout.width()) * 0.5,
            rect.y + (rect.h - layout.height()) * 0.5,
            Some(colour.with_alpha(progress)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_are_ordered_and_close_owns_the_corner_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let caption = Rect::new(240.0, 0.0, 960.0, theme::CAPTION_H);
            let controls = controls(caption);
            assert_eq!(
                controls.map(|control| control.action),
                [
                    Action::WindowMenu,
                    Action::Minimise,
                    Action::ToggleMaximise,
                    Action::Close,
                ]
            );
            assert!(
                controls
                    .windows(2)
                    .all(|pair| pair[0].rect.x < pair[1].rect.x)
            );
            assert_eq!(controls[3].rect.x + controls[3].rect.w, 1200.0);
            for control in controls {
                let (x, y) = control.rect.centre();
                assert_eq!(hit(caption, x, y), Some(control.action));
                let physical = (x * scale, y * scale);
                assert_eq!(
                    hit(caption, physical.0 / scale, physical.1 / scale),
                    Some(control.action)
                );
            }
        }
    }

    #[test]
    fn a_minimal_host_receives_actions_without_a_window_or_model() {
        let caption = Rect::new(0.0, 0.0, 800.0, theme::CAPTION_H);
        for control in controls(caption) {
            let (x, y) = control.rect.centre();
            assert_eq!(hit(caption, x, y), Some(control.action));
        }
        assert_eq!(reserved_width(caption), theme::CAPTION_BTN * 4.0);
        assert_eq!(direct_glyph(Action::ToggleMaximise, false), icons::SQUARE);
        assert_eq!(
            direct_glyph(Action::ToggleMaximise, true),
            icons::WINDOW_RESTORE
        );
        let manifest = include_str!("../Cargo.toml");
        assert!(
            manifest
                .lines()
                .all(|line| !line.trim_start().starts_with("koru-core "))
        );
        assert!(
            manifest
                .lines()
                .all(|line| !line.trim_start().starts_with("koru-window "))
        );
    }

    #[test]
    fn focused_windows_have_three_direct_controls_and_no_overflow_slot() {
        let caption = Rect::new(0.0, 0.0, 800.0, theme::CAPTION_H);
        let controls = direct_controls(caption);
        assert_eq!(
            controls.map(|control| control.action),
            [Action::Minimise, Action::ToggleMaximise, Action::Close]
        );
        assert_eq!(controls[2].rect.x + controls[2].rect.w, 800.0);
        assert_eq!(direct_hit(caption, 610.0, 18.0), None);
        for control in controls {
            let (x, y) = control.rect.centre();
            assert_eq!(direct_hit(caption, x, y), Some(control.action));
        }
    }
}
