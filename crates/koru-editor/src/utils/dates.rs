// SPDX-License-Identifier: MPL-2.0

//! Date text the editor puts on a chip.
//!
//! **The calendar arithmetic under it is not here any more.** `month_start`,
//! `days_in_month`, `shift_month`, `shift_days`, `first_weekday_col` and the
//! month/weekday names went down to [`koru_ui::dates`] with the month grid
//! that uses them ([ADR-196]) — they are facts about a calendar rather than
//! about a document, and the grid below cannot reach up here for them. They are
//! re-exported under their old path, because the chip labels below share
//! `month_abbr` with the grid's header and one definition is the whole point.
//!
//! What stays is the vocabulary: a *due* date is written differently from a
//! date, and "due" is the editor's word.
//!
//! [ADR-196]: ../../../../docs/decisions/ADR-201-225.md#adr-219-dategrid--the-calendar-is-a-widget-in-koru-ui-what-a-day-means-stays-with-the-editor

use chrono::NaiveDate;

pub use koru_ui::dates::{
    date_label, date_title, days_in_month, first_weekday_col, month_abbr, month_start, shift_days,
    shift_month, weekday_abbr,
};

/// The due chip's date text, against an injected `today` so the rules are
/// plain unit tests (the drawing seam resolves `today` once per pass):
///
/// - today → `Today`
/// - within the next six days → the weekday, `Thu` — near enough that the
///   day name is the useful fact
/// - anything else this year → `Aug 31`
/// - another year → `Aug 31, 2027`
///
/// A past date takes the same month-day form — no overdue styling here,
/// because overdue is a derived state a *view* computes (R0 draws none).
pub fn due_date_label(d: NaiveDate, today: NaiveDate) -> String {
    use chrono::Datelike;
    if d == today {
        return "Today".to_owned();
    }
    let ahead = d.signed_duration_since(today).num_days();
    if (1..=6).contains(&ahead) {
        return weekday_abbr(d).to_owned();
    }
    if d.year() == today.year() {
        date_label(d)
    } else {
        format!("{}, {}", date_label(d), d.year())
    }
}

/// The due chip's text for a timed value: the date by
/// [`due_date_label`]'s rules, then the wall-clock time, 24-hour —
/// `Thu 17:30`, `Aug 31 09:00`.
pub fn due_instant_label(local: chrono::NaiveDateTime, today: NaiveDate) -> String {
    use chrono::Timelike;
    format!(
        "{} {:02}:{:02}",
        due_date_label(local.date(), today),
        local.hour(),
        local.minute()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// The due chip's text rules, pinned: Today, then a weekday for the
    /// coming six days, then month-day, with the year only when it differs.
    #[test]
    fn the_due_chip_label_compresses_by_distance() {
        let today = day(2026, 8, 26); // a Wednesday
        assert_eq!(due_date_label(today, today), "Today");
        assert_eq!(due_date_label(day(2026, 8, 27), today), "Thu");
        assert_eq!(
            due_date_label(day(2026, 9, 1), today),
            "Tue",
            "six days out"
        );
        assert_eq!(
            due_date_label(day(2026, 9, 2), today),
            "Sep 2",
            "a week out is a date again"
        );
        assert_eq!(due_date_label(day(2026, 12, 24), today), "Dec 24");
        assert_eq!(due_date_label(day(2027, 1, 5), today), "Jan 5, 2027");
        // Past dates are plain — overdue is a view's derived state, not ink.
        assert_eq!(due_date_label(day(2026, 8, 12), today), "Aug 12");
        assert_eq!(
            due_date_label(day(2026, 8, 25), today),
            "Aug 25",
            "yesterday is not \"Tue\" — the weekday window looks forward only"
        );
    }

    #[test]
    fn the_timed_label_appends_a_24_hour_clock() {
        let today = day(2026, 8, 26);
        let t = day(2026, 8, 27).and_hms_opt(9, 5, 0).unwrap();
        assert_eq!(due_instant_label(t, today), "Thu 09:05");
        let far = day(2026, 10, 3).and_hms_opt(17, 30, 0).unwrap();
        assert_eq!(due_instant_label(far, today), "Oct 3 17:30");
    }
}
