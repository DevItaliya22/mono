// SPDX-License-Identifier: MPL-2.0

//! The dialog that says something went wrong, and what did not happen.
//!
//! The shell can raise one from anywhere a filesystem operation failed. It is a
//! scrim plus a centred [`floating_panel`](scene::floating_panel), the shape
//! `NewDocument` established, with a button row built the way `CloseDialog`'s
//! is — the same recipe rather than a fourth one.
//!
//! ## The parts, and why each is there
//!
//! - **A title that names the thing.** *Couldn't export "Research"*, not
//!   *Export failed* and certainly not *IO error*.
//! - **A body that says what did not happen.** [`ErrorReport::body`] carries
//!   it; this only lays it out. It is the sentence that stops somebody opening
//!   a file manager to find out whether a delete half-succeeded.
//! - **`Details`, folded away.** The raw OS error, verbatim, in the code font.
//!   Collapsed by default because it is unreadable to most people and the only
//!   thing that helps the rest.
//! - **`Copy error`.** So a bug report can contain the real message rather than
//!   a retyping of it.
//! - **Buttons.** *Cancel* always, then whatever the raise site offered, with
//!   the last drawn as the primary — *Try Again* where there is one.
//!
//! ## What is not here
//!
//! A name that is already taken, or empty, does **not** come through this. That
//! is not a failure of the filesystem, the field is still open, and a modal
//! would take the text away in order to complain about it. Those are an inline
//! hint on the field — see [`NamePrompt::hint`].
//!
//! [`NamePrompt::hint`]: koru_editor::components::name_prompt::NamePrompt::hint

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_workspace_host::{ErrorAction, ErrorReport};
// The shared button recipe. `CloseDialog` had already copied this once and a
// third copy is how a set of buttons stops agreeing what "primary" means.
use koru_ui::button::{self, ButtonRole, button_width};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

const WIDTH: f32 = 420.0;
const PAD: f32 = 20.0;
const RADIUS: f32 = 10.0;
const BTN_H: f32 = 32.0;
const BTN_GAP: f32 = 10.0;
/// The exclamation in its tinted disc.
const BADGE: f32 = 34.0;
// The three tiers this dialog speaks in, named once. The sizes are still
// wanted as bare numbers for the height arithmetic below, so each is read back
// off its own style rather than written twice.
const TITLE: theme::TextStyle =
    theme::TextStyle::new(theme::text_metrics::UI_BODY, theme::FontWeight::SemiBold);
const BODY: theme::TextStyle =
    theme::TextStyle::new(theme::text_metrics::SM, theme::FontWeight::Medium);
const SMALL: theme::TextStyle =
    theme::TextStyle::new(theme::text_metrics::XS, theme::FontWeight::Medium);
const TITLE_SIZE: f32 = TITLE.metrics.size;
const SMALL_SIZE: f32 = SMALL.metrics.size;
/// The `Details` well, when it is open.
const DETAILS_H: f32 = 64.0;

/// What the dialog wants done, once a press has landed on it.
pub(super) enum Outcome {
    /// Still open — a fold was toggled, or the press missed everything.
    Keep,
    Close,
    /// Copy the raw error to the clipboard, then stay open.
    Copy(String),
    /// Run one of the offered actions and close.
    Act(ErrorAction),
}

/// Which target a point is on.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Hit {
    Details,
    Copy,
    Cancel,
    Action(usize),
}

pub(super) struct ErrorDialog {
    pub(super) report: ErrorReport,
    /// Collapsed by default. The raw error helps one reader in twenty and
    /// frightens the other nineteen.
    open: bool,
    /// How far the details well is scrolled. The well is a fixed height and a
    /// long error — a URL, a path — wraps past it, so the text scrolls inside
    /// rather than overflowing onto the controls below.
    scroll: f32,
    hover: Option<Hit>,
}

impl ErrorDialog {
    pub(super) fn new(report: ErrorReport) -> Self {
        Self {
            report,
            open: false,
            scroll: 0.0,
            hover: None,
        }
    }

    /// How tall the panel is, summed from what it actually contains — so a
    /// report with no details and one with three lines of body both fit, and
    /// neither can drift from what is drawn.
    fn height(&self, engine: &mut TextEngine, w: f32) -> f32 {
        let text_w = w - PAD * 2.0 - BADGE - 14.0;
        let body = engine.simple_layout(&self.report.body, BODY);
        let body_h = wrapped_height(engine, &self.report.body, BODY, text_w).max(body.height());
        let mut h = PAD + TITLE_SIZE * 1.4 + 6.0 + body_h + PAD;
        if self.report.details.is_some() {
            h += SMALL_SIZE * 1.6 + 6.0;
            if self.open {
                h += DETAILS_H + 8.0;
            }
            h += SMALL_SIZE * 1.6 + 8.0;
        }
        h + BTN_H + PAD
    }

    fn panel(&self, engine: &mut TextEngine, viewport: (f32, f32)) -> Rect {
        let w = WIDTH.min((viewport.0 - 32.0).max(160.0));
        let h = self.height(engine, w).min((viewport.1 - 32.0).max(120.0));
        Rect::new(
            ((viewport.0 - w) * 0.5).max(8.0),
            ((viewport.1 - h) * 0.5).max(8.0),
            w,
            h,
        )
    }

    /// The badge, the title, the body — the fixed block at the top.
    fn text_x(&self, panel: Rect) -> f32 {
        panel.x + PAD + BADGE + 14.0
    }

    /// Every clickable rect, in one place, so drawing and hit-testing cannot
    /// disagree about where a button is.
    fn targets(&self, engine: &mut TextEngine, viewport: (f32, f32)) -> Vec<(Hit, Rect)> {
        let panel = self.panel(engine, viewport);
        let mut out = Vec::new();

        // The button row sits on the floor of the panel, right-aligned, and is
        // laid out from the right so the primary owns the corner.
        let mut x = panel.x + panel.w - PAD;
        let by = panel.y + panel.h - PAD - BTN_H;
        for (i, action) in self.report.actions.iter().enumerate().rev() {
            let w = button_width(engine, action.label());
            x -= w;
            out.push((Hit::Action(i), Rect::new(x, by, w, BTN_H)));
            x -= BTN_GAP;
        }
        let w = button_width(engine, "Cancel");
        x -= w;
        out.push((Hit::Cancel, Rect::new(x, by, w, BTN_H)));

        if self.report.details.is_some() {
            let tx = self.text_x(panel);
            // The disclosure sits under the body; its y is derived from the
            // button row rather than the body's height so the two cannot drift.
            let dy = by - 10.0 - SMALL_SIZE * 1.6;
            out.push((
                Hit::Copy,
                Rect::new(tx, dy, button_width(engine, "Copy error"), SMALL_SIZE * 1.6),
            ));
            let fold_y = if self.open {
                dy - 8.0 - DETAILS_H - SMALL_SIZE * 1.6
            } else {
                dy - SMALL_SIZE * 1.6
            };
            out.push((
                Hit::Details,
                Rect::new(
                    tx,
                    fold_y,
                    button_width(engine, "Details"),
                    SMALL_SIZE * 1.6,
                ),
            ));
        }
        out
    }

    fn hit_at(&self, engine: &mut TextEngine, viewport: (f32, f32), x: f32, y: f32) -> Option<Hit> {
        self.targets(engine, viewport)
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(h, _)| h)
    }

    pub(super) fn key(&mut self, key: &Key) -> Outcome {
        match key {
            // Enter runs the primary when there is one — the button in the
            // corner — and otherwise dismisses, which is what Cancel would do.
            Key::Named(NamedKey::Enter) => match self.report.actions.last() {
                Some(a) => Outcome::Act(a.clone()),
                None => Outcome::Close,
            },
            Key::Named(NamedKey::Escape) => Outcome::Close,
            // Everything else is swallowed: a modal that let keystrokes through
            // to the document behind it would take dictation during a warning.
            _ => Outcome::Keep,
        }
    }

    pub(super) fn mouse_down(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> Outcome {
        match self.hit_at(engine, viewport, x, y) {
            Some(Hit::Details) => {
                self.open = !self.open;
                self.scroll = 0.0;
                Outcome::Keep
            }
            Some(Hit::Copy) => match &self.report.details {
                Some(d) => Outcome::Copy(d.clone()),
                None => Outcome::Keep,
            },
            Some(Hit::Cancel) => Outcome::Close,
            Some(Hit::Action(i)) => match self.report.actions.get(i) {
                Some(a) => Outcome::Act(a.clone()),
                None => Outcome::Keep,
            },
            // **A press outside does not dismiss.** An error is the one panel
            // worth insisting on: dismissing it by clicking past it is how a
            // failed save gets missed, which is the failure this whole thing
            // exists to end.
            None => Outcome::Keep,
        }
    }

    /// Wheel input while the dialog is up. The dialog is modal, so the wheel
    /// has exactly one thing it could mean: move the details text, when it is
    /// open and taller than its well.
    pub(super) fn scroll_by(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        dy: f32,
    ) -> bool {
        if !self.open {
            return false;
        }
        let Some((well, content_h)) = self.details_well(engine, viewport) else {
            return false;
        };
        let max = (content_h - well.h).max(0.0);
        let next = (self.scroll + dy).clamp(0.0, max);
        if next == self.scroll {
            return false;
        }
        self.scroll = next;
        true
    }

    /// The open details well and the height of the wrapped text inside it,
    /// derived from the same targets the drawing uses so the two cannot
    /// disagree about where the well is.
    fn details_well(&self, engine: &mut TextEngine, viewport: (f32, f32)) -> Option<(Rect, f32)> {
        if !self.open {
            return None;
        }
        let details = self.report.details.as_ref()?;
        let panel = self.panel(engine, viewport);
        let tx = self.text_x(panel);
        let fold = self
            .targets(engine, viewport)
            .into_iter()
            .find(|(h, _)| *h == Hit::Details)?
            .1;
        let well = Rect::new(
            tx,
            fold.y + fold.h + 6.0,
            panel.x + panel.w - PAD - tx,
            DETAILS_H,
        );
        let content_h = engine
            .simple_layout_wrapped(details, SMALL, well.w - 20.0)
            .height()
            + 16.0;
        Some((well, content_h))
    }

    pub(super) fn mouse_move(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> bool {
        let was = self.hover;
        self.hover = self.hit_at(engine, viewport, x, y);
        was != self.hover
    }

    pub(super) fn cursor(&self) -> CursorIcon {
        match self.hover {
            Some(_) => koru_ui::cursor::PRESS,
            None => CursorIcon::Default,
        }
    }

    pub(super) fn draw(
        &self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        viewport: (f32, f32),
    ) {
        let p = theme::p();
        scene::fill(
            scene,
            root,
            p.scrim,
            &scene::rect(0.0, 0.0, viewport.0, viewport.1),
        );
        let panel = self.panel(engine, viewport);
        scene::floating_panel(
            scene,
            root,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            &scene::Panel::popover(RADIUS),
        );

        // The badge: an exclamation in a tinted disc, in the same red the close
        // button uses, so "this is bad" is one colour in the app rather than two.
        let bx = panel.x + PAD;
        let by = panel.y + PAD;
        scene::fill(
            scene,
            root,
            p.close_hover.with_alpha(0.18),
            &vello::kurbo::Circle::new(
                ((bx + BADGE * 0.5) as f64, (by + BADGE * 0.5) as f64),
                (BADGE * 0.5) as f64,
            ),
        );
        let bang = engine.simple_layout(
            "!",
            theme::TextStyle::new(theme::TextMetrics::new(17.0, 17.0), theme::FontWeight::Bold),
        );
        scene::draw_layout(
            scene,
            root,
            &bang,
            bx + (BADGE - bang.width()) * 0.5,
            by + (BADGE - bang.height()) * 0.5,
            Some(p.close_hover),
        );

        let tx = self.text_x(panel);
        let text_w = panel.x + panel.w - PAD - tx;
        let title = engine.simple_layout(&self.report.title, TITLE);
        scene::draw_layout_first_line(scene, root, &title, tx, by, Some(p.text));
        let body_y = by + TITLE_SIZE * 1.4 + 6.0;
        draw_wrapped(
            scene,
            root,
            engine,
            &self.report.body,
            tx,
            body_y,
            text_w,
            BODY,
            p.text,
        );

        for (hit, r) in self.targets(engine, viewport) {
            let hot = self.hover == Some(hit);
            match hit {
                Hit::Details => {
                    let caret = if self.open { "\u{25BE}" } else { "\u{25B8}" };
                    let label = engine.simple_layout(&format!("{caret} Details"), SMALL);
                    scene::draw_layout_first_line(
                        scene,
                        root,
                        &label,
                        r.x,
                        r.y,
                        Some(if hot { p.text } else { p.text_dim }),
                    );
                    if self.open
                        && let Some(details) = &self.report.details
                    {
                        let well =
                            Rect::new(tx, r.y + r.h + 6.0, panel.x + panel.w - PAD - tx, DETAILS_H);
                        scene::fill(
                            scene,
                            root,
                            p.code_bg,
                            &scene::rrect(well.x, well.y, well.w, well.h, 6.0),
                        );
                        // Wrapped, not elided: a path is the useful half of most
                        // of these messages and it is always at the end. Taller
                        // than the well, it scrolls inside a clip rather than
                        // overflowing onto the controls below.
                        let mono = engine.simple_layout_wrapped(details, SMALL, well.w - 20.0);
                        let content_h = mono.height() + 16.0;
                        let max = (content_h - well.h).max(0.0);
                        let scroll = self.scroll.min(max);
                        scene.push_clip_layer(
                            vello::peniko::Fill::NonZero,
                            root,
                            &scene::rrect(well.x, well.y, well.w, well.h, 6.0),
                        );
                        scene::draw_layout(
                            scene,
                            root,
                            &mono,
                            well.x + 10.0,
                            well.y + 8.0 - scroll,
                            Some(p.text),
                        );
                        scene.pop_layer();
                        if max > 0.0 {
                            let thumb_h = (well.h * well.h / content_h).max(20.0);
                            let thumb_y = well.y + 2.0 + scroll / max * (well.h - 4.0 - thumb_h);
                            scene::fill(
                                scene,
                                root,
                                p.text_dim.with_alpha(0.5),
                                &scene::rrect(well.x + well.w - 7.0, thumb_y, 3.0, thumb_h, 1.5),
                            );
                        }
                    }
                }
                Hit::Copy => {
                    let label = engine.simple_layout("Copy error", SMALL);
                    scene::draw_layout_first_line(
                        scene,
                        root,
                        &label,
                        r.x,
                        r.y,
                        Some(if hot { p.text } else { p.accent }),
                    );
                }
                Hit::Cancel => button::draw_button(
                    scene,
                    root,
                    engine,
                    r,
                    "Cancel",
                    ButtonRole::Secondary,
                    hot,
                    false,
                ),
                Hit::Action(i) => {
                    let last = i + 1 == self.report.actions.len();
                    let label = self.report.actions[i].label();
                    let role = if last {
                        ButtonRole::Primary
                    } else {
                        ButtonRole::Secondary
                    };
                    button::draw_button(scene, root, engine, r, label, role, hot, false);
                }
            }
        }
    }
}

/// How tall `text` is once wrapped to `w`.
fn wrapped_height(engine: &mut TextEngine, text: &str, style: theme::TextStyle, w: f32) -> f32 {
    engine.simple_layout_wrapped(text, style, w).height()
}

#[allow(clippy::too_many_arguments)]
fn draw_wrapped(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    style: theme::TextStyle,
    colour: vello::peniko::Color,
) {
    let layout = engine.simple_layout_wrapped(text, style, w);
    scene::draw_layout(scene, root, &layout, x, y, Some(colour));
}

#[cfg(test)]
mod tests;
