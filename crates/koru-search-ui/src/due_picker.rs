// SPDX-License-Identifier: MPL-2.0

//! The calendar a temporal `/key` opens, and the step that asks which side of
//! the date is meant.
//!
//! **The calendar is not here.** It is [`koru_ui::date_grid`], the same widget
//! the editor's due picker wears ([ADR-196]), so `2w`, `+3d` and `sept 10` mean
//! one thing in this app rather than two. What is here is the half a search box
//! needs and an editor does not: a **direction**. Setting a due says *this day*;
//! filtering by one says *before it*, *after it* or *the day itself*, and a
//! calendar has no cell for that question.
//!
//! **The step is content inside a fixed shell.** The popover's rect is the
//! grid's from the moment it opens and does not change when the step slides in,
//! so nothing reflows and the panel does not jump under the pointer; only the
//! two contents move, and [`DueFilterPicker::animating`] returns to `false`
//! when they land (ADR-26).
//!
//! **`/due:after` never sees the step.** The side is already named, so a chosen
//! day commits on the spot — asking again would be asking a question the person
//! has already answered.
//!
//! What it answers is **text for the query box** ([`Picked::Commit`]), never a
//! filter: the box is the model, `filters::split` reads it again on every
//! request, and the recents list replays a query as a string ([ADR-195] §5).
//!
//! [ADR-195]: ../../../docs/decisions/ADR-201-225.md#adr-218-filterchooser--the-search-box-parses-its-own-key-filters-and-the-chooser-is-built-from-the-definitions
//! [ADR-196]: ../../../docs/decisions/ADR-201-225.md#adr-219-dategrid--the-calendar-is-a-widget-in-koru-ui-what-a-day-means-stays-with-the-editor

use std::ops::Range;
use std::time::{Duration, Instant};

use chrono::{Duration as Delta, NaiveDateTime};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Fill;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use koru_draw::geometry::Rect;
use koru_draw::scene;
use koru_draw::text::TextEngine;
use koru_draw::theme;
use koru_ui::date_grid::{self, DateGrid, Pick};

/// How long the step takes to slide in. Short enough to read as one gesture,
/// long enough to say *the panel did not change, its contents did*.
const SLIDE: Duration = Duration::from_millis(140);

/// The heading band above the three rows: the day that was chosen, so the
/// question has its subject on screen while it is being answered.
const HEADING_H: f32 = 36.0;

/// One step row, matching every other popover row in this surface.
const ROW_H: f32 = 32.0;

/// Which side of the chosen date the filter means.
///
/// **Three, and they are the three the wire has.** `Before` and `After` are one
/// bound each; `Within` is the only one that needs two, and it is here rather
/// than as a general "between" because a search box asking for two dates in a
/// row is a form, not a gesture.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Side {
    Before,
    After,
    /// The chosen instant and the next twenty-four hours — a half-open
    /// `[t, t + 24h)`, which is what makes two adjacent windows partition a
    /// range instead of both claiming the edge.
    Within,
}

/// The rows of the step, in the order they are drawn.
pub(crate) const SIDES: [Side; 3] = [Side::Before, Side::After, Side::Within];

impl Side {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Before => "Before",
            Self::After => "After",
            Self::Within => "Within 24 hours",
        }
    }

    /// The operator this side is spelled with in the box — the wire's own
    /// token, because what is written back has to parse as the filter it came
    /// from.
    pub(crate) fn op_token(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
            Self::Within => "between",
        }
    }

    /// The side a qualifier already in the box names, or `None` for a spelling
    /// this step does not offer — which asks rather than guessing.
    pub(crate) fn of_op(op: koru_store_wire::search::AttrOp) -> Option<Self> {
        use koru_store_wire::search::AttrOp;
        match op {
            AttrOp::Before => Some(Self::Before),
            AttrOp::After => Some(Self::After),
            AttrOp::Between => Some(Self::Within),
            _ => None,
        }
    }
}

/// What the picker has to say back to the query box.
pub(crate) enum Picked {
    /// Handled; the panel stays open and nothing else may see the key.
    Consumed,
    /// Close the picker and let the box have this key — backspacing out of the
    /// filter that opened it is editing the query, not driving a calendar.
    Released,
    /// The whole `/key…` span becomes this text.
    Commit(String),
}

pub(crate) struct DueFilterPicker {
    /// Index into the surface's definitions — which attribute is being
    /// filtered.
    pub(crate) def: usize,
    /// The whole span in the query text this picker replaces on commit.
    pub(crate) span: Range<usize>,
    /// How the key is spelled in the box: `due`, not `koru.due`.
    segment: String,
    /// The side already named by `/due:before`. `Some` skips the step.
    qualifier: Option<Side>,
    pub(crate) grid: DateGrid,
    /// The instant that was chosen, once one has been. `None` while the
    /// calendar is showing.
    chosen: Option<NaiveDateTime>,
    /// Whether that instant carries a clock the box has to spell out. A day
    /// picked from a cell does not; `friday 17:30` does.
    timed: bool,
    /// When the step began sliding in, so the panel draws a movement rather
    /// than a jump — and stops asking for frames when it lands.
    slide_from: Option<Instant>,
    /// The highlighted row of the step, moved by the arrows.
    pub(crate) step: usize,
    /// The row under the pointer, or `None`.
    step_hover: Option<usize>,
}

impl DueFilterPicker {
    /// Open on `span`, showing `now`'s day.
    ///
    /// `now` is injected for the reason the editor's picker injects it: neither
    /// the grammar nor the grid reads a clock, so the one call that does is the
    /// caller's and a test can name a day.
    pub(crate) fn open(
        engine: &mut TextEngine,
        def: usize,
        segment: &str,
        span: Range<usize>,
        qualifier: Option<Side>,
        now: NaiveDateTime,
    ) -> Self {
        Self {
            def,
            span,
            segment: segment.to_owned(),
            qualifier,
            // No Clear row: there is nothing stored to clear here, and the way
            // out of a filter is to delete the text that carries it.
            grid: DateGrid::with_phrase_field(engine, now.date(), now, false),
            chosen: None,
            timed: false,
            slide_from: None,
            step: 0,
            step_hover: None,
        }
    }

    /// The popover's size — **the grid's, whichever half is showing**. That is
    /// the whole of "content-only": the shell is decided once, on open.
    pub(crate) fn size(&self) -> (f32, f32) {
        self.grid.size()
    }

    /// How far the step has slid in, 0 at the calendar and 1 at the step.
    pub(crate) fn slide(&self, now: Instant) -> f32 {
        let Some(from) = self.slide_from else {
            return 0.0;
        };
        let elapsed = now.saturating_duration_since(from).as_secs_f32();
        (elapsed / SLIDE.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// Whether a frame is still owed. **Returns to `false`**, which is what
    /// ADR-26 requires of the one exception to drawing only on change.
    pub(crate) fn animating(&self, now: Instant) -> bool {
        self.slide_from.is_some() && self.slide(now) < 1.0
    }

    /// Whether the step is the half that answers a key or a click.
    fn on_step(&self) -> bool {
        self.chosen.is_some()
    }

    /// The lower bound, as the box spells it: `2026-09-10`, or
    /// `2026-09-10T17:30` when a clock was typed.
    fn spell(&self, at: NaiveDateTime) -> String {
        if self.timed {
            at.format("%Y-%m-%dT%H:%M").to_string()
        } else {
            at.date().format("%Y-%m-%d").to_string()
        }
    }

    /// The filter text a side commits to, or `None` before a date is chosen.
    ///
    /// **`Within` is `lo..lo + 24h`**, spelled through the same `..` the
    /// grammar already reads for `between`, so what is written back parses as
    /// what was picked.
    pub(crate) fn commit_text(&self, side: Side) -> Option<String> {
        let at = self.chosen?;
        let value = match side {
            Side::Within => format!("{}..{}", self.spell(at), self.spell(at + Delta::hours(24))),
            _ => self.spell(at),
        };
        Some(format!(
            "{}{}:{} {value}",
            crate::filters::SIGIL,
            self.segment,
            side.op_token()
        ))
    }

    /// A date was named. Either it commits — the side was already spelled — or
    /// the step slides in to ask.
    fn choose(&mut self, at: NaiveDateTime, timed: bool, now: Instant) -> Picked {
        self.chosen = Some(at);
        self.timed = timed;
        if let Some(side) = self.qualifier
            && let Some(text) = self.commit_text(side)
        {
            return Picked::Commit(text);
        }
        self.slide_from = Some(now);
        self.step = 0;
        Picked::Consumed
    }

    /// A phrase the grid could not show: it carries a clock, so it is resolved
    /// here against the grid's own two reference points and used as the bound.
    ///
    /// A search bound is allowed to carry an hour where a month cell is not,
    /// which is why this is not simply refined to its day — throwing the clock
    /// away would answer a question nobody asked.
    fn timed_phrase(&mut self, phrase: &str, now: Instant) -> Picked {
        match koru_when::parse(phrase, self.grid.reference(), koru_when::DEFAULT_DATE_ORDER) {
            Some(koru_when::When::Wall(at)) => self.choose(at, true, now),
            Some(koru_when::When::Date(day)) => {
                self.choose(day.and_time(chrono::NaiveTime::MIN), false, now)
            }
            // The grammar declined it. The panel stays open with the text still
            // in the box, which is the only answer that sets no date the person
            // did not say.
            None => Picked::Consumed,
        }
    }

    pub(crate) fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
        now: Instant,
    ) -> Picked {
        if self.on_step() {
            return match key {
                Key::Named(NamedKey::ArrowDown) => {
                    self.step = (self.step + 1) % SIDES.len();
                    Picked::Consumed
                }
                Key::Named(NamedKey::ArrowUp) => {
                    self.step = (self.step + SIDES.len() - 1) % SIDES.len();
                    Picked::Consumed
                }
                Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) => {
                    match self.commit_text(SIDES[self.step]) {
                        Some(text) => Picked::Commit(text),
                        None => Picked::Consumed,
                    }
                }
                // Everything else is swallowed: the step is a question with
                // three answers, and stray typing must not reach the query box
                // behind it.
                _ => Picked::Consumed,
            };
        }
        // **Backspace out of an empty box hands the key back.** The grid would
        // otherwise eat it — it only reads Backspace as Clear, and there is no
        // Clear row here — leaving the one gesture that undoes `/due ` with
        // nothing to do.
        if matches!(
            key,
            Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete)
        ) && self.grid.phrase().is_none()
        {
            return Picked::Released;
        }
        match self.grid.key(engine, key, text, mods) {
            Pick::Day(day) => self.choose(day.and_time(chrono::NaiveTime::MIN), false, now),
            Pick::Timed(phrase) => self.timed_phrase(&phrase, now),
            Pick::None | Pick::Clear => Picked::Consumed,
        }
    }

    /// A click in panel-local coordinates.
    pub(crate) fn click(
        &mut self,
        engine: &mut TextEngine,
        x: f32,
        y: f32,
        now: Instant,
    ) -> Picked {
        if self.on_step() {
            return match self.row_at(y) {
                Some(row) => match self.commit_text(SIDES[row]) {
                    Some(text) => Picked::Commit(text),
                    None => Picked::Consumed,
                },
                None => Picked::Consumed,
            };
        }
        let field = date_grid::field_rect(date_grid::PANEL_W);
        if field.contains(x, y) {
            self.grid.click_field(engine, x, date_grid::PANEL_W);
            return Picked::Consumed;
        }
        match self.grid.click(x, y) {
            Pick::Day(day) => self.choose(day.and_time(chrono::NaiveTime::MIN), false, now),
            Pick::Timed(phrase) => self.timed_phrase(&phrase, now),
            Pick::None | Pick::Clear => Picked::Consumed,
        }
    }

    /// The step row a panel-local `y` is in.
    fn row_at(&self, y: f32) -> Option<usize> {
        let top = theme::PANEL_PAD + HEADING_H;
        if y < top {
            return None;
        }
        let row = ((y - top) / ROW_H).floor() as usize;
        (row < SIDES.len()).then_some(row)
    }

    /// Track the pointer over the step's rows. `true` means redraw.
    pub(crate) fn hover(&mut self, x: f32, y: f32) -> bool {
        let next = (self.on_step()
            && x >= 0.0
            && x <= date_grid::PANEL_W
            && y >= 0.0
            && y <= self.size().1)
            .then(|| self.row_at(y))
            .flatten();
        let changed = next != self.step_hover;
        self.step_hover = next;
        changed
    }

    /// Draw both halves inside the shell, offset by the slide and clipped to it.
    ///
    /// The panel's own frame is the shell's single `scene::floating_panel`
    /// call, made before this — a widget that drew its own would be the second
    /// recipe (`DESIGN.md`).
    pub(crate) fn draw(
        &mut self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        shell: Rect,
        radius: f32,
        now: Instant,
    ) {
        let t = self.slide(now);
        canvas.push_clip_layer(
            Fill::NonZero,
            root,
            &scene::rrect(shell.x, shell.y, shell.w, shell.h, radius),
        );
        if t < 1.0 {
            date_grid::draw(
                canvas,
                root,
                engine,
                &mut self.grid,
                shell.x - t * shell.w,
                shell.y,
                shell.w,
                false,
            );
        }
        if t > 0.0 {
            self.draw_step(canvas, root, engine, shell.x + (1.0 - t) * shell.w, shell.y);
        }
        canvas.pop_layer();
    }

    fn draw_step(
        &self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        px: f32,
        py: f32,
    ) {
        let Some(at) = self.chosen else { return };
        let p = theme::p();
        let heading = engine.simple_layout(
            &self.spell(at),
            theme::role::ui().with_weight(theme::FontWeight::SemiBold),
        );
        scene::draw_layout_first_line(
            canvas,
            root,
            &heading,
            px + theme::PANEL_PAD + 8.0,
            py + theme::PANEL_PAD + 4.0,
            Some(p.text),
        );
        let question = engine.simple_layout("Which side?", theme::role::hint());
        scene::draw_layout_first_line(
            canvas,
            root,
            &question,
            px + date_grid::PANEL_W - theme::PANEL_PAD - 8.0 - question.width(),
            py + theme::PANEL_PAD + 6.0,
            Some(p.row_hint),
        );

        let top = py + theme::PANEL_PAD + HEADING_H;
        for (index, side) in SIDES.iter().enumerate() {
            let row = Rect::new(
                px + theme::PANEL_PAD,
                top + index as f32 * ROW_H,
                date_grid::PANEL_W - theme::PANEL_PAD * 2.0,
                ROW_H - 2.0,
            );
            if self.step_hover == Some(index) || self.step == index {
                scene::fill(
                    canvas,
                    root,
                    p.hover_tint,
                    &scene::rrect(row.x, row.y, row.w, row.h, 7.0),
                );
            }
            // The window this side means, drawn dim on the right — the one
            // place a half-open range is legible before it is run.
            let window = self.window_hint(*side, at);
            let hint = engine.simple_layout(&window, theme::role::meta());
            scene::draw_layout_first_line(
                canvas,
                root,
                &hint,
                row.x + row.w - 8.0 - hint.width(),
                row.y + 8.0,
                Some(p.text_dim),
            );
            let label = engine.simple_layout(side.label(), theme::role::row());
            scene::draw_layout_first_line(
                canvas,
                root,
                &label,
                row.x + 10.0,
                row.y + 6.0,
                Some(p.text),
            );
        }
    }

    /// What a side means, spelled the way the store compares it: `<` excludes
    /// the edge, `>=` includes it.
    fn window_hint(&self, side: Side, at: NaiveDateTime) -> String {
        match side {
            Side::Before => format!("< {}", self.spell(at)),
            Side::After => format!(">= {}", self.spell(at)),
            Side::Within => format!("< {}", self.spell(at + Delta::hours(24))),
        }
    }
}

#[cfg(test)]
mod tests;
