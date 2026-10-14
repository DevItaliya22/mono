// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn canonical_shapes_accept_and_refuse_the_same_edges() {
    assert!(CalendarDate::parse("2028-02-29").is_some());
    assert!(CalendarDate::parse("2027-02-29").is_none());
    assert!(FloatingDateTime::parse("2026-08-30T09:00:00").is_some());
    assert!(FloatingDateTime::parse("2026-08-30T09:00:00Z").is_none());
    assert!(ZonedDateTime::new(42, "America/New_York").is_ok());
    assert!(ZonedDateTime::new(42, "+02:00").is_err());
    assert!(ZonedDateTime::new(42, "UTC+2").is_err());
}

#[test]
fn temporal_loro_encoding_round_trips_and_unknown_is_absent() {
    let values = [
        TemporalValue::Date {
            date: CalendarDate::parse("2026-08-30").unwrap(),
        },
        TemporalValue::Floating {
            wall: FloatingDateTime::parse("2026-08-30T09:00:00").unwrap(),
        },
        TemporalValue::Zoned {
            value: ZonedDateTime::new(42, "Europe/Zurich").unwrap(),
        },
    ];
    for value in values {
        assert_eq!(temporal_from_loro(&temporal_to_loro(&value)), Some(value));
    }

    let future = LoroValue::from(HashMap::from([
        (F_TYPE.to_owned(), LoroValue::from("future_clock")),
        (F_VALUE.to_owned(), LoroValue::from("whenever")),
    ]));
    assert_eq!(temporal_from_loro(&future), None);
}
