// SPDX-License-Identifier: MPL-2.0

//! What a day means, once the grid has named one.
//!
//! **The grid itself is not here.** The month, its cells, the phrase box over
//! [`koru_when`], the Clear row and every key they share went down to
//! [`koru_ui::date_grid`] ([ADR-196]), because a calendar is a widget and the
//! search surface needs the same one — `koru-search-ui` links neither this
//! crate nor `koru-core`, by design. What is left is the half that names the
//! editor's vocabulary: a [`NodeId`], `koru.due`, an [`AttrValue`], an
//! [`Outcome`].
//!
//! One grid, two commits. In its original mode it produces an **inline date
//! pill** — content, anchored in the text. Armed with a [`DueTarget`] (A3:
//! `/due`, the Due rows) the same grid commits `koru.due` through the
//! attribute outcomes instead, and grows a Clear row while the node has a due
//! — the color picker's pattern, as a footer under the grid because the panel
//! is a grid rather than a list.
//!
//! **The pill mode has one door left: clicking an existing pill's anchor.**
//! `/date` is gone — `/due` is the only way to put a date on a block
//! ([ADR-187]) — but a document written before that still holds date pills,
//! and they have to display and stay editable. Nothing migrates them.
//!
//! **In due mode the grid wears a phrase field** ([ADR-181], reversed). A month
//! grid can only say *which day*; `/due friday 17:30`, `+4h` and `in 2w` all
//! say things a grid has no cell for, and once a due chip is clickable the
//! picker is reached without typing a command at all — so a person editing a
//! date that way had nowhere left to type a relative phrase. A phrase naming a
//! **day** refines the grid's selection and the panel stays open; a phrase
//! naming a **time** the grid cannot show reaches here as
//! [`Pick::Timed`](koru_ui::date_grid::Pick::Timed) and travels on as
//! [`Outcome::CommandArg`], so `/due friday 17:30` and this box are given
//! meaning in exactly one place: the editor, where the zone seam that turns a
//! wall clock into a shared deadline lives ([ADR-191]).
//!
//! [ADR-187]: ../../../../docs/decisions/ADR-201-225.md#adr-210-titleattrs--a-title-row-takes-attributes-but-not-pills-and-date-is-gone
//! [ADR-181]: ../../../../docs/decisions/ADR-201-225.md#adr-204-duepriority--due-and-priority-enter-through-the-pickers-they-already-imply-and-a-typed-argument-commits-without-one
//! [ADR-191]: ../../../../docs/decisions/ADR-201-225.md#adr-214-whenparser--natural-language-dates-are-their-own-crate-with-two-reference-points-and-a-slash-order-that-is-a-parameter
//! [ADR-196]: ../../../../docs/decisions/ADR-201-225.md#adr-219-dategrid--the-calendar-is-a-widget-in-koru-ui-what-a-day-means-stays-with-the-editor

use chrono::{NaiveDate, NaiveDateTime};
use winit::keyboard::{Key, ModifiersState};

use super::Outcome;
use super::command_menu::Command;
use crate::trigger::{PillSpec, Trigger};
use crate::utils::when;
use koru_core::NodeId;
use koru_core::attributes::KEY_DUE;
use koru_core::node::AttrValue;
use koru_draw::text::TextEngine;
use koru_ui::date_grid::{DateGrid, Pick};

/// The grid's own numbers, under their old names — the four panels and the
/// container that place this popover ask for them here.
pub use koru_ui::date_grid::{CELL, GRID_H, HEADER_H, PANEL_W, WEEKDAY_H, field_band, field_rect};

/// What the grid is setting when it is armed for `koru.due` rather than an
/// inline pill.
pub struct DueTarget {
    /// The nodes the date applies to — one for `/due`, several for a
    /// multi-select apply from the context menu.
    pub nodes: Vec<NodeId>,
    /// Whether the (first) node already carries a due — decides the Clear
    /// row, because clearing nothing is not an offer.
    pub has_due: bool,
}

pub struct DatePicker {
    pub trigger: Trigger,
    /// The widget. Its `selected`, `query` and `now` are read through here.
    pub grid: DateGrid,
    /// `None` is the legacy pill mode, byte-for-byte unchanged; `Some` arms
    /// the attribute commit.
    pub due: Option<DueTarget>,
}

impl DatePicker {
    /// The legacy pill mode: a bare grid, no field, no Clear row.
    pub fn for_pill(trigger: Trigger, selected: NaiveDate) -> Self {
        Self {
            trigger,
            grid: DateGrid::bare(selected),
            due: None,
        }
    }

    /// Open armed for `koru.due` over `nodes`, the grid on `selected` —
    /// the node's current due date when it has one, else today — and a phrase
    /// field above it.
    ///
    /// `now` is the clock a typed phrase is resolved against, **injected**: the
    /// parser never reads system time and neither does the grid. It is the same
    /// clock the caller used to decide `selected`.
    pub fn for_due(
        engine: &mut TextEngine,
        trigger: Trigger,
        nodes: Vec<NodeId>,
        selected: NaiveDate,
        has_due: bool,
        now: NaiveDateTime,
    ) -> Self {
        Self {
            trigger,
            grid: DateGrid::with_phrase_field(engine, selected, now, has_due),
            due: Some(DueTarget { nodes, has_due }),
        }
    }

    /// Whether the panel ends in a Clear row.
    pub fn offers_clear(&self) -> bool {
        self.grid.offers_clear
    }

    /// Whether the panel carries the phrase field.
    pub fn has_field(&self) -> bool {
        self.grid.has_field()
    }

    /// The band the field costs above the grid — zero without one.
    pub fn band(&self) -> f32 {
        self.grid.band()
    }

    pub fn size(&self) -> (f32, f32) {
        self.grid.size()
    }

    /// The Clear row's top, in panel-local y.
    pub fn clear_row_top(&self) -> Option<f32> {
        self.grid.clear_row_top()
    }

    /// The day under a panel-local point.
    pub fn cell_at_local(&self, local_x: f32, local_y: f32) -> Option<NaiveDate> {
        self.grid.cell_at_local(local_x, local_y)
    }

    /// The trimmed phrase in the field, when there is one and it is not blank.
    pub fn phrase(&self) -> Option<String> {
        self.grid.phrase()
    }

    /// The two reference points a typed phrase is read against — the injected
    /// clock and the day the grid is highlighting.
    ///
    /// Read from *here* by `editor.rs` when a `/due <phrase>` argument arrives
    /// from the command menu while this panel is open, so the phrase is
    /// resolved against the same selection the grid is showing
    /// ([ADR-191] §3).
    ///
    /// [ADR-191]: ../../../../docs/decisions/ADR-201-225.md#adr-214-whenparser--natural-language-dates-are-their-own-crate-with-two-reference-points-and-a-slash-order-that-is-a-parameter
    pub fn reference(&self) -> when::Reference {
        self.grid.reference()
    }

    /// What picking `date` means in this mode.
    ///
    /// A cell in a month grid names a **day**, so it commits a `DateOnly` —
    /// never a midnight instant, which would be a different day for a third of
    /// the planet ([ADR-183]).
    ///
    /// [ADR-183]: ../../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
    fn commit(&self, date: NaiveDate) -> Outcome {
        match &self.due {
            Some(target) => Outcome::SetAttr(
                target.nodes.clone(),
                KEY_DUE.to_owned(),
                AttrValue::Date(koru_core::time::DateOnly::new(date)),
            ),
            None => Outcome::Commit(PillSpec::date(date)),
        }
    }

    /// The grid's answer in the editor's vocabulary.
    ///
    /// This is the whole adapter: four arms, and the one that carries a
    /// `NodeId` is the reason the grid below cannot answer it itself. A
    /// [`Pick::Clear`] on a picker with no `due` cannot arise — the grid only
    /// offers the row when the caller armed it — and answering `Consumed` is
    /// what keeps that unreachable case from being a panic.
    fn resolve(&self, pick: Pick) -> Outcome {
        match pick {
            Pick::None => Outcome::Consumed,
            Pick::Day(date) => self.commit(date),
            Pick::Timed(phrase) => Outcome::CommandArg(Command::Due, phrase),
            Pick::Clear => match &self.due {
                Some(target) => Outcome::ClearAttr(target.nodes.clone(), KEY_DUE.to_owned()),
                None => Outcome::Consumed,
            },
        }
    }

    pub fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Outcome {
        let pick = self.grid.key(engine, key, text, mods);
        self.resolve(pick)
    }

    pub fn click(&self, local_x: f32, local_y: f32) -> Outcome {
        self.resolve(self.grid.click(local_x, local_y))
    }

    /// Put the caret in the phrase field at a panel-local x.
    pub fn click_field(&mut self, engine: &mut TextEngine, x: f32, panel_w: f32) {
        self.grid.click_field(engine, x, panel_w);
    }

    pub fn drag_field(&mut self, engine: &mut TextEngine, x: f32, panel_w: f32) {
        self.grid.drag_field(engine, x, panel_w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::dates;
    use chrono::Datelike;
    use koru_draw::theme;
    use koru_ui::date_grid::cell_rect;
    use winit::keyboard::NamedKey;

    /// Centre of the cell the grid draws `day` in, in panel-local coordinates,
    /// shifted down by whatever band `p` wears. `cell_rect` is the widget's own
    /// definition, so this cannot drift from the drawing.
    fn cell_centre_in(p: &DatePicker, month: NaiveDate, day: u32) -> (f32, f32) {
        let r = cell_rect(month, day);
        (r.x + r.w * 0.5, r.y + r.h * 0.5 + p.band())
    }

    /// A constructed key press. `KeyEvent` cannot be built in a test, which is
    /// why `key` takes the three pieces.
    fn named(p: &mut DatePicker, engine: &mut TextEngine, k: NamedKey) -> Outcome {
        p.key(engine, &Key::Named(k), None, ModifiersState::empty())
    }

    fn typed(p: &mut DatePicker, engine: &mut TextEngine, s: &str) {
        for ch in s.chars() {
            let text = ch.to_string();
            p.key(
                engine,
                &Key::Character(text.clone().into()),
                Some(&text),
                ModifiersState::empty(),
            );
        }
    }

    fn picker() -> DatePicker {
        DatePicker::for_pill(
            Trigger::at_start(),
            NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
        )
    }

    /// Stray typing must not reach the document behind the panel, or it extends
    /// the trigger text and the picker closes underneath itself.
    #[test]
    fn stray_typing_is_swallowed_not_ignored() {
        let mut engine = TextEngine::new();
        let mut p = picker();
        assert!(matches!(
            p.key(
                &mut engine,
                &Key::Character("x".into()),
                Some("x"),
                ModifiersState::empty()
            ),
            Outcome::Consumed
        ));
    }

    /// **The pill mode is untouched by the due arming, by `/date`'s removal,
    /// and by the phrase field.** Enter and a cell click still answer
    /// `Commit(PillSpec)`, there is no Clear row and no box to type in — which
    /// is what keeps a date pill written before [ADR-187] editable by clicking
    /// it.
    #[test]
    fn the_pill_mode_still_commits_a_pill_and_wears_no_clear_row() {
        let mut engine = TextEngine::new();
        let mut p = picker();
        assert!(p.clear_row_top().is_none());
        assert!(!p.has_field());
        assert_eq!(p.band(), 0.0, "and costs no band");
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Enter),
            Outcome::Commit(_)
        ));
        // Delete is stray typing here, not a clear.
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Delete),
            Outcome::Consumed
        ));
        let (x, y) = cell_centre_in(&p, NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(), 2);
        assert!(matches!(p.click(x, y), Outcome::Commit(_)));
    }

    /// The grid's keys still reach it through the adapter — one press of each
    /// kind, because the split itself is pinned beside the widget.
    #[test]
    fn arrows_move_by_a_day_and_a_week_and_pages_by_a_month() {
        let mut engine = TextEngine::new();
        let mut p = picker();
        named(&mut p, &mut engine, NamedKey::ArrowRight);
        assert_eq!(
            p.grid.selected,
            NaiveDate::from_ymd_opt(2026, 8, 3).unwrap()
        );
        named(&mut p, &mut engine, NamedKey::ArrowDown);
        assert_eq!(
            p.grid.selected,
            NaiveDate::from_ymd_opt(2026, 8, 10).unwrap()
        );
        named(&mut p, &mut engine, NamedKey::PageUp);
        assert_eq!(
            p.grid.selected,
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()
        );
    }

    /// A fixed clock, so every phrase below is a plain assertion: Sunday
    /// 2 August 2026, 10:15.
    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, 2)
            .unwrap()
            .and_hms_opt(10, 15, 0)
            .unwrap()
    }

    fn due_picker(engine: &mut TextEngine, has_due: bool) -> (DatePicker, NodeId) {
        let node = NodeId::new();
        let p = DatePicker::for_due(
            engine,
            Trigger::at_start(),
            vec![node],
            NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
            has_due,
            now(),
        );
        (p, node)
    }

    /// Armed for `koru.due`, the same grid commits the attribute — a
    /// **calendar date**, the shape for a day with no time. Never a midnight
    /// instant: that would be a different day for a third of the planet.
    #[test]
    fn the_due_mode_commits_the_attribute_on_enter_and_click() {
        let mut engine = TextEngine::new();
        let (mut p, node) = due_picker(&mut engine, false);
        let out = named(&mut p, &mut engine, NamedKey::Enter);
        assert!(
            matches!(&out, Outcome::SetAttr(nodes, key, AttrValue::Date(d))
                if nodes == &vec![node]
                    && key == KEY_DUE
                    && d.date() == NaiveDate::from_ymd_opt(2026, 8, 2).unwrap()),
            "got {out:?}"
        );
        let (x, y) = cell_centre_in(&p, NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(), 14);
        assert!(matches!(
            p.click(x, y),
            Outcome::SetAttr(_, _, AttrValue::Date(d))
                if d.date() == NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()
        ));
    }

    /// The Clear row exists exactly while the node has a due, sits under the
    /// grid, and clears from a click, Delete or Backspace. The panel is
    /// taller by that row, so drawing and hit-testing share the geometry.
    #[test]
    fn the_clear_row_exists_only_with_a_due_and_clears() {
        let mut engine = TextEngine::new();
        let (p, _) = due_picker(&mut engine, false);
        assert!(p.clear_row_top().is_none(), "nothing set, nothing to clear");

        let (mut p, node) = due_picker(&mut engine, true);
        let top = p.clear_row_top().expect("a due offers Clear");
        let (_, h) = p.size();
        assert!(top + theme::ROW_H <= h, "the row fits inside the panel");
        let (_, without) = due_picker(&mut engine, false).0.size();
        assert_eq!(h, without + theme::ROW_H, "the panel grew by the row");

        assert!(matches!(
            p.click(PANEL_W * 0.5, top + theme::ROW_H * 0.5),
            Outcome::ClearAttr(nodes, key) if nodes == vec![node] && key == KEY_DUE
        ));
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Delete),
            Outcome::ClearAttr(_, _)
        ));
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Backspace),
            Outcome::ClearAttr(_, _)
        ));
    }

    // ------------------------------------------------- the phrase field

    /// The due picker wears a field, the pill picker does not, and the panel
    /// is taller by exactly the band the grid is pushed down by — so a cell
    /// is still clickable where it is drawn.
    #[test]
    fn the_due_picker_wears_a_field_and_the_grid_moves_down_by_its_band() {
        let mut engine = TextEngine::new();
        let (p, _) = due_picker(&mut engine, false);
        assert!(p.has_field());
        assert!(p.band() > 0.0);
        assert_eq!(p.size().1, picker().size().1 + p.band());
        assert_eq!(p.size().0, PANEL_W, "the field does not widen the panel");

        // The grid's own arithmetic is unmoved; only the band is taken off.
        let month = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        for day in 1..=dates::days_in_month(month) {
            let (x, y) = cell_centre_in(&p, month, day);
            assert_eq!(p.cell_at_local(x, y), month.with_day(day), "day {day}");
        }
        // And the field's own box is above all of it.
        assert!(field_rect(PANEL_W).y + field_rect(PANEL_W).h <= p.band());
    }

    /// The grammar the box accepts is `koru-when`'s, and a phrase naming a
    /// **time** commits as [`Outcome::CommandArg`] — which is where
    /// `/due friday 17:30` is given meaning too, because the zone seam is the
    /// editor's. A grid has no cell for an hour, so this is the one kind of
    /// phrase the panel cannot show back to you.
    #[test]
    fn a_timed_phrase_commits_as_a_due_argument() {
        let mut engine = TextEngine::new();
        for phrase in ["4h", "+4h", "in 4h", "30m", "friday 17:30", "aug 31 at 9pm"] {
            let (mut p, _) = due_picker(&mut engine, false);
            typed(&mut p, &mut engine, phrase);
            assert_eq!(p.phrase().as_deref(), Some(phrase));
            let out = named(&mut p, &mut engine, NamedKey::Enter);
            assert!(
                matches!(&out, Outcome::CommandArg(Command::Due, arg) if arg == phrase),
                "{phrase:?} gave {out:?}"
            );
        }
    }

    /// **A phrase naming a day moves the grid and does not commit** — the panel
    /// stays open, the box is emptied, and nothing is set yet. This is the half
    /// of Enter that changed: it used to commit and close.
    #[test]
    fn a_day_phrase_moves_the_selection_and_leaves_the_panel_open() {
        let mut engine = TextEngine::new();
        for (phrase, expect) in [
            ("friday", (2026, 8, 7)),
            ("2w", (2026, 8, 16)),
            ("+2w", (2026, 8, 16)),
            ("3d", (2026, 8, 5)),
            ("aug 31", (2026, 8, 31)),
            ("10/28", (2026, 10, 28)),
            ("sept 10", (2026, 9, 10)),
        ] {
            let (mut p, _) = due_picker(&mut engine, false);
            typed(&mut p, &mut engine, phrase);
            let out = named(&mut p, &mut engine, NamedKey::Enter);
            assert!(
                matches!(out, Outcome::Consumed),
                "{phrase:?} committed instead of refining: {out:?}"
            );
            let (y, m, d) = expect;
            assert_eq!(
                p.grid.selected,
                NaiveDate::from_ymd_opt(y, m, d).unwrap(),
                "{phrase:?}"
            );
            assert_eq!(p.phrase(), None, "{phrase:?} left the box emptied");
        }
    }

    /// **The picker is a small calculator.** Each phrase reads against where
    /// the last one landed, which is the whole reason `koru-when`'s sign binds
    /// to the *selection* rather than to the clock — and it is only visible
    /// once the selection has moved off today.
    ///
    /// Then the commit: the box is empty after a refine, so Enter is the grid's
    /// again and a second press sets the day that was aimed at. Nobody is
    /// stranded.
    #[test]
    fn phrases_refine_the_selection_and_a_second_enter_commits_it() {
        let mut engine = TextEngine::new();
        let (mut p, node) = due_picker(&mut engine, false);

        // `2w` is unsigned, so it counts from the clock: 2 Aug + 14 days.
        typed(&mut p, &mut engine, "2w");
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Enter),
            Outcome::Consumed
        ));
        assert_eq!(
            p.grid.selected,
            NaiveDate::from_ymd_opt(2026, 8, 16).unwrap()
        );

        // `+3d` is signed, so it counts from *that*, not from the clock.
        typed(&mut p, &mut engine, "+3d");
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Enter),
            Outcome::Consumed
        ));
        assert_eq!(
            p.grid.selected,
            NaiveDate::from_ymd_opt(2026, 8, 19).unwrap(),
            "three days on from the selection, not from 2 August"
        );

        // And backwards, which has no unsigned spelling at all.
        typed(&mut p, &mut engine, "-1w");
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Enter),
            Outcome::Consumed
        ));
        assert_eq!(
            p.grid.selected,
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()
        );

        // The box is empty, so Enter belongs to the grid: this is the commit.
        assert_eq!(p.phrase(), None);
        let out = named(&mut p, &mut engine, NamedKey::Enter);
        assert!(
            matches!(&out, Outcome::SetAttr(nodes, key, AttrValue::Date(d))
                if nodes == &vec![node]
                    && key == KEY_DUE
                    && d.date() == NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()),
            "got {out:?}"
        );
    }

    /// Clicking a day is the other commit gesture, and a refine does not take
    /// it away — the grid is live throughout.
    #[test]
    fn a_refined_selection_can_also_be_committed_by_clicking_a_day() {
        let mut engine = TextEngine::new();
        let (mut p, _) = due_picker(&mut engine, false);
        typed(&mut p, &mut engine, "+1mo");
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Enter),
            Outcome::Consumed
        ));
        assert_eq!(
            p.grid.selected,
            NaiveDate::from_ymd_opt(2026, 9, 2).unwrap()
        );
        // The grid is showing September now, so a cell click names a September
        // day — which is what makes the refine worth having.
        let (x, y) = cell_centre_in(&p, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(), 20);
        assert!(matches!(
            p.click(x, y),
            Outcome::SetAttr(_, _, AttrValue::Date(d))
                if d.date() == NaiveDate::from_ymd_opt(2026, 9, 20).unwrap()
        ));
    }

    /// **A bare time lands here for free**, because the box *is* the parser —
    /// and it is the surface that gains most from it: a month grid cannot say
    /// "at five" at all. The ambiguity guard travels with it, so a bare number
    /// still commits nothing.
    #[test]
    fn the_phrase_box_takes_a_bare_time_and_still_refuses_a_bare_number() {
        let mut engine = TextEngine::new();
        for phrase in ["5pm", "9am", "17:30", "9:15pm"] {
            let (mut p, _) = due_picker(&mut engine, false);
            typed(&mut p, &mut engine, phrase);
            let out = named(&mut p, &mut engine, NamedKey::Enter);
            assert!(
                matches!(&out, Outcome::CommandArg(Command::Due, arg) if arg == phrase),
                "{phrase:?} gave {out:?}"
            );
        }
        for phrase in ["31", "2026", "9"] {
            let (mut p, _) = due_picker(&mut engine, false);
            typed(&mut p, &mut engine, phrase);
            assert!(
                matches!(
                    named(&mut p, &mut engine, NamedKey::Enter),
                    Outcome::Consumed
                ),
                "{phrase:?} should commit nothing"
            );
        }
    }

    /// **Junk commits nothing, clears nothing, moves nothing and keeps the
    /// text**, and the grid is still usable underneath — the panel stays open.
    ///
    /// The "keeps the text" half is what separates a refusal from a successful
    /// refine, which empties the box: emptying it here would look exactly like
    /// having understood the phrase.
    #[test]
    fn an_unparseable_phrase_neither_commits_nor_clears() {
        let mut engine = TextEngine::new();
        let (mut p, _) = due_picker(&mut engine, true);
        let held = p.grid.selected;
        typed(&mut p, &mut engine, "asdf");
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Enter),
            Outcome::Consumed
        ));
        assert_eq!(p.grid.selected, held, "the grid did not move");
        assert_eq!(
            p.phrase().as_deref(),
            Some("asdf"),
            "and the text is still there"
        );
        // Not a clear either, even though this node has a due.
        let (x, y) = cell_centre_in(&p, NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(), 9);
        assert!(matches!(
            p.click(x, y),
            Outcome::SetAttr(_, _, AttrValue::Date(d))
                if d.date() == NaiveDate::from_ymd_opt(2026, 8, 9).unwrap()
        ));
    }

    /// The keyboard split: Up/Down and the pages are always the grid's;
    /// Left/Right and Backspace belong to the field the moment there is
    /// something in it, and to the grid (and the Clear row) while it is empty.
    #[test]
    fn the_field_takes_the_keys_it_needs_and_leaves_the_rest_to_the_grid() {
        let mut engine = TextEngine::new();
        let (mut p, _) = due_picker(&mut engine, true);

        // Empty: Left steps a day and Backspace clears.
        named(&mut p, &mut engine, NamedKey::ArrowLeft);
        assert_eq!(
            p.grid.selected,
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap()
        );
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Backspace),
            Outcome::ClearAttr(_, _)
        ));

        // Typed: the same two keys are the caret's, and the grid is unmoved.
        typed(&mut p, &mut engine, "fri");
        let held = p.grid.selected;
        named(&mut p, &mut engine, NamedKey::ArrowLeft);
        assert_eq!(p.grid.selected, held, "the caret moved, not the month");
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Backspace),
            Outcome::Consumed
        ));
        assert_eq!(
            p.phrase().as_deref(),
            Some("fi"),
            "the caret had stepped left, so Backspace took the `r`"
        );

        // Up/Down and the pages never belong to a one-line box.
        named(&mut p, &mut engine, NamedKey::ArrowDown);
        assert_eq!(p.grid.selected, dates::shift_days(held, 7));
        named(&mut p, &mut engine, NamedKey::PageDown);
        assert_eq!(
            p.grid.selected,
            dates::shift_month(dates::shift_days(held, 7), 1)
        );
    }

    /// **Alt+C is the Clear row's accelerator, and the only keyboard route to
    /// it while a phrase is typed** — Delete and Backspace belong to the caret
    /// then, which left Clear unreachable from the keyboard for anyone who had
    /// started typing. Constructed keys throughout: a `KeyEvent` cannot be
    /// built in a test, which is why `key` takes the three pieces.
    #[test]
    fn alt_c_clears_from_the_phrase_field() {
        let mut engine = TextEngine::new();
        let alt_c = |p: &mut DatePicker, engine: &mut TextEngine| {
            p.key(
                engine,
                &Key::Character("c".into()),
                Some("c"),
                ModifiersState::ALT,
            )
        };

        // With something typed, where Delete and Backspace are the caret's.
        let (mut p, node) = due_picker(&mut engine, true);
        typed(&mut p, &mut engine, "fri");
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Backspace),
            Outcome::Consumed
        ));
        assert!(matches!(
            alt_c(&mut p, &mut engine),
            Outcome::ClearAttr(nodes, key) if nodes == vec![node] && key == KEY_DUE
        ));
        assert_eq!(p.phrase().as_deref(), Some("fr"), "and typed no `c`");

        // Windows delivers a Ctrl-style letter as its ASCII control code in
        // `text` often enough that `ctrl_letter` normalises both; Alt+C has to
        // be recognised through the same function or it fires on one platform.
        let (mut p, _) = due_picker(&mut engine, true);
        assert!(matches!(
            p.key(
                &mut engine,
                &Key::Character("\u{3}".into()),
                Some("\u{3}"),
                ModifiersState::ALT
            ),
            Outcome::ClearAttr(_, _)
        ));

        // Nothing to clear: swallowed, never typed. A `c` in the box here would
        // be the same key doing two unrelated things.
        let (mut p, _) = due_picker(&mut engine, false);
        assert!(matches!(alt_c(&mut p, &mut engine), Outcome::Consumed));
        assert_eq!(p.phrase(), None, "Alt+C is not a character");

        // A plain `c` is still a character, and Ctrl+Alt+C is AltGr's, not
        // ours — neither may clear.
        let (mut p, _) = due_picker(&mut engine, true);
        typed(&mut p, &mut engine, "c");
        assert_eq!(p.phrase().as_deref(), Some("c"));
        assert!(matches!(
            p.key(
                &mut engine,
                &Key::Character("c".into()),
                Some("c"),
                ModifiersState::CONTROL | ModifiersState::ALT
            ),
            Outcome::Consumed
        ));
        assert!(matches!(
            p.key(
                &mut engine,
                &Key::Character("c".into()),
                Some("c"),
                ModifiersState::ALT | ModifiersState::SHIFT
            ),
            Outcome::Consumed
        ));
    }

    /// Whitespace is not a phrase: Enter with a blank box is the grid's.
    #[test]
    fn a_blank_box_leaves_enter_to_the_grid() {
        let mut engine = TextEngine::new();
        let (mut p, _) = due_picker(&mut engine, false);
        typed(&mut p, &mut engine, "  ");
        assert_eq!(p.phrase(), None);
        assert!(matches!(
            named(&mut p, &mut engine, NamedKey::Enter),
            Outcome::SetAttr(_, _, AttrValue::Date(_))
        ));
    }
}
