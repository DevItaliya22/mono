// SPDX-License-Identifier: MPL-2.0

//! **One layout pass, walked twice.**
//!
//! Every line and every control is measured once into a [`Paint`] list and
//! a [`Control`] list, and both the draw pass and every hit test walk those
//! lists — so a control cannot be pressable somewhere other than where it
//! is drawn, and a test can press one without a GPU. The same shape
//! `koru-workspaces-ui` and `koru-settings-ui` use, because chrome geometry
//! lives in one function shared by drawing and hit-testing (DESIGN).
//!
//! **What this column adds is the consequence line.** D36 asks the surface
//! to say which action is recoverable and which is not *before* the click,
//! and a tooltip is after: you have to be over the control already, and on
//! a touchpad you may never be. So a consequence is an ordinary [`Line`] of
//! text under the control it belongs to, drawn always, in the same pass.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::theme;
use vello::peniko::Color;

use crate::action::Action;

#[cfg(test)]
mod tests;

pub(crate) const COLUMN_W: f32 = 620.0;
pub(crate) const PAD_X: f32 = 24.0;
pub(crate) const TOP: f32 = 20.0;
pub(crate) const TAB_H: f32 = 30.0;
pub(crate) const TAB_GAP: f32 = 6.0;
pub(crate) const TAB_PAD_X: f32 = 12.0;
pub(crate) const TAB_MIN_W: f32 = 64.0;
pub(crate) const SECTION_GAP: f32 = 20.0;
/// The run's name, at the top of all three views.
pub(crate) const TITLE_H: f32 = 32.0;
pub(crate) const SECTION_H: f32 = 24.0;
pub(crate) const LINE_H: f32 = 20.0;
pub(crate) const NOTE_GAP: f32 = 4.0;
pub(crate) const ROW_GAP: f32 = 6.0;
pub(crate) const CHIP_H: f32 = 28.0;
pub(crate) const CHIP_GAP: f32 = 6.0;
pub(crate) const CHIP_PAD_X: f32 = 11.0;
pub(crate) const CHIP_MIN_W: f32 = 64.0;
pub(crate) const FIELD_H: f32 = 30.0;
/// The tone a control that is present but not pressable is drawn in. The
/// only such control here is a chip whose operation is in flight.
pub(crate) const DIMMED: f32 = 0.45;
/// **How big a chip's leading icon is drawn**, and the size the row that
/// measures the chip reserves for it.
///
/// The size of the word beside it, because it *is* beside a word rather
/// than standing on its own: a glyph a step larger than its label reads as
/// a button that happens to have text, which a rating chip is not. Taken
/// from the role the label is laid out in rather than written twice, and
/// read by both the measuring pass and the drawing pass so the two cannot
/// disagree about how wide the chip has to be.
pub(crate) fn chip_icon_size() -> f32 {
    Line::Meta.style().metrics.size
}
/// The clear space between a chip's icon and its word.
pub(crate) const CHIP_ICON_GAP: f32 = 6.0;

/// Which type role a line of text carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    /// **The run's name**, at the top of all three views. The one line on
    /// this surface that is neither a heading over a group nor a piece of
    /// the run's content: it is what the run is *called*.
    Title,
    /// A heading over a group.
    Section,
    /// The thing itself — an objective, a step, a result's title.
    Body,
    /// A label beside a value, and a value beside a label.
    Meta,
    /// **What an action would do.** Drawn under the control, always.
    Consequence,
    /// A time, a count, a device — quieter than meta.
    Hint,
}

impl Line {
    pub(crate) fn style(self) -> theme::TextStyle {
        match self {
            Self::Title => theme::role::subtitle(),
            Self::Section => theme::role::section_heading(),
            Self::Body => theme::role::row(),
            Self::Meta | Self::Consequence => theme::role::meta(),
            Self::Hint => theme::role::hint(),
        }
    }

    pub(crate) fn tone(self) -> Color {
        let p = theme::p();
        match self {
            Self::Title => p.text,
            Self::Section => p.text_dim,
            Self::Body => p.text,
            Self::Meta => p.row_hint,
            Self::Consequence => p.text_dim,
            Self::Hint => p.row_hint,
        }
    }
}

/// One piece of text the draw pass puts on the screen.
#[derive(Clone, Debug)]
pub(crate) struct Paint {
    pub(crate) text: String,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) line: Line,
    /// Wrapped text repeats this exact width in the draw pass, so
    /// measurement and paint cannot disagree.
    pub(crate) max_width: Option<f32>,
}

impl Paint {
    pub(crate) fn style(&self) -> theme::TextStyle {
        self.line.style()
    }

    pub(crate) fn tone(&self) -> Color {
        self.line.tone()
    }
}

/// What a control looks like.
///
/// **The two faces are the visible half of D36.** An ordinary [`Chip`] is a
/// recoverable action — including *end the step and take the session*,
/// which sounds destructive and is not. [`Danger`] is worn by exactly one
/// control in this surface, *end the run*, and it is the only one that
/// opens a confirmation.
///
/// [`Chip`]: Face::Chip
/// [`Danger`]: Face::Danger
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Face {
    /// A view selector, on or off.
    Tab(String, bool),
    /// A word-sized button; the ordinary, recoverable kind.
    Chip(String),
    /// A quiet destructive button: ordinary chip geometry, red text.
    Danger(String),
    /// A rating gesture, drawn with its glyph, on when it is the rating.
    Rating(String, char, bool),
    /// The note field. Its text lives in the surface, not here.
    Field,
}

/// One thing a press can land on.
#[derive(Clone, Debug)]
pub(crate) struct Control {
    pub(crate) rect: Rect,
    pub(crate) action: Action,
    pub(crate) face: Face,
    /// `false` while the operation behind it is in flight: drawn dimmed,
    /// and a press on it does nothing. There is no *reason* string here as
    /// `koru-workspaces-ui` has, because this surface never lays out a
    /// control it may not perform — on a device that is not executing the
    /// run there are no controls at all (D15).
    pub(crate) enabled: bool,
}

impl Control {
    /// **The word a control is known by**, which is how a test presses one.
    pub(crate) fn label(&self) -> &str {
        match &self.face {
            Face::Tab(label, _)
            | Face::Chip(label)
            | Face::Danger(label)
            | Face::Rating(label, _, _) => label,
            Face::Field => "",
        }
    }
}

/// The accumulator the sections push onto.
pub(crate) struct Column<'a> {
    engine: &'a mut TextEngine,
    x: f32,
    w: f32,
    y: f32,
    paints: Vec<Paint>,
    controls: Vec<Control>,
    markdown: Option<(f32, f32)>,
}

/// What one layout pass produced.
///
/// `markdown` is where a rendered document begins, when the pass laid one
/// out. The document's own pieces live in [`koru_markdown_view::MarkdownView`]
/// — measured there, drawn there — and this column only says where they
/// start and how much room they took, so the two cannot disagree about the
/// height of the thing they are both scrolling.
#[derive(Default, Debug)]
pub(crate) struct Laid {
    pub(crate) paints: Vec<Paint>,
    pub(crate) controls: Vec<Control>,
    pub(crate) bottom: f32,
    pub(crate) markdown: Option<(f32, f32)>,
}

impl<'a> Column<'a> {
    pub(crate) fn new(engine: &'a mut TextEngine, x: f32, w: f32, top: f32) -> Self {
        Self {
            engine,
            x,
            w,
            y: top,
            paints: Vec::new(),
            controls: Vec::new(),
            markdown: None,
        }
    }

    pub(crate) fn finish(self) -> Laid {
        Laid {
            paints: self.paints,
            controls: self.controls,
            bottom: self.y,
            markdown: self.markdown,
        }
    }

    /// **A rendered Markdown document, laid out at this width.**
    ///
    /// The measuring is the document surface's; this pass only records where
    /// it starts and moves past it, so scrolling, the content height and the
    /// hit test all come from one number as they do for every other row.
    pub(crate) fn markdown(&mut self, view: &mut koru_markdown_view::MarkdownView) {
        view.relayout(self.engine, self.w);
        self.markdown = Some((self.x, self.y));
        self.y += view.height() + SECTION_GAP;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.paints.is_empty() && self.controls.is_empty()
    }

    pub(crate) fn gap(&mut self, amount: f32) {
        self.y += amount;
    }

    /// **The run's name**, above everything. Its own method rather than a
    /// `line` with a role, because it sets the height of the band the tabs
    /// sit under and that number belongs beside the other geometry.
    pub(crate) fn title(&mut self, text: &str) {
        self.push_text(Line::Title, text, self.x);
        self.y += TITLE_H;
    }

    pub(crate) fn section(&mut self, text: &str) {
        if !self.is_empty() {
            self.y += SECTION_GAP;
        }
        self.push_text(Line::Section, text, self.x);
        self.y += SECTION_H;
    }

    /// One line that does not wrap.
    pub(crate) fn line(&mut self, line: Line, text: &str) {
        self.push_text(line, text, self.x);
        self.y += LINE_H;
    }

    /// **One line with a single control at the right of it.**
    ///
    /// For the case where the fact and what to do about it are one sentence:
    /// *this run has ended* and *ask for it again*. A control laid out further
    /// down would read as a separate offer, and the eye would have to go
    /// looking for it.
    ///
    /// The text is centred against the control's height so the two sit on one
    /// line, and the control is measured like every other chip.
    pub(crate) fn line_beside(
        &mut self,
        line: Line,
        text: &str,
        action: Action,
        label: String,
        enabled: bool,
    ) {
        let measured = self.engine.simple_layout(&label, Line::Meta.style());
        let width = (measured.width() + CHIP_PAD_X * 2.0).max(CHIP_MIN_W);
        self.paints.push(Paint {
            text: text.to_owned(),
            x: self.x,
            y: self.y + (CHIP_H - LINE_H) * 0.5,
            line,
            max_width: Some((self.w - width - CHIP_GAP).max(1.0)),
        });
        self.controls.push(Control {
            rect: Rect::new(self.x + self.w - width, self.y, width, CHIP_H),
            action,
            face: Face::Chip(label),
            enabled,
        });
        self.y += CHIP_H + ROW_GAP;
    }

    /// A label and its value on one line, the value at the right.
    pub(crate) fn pair(&mut self, label: &str, value: &str) {
        self.push_text(Line::Meta, label, self.x);
        let measured = self.engine.simple_layout(value, Line::Body.style());
        let x = (self.x + self.w - measured.width()).max(self.x);
        self.paints.push(Paint {
            text: value.to_owned(),
            x,
            y: self.y,
            line: Line::Body,
            max_width: None,
        });
        self.y += LINE_H;
    }

    /// Text that wraps to the column.
    pub(crate) fn wrapped(&mut self, line: Line, text: &str) {
        let width = self.w.max(1.0);
        let layout = self.engine.simple_layout_wrapped(text, line.style(), width);
        self.paints.push(Paint {
            text: text.to_owned(),
            x: self.x,
            y: self.y,
            line,
            max_width: Some(width),
        });
        self.y += layout.height().max(LINE_H) + NOTE_GAP;
    }

    /// **What the control above would do.** Indented under it, wrapped,
    /// and never conditional on hover.
    pub(crate) fn consequence(&mut self, text: &str) {
        let width = (self.w - CHIP_GAP).max(1.0);
        let layout = self
            .engine
            .simple_layout_wrapped(text, Line::Consequence.style(), width);
        self.paints.push(Paint {
            text: text.to_owned(),
            x: self.x,
            y: self.y,
            line: Line::Consequence,
            max_width: Some(width),
        });
        self.y += layout.height().max(LINE_H) + ROW_GAP;
    }

    fn push_text(&mut self, line: Line, text: &str, x: f32) {
        self.paints.push(Paint {
            text: text.to_owned(),
            x,
            y: self.y,
            line,
            max_width: None,
        });
    }

    /// The three view selectors, in one row.
    pub(crate) fn tabs<I: IntoIterator<Item = (Action, String, bool)>>(&mut self, tabs: I) {
        let mut x = self.x;
        for (action, label, on) in tabs {
            let measured = self.engine.simple_layout(&label, Line::Body.style());
            let width = (measured.width() + TAB_PAD_X * 2.0).max(TAB_MIN_W);
            self.controls.push(Control {
                rect: Rect::new(x, self.y, width, TAB_H),
                action,
                face: Face::Tab(label, on),
                enabled: true,
            });
            x += width + TAB_GAP;
        }
        self.y += TAB_H;
    }

    /// A row of word-sized buttons, wrapping at the column's right edge.
    ///
    /// **Measured, not gridded**: *end the step and take the session* is
    /// three times the width of *resume*, and a fixed grid would clip one
    /// or waste the row on the other.
    pub(crate) fn chips<I: IntoIterator<Item = (Action, Face, bool)>>(&mut self, items: I) {
        let right = self.x + self.w;
        let mut x = self.x;
        let mut used = false;
        for (action, face, enabled) in items {
            let label = match &face {
                Face::Tab(label, _)
                | Face::Chip(label)
                | Face::Danger(label)
                | Face::Rating(label, _, _) => label.clone(),
                Face::Field => String::new(),
            };
            let measured = self.engine.simple_layout(&label, Line::Meta.style());
            let glyph = match face {
                Face::Rating(..) => chip_icon_size() + CHIP_ICON_GAP,
                _ => 0.0,
            };
            let width = (measured.width() + CHIP_PAD_X * 2.0 + glyph).max(CHIP_MIN_W);
            if used && x + width > right {
                x = self.x;
                self.y += CHIP_H + CHIP_GAP;
            }
            self.controls.push(Control {
                rect: Rect::new(x, self.y, width, CHIP_H),
                action,
                face,
                enabled,
            });
            x += width + CHIP_GAP;
            used = true;
        }
        if used {
            self.y += CHIP_H + ROW_GAP;
        }
    }

    /// The note field: full width, one line tall.
    pub(crate) fn field(&mut self, action: Action) -> Rect {
        let rect = Rect::new(self.x, self.y, self.w, FIELD_H);
        self.controls.push(Control {
            rect,
            action,
            face: Face::Field,
            enabled: true,
        });
        self.y += FIELD_H + ROW_GAP;
        rect
    }
}

impl Laid {
    /// Which control a point in the column's own space lands on.
    ///
    /// The same list the draw pass walks, so a control cannot be pressable
    /// somewhere other than where it is drawn — the rule the module opens
    /// with, stated once for both surfaces that keep a [`Laid`].
    pub(crate) fn control_at(&self, x: f32, y: f32) -> Option<usize> {
        self.controls
            .iter()
            .position(|control| control.rect.contains(x, y))
    }

    /// The next enabled control in the ring, walked by Tab and the arrows.
    /// `None` when there is nothing to land on.
    pub(crate) fn step(&self, focus: Option<usize>, forward: bool) -> Option<usize> {
        let enabled: Vec<usize> = self
            .controls
            .iter()
            .enumerate()
            .filter(|(_, control)| control.enabled)
            .map(|(index, _)| index)
            .collect();
        if enabled.is_empty() {
            return None;
        }
        let position = focus.and_then(|current| enabled.iter().position(|index| *index == current));
        let next = match (position, forward) {
            (None, true) => 0,
            (None, false) => enabled.len() - 1,
            (Some(at), true) => (at + 1) % enabled.len(),
            (Some(at), false) => (at + enabled.len() - 1) % enabled.len(),
        };
        Some(enabled[next])
    }
}

/// **One control, drawn.** Shared by both surfaces in this crate, so a chip
/// on the run views and a chip on the result tab cannot drift apart.
///
/// [`Face::Field`] is not here: a field's text lives on the surface that
/// owns it, and only one surface has one. Its owner draws it and calls this
/// for everything else.
pub(crate) fn draw_control(
    scene_out: &mut vello::Scene,
    root: vello::kurbo::Affine,
    engine: &mut TextEngine,
    control: &Control,
    hovered: bool,
    focused: bool,
) {
    let p = theme::p();
    let r = control.rect;
    let alpha = if control.enabled { 1.0 } else { DIMMED };
    match &control.face {
        // Drawn by the surface that owns the text in it, ring and all.
        Face::Field => return,
        Face::Tab(label, on) => {
            let shape = koru_draw::scene::rrect(r.x, r.y, r.w, r.h, 6.0);
            if *on {
                koru_draw::scene::fill(scene_out, root, p.panel_bg, &shape);
                koru_draw::scene::stroke(scene_out, root, p.panel_border, theme::BORDER_W, &shape);
            } else if hovered {
                koru_draw::scene::fill(scene_out, root, p.hover_tint, &shape);
            }
            let tone = if *on { p.text } else { p.text_dim };
            centre(
                scene_out,
                root,
                engine,
                label,
                r,
                theme::role::row(),
                tone,
                1.0,
            );
        }
        Face::Chip(label) => {
            let shape = koru_draw::scene::rrect(r.x, r.y, r.w, r.h, 6.0);
            koru_draw::scene::fill(scene_out, root, p.chip_bg, &shape);
            if hovered && control.enabled {
                koru_draw::scene::fill(scene_out, root, p.hover_tint, &shape);
            }
            centre(
                scene_out,
                root,
                engine,
                label,
                r,
                theme::role::meta(),
                p.text,
                alpha,
            );
        }
        Face::Danger(label) => {
            // **Outlined and red, never filled in the accent colour**
            // (DESIGN): filling it would make ending a run look like the
            // suggested route through this surface.
            let shape = koru_draw::scene::rrect(r.x, r.y, r.w, r.h, 6.0);
            koru_draw::scene::fill(scene_out, root, p.chip_bg, &shape);
            if hovered && control.enabled {
                koru_draw::scene::fill(scene_out, root, p.hover_tint, &shape);
            }
            koru_draw::scene::stroke(scene_out, root, p.close_hover, theme::BORDER_W, &shape);
            centre(
                scene_out,
                root,
                engine,
                label,
                r,
                theme::role::meta(),
                p.close_hover,
                alpha,
            );
        }
        Face::Rating(label, glyph, on) => {
            let shape = koru_draw::scene::rrect(r.x, r.y, r.w, r.h, 6.0);
            koru_draw::scene::fill(scene_out, root, p.chip_bg, &shape);
            if *on {
                koru_draw::scene::fill(scene_out, root, p.row_selected, &shape);
            } else if hovered && control.enabled {
                koru_draw::scene::fill(scene_out, root, p.hover_tint, &shape);
            }
            let tone = if *on { p.accent } else { p.text };
            // **An icon codepoint has to be asked of the icon family.**
            // Phosphor's glyphs sit in the Private Use Area, and a text
            // role has nothing there — laid out through `simple_layout`
            // the thumbs drew as missing-glyph boxes, which is what the
            // maintainer saw. `icon_layout` is the one door to that family, and
            // it deliberately has no fallback for the same reason.
            let laid = engine.icon_layout(*glyph, chip_icon_size());
            koru_draw::scene::draw_layout(
                scene_out,
                root,
                &laid,
                r.x + CHIP_PAD_X,
                r.y + (r.h - laid.height()) * 0.5,
                Some(tone),
            );
            let text = engine.simple_layout(label, theme::role::meta());
            koru_draw::scene::draw_layout(
                scene_out,
                root,
                &text,
                r.x + CHIP_PAD_X + chip_icon_size() + CHIP_ICON_GAP,
                r.y + (r.h - text.height()) * 0.5,
                Some(tone),
            );
        }
    }
    if focused {
        koru_draw::scene::stroke(
            scene_out,
            root,
            p.focus_ring,
            theme::BORDER_W,
            &koru_draw::scene::rrect(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0, 8.0),
        );
    }
}

/// A control's word, centred in it and clipped to it, so a long word in one
/// language cannot spill over the control beside it.
#[allow(clippy::too_many_arguments)]
fn centre(
    scene_out: &mut vello::Scene,
    root: vello::kurbo::Affine,
    engine: &mut TextEngine,
    label: &str,
    r: Rect,
    style: theme::TextStyle,
    tone: Color,
    alpha: f32,
) {
    let laid = engine.simple_layout(label, style);
    let tone = tone.multiply_alpha(alpha);
    scene_out.push_clip_layer(
        vello::peniko::Fill::NonZero,
        root,
        &koru_draw::scene::rect(r.x, r.y, r.w, r.h),
    );
    koru_draw::scene::draw_layout(
        scene_out,
        root,
        &laid,
        r.x + (r.w - laid.width()) * 0.5,
        r.y + (r.h - laid.height()) * 0.5,
        Some(tone),
    );
    scene_out.pop_layer();
}
