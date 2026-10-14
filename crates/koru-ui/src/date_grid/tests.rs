// SPDX-License-Identifier: MPL-2.0

//! The widget's own contract, on [`Pick`] — no node, no attribute, no
//! document. What a day *means* is proved beside the adapter that decides,
//! `koru_editor::components::date_picker`.

use super::*;

/// Centre of the cell the renderer draws `day` in, measured from the top of
/// the grid. Both come from [`cell_rect`], which is the point: the round-trip
/// below cannot pass by two copies of the arithmetic agreeing with each other.
fn cell_centre(month: NaiveDate, day: u32) -> (f32, f32) {
    let r = cell_rect(month, day);
    (r.x + r.w * 0.5, r.y + r.h * 0.5)
}

/// The same point in **panel**-local coordinates: shifted down by whatever band
/// `g` wears, which is what `draw` does to the grid's origin.
fn cell_centre_in(g: &DateGrid, month: NaiveDate, day: u32) -> (f32, f32) {
    let (x, y) = cell_centre(month, day);
    (x, y + g.band())
}

/// A constructed key press. `KeyEvent` cannot be built in a test, which is
/// why `key` takes the three pieces.
fn named(g: &mut DateGrid, engine: &mut TextEngine, k: NamedKey) -> Pick {
    g.key(engine, &Key::Named(k), None, ModifiersState::empty())
}

fn typed(g: &mut DateGrid, engine: &mut TextEngine, s: &str) {
    for ch in s.chars() {
        let text = ch.to_string();
        g.key(
            engine,
            &Key::Character(text.clone().into()),
            Some(&text),
            ModifiersState::empty(),
        );
    }
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// A fixed clock, so every phrase below is a plain assertion: Sunday
/// 2 August 2026, 10:15.
fn now() -> NaiveDateTime {
    day(2026, 8, 2).and_hms_opt(10, 15, 0).unwrap()
}

fn bare() -> DateGrid {
    DateGrid::bare(day(2026, 8, 2))
}

fn with_field(engine: &mut TextEngine, offers_clear: bool) -> DateGrid {
    DateGrid::with_phrase_field(engine, day(2026, 8, 2), now(), offers_clear)
}

/// Every day in a month hit-tests back to itself. Two months, because a
/// month starting on Sunday is the off-by-one case.
#[test]
fn every_day_cell_hit_tests_back_to_its_own_date() {
    for month in [day(2026, 8, 1), day(2026, 3, 1)] {
        for d in 1..=dates::days_in_month(month) {
            let (x, y) = cell_centre(month, d);
            assert_eq!(cell_at(month, x, y), month.with_day(d), "{month} day {d}");
        }
    }
}

#[test]
fn clicks_outside_the_day_cells_select_nothing() {
    let month = day(2026, 8, 1);
    assert_eq!(cell_at(month, -5.0, 60.0), None, "left of the grid");
    assert_eq!(cell_at(month, 20.0, 4.0), None, "up in the header");
    assert_eq!(cell_at(month, 9999.0, 60.0), None, "past the last column");
}

#[test]
fn arrows_move_by_a_day_and_a_week_and_pages_by_a_month() {
    let mut engine = TextEngine::new();
    let mut g = bare();
    named(&mut g, &mut engine, NamedKey::ArrowRight);
    assert_eq!(g.selected, day(2026, 8, 3));
    named(&mut g, &mut engine, NamedKey::ArrowDown);
    assert_eq!(g.selected, day(2026, 8, 10));
    named(&mut g, &mut engine, NamedKey::PageUp);
    assert_eq!(g.selected, day(2026, 7, 10));
}

/// Stray typing must be swallowed rather than ignored, or it reaches whatever
/// the panel is floating over — in the document editor, the trigger text the
/// panel is anchored to, which closes the panel underneath itself.
#[test]
fn stray_typing_is_swallowed_not_ignored() {
    let mut engine = TextEngine::new();
    let mut g = bare();
    assert_eq!(
        g.key(
            &mut engine,
            &Key::Character("x".into()),
            Some("x"),
            ModifiersState::empty()
        ),
        Pick::None
    );
    assert_eq!(g.selected, day(2026, 8, 2), "and moved nothing");
}

/// A bare grid is the whole widget minus its two optional parts: Enter and a
/// cell click name a day, there is no Clear row and no box to type in.
#[test]
fn a_bare_grid_wears_no_field_and_no_clear_row() {
    let mut engine = TextEngine::new();
    let mut g = bare();
    assert!(g.clear_row_top().is_none());
    assert!(!g.has_field());
    assert_eq!(g.band(), 0.0, "and costs no band");
    assert_eq!(
        named(&mut g, &mut engine, NamedKey::Enter),
        Pick::Day(day(2026, 8, 2))
    );
    // Delete is stray typing here, not a clear.
    assert_eq!(named(&mut g, &mut engine, NamedKey::Delete), Pick::None);
    let (x, y) = cell_centre(day(2026, 8, 1), 2);
    assert_eq!(g.click(x, y), Pick::Day(day(2026, 8, 2)));
}

/// The Clear row exists exactly while it is offered, sits under the grid, and
/// is asked for by a click, Delete or Backspace. The panel is taller by that
/// row, so drawing and hit-testing share the geometry.
#[test]
fn the_clear_row_exists_only_when_offered_and_answers_clear() {
    let mut engine = TextEngine::new();
    let g = with_field(&mut engine, false);
    assert!(g.clear_row_top().is_none(), "nothing set, nothing to clear");

    let mut g = with_field(&mut engine, true);
    let top = g.clear_row_top().expect("an offer means a row");
    let (_, h) = g.size();
    assert!(top + theme::ROW_H <= h, "the row fits inside the panel");
    let (_, without) = with_field(&mut engine, false).size();
    assert_eq!(h, without + theme::ROW_H, "the panel grew by the row");

    assert_eq!(
        g.click(PANEL_W * 0.5, top + theme::ROW_H * 0.5),
        Pick::Clear
    );
    assert_eq!(named(&mut g, &mut engine, NamedKey::Delete), Pick::Clear);
    assert_eq!(named(&mut g, &mut engine, NamedKey::Backspace), Pick::Clear);
}

// ------------------------------------------------------- the phrase field

/// The field pushes the grid down by exactly its band — so a cell is still
/// clickable where it is drawn — and does not widen the panel.
#[test]
fn the_phrase_field_pushes_the_grid_down_by_its_band() {
    let mut engine = TextEngine::new();
    let g = with_field(&mut engine, false);
    assert!(g.has_field());
    assert!(g.band() > 0.0);
    assert_eq!(g.size().1, bare().size().1 + g.band());
    assert_eq!(g.size().0, PANEL_W, "the field does not widen the panel");

    // The grid's own arithmetic is unmoved; only the band is taken off.
    let month = day(2026, 8, 1);
    for d in 1..=dates::days_in_month(month) {
        let (x, y) = cell_centre_in(&g, month, d);
        assert_eq!(g.cell_at_local(x, y), month.with_day(d), "day {d}");
    }
    // And the field's own box is above all of it.
    assert!(field_rect(PANEL_W).y + field_rect(PANEL_W).h <= g.band());
}

/// The grammar the box accepts is `koru-when`'s, and a phrase naming a **time**
/// is handed up verbatim: a grid has no cell for an hour, so this is the one
/// kind of phrase the panel cannot show back to you.
#[test]
fn a_timed_phrase_is_handed_up_verbatim() {
    let mut engine = TextEngine::new();
    for phrase in ["4h", "+4h", "in 4h", "30m", "friday 17:30", "aug 31 at 9pm"] {
        let mut g = with_field(&mut engine, false);
        typed(&mut g, &mut engine, phrase);
        assert_eq!(g.phrase().as_deref(), Some(phrase));
        assert_eq!(
            named(&mut g, &mut engine, NamedKey::Enter),
            Pick::Timed(phrase.to_owned()),
            "{phrase:?}"
        );
    }
}

/// **A phrase naming a day moves the grid and picks nothing** — the panel stays
/// open, the box is emptied, and the host is told nothing yet.
#[test]
fn a_day_phrase_moves_the_selection_and_picks_nothing() {
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
        let mut g = with_field(&mut engine, false);
        typed(&mut g, &mut engine, phrase);
        assert_eq!(
            named(&mut g, &mut engine, NamedKey::Enter),
            Pick::None,
            "{phrase:?} picked instead of refining"
        );
        let (y, m, d) = expect;
        assert_eq!(g.selected, day(y, m, d), "{phrase:?}");
        assert_eq!(g.phrase(), None, "{phrase:?} left the box emptied");
    }
}

/// **The grid is a small calculator.** Each phrase reads against where the last
/// one landed, which is the whole reason `koru-when`'s sign binds to the
/// *selection* rather than to the clock — and it is only visible once the
/// selection has moved off today.
///
/// Then the pick: the box is empty after a refine, so Enter is the grid's again
/// and a second press names the day that was aimed at. Nobody is stranded.
#[test]
fn phrases_refine_the_selection_and_a_second_enter_picks_the_day() {
    let mut engine = TextEngine::new();
    let mut g = with_field(&mut engine, false);

    // `2w` is unsigned, so it counts from the clock: 2 Aug + 14 days.
    typed(&mut g, &mut engine, "2w");
    assert_eq!(named(&mut g, &mut engine, NamedKey::Enter), Pick::None);
    assert_eq!(g.selected, day(2026, 8, 16));

    // `+3d` is signed, so it counts from *that*, not from the clock.
    typed(&mut g, &mut engine, "+3d");
    assert_eq!(named(&mut g, &mut engine, NamedKey::Enter), Pick::None);
    assert_eq!(
        g.selected,
        day(2026, 8, 19),
        "three days on from the selection, not from 2 August"
    );

    // And backwards, which has no unsigned spelling at all.
    typed(&mut g, &mut engine, "-1w");
    assert_eq!(named(&mut g, &mut engine, NamedKey::Enter), Pick::None);
    assert_eq!(g.selected, day(2026, 8, 12));

    // The box is empty, so Enter belongs to the grid: this is the pick.
    assert_eq!(g.phrase(), None);
    assert_eq!(
        named(&mut g, &mut engine, NamedKey::Enter),
        Pick::Day(day(2026, 8, 12))
    );
}

/// Clicking a day is the other picking gesture, and a refine does not take it
/// away — the grid is live throughout.
#[test]
fn a_refined_selection_can_also_be_picked_by_clicking_a_day() {
    let mut engine = TextEngine::new();
    let mut g = with_field(&mut engine, false);
    typed(&mut g, &mut engine, "+1mo");
    assert_eq!(named(&mut g, &mut engine, NamedKey::Enter), Pick::None);
    assert_eq!(g.selected, day(2026, 9, 2));
    // The grid is showing September now, so a cell click names a September
    // day — which is what makes the refine worth having.
    let (x, y) = cell_centre_in(&g, day(2026, 9, 1), 20);
    assert_eq!(g.click(x, y), Pick::Day(day(2026, 9, 20)));
}

/// **A bare time lands here for free**, because the box *is* the parser — and
/// it is the surface that gains most from it: a month grid cannot say "at five"
/// at all. The ambiguity guard travels with it, so a bare number still picks
/// nothing.
#[test]
fn the_phrase_box_takes_a_bare_time_and_still_refuses_a_bare_number() {
    let mut engine = TextEngine::new();
    for phrase in ["5pm", "9am", "17:30", "9:15pm"] {
        let mut g = with_field(&mut engine, false);
        typed(&mut g, &mut engine, phrase);
        assert_eq!(
            named(&mut g, &mut engine, NamedKey::Enter),
            Pick::Timed(phrase.to_owned()),
            "{phrase:?}"
        );
    }
    for phrase in ["31", "2026", "9"] {
        let mut g = with_field(&mut engine, false);
        typed(&mut g, &mut engine, phrase);
        assert_eq!(
            named(&mut g, &mut engine, NamedKey::Enter),
            Pick::None,
            "{phrase:?} should pick nothing"
        );
    }
}

/// **Junk picks nothing, clears nothing, moves nothing and keeps the text**,
/// and the grid is still usable underneath — the panel stays open.
///
/// The "keeps the text" half is what separates a refusal from a successful
/// refine, which empties the box: emptying it here would look exactly like
/// having understood the phrase.
#[test]
fn an_unparseable_phrase_neither_picks_nor_clears() {
    let mut engine = TextEngine::new();
    let mut g = with_field(&mut engine, true);
    let held = g.selected;
    typed(&mut g, &mut engine, "asdf");
    assert_eq!(named(&mut g, &mut engine, NamedKey::Enter), Pick::None);
    assert_eq!(g.selected, held, "the grid did not move");
    assert_eq!(
        g.phrase().as_deref(),
        Some("asdf"),
        "and the text is still there"
    );
    // Not a clear either, even though this grid offers the row.
    let (x, y) = cell_centre_in(&g, day(2026, 8, 1), 9);
    assert_eq!(g.click(x, y), Pick::Day(day(2026, 8, 9)));
}

/// The keyboard split: Up/Down and the pages are always the grid's; Left/Right
/// and Backspace belong to the field the moment there is something in it, and
/// to the grid (and the Clear row) while it is empty.
#[test]
fn the_field_takes_the_keys_it_needs_and_leaves_the_rest_to_the_grid() {
    let mut engine = TextEngine::new();
    let mut g = with_field(&mut engine, true);

    // Empty: Left steps a day and Backspace clears.
    named(&mut g, &mut engine, NamedKey::ArrowLeft);
    assert_eq!(g.selected, day(2026, 8, 1));
    assert_eq!(named(&mut g, &mut engine, NamedKey::Backspace), Pick::Clear);

    // Typed: the same two keys are the caret's, and the grid is unmoved.
    typed(&mut g, &mut engine, "fri");
    let held = g.selected;
    named(&mut g, &mut engine, NamedKey::ArrowLeft);
    assert_eq!(g.selected, held, "the caret moved, not the month");
    assert_eq!(named(&mut g, &mut engine, NamedKey::Backspace), Pick::None);
    assert_eq!(
        g.phrase().as_deref(),
        Some("fi"),
        "the caret had stepped left, so Backspace took the `r`"
    );

    // Up/Down and the pages never belong to a one-line box.
    named(&mut g, &mut engine, NamedKey::ArrowDown);
    assert_eq!(g.selected, dates::shift_days(held, 7));
    named(&mut g, &mut engine, NamedKey::PageDown);
    assert_eq!(
        g.selected,
        dates::shift_month(dates::shift_days(held, 7), 1)
    );
}

/// **Alt+C is the Clear row's accelerator, and the only keyboard route to it
/// while a phrase is typed** — Delete and Backspace belong to the caret then,
/// which left Clear unreachable from the keyboard for anyone who had started
/// typing. Constructed keys throughout: a `KeyEvent` cannot be built in a test,
/// which is why `key` takes the three pieces.
#[test]
fn alt_c_asks_to_clear_from_the_phrase_field() {
    let mut engine = TextEngine::new();
    let alt_c = |g: &mut DateGrid, engine: &mut TextEngine| {
        g.key(
            engine,
            &Key::Character("c".into()),
            Some("c"),
            ModifiersState::ALT,
        )
    };

    // With something typed, where Delete and Backspace are the caret's.
    let mut g = with_field(&mut engine, true);
    typed(&mut g, &mut engine, "fri");
    assert_eq!(named(&mut g, &mut engine, NamedKey::Backspace), Pick::None);
    assert_eq!(alt_c(&mut g, &mut engine), Pick::Clear);
    assert_eq!(g.phrase().as_deref(), Some("fr"), "and typed no `c`");

    // Windows delivers a Ctrl-style letter as its ASCII control code in `text`
    // often enough that `ctrl_letter` normalises both; Alt+C has to be
    // recognised through the same function or it fires on one platform.
    let mut g = with_field(&mut engine, true);
    assert_eq!(
        g.key(
            &mut engine,
            &Key::Character("\u{3}".into()),
            Some("\u{3}"),
            ModifiersState::ALT
        ),
        Pick::Clear
    );

    // Nothing to clear: swallowed, never typed. A `c` in the box here would be
    // the same key doing two unrelated things.
    let mut g = with_field(&mut engine, false);
    assert_eq!(alt_c(&mut g, &mut engine), Pick::None);
    assert_eq!(g.phrase(), None, "Alt+C is not a character");

    // A plain `c` is still a character, and Ctrl+Alt+C is AltGr's, not ours —
    // neither may clear.
    let mut g = with_field(&mut engine, true);
    typed(&mut g, &mut engine, "c");
    assert_eq!(g.phrase().as_deref(), Some("c"));
    assert_eq!(
        g.key(
            &mut engine,
            &Key::Character("c".into()),
            Some("c"),
            ModifiersState::CONTROL | ModifiersState::ALT
        ),
        Pick::None
    );
    assert_eq!(
        g.key(
            &mut engine,
            &Key::Character("c".into()),
            Some("c"),
            ModifiersState::ALT | ModifiersState::SHIFT
        ),
        Pick::None
    );
}

/// Whitespace is not a phrase: Enter with a blank box is the grid's.
#[test]
fn a_blank_box_leaves_enter_to_the_grid() {
    let mut engine = TextEngine::new();
    let mut g = with_field(&mut engine, false);
    typed(&mut g, &mut engine, "  ");
    assert_eq!(g.phrase(), None);
    assert_eq!(
        named(&mut g, &mut engine, NamedKey::Enter),
        Pick::Day(day(2026, 8, 2))
    );
}
