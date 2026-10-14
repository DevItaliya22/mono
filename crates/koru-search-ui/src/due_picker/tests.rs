// SPDX-License-Identifier: MPL-2.0

//! The picker's own contract: what the three answers become, when the step is
//! skipped, and that the panel stops asking for frames.
//!
//! What the *query box* then does with the text is proved beside the surface,
//! in `lib.rs`'s suite — the same split the editor's date picker and the grid
//! below it already use.

use super::*;
use chrono::{Datelike, NaiveDate};

fn clock() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 9, 3)
        .unwrap()
        .and_hms_opt(10, 15, 0)
        .unwrap()
}

/// A picker on `koru.due`, opened over the empty value of `/due `.
fn picker(engine: &mut TextEngine, qualifier: Option<Side>) -> DueFilterPicker {
    DueFilterPicker::open(engine, 0, "due", 0..5, qualifier, clock())
}

fn named(p: &mut DueFilterPicker, engine: &mut TextEngine, k: NamedKey, now: Instant) -> Picked {
    p.key(engine, &Key::Named(k), None, ModifiersState::empty(), now)
}

fn typed(p: &mut DueFilterPicker, engine: &mut TextEngine, s: &str, now: Instant) {
    for ch in s.chars() {
        let text = ch.to_string();
        p.key(
            engine,
            &Key::Character(text.clone().into()),
            Some(&text),
            ModifiersState::empty(),
            now,
        );
    }
}

fn committed(outcome: Picked) -> String {
    match outcome {
        Picked::Commit(text) => text,
        Picked::Consumed => panic!("expected a commit, the panel stayed open"),
        Picked::Released => panic!("expected a commit, the key was handed back"),
    }
}

/// **The three rows are the three operators**, and each is spelled with the
/// wire's own token — so what is written into the box parses back as the filter
/// it came from.
#[test]
fn the_three_answers_are_the_three_operators() {
    let mut engine = TextEngine::new();
    let mut p = picker(&mut engine, None);
    p.grid.selected = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
    let now = Instant::now();
    assert!(
        matches!(
            named(&mut p, &mut engine, NamedKey::Enter, now),
            Picked::Consumed
        ),
        "with no side named the step asks rather than committing"
    );

    assert_eq!(
        p.commit_text(Side::Before).unwrap(),
        "/due:before 2026-09-10"
    );
    assert_eq!(p.commit_text(Side::After).unwrap(), "/due:after 2026-09-10");
    assert_eq!(
        p.commit_text(Side::Within).unwrap(),
        "/due:between 2026-09-10..2026-09-11",
        "within 24 hours is a half-open pair one day apart"
    );
}

/// The arrows move the step and Enter takes the row they landed on.
#[test]
fn the_arrows_choose_which_side() {
    let mut engine = TextEngine::new();
    let mut p = picker(&mut engine, None);
    p.grid.selected = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
    let now = Instant::now();
    named(&mut p, &mut engine, NamedKey::Enter, now);
    assert_eq!(p.step, 0);

    named(&mut p, &mut engine, NamedKey::ArrowDown, now);
    assert_eq!(p.step, 1);
    assert_eq!(
        committed(named(&mut p, &mut engine, NamedKey::Enter, now)),
        "/due:after 2026-09-10"
    );

    // And it wraps, the way every other list in this surface does.
    let mut p = picker(&mut engine, None);
    p.grid.selected = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
    named(&mut p, &mut engine, NamedKey::Enter, now);
    named(&mut p, &mut engine, NamedKey::ArrowUp, now);
    assert_eq!(p.step, SIDES.len() - 1);
}

/// **`/due:after` never sees the step.** The side is already named, so a chosen
/// day commits on the spot rather than asking a question that has an answer.
#[test]
fn a_named_qualifier_skips_the_step() {
    let mut engine = TextEngine::new();
    let now = Instant::now();
    for (side, expected) in [
        (Side::Before, "/due:before 2026-09-10"),
        (Side::After, "/due:after 2026-09-10"),
        (Side::Within, "/due:between 2026-09-10..2026-09-11"),
    ] {
        let mut p = picker(&mut engine, Some(side));
        p.grid.selected = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        assert_eq!(
            committed(named(&mut p, &mut engine, NamedKey::Enter, now)),
            expected
        );
        assert!(
            !p.animating(now),
            "and nothing slid, because nothing was asked"
        );
    }
}

/// A phrase carrying a clock keeps it: the grid has no cell for an hour, but a
/// search bound has room for one, so refining to the day would throw away the
/// thing that was just typed.
#[test]
fn a_timed_phrase_keeps_its_clock() {
    let mut engine = TextEngine::new();
    let mut p = picker(&mut engine, Some(Side::Before));
    let now = Instant::now();
    typed(&mut p, &mut engine, "sept 10 17:30", now);
    assert_eq!(
        committed(named(&mut p, &mut engine, NamedKey::Enter, now)),
        "/due:before 2026-09-10T17:30"
    );
}

/// A phrase naming a **day** refines the grid and does not commit — the widget's
/// own rule, still true through this host.
#[test]
fn a_day_phrase_refines_rather_than_committing() {
    let mut engine = TextEngine::new();
    let mut p = picker(&mut engine, Some(Side::After));
    let now = Instant::now();
    typed(&mut p, &mut engine, "sept 10", now);
    assert!(matches!(
        named(&mut p, &mut engine, NamedKey::Enter, now),
        Picked::Consumed
    ));
    assert_eq!(
        p.grid.selected,
        NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()
    );
    assert_eq!(
        committed(named(&mut p, &mut engine, NamedKey::Enter, now)),
        "/due:after 2026-09-10",
        "the second Enter is the pick"
    );
}

/// **The slide lands.** `animating` is the one exception to drawing only on
/// change (ADR-26), so it has to return to `false` — a panel that never stops
/// asking for frames burns a core for as long as it is open.
#[test]
fn the_slide_stops_asking_for_frames() {
    let mut engine = TextEngine::new();
    let mut p = picker(&mut engine, None);
    let start = Instant::now();
    assert!(!p.animating(start), "nothing moves before a date is named");
    named(&mut p, &mut engine, NamedKey::Enter, start);
    assert!(p.animating(start));
    assert_eq!(p.slide(start), 0.0);
    assert_eq!(p.slide(start + SLIDE), 1.0);
    assert!(!p.animating(start + SLIDE));
    assert!(!p.animating(start + SLIDE * 4));
}

/// Backspace in an empty box is handed back to the query, because backspacing
/// out of `/due ` is editing the query rather than driving a calendar. The grid
/// alone would swallow it: it reads Backspace as Clear, and there is no Clear
/// row here.
#[test]
fn backspace_in_an_empty_box_is_handed_back() {
    let mut engine = TextEngine::new();
    let mut p = picker(&mut engine, None);
    let now = Instant::now();
    assert!(matches!(
        named(&mut p, &mut engine, NamedKey::Backspace, now),
        Picked::Released
    ));

    // With a phrase typed it belongs to the caret, as it does in every other
    // field in the app.
    let mut p = picker(&mut engine, None);
    typed(&mut p, &mut engine, "sep", now);
    assert!(matches!(
        named(&mut p, &mut engine, NamedKey::Backspace, now),
        Picked::Consumed
    ));
}

/// A click on a day cell names it, and a click on a step row answers.
#[test]
fn a_click_names_a_day_and_then_a_side() {
    let mut engine = TextEngine::new();
    let mut p = picker(&mut engine, None);
    let now = Instant::now();
    let cell = koru_ui::date_grid::cell_rect(p.grid.selected, 10);
    let outcome = p.click(
        &mut engine,
        cell.x + cell.w * 0.5,
        cell.y + cell.h * 0.5 + p.grid.band(),
        now,
    );
    assert!(matches!(outcome, Picked::Consumed));
    assert_eq!(
        p.commit_text(Side::Before).unwrap(),
        format!(
            "/due:before {}",
            p.grid.selected.with_day(10).unwrap().format("%Y-%m-%d")
        )
    );

    // The second row of the step is After.
    let y = theme::PANEL_PAD + HEADING_H + ROW_H * 1.5;
    assert!(committed(p.click(&mut engine, 40.0, y, now)).starts_with("/due:after "));
}
