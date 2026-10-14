// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_core::time::{FloatingDateTime, Temporal, TimeZoneName, ZoneRules, ZonedDateTime};

fn wall(text: &str) -> FloatingDateTime {
    FloatingDateTime::parse(text).unwrap()
}

#[test]
fn the_moved_system_zone_preserves_the_editor_contract() {
    for zone in [
        SystemZone::utc(),
        SystemZone::from_name(&TimeZoneName::parse("Europe/Zurich").unwrap()).unwrap(),
        SystemZone::detect(),
    ] {
        let zoned = zone.resolve_for_shared(None, wall("2026-08-30T17:00:00"));
        assert!(!zoned.zone().as_str().is_empty());
        assert_eq!(zoned.zone(), zone.zone());
        assert_eq!(zone.display_wall(&zoned), wall("2026-08-30T17:00:00"));
    }
}

#[test]
fn an_unnameable_device_fallback_is_utc_and_honest() {
    let zone = SystemZone::utc();
    let zoned = zone.resolve_for_shared(None, wall("2026-08-30T17:00:00"));
    assert_eq!(zoned.zone().as_str(), "UTC");
    assert_eq!(zone.display_wall(&zoned), wall("2026-08-30T17:00:00"));
}

#[test]
fn display_never_changes_the_stored_semantic_zone() {
    let elsewhere = ZonedDateTime::new(
        1_790_000_000_000,
        TimeZoneName::parse("Pacific/Auckland").unwrap(),
    );
    let here = SystemZone::detect();
    let _ = here.display_wall(&elsewhere);
    let _ = here.display_date(&elsewhere);
    assert_eq!(elsewhere.zone().as_str(), "Pacific/Auckland");
}

#[test]
fn the_three_types_stay_distinct_after_the_move() {
    let zone = SystemZone::utc();
    let day = koru_core::time::DateOnly::parse("2026-08-30").unwrap();
    assert_eq!(Temporal::Date(day).zone_free_date(), Some(day));
    let floating = wall("2026-08-30T17:00:00");
    assert_eq!(floating.date(), day);
    let zoned = zone.resolve_for_shared(None, floating);
    assert_eq!(Temporal::Zoned(zoned).zone_free_date(), None);
}
