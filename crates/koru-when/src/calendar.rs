// SPDX-License-Identifier: MPL-2.0

//! Calendar arithmetic over `NaiveDate`.
//!
//! Three functions, all total: nothing here panics on a month boundary, a leap
//! year or a year-crossing shift, because a duration phrase can name any of
//! them and the grammar above has no arm for "the calendar refused".
//!
//! It is a copy of the three rules `koru-editor`'s picker uses
//! (`utils/dates.rs`), and deliberately so — this crate links nothing above
//! `chrono`, and a date parser that had to acquire an editor to add a month
//! would be the wrong shape. The shared half is nine lines of arithmetic with
//! a test each.

use chrono::{Datelike, NaiveDate, TimeDelta};

/// Days in `d`'s month — 28, 29, 30 or 31.
pub fn days_in_month(d: NaiveDate) -> u32 {
    let (y, m) = (d.year(), d.month());
    let next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    };
    // Both `unwrap`s are over a first-of-month one month away, which always
    // exists inside chrono's range.
    next.unwrap()
        .signed_duration_since(d.with_day(1).unwrap())
        .num_days() as u32
}

/// Shift by whole months, **clamping the day**: Jan 31 + 1 month is Feb 28
/// (Feb 29 in a leap year), and Mar 31 − 1 month is Feb 28.
///
/// Clamping rather than overflowing is the only answer that keeps `+1mo` in
/// the month it names. Rolling into March would make "a month from the 31st"
/// skip February entirely.
pub fn shift_month(d: NaiveDate, delta: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + delta;
    let (y, m0) = (total.div_euclid(12), total.rem_euclid(12) as u32);
    let Some(first) = NaiveDate::from_ymd_opt(y, m0 + 1, 1) else {
        return d;
    };
    let day = d.day().min(days_in_month(first));
    first.with_day(day).unwrap_or(d)
}

/// Shift by whole days, in either direction.
pub fn shift_days(d: NaiveDate, delta: i64) -> NaiveDate {
    d.checked_add_signed(TimeDelta::days(delta)).unwrap_or(d)
}
