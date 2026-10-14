// SPDX-License-Identifier: MPL-2.0

//! Localized words for one scheduled delegation rule.
//!
//! This adapter sits in the app because it is the first layer that may name
//! both `koru-recurrence` and `koru-i18n`. The string table remains a
//! dependency-free leaf, while recurrence remains presentation-free. Callers
//! supply a typed projected occurrence and, for local floating rules, explicit
//! current-device zone rules. Formatting never reads a clock, Store, network,
//! profile, device registry, or OS timezone.

use chrono::{Datelike, Timelike};
use koru_core::time::ZoneRules;
use koru_i18n::time::{DateTimeParts, LocaleFormat};
use koru_i18n::{Key, Lang, fill, number, t};
use koru_recurrence::{
    CronParts, MinutePattern, Occurrence, RecurrenceRule, RuleShape, Weekday, WeekdaySet,
};
use koru_time_zone::{IanaZoneRules, SystemZone, ZoneError};

/// A validated rule plus the only next-occurrence authority it can have here.
///
/// The fields and representation are private so a remote floating rule cannot
/// carry an exact epoch, and a zoned rule cannot accidentally use the viewer's
/// zone. Sibling schedule surfaces construct one through the three named
/// methods after matching their typed API projection.
pub(crate) struct ScheduleProjection<'a> {
    rule: &'a RecurrenceRule,
    authority: NextAuthority<'a>,
}

enum NextAuthority<'a> {
    LocalFloating {
        current_device_zone: &'a SystemZone,
        next: Option<Occurrence>,
    },
    OwnerLocal,
    Zoned {
        next: Option<Occurrence>,
    },
}

/// A caller selected a projection constructor that disagrees with the rule's
/// typed floating/zoned variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScheduleProjectionError {
    ExpectedFloating,
    ExpectedZoned,
}

impl<'a> ScheduleProjection<'a> {
    /// A floating rule evaluated by this device. The explicit zone is the
    /// device zone used by recurrence evaluation for the supplied occurrence.
    pub(crate) fn local_floating(
        rule: &'a RecurrenceRule,
        current_device_zone: &'a SystemZone,
        next: Option<Occurrence>,
    ) -> Result<Self, ScheduleProjectionError> {
        if !matches!(rule, RecurrenceRule::Floating { .. }) {
            return Err(ScheduleProjectionError::ExpectedFloating);
        }
        Ok(Self {
            rule,
            authority: NextAuthority::LocalFloating {
                current_device_zone,
                next,
            },
        })
    }

    /// A floating rule evaluated by another device. There is intentionally no
    /// occurrence argument: its exact wall-clock reading is owner-local.
    pub(crate) fn owner_local(rule: &'a RecurrenceRule) -> Result<Self, ScheduleProjectionError> {
        if !matches!(rule, RecurrenceRule::Floating { .. }) {
            return Err(ScheduleProjectionError::ExpectedFloating);
        }
        Ok(Self {
            rule,
            authority: NextAuthority::OwnerLocal,
        })
    }

    /// A zoned rule. Its next epoch is always converted through the canonical
    /// IANA zone carried by `rule`, never through a viewer zone.
    pub(crate) fn zoned(
        rule: &'a RecurrenceRule,
        next: Option<Occurrence>,
    ) -> Result<Self, ScheduleProjectionError> {
        if !matches!(rule, RecurrenceRule::Zoned { .. }) {
            return Err(ScheduleProjectionError::ExpectedZoned);
        }
        Ok(Self {
            rule,
            authority: NextAuthority::Zoned { next },
        })
    }
}

/// The two lines a schedule list needs from recurrence presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScheduleWords {
    pub(crate) rule: String,
    /// Absent when the caller has no exact next firing to show. A remote
    /// floating rule instead carries an explicit owner-local explanation.
    pub(crate) next: Option<String>,
}

/// Describe a recurrence and, when knowable, its next firing.
///
/// An exact next value enters only as a minute-aligned [`Occurrence`] epoch.
/// Zoned rules convert it in their own canonical IANA zone. Local floating
/// rules convert it in the explicitly supplied current-device zone. The owner-
/// local projection has no exact-occurrence field to misuse.
pub(crate) fn schedule_words(
    projection: &ScheduleProjection<'_>,
    locale: &LocaleFormat,
) -> Result<ScheduleWords, ZoneError> {
    let base = format_parts(projection.rule.cron().parts(), locale);
    let (rule, next) = match (projection.rule, &projection.authority) {
        (RecurrenceRule::Floating { .. }, NextAuthority::OwnerLocal) => (
            fill(t(locale.lang(), Key::ScheduleRuleOwnerLocal), &[&base]),
            Some(t(locale.lang(), Key::ScheduleNextOwnerLocal).to_owned()),
        ),
        (
            RecurrenceRule::Floating { .. },
            NextAuthority::LocalFloating {
                current_device_zone,
                next,
            },
        ) => (
            base,
            next.map(|next| format_next(locale, next, *current_device_zone)),
        ),
        (RecurrenceRule::Zoned { zone, .. }, NextAuthority::Zoned { next }) => {
            let rules = IanaZoneRules::parse(zone.as_str())?;
            (
                fill(
                    t(locale.lang(), Key::ScheduleRuleZoned),
                    &[&base, zone.as_str()],
                ),
                next.map(|next| format_next(locale, next, &rules)),
            )
        }
        _ => unreachable!("ScheduleProjection constructors preserve rule authority"),
    };
    Ok(ScheduleWords { rule, next })
}

/// Turn validated picker fields into one complete localized sentence.
///
/// This is also the advanced-field preview path: accepted cron text first
/// becomes `CronParts`, so there is no second parser or weekday ordering rule.
pub(crate) fn format_parts(parts: CronParts, locale: &LocaleFormat) -> String {
    let lang = locale.lang();
    match parts.shape() {
        RuleShape::Hourly => match parts.minute_pattern() {
            MinutePattern::At(minute) => fill(
                t(lang, Key::ScheduleRuleHourly),
                &[&number::decimal(lang, u64::from(minute))],
            ),
            MinutePattern::Every(1) => t(lang, Key::ScheduleRuleEveryMinute).to_owned(),
            MinutePattern::Every(step) => fill(
                t(lang, Key::ScheduleRuleEveryMinutes),
                &[&number::decimal(lang, u64::from(step))],
            ),
            MinutePattern::List(minutes) => {
                let minutes = minutes
                    .iter()
                    .map(|minute| number::decimal(lang, u64::from(minute)))
                    .collect();
                fill(
                    t(lang, Key::ScheduleRuleHourlyMinutes),
                    &[&format_list(lang, minutes)],
                )
            }
        },
        RuleShape::Daily => fill(
            t(lang, Key::ScheduleRuleDaily),
            &[&format_time(locale, parts)],
        ),
        RuleShape::Weekly => format_weekly(parts, locale),
        RuleShape::Monthly => fill(
            t(lang, Key::ScheduleRuleMonthly),
            &[
                &number::decimal(
                    lang,
                    u64::from(
                        parts
                            .day_of_month()
                            .expect("monthly parts always carry a day"),
                    ),
                ),
                &format_time(locale, parts),
            ],
        ),
    }
}

fn format_weekly(parts: CronParts, locale: &LocaleFormat) -> String {
    let lang = locale.lang();
    let weekdays = parts
        .weekdays()
        .expect("weekly parts always carry weekdays");
    let time = format_time(locale, parts);
    let template = match weekdays.bits() {
        0b0111_1111 => Key::ScheduleRuleDaily,
        0b0001_1111 => Key::ScheduleRuleWeekdays,
        0b0110_0000 => Key::ScheduleRuleWeekend,
        _ => {
            let days = format_weekday_list(lang, weekdays);
            return fill(t(lang, Key::ScheduleRuleWeekly), &[&days, &time]);
        }
    };
    fill(t(lang, template), &[&time])
}

fn format_time(locale: &LocaleFormat, parts: CronParts) -> String {
    locale.time_input(DateTimeParts {
        // Only hour and minute participate in `time_input`; real calendar
        // values keep this helper honest if that implementation is tightened.
        year: 2000,
        month: 1,
        day: 1,
        hour: parts.hour().expect("non-hourly parts always carry an hour"),
        minute: parts.minute(),
        second: 0,
    })
}

fn format_next(locale: &LocaleFormat, next: Occurrence, zone: &dyn ZoneRules) -> String {
    let wall = zone.wall_of(next.instant_ms());
    let value = locale.date_time(DateTimeParts {
        year: wall.year(),
        month: wall.month() as u8,
        day: wall.day() as u8,
        hour: wall.hour() as u8,
        minute: wall.minute() as u8,
        second: wall.second() as u8,
    });
    fill(t(locale.lang(), Key::ScheduleNextExact), &[&value])
}

fn format_weekday_list(lang: Lang, weekdays: WeekdaySet) -> String {
    let names = weekdays
        .iter()
        .map(|day| t(lang, weekday_key(day)).to_owned())
        .collect::<Vec<_>>();
    format_list(lang, names)
}

fn format_list(lang: Lang, mut names: Vec<String>) -> String {
    match names.len() {
        0 => unreachable!("validated lists are non-empty"),
        1 => names.pop().expect("one name exists"),
        2 => fill(t(lang, Key::ScheduleListPair), &[&names[0], &names[1]]),
        _ => {
            let last = names.pop().expect("at least three names exist");
            let mut joined = names.remove(0);
            for name in names {
                joined = fill(t(lang, Key::ScheduleListMiddle), &[&joined, &name]);
            }
            fill(t(lang, Key::ScheduleListFinal), &[&joined, &last])
        }
    }
}

const fn weekday_key(day: Weekday) -> Key {
    match day {
        Weekday::Monday => Key::ScheduleMonday,
        Weekday::Tuesday => Key::ScheduleTuesday,
        Weekday::Wednesday => Key::ScheduleWednesday,
        Weekday::Thursday => Key::ScheduleThursday,
        Weekday::Friday => Key::ScheduleFriday,
        Weekday::Saturday => Key::ScheduleSaturday,
        Weekday::Sunday => Key::ScheduleSunday,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use koru_core::time::TimeZoneName;
    use koru_i18n::time::HourCycle;
    use koru_recurrence::{CronExpression, Weekday};

    fn locale(tag: &str, clock: HourCycle) -> LocaleFormat {
        LocaleFormat::for_system(Some(tag), Some(clock))
    }

    fn floating(parts: CronParts) -> RecurrenceRule {
        RecurrenceRule::floating(CronExpression::from_parts(parts).unwrap())
    }

    fn zone(name: &str) -> IanaZoneRules {
        IanaZoneRules::parse(name).unwrap()
    }

    fn system_zone(name: &str) -> SystemZone {
        SystemZone::from_name(&TimeZoneName::parse(name).unwrap()).unwrap()
    }

    fn next() -> Occurrence {
        let instant = Utc
            .with_ymd_and_hms(2026, 9, 14, 1, 7, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        Occurrence::from_instant_ms(instant).unwrap()
    }

    #[test]
    fn every_shape_is_a_complete_sentence_in_every_language() {
        let weekdays = WeekdaySet::from_days([Weekday::Monday, Weekday::Wednesday]).unwrap();
        let parts = [
            CronParts::hourly(17).unwrap(),
            CronParts::daily(9, 5).unwrap(),
            CronParts::weekly(weekdays, 9, 5).unwrap(),
            CronParts::monthly(23, 21, 7).unwrap(),
        ];
        for lang in Lang::ALL {
            let locale = locale(lang.tag(), HourCycle::H24);
            let local_zone = system_zone("Europe/Zurich");
            for parts in parts {
                let floating = floating(parts);
                let local =
                    ScheduleProjection::local_floating(&floating, &local_zone, Some(next()))
                        .unwrap();
                let local = schedule_words(&local, &locale).unwrap();
                assert_complete(lang, &local);

                let remote = ScheduleProjection::owner_local(&floating).unwrap();
                let remote = schedule_words(&remote, &locale).unwrap();
                assert_complete(lang, &remote);

                let zoned = RecurrenceRule::zoned(
                    CronExpression::from_parts(parts).unwrap(),
                    "America/New_York",
                )
                .unwrap();
                let projection = ScheduleProjection::zoned(&zoned, Some(next())).unwrap();
                assert_complete(lang, &schedule_words(&projection, &locale).unwrap());
            }
        }
    }

    #[test]
    fn advanced_minute_rules_are_complete_localized_sentences() {
        for lang in Lang::ALL {
            let locale = locale(lang.tag(), HourCycle::H24);
            let utc = system_zone("UTC");
            for (cron, expected_en) in [
                ("*/1 * * * *", "Every minute"),
                ("*/5 * * * *", "Every 5 minutes"),
                ("5,30,45 * * * *", "Every hour at minutes 5, 30, and 45"),
            ] {
                let rule = RecurrenceRule::floating(CronExpression::parse(cron).unwrap());
                let projection =
                    ScheduleProjection::local_floating(&rule, &utc, Some(next())).unwrap();
                let words = schedule_words(&projection, &locale).unwrap();
                assert_complete(lang, &words);
                if lang == Lang::En {
                    assert_eq!(words.rule, expected_en);
                }
                if lang == Lang::Ar && cron != "*/1 * * * *" {
                    assert_balanced_isolates(&words.rule);
                }
            }
        }
    }

    fn assert_complete(lang: Lang, words: &ScheduleWords) {
        assert!(!words.rule.trim().is_empty(), "{}", lang.tag());
        assert!(!words.rule.contains("{}"), "{}: {}", lang.tag(), words.rule);
        let next = words
            .next
            .as_deref()
            .expect("the case supplies a next line");
        assert!(!next.trim().is_empty(), "{}", lang.tag());
        assert!(!next.contains("{}"), "{}: {next}", lang.tag());
    }

    #[test]
    fn every_nonempty_weekday_set_formats_in_every_language() {
        for bits in 1..=0b0111_1111 {
            let weekdays = WeekdaySet::from_bits(bits).unwrap();
            for lang in Lang::ALL {
                let words = format_parts(
                    CronParts::weekly(weekdays, 9, 0).unwrap(),
                    &locale(lang.tag(), HourCycle::H24),
                );
                assert!(!words.trim().is_empty(), "{} set {bits:07b}", lang.tag());
                assert!(
                    !words.contains("{}"),
                    "{} set {bits:07b}: {words}",
                    lang.tag()
                );
            }
        }
    }

    #[test]
    fn weekday_sets_use_canonical_order_and_local_list_grammar() {
        let unordered =
            WeekdaySet::from_days([Weekday::Sunday, Weekday::Wednesday, Weekday::Monday]).unwrap();
        let parts = CronParts::weekly(unordered, 9, 0).unwrap();
        assert_eq!(
            format_parts(parts, &locale("en-US", HourCycle::H12)),
            "Every Monday, Wednesday, and Sunday at 9:00 AM"
        );
        assert_eq!(
            format_parts(parts, &locale("es-ES", HourCycle::H24)),
            "Cada lunes, miércoles y domingo a las 09:00"
        );
        assert_eq!(
            format_parts(parts, &locale("ja-JP", HourCycle::H24)),
            "毎週月曜日、水曜日、日曜日の 09:00"
        );
    }

    #[test]
    fn common_weekday_weekend_and_all_day_sets_have_direct_sentences() {
        let at_nine = |bits| CronParts::weekly(WeekdaySet::from_bits(bits).unwrap(), 9, 0).unwrap();
        let en = locale("en-US", HourCycle::H12);
        assert_eq!(
            format_parts(at_nine(0b0001_1111), &en),
            "Every weekday at 9:00 AM"
        );
        assert_eq!(
            format_parts(at_nine(0b0110_0000), &en),
            "Every weekend day at 9:00 AM"
        );
        assert_eq!(
            format_parts(at_nine(0b0111_1111), &en),
            "Every day at 9:00 AM"
        );
    }

    #[test]
    fn time_and_number_formatting_follow_region_language_and_hour_cycle() {
        let daily = CronParts::daily(21, 7).unwrap();
        assert_eq!(
            format_parts(daily, &locale("en-US", HourCycle::H12)),
            "Every day at 9:07 PM"
        );
        assert_eq!(
            format_parts(daily, &locale("en-GB", HourCycle::H24)),
            "Every day at 21:07"
        );
        assert_eq!(
            format_parts(daily, &locale("ar-EG", HourCycle::H24)),
            "كل يوم في \u{2068}٢١:٠٧\u{2069}"
        );
        assert!(
            format_parts(
                CronParts::hourly(17).unwrap(),
                &locale("ar-EG", HourCycle::H24)
            )
            .contains("١٧")
        );
        assert!(
            format_parts(
                CronParts::monthly(23, 9, 0).unwrap(),
                &locale("ar-EG", HourCycle::H24)
            )
            .contains("٢٣")
        );
        let rule = floating(daily);
        let utc = system_zone("UTC");
        let projection = ScheduleProjection::local_floating(&rule, &utc, Some(next())).unwrap();
        let next = schedule_words(&projection, &locale("ar-EG", HourCycle::H24))
            .unwrap()
            .next
            .unwrap();
        assert!(next.contains("١٤/٠٩/٢٠٢٦ ٠١:٠٧"), "{next}");
    }

    #[test]
    fn projection_constructors_make_rule_authority_mismatches_fail() {
        let floating = floating(CronParts::daily(9, 0).unwrap());
        let zoned = RecurrenceRule::zoned(
            CronExpression::from_parts(CronParts::daily(9, 0).unwrap()).unwrap(),
            "America/New_York",
        )
        .unwrap();
        let utc = system_zone("UTC");
        assert!(matches!(
            ScheduleProjection::local_floating(&zoned, &utc, Some(next())),
            Err(ScheduleProjectionError::ExpectedFloating)
        ));
        assert!(matches!(
            ScheduleProjection::owner_local(&zoned),
            Err(ScheduleProjectionError::ExpectedFloating)
        ));
        assert!(matches!(
            ScheduleProjection::zoned(&floating, Some(next())),
            Err(ScheduleProjectionError::ExpectedZoned)
        ));
    }

    #[test]
    fn owner_local_floating_rule_has_no_exact_occurrence_input_or_output() {
        let locale = locale("en-US", HourCycle::H12);
        let rule = floating(CronParts::daily(9, 0).unwrap());
        // `owner_local` deliberately has no `Occurrence` parameter.
        let projection = ScheduleProjection::owner_local(&rule).unwrap();
        let words = schedule_words(&projection, &locale).unwrap();
        assert_eq!(
            words.rule,
            "Every day at 9:00 AM in the owning device’s local time"
        );
        assert_eq!(
            words.next.as_deref(),
            Some("Next: determined in the owning device’s local time")
        );
    }

    #[test]
    fn zoned_exact_next_uses_the_rule_zone_not_the_viewer_zone() {
        let cron = CronExpression::from_parts(CronParts::daily(9, 0).unwrap()).unwrap();
        let rule = RecurrenceRule::zoned(cron, "America/New_York").unwrap();
        let viewer_zone = zone("Asia/Tokyo");
        assert_eq!(viewer_zone.wall_of(next().instant_ms()).hour(), 10);

        let projection = ScheduleProjection::zoned(&rule, Some(next())).unwrap();
        let words = schedule_words(&projection, &locale("en-US", HourCycle::H12)).unwrap();
        assert_eq!(
            words.rule,
            "Every day at 9:00 AM in the America/New_York time zone"
        );
        assert_eq!(words.next.as_deref(), Some("Next: 09/13/2026 9:07 PM"));
        assert!(!words.next.unwrap().contains("10:07 AM"));
    }

    #[test]
    fn local_floating_exact_next_uses_the_explicit_current_device_zone() {
        let rule = floating(CronParts::daily(9, 0).unwrap());
        let tokyo = system_zone("Asia/Tokyo");
        let projection = ScheduleProjection::local_floating(&rule, &tokyo, Some(next())).unwrap();
        let words = schedule_words(&projection, &locale("en-US", HourCycle::H12)).unwrap();
        assert_eq!(words.rule, "Every day at 9:00 AM");
        assert_eq!(words.next.as_deref(), Some("Next: 09/14/2026 10:07 AM"));
    }

    #[test]
    fn arabic_bounds_dynamic_values_with_unicode_isolates() {
        let weekdays =
            WeekdaySet::from_days([Weekday::Sunday, Weekday::Wednesday, Weekday::Monday]).unwrap();
        let rule_words = format_parts(
            CronParts::weekly(weekdays, 21, 7).unwrap(),
            &locale("ar-EG", HourCycle::H24),
        );
        let monday = rule_words.find("الاثنين").unwrap();
        let wednesday = rule_words.find("الأربعاء").unwrap();
        let sunday = rule_words.find("الأحد").unwrap();
        assert!(monday < wednesday && wednesday < sunday, "{rule_words}");
        assert!(rule_words.contains("\u{2068}الاثنين\u{2069}"));
        assert!(rule_words.contains("\u{2068}٢١:٠٧\u{2069}"));
        assert_balanced_isolates(&rule_words);

        let cron = CronExpression::from_parts(CronParts::daily(9, 0).unwrap()).unwrap();
        let zoned = RecurrenceRule::zoned(cron, "America/New_York").unwrap();
        let projection = ScheduleProjection::zoned(&zoned, Some(next())).unwrap();
        let words = schedule_words(&projection, &locale("ar-EG", HourCycle::H24)).unwrap();
        assert!(
            words.rule.contains("\u{2068}America/New_York\u{2069}"),
            "{}",
            words.rule
        );
        assert!(
            words
                .next
                .as_deref()
                .unwrap()
                .contains("\u{2068}١٣/٠٩/٢٠٢٦ ٢١:٠٧\u{2069}"),
            "{:?}",
            words.next
        );
        assert_balanced_isolates(&words.rule);
        assert_balanced_isolates(words.next.as_deref().unwrap());

        // The string contract establishes bounded bidi runs; it does not
        // claim that the later schedule surface mirrors its surrounding UI.
        assert!(
            !words
                .rule
                .contains(['\u{202a}', '\u{202b}', '\u{202d}', '\u{202e}'])
        );
    }

    #[test]
    fn every_arabic_schedule_placeholder_is_an_isolated_run() {
        let templates = [
            Key::ScheduleRuleHourly,
            Key::ScheduleRuleEveryMinutes,
            Key::ScheduleRuleHourlyMinutes,
            Key::ScheduleRuleDaily,
            Key::ScheduleRuleWeekdays,
            Key::ScheduleRuleWeekend,
            Key::ScheduleRuleWeekly,
            Key::ScheduleRuleMonthly,
            Key::ScheduleRuleOwnerLocal,
            Key::ScheduleRuleZoned,
            Key::ScheduleNextExact,
            Key::ScheduleListPair,
            Key::ScheduleListMiddle,
            Key::ScheduleListFinal,
        ];
        for key in templates {
            let template = t(Lang::Ar, key);
            assert_eq!(
                template.matches("{}").count(),
                template.matches("\u{2068}{}\u{2069}").count(),
                "{key:?}: {template:?}"
            );
        }
    }

    fn assert_balanced_isolates(value: &str) {
        let mut depth = 0;
        for ch in value.chars() {
            match ch {
                '\u{2068}' => depth += 1,
                '\u{2069}' => {
                    assert!(depth > 0, "unmatched PDI in {value:?}");
                    depth -= 1;
                }
                _ => {}
            }
        }
        assert_eq!(depth, 0, "unclosed FSI in {value:?}");
        assert!(value.contains('\u{2068}') && value.contains('\u{2069}'));
    }
}
