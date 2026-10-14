// SPDX-License-Identifier: MPL-2.0

//! **The dialog a card's or a node's text is written in** (CANVAS Q-4).
//!
//! Tonight a card is not edited in place: double-clicking one, Enter on a
//! selected one, or making a card or an agent or output node opens this small
//! modal over the canvas. It is the modal recipe DESIGN.md's *Modals* section
//! gives — the shared scrim (`koru_ui::modal`), a raised
//! `scene::floating_panel`, the button row from the right with the primary in
//! the corner — around the shared multi-line [`TextArea`] the delegation
//! window's objective uses, so Enter is a new line and the primary chord saves.
//!
//! **It writes nothing until it is confirmed**, and exactly one undo step
//! when it is: what was typed lives in the field, never in the document, and
//! the caller turns *Save* into one `koru_core::canvas` writer. Like the
//! delegation window it does not close on a press outside — losing something
//! somebody has just written out costs more than a stray click — so Esc and
//! *Cancel* are the ways out.
//!
//! [`geometry`] is the one place a part of it is placed: drawing and hit
//! testing both read it.

use koru_core::NodeId;
use koru_core::canvas::ObjectKind;
use koru_core::node::CanvasPoint;
use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{inspect, scene, theme};
use koru_i18n::{Key, Lang, fill, t};
use koru_ui::TextArea;
use koru_ui::button::{self, ButtonRole};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};

/// The widest the panel gets, and the air inside it.
const WIDTH: f32 = 480.0;
const PAD: f32 = 16.0;
const RADIUS: f32 = 10.0;
/// Six lines of the field's body text.
const AREA_H: f32 = 156.0;
const GAP: f32 = 10.0;
const TITLE_H: f32 = 22.0;
const HINT_H: f32 = 16.0;

/// What the dialog is writing.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DialogMode {
    /// A card that does not exist yet, at this canvas point. Nothing is made
    /// unless the dialog is saved with something in it.
    NewCard { at: CanvasPoint },
    /// The text of a card or a node that exists: a start node's note, an
    /// agent node's instruction, an output node's page title.
    Edit { node: NodeId, kind: ObjectKind },
}

impl DialogMode {
    fn title(self) -> Key {
        match self {
            Self::NewCard { .. } => Key::CanvasDialogNewCard,
            Self::Edit { kind, .. } => match kind {
                ObjectKind::Start => Key::CanvasDialogEditStart,
                ObjectKind::Agent => Key::CanvasDialogAgentInstruction,
                ObjectKind::Output => Key::CanvasDialogOutputTitle,
                ObjectKind::Card | ObjectKind::Image => Key::CanvasDialogEditCard,
            },
        }
    }

    fn placeholder(self) -> Key {
        match self {
            Self::Edit {
                kind: ObjectKind::Agent,
                ..
            } => Key::CanvasDialogAgentPlaceholder,
            Self::Edit {
                kind: ObjectKind::Output,
                ..
            } => Key::CanvasDialogOutputPlaceholder,
            _ => Key::CanvasDialogCardPlaceholder,
        }
    }
}

/// The dialog's two buttons.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Button {
    Cancel,
    Save,
}

/// What a key or a release came to.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Keep,
    Save,
    Cancel,
}

/// Where every part of the dialog is, in the view's space.
#[derive(Copy, Clone, Debug)]
pub struct Geometry {
    pub panel: Rect,
    pub title: (f32, f32),
    pub area: Rect,
    pub hint: (f32, f32),
    pub cancel: Rect,
    pub save: Rect,
}

/// **The one layout of the dialog**, read by drawing and by every hit test.
pub fn geometry(engine: &mut TextEngine, viewport: (f32, f32), lang: Lang) -> Geometry {
    let (vw, vh) = viewport;
    let w = WIDTH.min(vw - PAD * 2.0).max(1.0);
    let chrome = PAD + TITLE_H + GAP + GAP + HINT_H + GAP + button::H + PAD;
    // A short window gives the field what is left rather than pushing the
    // buttons off the bottom.
    let area_h = AREA_H.min((vh - chrome - PAD * 2.0).max(48.0));
    let h = chrome + area_h;
    let x = ((vw - w) * 0.5).max(0.0);
    let y = ((vh - h) * 0.5).max(PAD.min(vh * 0.5));
    let inner = w - PAD * 2.0;
    let title = (x + PAD, y + PAD);
    let area = Rect::new(x + PAD, y + PAD + TITLE_H + GAP, inner, area_h);
    let hint = (x + PAD, area.y + area.h + GAP);
    let buttons_y = hint.1 + HINT_H + GAP;
    let widths = [
        button::button_width(engine, t(lang, Key::CanvasDialogCancel)),
        button::button_width(engine, t(lang, Key::CanvasDialogSave)),
    ];
    let row = button::row_from_right(x + w - PAD, buttons_y, &widths);
    Geometry {
        panel: Rect::new(x, y, w, h),
        title,
        area,
        hint,
        cancel: row[0],
        save: row[1],
    }
}

/// The dialog, while it is open.
pub struct TextDialog {
    pub mode: DialogMode,
    area: TextArea,
    hover: Option<Button>,
    pressed: Option<Button>,
    selecting: bool,
    /// Where the parts were last placed — by the last draw or pointer event —
    /// so the cursor, which is asked with `&self`, reads the same geometry.
    placed: Option<Geometry>,
}

impl TextDialog {
    /// Open over `text`, the caret after it.
    pub fn open(engine: &mut TextEngine, mode: DialogMode, text: &str) -> Self {
        let mut area = TextArea::new(engine, theme::role::body());
        if !text.is_empty() {
            area.set_text(engine, text);
        }
        Self {
            mode,
            area,
            hover: None,
            pressed: None,
            selecting: false,
            placed: None,
        }
    }

    /// Place the parts for `viewport`, and remember where.
    fn place(&mut self, engine: &mut TextEngine, viewport: (f32, f32)) -> Geometry {
        let g = geometry(engine, viewport, crate::format::lang());
        self.placed = Some(g);
        g
    }

    /// What the field says now.
    pub fn value(&self) -> String {
        self.area.value()
    }

    /// Whether a press in the field is still selecting text.
    pub fn is_dragging(&self) -> bool {
        self.selecting
    }

    /// A key, while the dialog owns the keyboard — which is every key.
    ///
    /// **Esc cancels and the primary chord with Enter saves**, from anywhere:
    /// a plain Enter is a new line, as in every multi-line field in Koru.
    pub fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &WinitKey,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Outcome {
        match key {
            WinitKey::Named(NamedKey::Escape) => return Outcome::Cancel,
            WinitKey::Named(NamedKey::Enter) if koru_keys::is_chord(mods) => {
                return Outcome::Save;
            }
            // There is nowhere for Tab to go but out, and out is the canvas
            // the dialog stands over.
            WinitKey::Named(NamedKey::Tab) => return Outcome::Keep,
            _ => {}
        }
        let _ = self.area.key(engine, key, text, mods);
        Outcome::Keep
    }

    /// A press anywhere while the dialog stands. It is always the dialog's:
    /// a press outside the panel neither closes it nor reaches the canvas.
    pub fn mouse_down(&mut self, engine: &mut TextEngine, viewport: (f32, f32), x: f32, y: f32) {
        let g = self.place(engine, viewport);
        self.pressed = None;
        if g.save.contains(x, y) {
            self.pressed = Some(Button::Save);
        } else if g.cancel.contains(x, y) {
            self.pressed = Some(Button::Cancel);
        } else if self.area.click(engine, g.area, x, y) {
            self.selecting = true;
        }
    }

    pub fn mouse_move(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> bool {
        let g = self.place(engine, viewport);
        if self.selecting {
            self.area.drag(engine, g.area, x, y);
            return true;
        }
        let hover = if g.save.contains(x, y) {
            Some(Button::Save)
        } else if g.cancel.contains(x, y) {
            Some(Button::Cancel)
        } else {
            None
        };
        let changed = hover != self.hover;
        self.hover = hover;
        changed
    }

    /// The release: a button acts only if it is still under the pointer, so a
    /// press can be taken back by sliding off.
    pub fn mouse_up(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        pointer: (f32, f32),
    ) -> Outcome {
        self.selecting = false;
        self.area.release();
        let g = self.place(engine, viewport);
        match self.pressed.take() {
            Some(Button::Save) if g.save.contains(pointer.0, pointer.1) => Outcome::Save,
            Some(Button::Cancel) if g.cancel.contains(pointer.0, pointer.1) => Outcome::Cancel,
            _ => Outcome::Keep,
        }
    }

    /// Where the pointer shape is decided: the text cursor over the field,
    /// the press cursor over a button — from where they were last placed.
    pub fn over_field(&self, x: f32, y: f32) -> bool {
        self.placed.is_some_and(|g| g.area.contains(x, y))
    }

    pub fn over_button(&self, x: f32, y: f32) -> bool {
        self.placed
            .is_some_and(|g| g.save.contains(x, y) || g.cancel.contains(x, y))
    }

    pub fn scroll(&mut self, dy: f32) {
        self.area.scroll_by(dy);
    }

    /// The heading a screen reader is told the dialog is.
    pub fn title(&self, lang: Lang) -> &'static str {
        t(lang, self.mode.title())
    }

    /// Paint the dialog over the whole view.
    pub fn draw(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        viewport: (f32, f32),
    ) {
        let lang = crate::format::lang();
        let g = self.place(engine, viewport);
        let p = theme::p();
        let mode = self.mode;
        let hover = self.hover;
        koru_ui::modal::draw(scene, root, viewport, |scene| {
            let _dialog = inspect::scope(
                "KoruCanvasTextDialog",
                "component",
                root,
                scene::rect(g.panel.x, g.panel.y, g.panel.w, g.panel.h),
                |r| {
                    r.property(
                        "implementation",
                        "canvas_view::dialog",
                        "app surface on the shared modal recipe",
                    );
                    r.property("mode", format!("{mode:?}"), "DialogMode");
                },
            );
            scene::floating_panel(
                scene,
                root,
                g.panel.x,
                g.panel.y,
                g.panel.w,
                g.panel.h,
                &scene::Panel::popover(RADIUS),
            );
            let title = engine.simple_layout(t(lang, mode.title()), theme::role::section_heading());
            scene::draw_layout_first_line(scene, root, &title, g.title.0, g.title.1, Some(p.text));

            // The field wears the delegation form's frame: filled, a hairline,
            // and the focus ring because it always has the keyboard.
            let frame = scene::rrect(g.area.x, g.area.y, g.area.w, g.area.h, 6.0);
            scene::fill(scene, root, p.panel_bg, &frame);
            scene::stroke(scene, root, p.focus_ring, theme::BORDER_W * 2.0, &frame);
            self.area.draw(
                scene,
                root,
                engine,
                g.area,
                Some(t(lang, mode.placeholder())),
                true,
            );

            let chord = koru_keys::label_for(koru_keys::PRIMARY, koru_keys::named(NamedKey::Enter));
            let hint = fill(t(lang, Key::CanvasDialogHint), &[chord.as_str()]);
            let hint = engine.simple_layout(&hint, theme::role::hint());
            scene::draw_layout_first_line(scene, root, &hint, g.hint.0, g.hint.1, Some(p.text_dim));

            button::draw_button(
                scene,
                root,
                engine,
                g.cancel,
                t(lang, Key::CanvasDialogCancel),
                ButtonRole::Secondary,
                hover == Some(Button::Cancel),
                false,
            );
            button::draw_button(
                scene,
                root,
                engine,
                g.save,
                t(lang, Key::CanvasDialogSave),
                ButtonRole::Primary,
                hover == Some(Button::Save),
                false,
            );
        });
    }
}
