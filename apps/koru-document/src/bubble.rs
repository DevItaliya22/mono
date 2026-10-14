// SPDX-License-Identifier: MPL-2.0

//! The formatting bubble: chrome that exists only while there is a selection.
//!
//! It replaces the toolbar strip that used to run across the top of every
//! document. That strip carried the same commands, and carried them all the
//! time — a permanently dimmed Bold button teaches nothing, and twenty icons
//! above a blank page is furniture rather than a tool. Selecting words is
//! already the gesture that means "I want to do something to these"; the bubble
//! is the answer to it.
//!
//! **One function owns the geometry.** [`buttons`] is called by the renderer
//! and by [`hit`] alike, and takes the panel rect [`panel`] computed — so a
//! button is clickable exactly where it is drawn (`docs/features/design/DESIGN.md`). It
//! deliberately needs no font context: every button is a fixed-size icon, so
//! the arithmetic is pure and testable without a viewport.

use koru_draw::geometry::Rect;
use koru_draw::theme;
use koru_draw::{icons, scene};
use vello::Scene;
use vello::kurbo::Affine;

use crate::docview::{self, Command};

/// What a bubble button does.
///
/// Most are a [`Command`] the old toolbar already had — the bubble is a second
/// layout over the same set, not a second vocabulary. The two that are not are
/// the menus: what a *line* is, and the commands that did not earn a place on
/// the front.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Button {
    /// Opens the block-type menu: Text, H1–H4, Tasks, List, Divider.
    BlockType,
    /// Opens the three list styles. It stays pressed whenever the focused row
    /// is any kind of list, like Bold does for an active mark.
    List,
    Cmd(Command),
    /// Opens the overflow menu: alignment and super/subscript.
    More,
}

/// The layout, in order. A `None` is a separator between groups.
///
/// `Link` sits with the inline marks rather than with the block menu because
/// that is what it is — a thing you do to selected words. `Clear` ends the
/// inline run for the same reason: it is the undo of everything before it.
///
/// **Alignment is on the front**, where every editor puts it and where the eye
/// goes looking. It is a block property rather than a mark, so it sits in its
/// own group after the inline run — behind the `⋯` it was a control nobody
/// found.
///
/// **The `⋯` itself is off the bubble for now.** All it held was super- and
/// subscript, and neither command works yet — a menu of two broken rows
/// advertises something that is not there, the same reason the context
/// column's toggle is hidden. `Button::More` and the overflow menu machinery
/// stay; restoring the two rows is putting `None, Some(Button::More)` back at
/// the end of this array once `SetScript` actually applies the mark.
const LAYOUT: [Option<Button>; 16] = [
    Some(Button::BlockType),
    Some(Button::List),
    None,
    Some(Button::Cmd(Command::Bold)),
    Some(Button::Cmd(Command::Italic)),
    Some(Button::Cmd(Command::Underline)),
    Some(Button::Cmd(Command::Strike)),
    None,
    Some(Button::Cmd(Command::Highlight)),
    Some(Button::Cmd(Command::Code)),
    Some(Button::Cmd(Command::Link)),
    Some(Button::Cmd(Command::ClearFormatting)),
    None,
    Some(Button::Cmd(Command::AlignStart)),
    Some(Button::Cmd(Command::AlignCenter)),
    Some(Button::Cmd(Command::AlignEnd)),
];

/// The bubble's own metrics. Tighter than the old toolbar's: it floats over the
/// text rather than sitting in a band of its own, so every pixel of it is in
/// the reader's way until they are done with it.
const BTN: f32 = 28.0;
const GAP: f32 = 1.0;
const SEP: f32 = 11.0;
const PAD_X: f32 = 5.0;
const PAD_Y: f32 = 5.0;
pub const RADIUS: f32 = 8.0;
/// How far above (or below) the selection the bubble floats.
const OFFSET: f32 = 8.0;
const EDGE_MARGIN: f32 = 8.0;
/// The block-type button carries the current block's icon and a chevron.
const BLOCK_W: f32 = 46.0;

impl Button {
    fn width(self) -> f32 {
        match self {
            Self::BlockType | Self::List => BLOCK_W,
            _ => BTN,
        }
    }

    /// What the button says when hovered, with the chord that does the same.
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::BlockType => "Turn into",
            Self::List => "List",
            Self::More => "More",
            Self::Cmd(cmd) => cmd.tooltip(),
        }
    }

    /// The list button names the active list style, so hovering it teaches the
    /// same shortcut as the corresponding command.
    pub fn tooltip_for(self, state: &docview::State) -> &'static str {
        match (self, state.list_style) {
            (Self::List, Some(koru_core::ListStyle::Bullet)) => Command::BulletedList.tooltip(),
            (Self::List, Some(koru_core::ListStyle::Numbered)) => Command::NumberedList.tooltip(),
            (Self::List, Some(koru_core::ListStyle::Checkbox)) => Command::Tasks.tooltip(),
            _ => self.tooltip(),
        }
    }
}

/// The list trigger mirrors its selected style, while an ordinary paragraph
/// starts from the familiar bulleted-list glyph.
pub fn list_icon(state: &docview::State) -> char {
    match state.list_style {
        Some(koru_core::ListStyle::Numbered) => icons::LIST_NUM,
        Some(koru_core::ListStyle::Checkbox) => icons::LIST_CHECKS,
        Some(koru_core::ListStyle::Bullet) | Some(_) | None => icons::LIST,
    }
}

/// One button and where it sits, in window coordinates.
pub struct Placed {
    pub button: Button,
    pub rect: Rect,
}

/// The bubble's total width — the sum of what is in it.
pub fn width() -> f32 {
    let mut w = PAD_X * 2.0;
    for (i, slot) in LAYOUT.iter().enumerate() {
        match slot {
            Some(b) => {
                w += b.width();
                if i + 1 < LAYOUT.len() && LAYOUT[i + 1].is_some() {
                    w += GAP;
                }
            }
            None => w += SEP,
        }
    }
    w
}

pub fn height() -> f32 {
    BTN + PAD_Y * 2.0
}

/// Where the bubble sits, given the selection's first line rect in window
/// coordinates and the chrome this view draws above its rows.
///
/// Above the selection by preference — a bubble below it covers the next line
/// of what you are reading — flipping under when there is no room, and clamped
/// to the viewport. The same prefer/flip/clamp recipe `overlay_panel_rect`
/// uses for the editor's pickers, and for the same reason: a panel that runs
/// off the top of the window is a panel you cannot use.
pub fn panel(selection: Rect, viewport: (f32, f32), chrome_h: f32) -> Rect {
    let (w, h) = (width(), height());
    let x = (selection.x + selection.w * 0.5 - w * 0.5)
        .clamp(EDGE_MARGIN, (viewport.0 - w - EDGE_MARGIN).max(EDGE_MARGIN));
    let above = selection.y - h - OFFSET;
    let y = if above < chrome_h + EDGE_MARGIN {
        // No room over the selection: drop under it, and clamp to the floor so
        // a selection on the last visible line still shows its bubble.
        (selection.y + selection.h + OFFSET).min((viewport.1 - h - EDGE_MARGIN).max(0.0))
    } else {
        above
    };
    Rect::new(x, y, w, h)
}

/// Every button and where it sits within `panel`.
pub fn buttons(panel: Rect) -> Vec<Placed> {
    let mut out = Vec::with_capacity(LAYOUT.len());
    let mut x = panel.x + PAD_X;
    let y = panel.y + PAD_Y;
    for (i, slot) in LAYOUT.iter().enumerate() {
        match slot {
            Some(button) => {
                let w = button.width();
                out.push(Placed {
                    button: *button,
                    rect: Rect::new(x, y, w, BTN),
                });
                x += w;
                if i + 1 < LAYOUT.len() && LAYOUT[i + 1].is_some() {
                    x += GAP;
                }
            }
            None => x += SEP,
        }
    }
    out
}

/// Separator positions within `panel`, so the renderer draws them where the
/// gaps are rather than computing its own.
pub fn separators(panel: Rect) -> Vec<f32> {
    let mut out = Vec::new();
    let mut x = panel.x + PAD_X;
    for (i, slot) in LAYOUT.iter().enumerate() {
        match slot {
            Some(button) => {
                x += button.width();
                if i + 1 < LAYOUT.len() && LAYOUT[i + 1].is_some() {
                    x += GAP;
                }
            }
            None => {
                out.push(x + SEP * 0.5);
                x += SEP;
            }
        }
    }
    out
}

/// The button under a point, if any.
pub fn hit(panel: Rect, x: f32, y: f32) -> Option<Button> {
    buttons(panel)
        .into_iter()
        .find(|b| b.rect.contains(x, y))
        .map(|b| b.button)
}

/// The rect a menu opened from `button` hangs under.
pub fn button_rect(panel: Rect, button: Button) -> Option<Rect> {
    buttons(panel)
        .into_iter()
        .find(|b| b.button == button)
        .map(|b| b.rect)
}

/// Paint the bubble.
///
/// `state` says which buttons draw pressed; `block_icon` is the glyph the
/// block-type button shows (the current block's own menu icon).
pub fn draw(
    scene: &mut Scene,
    root: Affine,
    engine: &mut koru_draw::text::TextEngine,
    panel: Rect,
    state: &docview::State,
    block_icon: char,
    pointer: (f32, f32),
) {
    let pal = theme::p();
    // Raised, and borderless: it is a thing you press, so it lifts — but its
    // own dark surface is the edge, and a hairline on top of that reads as a
    // second frame.
    scene::floating_panel(
        scene,
        root,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        &scene::Panel::popover(RADIUS)
            .fill(pal.bubble_bg)
            .borderless(),
    );

    for x in separators(panel) {
        scene::fill(
            scene,
            root,
            pal.bubble_hover,
            &scene::rect(x, panel.y + PAD_Y + BTN * 0.22, 1.0, BTN * 0.56),
        );
    }

    let hovered = hit(panel, pointer.0, pointer.1);
    for placed in buttons(panel) {
        let r = placed.rect;
        let active = match placed.button {
            Button::Cmd(cmd) => state.is_active(cmd),
            Button::List => state.list_style.is_some(),
            // A menu button is never "on"; it is a door.
            Button::BlockType | Button::More => false,
        };
        if active || hovered == Some(placed.button) {
            scene::fill(
                scene,
                root,
                pal.bubble_hover,
                &scene::rrect(r.x, r.y, r.w, r.h, 5.0),
            );
        }
        match placed.button {
            Button::BlockType | Button::List => {
                // The icon names the block the way its menu row does; the
                // chevron is what says the button opens something. The pair
                // centres as one unit.
                let icon = if placed.button == Button::List {
                    list_icon(state)
                } else {
                    block_icon
                };
                let glyph = engine.icon_layout(icon, theme::ICON_SIZE);
                let caret = engine.icon_layout(icons::CARET_DOWN, 11.0);
                let gap = 5.0;
                let start = r.x + (r.w - glyph.width() - gap - caret.width()) * 0.5;
                scene::draw_layout(
                    scene,
                    root,
                    &glyph,
                    start,
                    r.y + (r.h - glyph.height()) * 0.5,
                    Some(pal.bubble_fg),
                );
                scene::draw_layout(
                    scene,
                    root,
                    &caret,
                    start + glyph.width() + gap,
                    r.y + (r.h - caret.height()) * 0.5,
                    Some(pal.bubble_fg),
                );
            }
            Button::More => {
                scene::dots_three(scene, root, r, pal.bubble_fg);
            }
            Button::Cmd(cmd) => {
                // The highlight button shows the color it would apply, so the
                // swatch is visible without opening anything — the one thing
                // the old toolbar did that is worth keeping verbatim.
                if cmd == Command::Highlight {
                    let swatch = state
                        .style
                        .highlight
                        .as_deref()
                        .map(theme::highlight)
                        .unwrap_or_else(|| theme::highlight(theme::highlight_names()[0]));
                    scene::fill(
                        scene,
                        root,
                        swatch,
                        &scene::rrect(r.x + 5.0, r.y + r.h - 7.0, r.w - 10.0, 3.0, 1.5),
                    );
                }
                let glyph = engine.icon_layout(cmd.icon(), theme::ICON_SIZE);
                scene::draw_layout(
                    scene,
                    root,
                    &glyph,
                    r.x + (r.w - glyph.width()) * 0.5,
                    r.y + (r.h - glyph.height()) * 0.5,
                    Some(pal.bubble_fg),
                );
            }
        }
    }
}

/// The hover tooltip, drawn under the bubble. Uses the panel recipe rather
/// than the bubble's own surface: a tooltip is a note about a control, not part
/// of it.
#[allow(clippy::too_many_arguments)]
pub fn draw_tooltip(
    scene: &mut Scene,
    root: Affine,
    engine: &mut koru_draw::text::TextEngine,
    panel: Rect,
    viewport: (f32, f32),
    pointer: (f32, f32),
    progress: f32,
    state: &docview::State,
) {
    let Some(button) = hit(panel, pointer.0, pointer.1) else {
        return;
    };
    let pal = theme::p();
    let text = button.tooltip_for(state);
    let tip = engine.simple_layout(text, theme::role::hint());
    let pad = 6.0;
    let (tw, th) = (tip.width() + pad * 2.0, tip.height() + pad);
    let tx = (pointer.0 - tw * 0.5).clamp(4.0, (viewport.0 - tw - 4.0).max(4.0));
    let ty = panel.y + panel.h + 6.0;

    // The same wait, rise and fade the `+`'s hint uses — one idiom for every
    // tooltip, so hovering a button here and hovering the margin feel like the
    // same gesture rather than two different features.
    let rise = (1.0 - progress) * theme::motion::tooltip().rise;
    let group = root * Affine::translate((0.0, rise as f64));
    let panel = scene::Panel::tooltip(5.0);
    let opaque = progress >= 1.0;
    if !opaque {
        // vello allocates a texture per `push_layer`, so the group is skipped
        // outright once the fade is over.
        let b = scene::panel_bounds(tx, ty, tw, th, panel.elevation);
        scene::fade_layer(scene, group, progress, &scene::rect(b.x, b.y, b.w, b.h));
    }
    // The same chip as every other tooltip, sitting under a bar of the same
    // colour. The 6px of page between them is what separates the two.
    scene::floating_panel(scene, group, tx, ty, tw, th, &panel);
    scene::draw_layout(
        scene,
        group,
        &tip,
        tx + pad,
        ty + pad * 0.5,
        Some(pal.bubble_fg),
    );
    if !opaque {
        scene.pop_layer();
    }
}

#[cfg(test)]
mod tests;
