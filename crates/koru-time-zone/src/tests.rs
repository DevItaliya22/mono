// SPDX-License-Identifier: MPL-2.0

use super::*;
use chrono::{Datelike, Timelike};

fn wall(text: &str) -> FloatingDateTime {
    FloatingDateTime::parse(text).unwrap()
}

#[test]
fn explicit_names_are_database_validated_and_abbreviations_are_not_guessed() {
    let new_york = IanaZoneRules::parse("America/New_York").unwrap();
    assert_eq!(new_york.canonical_name().as_str(), "America/New_York");
    let eastern_alias = IanaZoneRules::parse("US/Eastern").unwrap();
    assert_eq!(
        eastern_alias.canonical_name().as_str(),
        "America/New_York",
        "IANA Link names never cross the resolver edge as aliases"
    );
    let calcutta_alias = IanaZoneRules::parse("Asia/Calcutta").unwrap();
    assert_eq!(calcutta_alias.canonical_name().as_str(), "Asia/Kolkata");
    assert_eq!(
        IanaZoneRules::parse("EST"),
        Err(ZoneError::AmbiguousAbbreviation)
    );
    assert_eq!(
        IanaZoneRules::parse("CST"),
        Err(ZoneError::AmbiguousAbbreviation)
    );
    assert_eq!(
        IanaZoneRules::parse("Mars/Olympus"),
        Err(ZoneError::UnknownZone)
    );
    assert!(IanaZoneRules::parse("UTC").is_ok());
}

#[test]
fn iana_rules_keep_the_documented_fold_and_gap_policy() {
    let rules = IanaZoneRules::parse("America/New_York").unwrap();

    let gap = wall("2026-03-08T02:30:00");
    let gap_instant = rules.instant_of(gap.wall());
    let gap_utc = chrono::DateTime::from_timestamp_millis(gap_instant).unwrap();
    assert_eq!((gap_utc.hour(), gap_utc.minute()), (7, 30));

    let fold = wall("2026-11-01T01:30:00");
    let fold_instant = rules.instant_of(fold.wall());
    let fold_utc = chrono::DateTime::from_timestamp_millis(fold_instant).unwrap();
    assert_eq!((fold_utc.hour(), fold_utc.minute()), (5, 30));
}

#[test]
fn arbitrary_workspace_defaults_use_their_own_rules() {
    let device = SystemZone::utc();
    let new_york = TimeZoneName::parse("America/New_York").unwrap();
    let value = device
        .try_resolve_for_shared(Some(&new_york), wall("2026-08-30T17:00:00"))
        .unwrap();
    assert_eq!(value.zone().as_str(), "America/New_York");
    let utc = chrono::DateTime::from_timestamp_millis(value.instant_ms()).unwrap();
    assert_eq!((utc.hour(), utc.minute()), (21, 0));
    assert_eq!((utc.year(), utc.month(), utc.day()), (2026, 8, 30));
}

#[test]
fn system_zone_round_trips_and_display_does_not_rewrite_semantics() {
    let zone = SystemZone::from_name(&TimeZoneName::parse("Europe/Zurich").unwrap()).unwrap();
    let value = zone.resolve_for_shared(None, wall("2026-08-30T17:00:00"));
    assert_eq!(zone.display_wall(&value), wall("2026-08-30T17:00:00"));

    let elsewhere = ZonedDateTime::new(
        value.instant_ms(),
        TimeZoneName::parse("Pacific/Auckland").unwrap(),
    );
    let _ = zone.display_date(&elsewhere);
    assert_eq!(elsewhere.zone().as_str(), "Pacific/Auckland");
}

#[test]
fn canonical_wall_resolution_carries_the_zone_and_compiled_rules_version() {
    let rules = IanaZoneRules::parse("US/Eastern").unwrap();
    let resolved = rules.resolve_canonical_wall("2026-07-01T09:00:00").unwrap();
    assert_eq!(resolved.rules.canonical_zone, "America/New_York");
    assert_eq!(
        resolved.rules.database_version,
        chrono_tz::IANA_TZDB_VERSION
    );

    let utc = chrono::DateTime::from_timestamp_millis(resolved.instant_ms).unwrap();
    assert_eq!((utc.hour(), utc.minute()), (13, 0));
    assert_eq!(
        rules.resolve_canonical_wall("not-a-wall"),
        Err(ZoneError::InvalidWall)
    );
}
