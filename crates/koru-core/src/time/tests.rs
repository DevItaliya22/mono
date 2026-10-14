// SPDX-License-Identifier: MPL-2.0

use super::*;
use chrono::TimeZone;

/// A fixed-offset stand-in for a real zone, so the conversion rules can be
/// tested without a timezone database. Real zones arrive through
/// `koru-editor`'s `SystemZone`, which uses the machine's.
struct FixedZone {
    name: TimeZoneName,
    offset_seconds: i32,
}

impl FixedZone {
    fn new(name: &str, offset_seconds: i32) -> Self {
        Self {
            name: TimeZoneName::parse(name).expect("a test zone name is valid"),
            offset_seconds,
        }
    }
}

impl ZoneRules for FixedZone {
    fn zone(&self) -> &TimeZoneName {
        &self.name
    }

    fn instant_of(&self, wall: NaiveDateTime) -> i64 {
        let offset = chrono::FixedOffset::east_opt(self.offset_seconds).unwrap();
        offset
            .from_local_datetime(&wall)
            .single()
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_default()
    }

    fn wall_of(&self, instant_ms: i64) -> NaiveDateTime {
        let offset = chrono::FixedOffset::east_opt(self.offset_seconds).unwrap();
        chrono::DateTime::from_timestamp_millis(instant_ms)
            .map(|utc| utc.with_timezone(&offset).naive_local())
            .unwrap_or_default()
    }
}

fn day(y: i32, m: u32, d: u32) -> DateOnly {
    DateOnly::new(NaiveDate::from_ymd_opt(y, m, d).unwrap())
}

/// The three tokens, and the one that is gone. `datetime` used to mean
/// "instant plus optional zone" — the ambiguous shape this whole module
/// exists to retire — so it must not decode to anything at all.
#[test]
fn the_storage_tokens_are_three_and_the_ambiguous_one_is_retired() {
    assert_eq!(TemporalKind::Date.token(), "date");
    assert_eq!(TemporalKind::Floating.token(), "datetime_floating");
    assert_eq!(TemporalKind::Zoned.token(), "datetime_zoned");
    for kind in TemporalKind::ALL {
        assert_eq!(TemporalKind::parse(kind.token()), Some(kind));
    }
    assert_eq!(
        TemporalKind::parse("datetime"),
        None,
        "the R0 token meant instant + optional zone and has no meaning here"
    );
    assert_eq!(TemporalKind::parse("timestamp"), None);
}

/// A calendar date's storage spelling is zero-padded, because a projection
/// compares those strings and `2026-9-1` would sort before every August.
#[test]
fn a_date_round_trips_zero_padded() {
    let d = day(2026, 9, 1);
    assert_eq!(d.to_storage(), "2026-09-01");
    assert_eq!(DateOnly::parse("2026-09-01"), Some(d));
    assert_eq!(DateOnly::parse("2026-9-1"), None, "unpadded is refused");
    assert_eq!(DateOnly::parse("2026-13-01"), None);
    assert_eq!(DateOnly::parse(""), None);
}

#[test]
fn a_floating_value_round_trips_without_a_zone_marker() {
    let f = FloatingDateTime::new(
        NaiveDate::from_ymd_opt(2026, 8, 30)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap(),
    );
    assert_eq!(f.to_storage(), "2026-08-30T09:00:00");
    assert_eq!(FloatingDateTime::parse("2026-08-30T09:00:00"), Some(f));
    assert_eq!(
        FloatingDateTime::parse("2026-08-30T09:00:00Z"),
        None,
        "a Z is a zone claim, and a floating value makes none"
    );
    assert_eq!(FloatingDateTime::parse("2026-08-30T09:00:00+02:00"), None);
}

/// An offset is not a timezone: DST changes what a future offset will be, so
/// a name that is really an offset must never reach storage.
#[test]
fn a_zone_name_is_a_name_and_never_an_offset() {
    for good in [
        "UTC",
        "GMT",
        "America/New_York",
        "Europe/Zurich",
        "Etc/GMT+5",
    ] {
        assert!(
            TimeZoneName::parse(good).is_some(),
            "{good} is a real zone name"
        );
    }
    for bad in ["", "+02:00", "-0500", "UTC+2", "GMT-3", "UTC 2", "Zürich"] {
        assert!(
            TimeZoneName::parse(bad).is_none(),
            "{bad:?} is not a zone name"
        );
    }
    let long: String = std::iter::repeat_n('a', MAX_ZONE_CHARS + 1).collect();
    assert!(TimeZoneName::parse(&long).is_none());
}

/// The conversion story, end to end — and every step of it names its zone.
#[test]
fn converting_between_the_forms_always_takes_a_zone() {
    let zurich = FixedZone::new("Europe/Zurich", 2 * 3600);
    let tokyo = FixedZone::new("Asia/Tokyo", 9 * 3600);

    // A day plus a wall-clock time is still floating: naming an hour does not
    // name a place.
    let floating = day(2026, 8, 30).at(NaiveTime::from_hms_opt(9, 0, 0).unwrap());
    assert_eq!(floating.to_storage(), "2026-08-30T09:00:00");

    // Resolving it in a zone is what makes it an instant.
    let zoned = floating.in_zone(&zurich);
    assert_eq!(zoned.zone().as_str(), "Europe/Zurich");

    // Read from Tokyo, the same instant is a different wall clock — and that
    // is display, not meaning: the zone the value carries is unchanged.
    assert_eq!(zoned.wall_in(&zurich), floating);
    assert_eq!(
        zoned.wall_in(&tokyo).to_storage(),
        "2026-08-30T16:00:00",
        "seven hours ahead"
    );
    assert_eq!(
        zoned.zone().as_str(),
        "Europe/Zurich",
        "meaning did not move"
    );
}

/// The date-only-through-UTC bug, pinned: a deadline late in a Zurich evening
/// is a *different day* in UTC, so `date_in` refuses to have a default.
#[test]
fn a_zoned_values_day_depends_on_where_you_read_it() {
    let zurich = FixedZone::new("Europe/Zurich", 2 * 3600);
    let utc = FixedZone::new("UTC", 0);
    let zoned = day(2026, 8, 30)
        .at(NaiveTime::from_hms_opt(23, 30, 0).unwrap())
        .in_zone(&zurich);
    assert_eq!(zoned.date_in(&zurich), day(2026, 8, 30));
    assert_eq!(
        zoned.date_in(&utc),
        day(2026, 8, 30),
        "21:30 UTC, still the 30th"
    );
    let late = day(2026, 8, 31)
        .at(NaiveTime::from_hms_opt(1, 30, 0).unwrap())
        .in_zone(&zurich);
    assert_eq!(late.date_in(&zurich), day(2026, 8, 31));
    assert_eq!(
        late.date_in(&utc),
        day(2026, 8, 30),
        "23:30 UTC the night before — this is the bug the types prevent"
    );
}

/// `zone_free_date` is the type-honest replacement for "a timed value is just
/// `None`": two of the three can answer without a zone, and the third says so.
#[test]
fn only_a_zoned_value_refuses_to_name_a_day_on_its_own() {
    let zurich = FixedZone::new("Europe/Zurich", 2 * 3600);
    let d = day(2026, 8, 30);
    assert_eq!(Temporal::Date(d).zone_free_date(), Some(d));
    assert_eq!(
        Temporal::Floating(d.at(NaiveTime::from_hms_opt(23, 59, 0).unwrap())).zone_free_date(),
        Some(d),
        "dropping a time is not a conversion between places"
    );
    assert_eq!(
        Temporal::Zoned(
            d.at(NaiveTime::from_hms_opt(9, 0, 0).unwrap())
                .in_zone(&zurich)
        )
        .zone_free_date(),
        None
    );
}

#[test]
fn a_temporal_value_reports_its_own_token() {
    let zurich = FixedZone::new("Europe/Zurich", 2 * 3600);
    let d = day(2026, 8, 30);
    assert_eq!(Temporal::Date(d).token(), "date");
    assert_eq!(
        Temporal::Floating(d.at(NaiveTime::MIN)).token(),
        "datetime_floating"
    );
    assert_eq!(
        Temporal::Zoned(d.at(NaiveTime::MIN).in_zone(&zurich)).token(),
        "datetime_zoned"
    );
}

/// Rule 6: the workspace default decides the zone a *new* shared value is
/// resolved in, and nothing else. It cannot reach a value already stored,
/// because a stored value carries its own zone and this function is never on
/// a read path.
#[test]
fn the_workspace_default_answers_for_new_values_only() {
    let device = TimeZoneName::parse("Europe/Zurich").unwrap();
    let workspace = TimeZoneName::parse("America/New_York").unwrap();
    assert_eq!(zone_for_new_shared_value(None, &device), device);
    assert_eq!(
        zone_for_new_shared_value(Some(&workspace), &device),
        workspace
    );
    // A value already written keeps its own zone whatever either default says.
    let already = ZonedDateTime::new(0, TimeZoneName::parse("Asia/Tokyo").unwrap());
    assert_eq!(already.zone().as_str(), "Asia/Tokyo");
}
