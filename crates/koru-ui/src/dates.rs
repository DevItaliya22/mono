// SPDX-License-Identifier: MPL-2.0

//! Calendar arithmetic for the month grid.
//!
//! Pure functions over `NaiveDate` — no state, no chrome, no knowledge of the
//! widget that uses them. They were `koru_editor::utils::dates` and came down
//! with [`date_grid`](crate::date_grid), because a month shift and a
//! Monday-first column are facts about a calendar rather than about a document.
//! `koru_editor::utils::dates` re-exports every one of them, so the chip
//! labels that also use `month_abbr` say nothing new — and the two callers
//! still share one definition rather than growing a second.

use chrono::{Datelike, NaiveDate, TimeDelta};

/// The 1st of `d`'s month.
pub fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap()
}

/// Days in `d`'s month.
pub fn days_in_month(d: NaiveDate) -> u32 {
    let (y, m) = (d.year(), d.month());
    let next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    };
    next.unwrap()
        .signed_duration_since(month_start(d))
        .num_days() as u32
}

/// Shift by whole months, clamping the day (Jan 31 + 1 month -> Feb 28).
pub fn shift_month(d: NaiveDate, delta: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + delta;
    let (y, m0) = (total.div_euclid(12), total.rem_euclid(12) as u32);
    let first = NaiveDate::from_ymd_opt(y, m0 + 1, 1).unwrap();
    let day = d.day().min(days_in_month(first));
    first.with_day(day).unwrap()
}

pub fn shift_days(d: NaiveDate, delta: i64) -> NaiveDate {
    d.checked_add_signed(TimeDelta::days(delta)).unwrap_or(d)
}

/// Column of the 1st of `d`'s month in a Monday-first grid.
pub fn first_weekday_col(d: NaiveDate) -> u32 {
    month_start(d).weekday().num_days_from_monday()
}

pub fn month_abbr(m: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    NAMES[(m as usize - 1).min(11)]
}

pub fn date_label(d: NaiveDate) -> String {
    format!("{} {}", month_abbr(d.month()), d.day())
}

/// The grid header's own text: `Aug 2026`.
pub fn date_title(d: NaiveDate) -> String {
    format!("{} {}", month_abbr(d.month()), d.year())
}

/// "Mon" through "Sun".
pub fn weekday_abbr(d: NaiveDate) -> &'static str {
    const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    NAMES[d.weekday().num_days_from_monday() as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Month arithmetic has three ways to be wrong, and all three have bitten
    /// somebody: the short-month clamp, the year boundary going forward, and the
    /// year boundary going back.
    #[test]
    fn shifting_a_month_clamps_the_day_and_crosses_the_year() {
        assert_eq!(
            shift_month(day(2026, 1, 31), 1),
            day(2026, 2, 28),
            "Jan 31 has no counterpart in February"
        );
        assert_eq!(shift_month(day(2026, 12, 5), 1), day(2027, 1, 5));
        assert_eq!(shift_month(day(2026, 1, 5), -1), day(2025, 12, 5));
    }

    #[test]
    fn february_knows_about_leap_years() {
        assert_eq!(days_in_month(day(2024, 2, 1)), 29);
        assert_eq!(days_in_month(day(2026, 2, 1)), 28);
        assert_eq!(days_in_month(day(2026, 12, 1)), 31);
    }

    /// The grid is Monday-first, so a month starting on Sunday is column 6 —
    /// off by one here and every date in the picker lands a day out.
    #[test]
    fn the_grid_starts_on_monday() {
        assert_eq!(
            first_weekday_col(day(2026, 6, 15)),
            0,
            "1 June 2026 is a Monday"
        );
        assert_eq!(
            first_weekday_col(day(2026, 3, 3)),
            6,
            "1 March 2026 is a Sunday"
        );
    }

    #[test]
    fn shifting_days_crosses_months_and_years() {
        assert_eq!(shift_days(day(2026, 1, 31), 1), day(2026, 2, 1));
        assert_eq!(shift_days(day(2026, 1, 1), -1), day(2025, 12, 31));
    }

    #[test]
    fn an_out_of_range_month_does_not_panic() {
        assert_eq!(month_abbr(1), "Jan");
        assert_eq!(month_abbr(12), "Dec");
        assert_eq!(
            month_abbr(99),
            "Dec",
            "clamped rather than indexed off the end"
        );
    }
}
