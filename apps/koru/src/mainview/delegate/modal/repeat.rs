// SPDX-License-Identifier: MPL-2.0

//! Pure recurrence-editor state for the delegation modal.
//!
//! The picker and advanced field are projections of one validated [`CronParts`]
//! value. Advanced-only minute steps/lists leave the four simple picker chips
//! unselected; choosing one of those chips deliberately returns to a picker-
//! representable rule. Invalid advanced text is retained for correction but
//! never replaces the last valid semantic value.

use koru_draw::text::TextEngine;
use koru_i18n::time::{DateTimeParts, LocaleFormat};
use koru_i18n::{Key, fill, t};
use koru_recurrence::{CronExpression, CronParts, RecurrenceRule, RuleShape, Weekday, WeekdaySet};
use koru_ui::TextField;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    RunOnce,
    Repeat,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum TimeBasis {
    Floating,
    Zoned,
}

pub(crate) struct RepeatEditor {
    pub(crate) basis: TimeBasis,
    pub(crate) frequency: RuleShape,
    pub(crate) picker_active: bool,
    pub(crate) weekdays: WeekdaySet,
    pub(crate) time: TextField,
    pub(crate) day: TextField,
    pub(crate) advanced: TextField,
    pub(crate) zone: TextField,
    pub(crate) parts: CronParts,
    pub(crate) advanced_valid: bool,
    locale: LocaleFormat,
}

impl RepeatEditor {
    pub(crate) fn new(engine: &mut TextEngine, locale: LocaleFormat, current_zone: &str) -> Self {
        let parts = CronParts::daily(9, 0).expect("the default daily time is valid");
        let weekdays = WeekdaySet::from_bits(0b0001_1111).expect("weekdays are non-empty");
        let mut editor = Self {
            basis: TimeBasis::Floating,
            frequency: RuleShape::Daily,
            picker_active: true,
            weekdays,
            time: TextField::new(engine, koru_draw::theme::role::ui()),
            day: TextField::new(engine, koru_draw::theme::role::ui()),
            advanced: TextField::new(engine, koru_draw::theme::role::ui()),
            zone: TextField::new(engine, koru_draw::theme::role::ui()),
            parts,
            advanced_valid: true,
            locale,
        };
        editor.zone.set_text(engine, current_zone);
        editor.sync_fields(engine);
        editor
    }

    /// Replace the editor projection with one stored semantic rule. This is
    /// used by schedule edit; the advanced field remains the canonical escape
    /// hatch for valid rules the four picker chips cannot represent.
    pub(crate) fn set_rule(&mut self, engine: &mut TextEngine, rule: &RecurrenceRule) {
        self.parts = rule.cron().parts();
        match rule.zone_name() {
            Some(zone) => {
                self.basis = TimeBasis::Zoned;
                self.zone.set_text(engine, zone);
            }
            None => self.basis = TimeBasis::Floating,
        }
        self.sync_fields(engine);
    }

    pub(crate) fn set_frequency(&mut self, engine: &mut TextEngine, frequency: RuleShape) {
        self.frequency = frequency;
        self.picker_active = true;
        self.parts = self.parts_for_picker().unwrap_or_else(|| match frequency {
            RuleShape::Hourly => CronParts::hourly(0).expect("default hourly rule"),
            RuleShape::Daily => CronParts::daily(9, 0).expect("default daily rule"),
            RuleShape::Weekly => {
                CronParts::weekly(self.weekdays, 9, 0).expect("default weekly rule")
            }
            RuleShape::Monthly => CronParts::monthly(1, 9, 0).expect("default monthly rule"),
        });
        self.sync_fields(engine);
    }

    pub(crate) fn toggle_weekday(&mut self, engine: &mut TextEngine, day: Weekday) {
        let bits = self.weekdays.bits() ^ (1 << day as u8);
        if let Ok(days) = WeekdaySet::from_bits(bits) {
            self.weekdays = days;
            self.accept_picker(engine);
        }
    }

    pub(crate) fn accept_picker(&mut self, engine: &mut TextEngine) {
        if let Some(parts) = self.parts_for_picker() {
            self.parts = parts;
            self.picker_active = true;
            self.advanced_valid = true;
            let cron = CronExpression::from_parts(parts).expect("picker parts are validated");
            self.advanced.set_text(engine, cron.as_str());
        }
    }

    /// Re-evaluate advanced text without canonicalizing the still-authored
    /// value. Used after every keystroke so an invalid partial stays visible.
    pub(crate) fn advanced_changed(&mut self, engine: &mut TextEngine) {
        match CronExpression::parse(&self.advanced.value()) {
            Ok(cron) => {
                self.parts = cron.parts();
                self.advanced_valid = true;
                self.sync_picker_from_parts(engine);
            }
            Err(_) => self.advanced_valid = false,
        }
    }

    pub(crate) fn settle_advanced(&mut self, engine: &mut TextEngine) {
        if self.advanced_valid {
            let cron = CronExpression::from_parts(self.parts).expect("validated advanced parts");
            self.advanced.set_text(engine, cron.as_str());
        }
    }

    pub(crate) fn rule(&self) -> Option<RecurrenceRule> {
        if !self.advanced_valid {
            return None;
        }
        let parts = if self.picker_active {
            self.parts_for_picker()?
        } else {
            self.parts
        };
        let cron = CronExpression::from_parts(parts).ok()?;
        match self.basis {
            TimeBasis::Floating => Some(RecurrenceRule::floating(cron)),
            TimeBasis::Zoned => RecurrenceRule::zoned(cron, &self.zone.value()).ok(),
        }
    }

    pub(crate) fn preview(&self) -> String {
        let base = crate::schedule_words::format_parts(self.parts, &self.locale);
        if self.basis != TimeBasis::Zoned || !self.zone_valid() {
            return base;
        }
        fill(
            t(self.locale.lang(), Key::ScheduleRuleZoned),
            &[&base, &self.zone.value()],
        )
    }

    pub(crate) fn time_valid(&self) -> bool {
        self.frequency == RuleShape::Hourly
            || self.locale.parse_time_input(&self.time.value()).is_some()
    }

    pub(crate) fn day_valid(&self) -> bool {
        self.frequency != RuleShape::Monthly
            || self
                .locale
                .parse_whole_number_input(&self.day.value())
                .is_some_and(|day| (1..=31).contains(&day))
    }

    pub(crate) fn zone_valid(&self) -> bool {
        if self.basis == TimeBasis::Floating {
            return true;
        }
        let Ok(cron) = CronExpression::from_parts(self.parts) else {
            return false;
        };
        RecurrenceRule::zoned(cron, &self.zone.value()).is_ok()
    }

    fn parts_for_picker(&self) -> Option<CronParts> {
        let (hour, minute) = self.locale.parse_time_input(&self.time.value())?;
        let hour = u8::try_from(hour).ok()?;
        let minute = u8::try_from(minute).ok()?;
        match self.frequency {
            RuleShape::Hourly => CronParts::hourly(minute).ok(),
            RuleShape::Daily => CronParts::daily(hour, minute).ok(),
            RuleShape::Weekly => CronParts::weekly(self.weekdays, hour, minute).ok(),
            RuleShape::Monthly => {
                let day =
                    u8::try_from(self.locale.parse_whole_number_input(&self.day.value())?).ok()?;
                CronParts::monthly(day, hour, minute).ok()
            }
        }
    }

    fn sync_fields(&mut self, engine: &mut TextEngine) {
        self.sync_picker_from_parts(engine);
        let cron = CronExpression::from_parts(self.parts).expect("stored parts are valid");
        self.advanced.set_text(engine, cron.as_str());
        self.advanced_valid = true;
    }

    fn sync_picker_from_parts(&mut self, engine: &mut TextEngine) {
        self.frequency = self.parts.shape();
        self.picker_active = self.parts.picker_compatible();
        if let Some(days) = self.parts.weekdays() {
            self.weekdays = days;
        }
        if let Some(day) = self.parts.day_of_month() {
            self.day
                .set_text(engine, &self.locale.whole_number_input(day.into()));
        } else if self.day.raw_text().is_empty() {
            self.day
                .set_text(engine, &self.locale.whole_number_input(1));
        }
        let hour = self.parts.hour().unwrap_or(0);
        self.time.set_text(
            engine,
            &self.locale.time_input(DateTimeParts {
                year: 2000,
                month: 1,
                day: 1,
                hour,
                minute: self.parts.minute(),
                second: 0,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_i18n::time::HourCycle;

    fn editor(tag: &str, clock: HourCycle) -> (RepeatEditor, TextEngine) {
        let mut engine = TextEngine::new();
        let editor = RepeatEditor::new(
            &mut engine,
            LocaleFormat::for_system(Some(tag), Some(clock)),
            "America/New_York",
        );
        (editor, engine)
    }

    #[test]
    fn locale_controls_the_authored_time_field() {
        let (us, _) = editor("en-US", HourCycle::H12);
        let (de, _) = editor("de-DE", HourCycle::H24);
        assert_eq!(us.time.value(), "9:00 AM");
        assert_eq!(de.time.value(), "09:00");
    }

    #[test]
    fn valid_advanced_text_round_trips_canonically_into_the_picker() {
        let (mut editor, mut engine) = editor("en-US", HourCycle::H12);
        editor
            .advanced
            .set_text(&mut engine, "7 21 * * SUN,MON,WED");
        editor.advanced_changed(&mut engine);

        assert!(editor.advanced_valid);
        assert_eq!(editor.frequency, RuleShape::Weekly);
        assert_eq!(editor.weekdays.bits(), 0b0100_0101);
        assert_eq!(editor.time.value(), "9:07 PM");
        let rule = editor.rule().expect("valid advanced rule");
        assert_eq!(rule.cron().as_str(), "7 21 * * Mon,Wed,Sun");
        editor.settle_advanced(&mut engine);
        assert_eq!(editor.advanced.value(), "7 21 * * Mon,Wed,Sun");
    }

    #[test]
    fn invalid_advanced_text_keeps_the_last_picker_value_and_refuses_submit() {
        let (mut editor, mut engine) = editor("en-US", HourCycle::H12);
        let before = editor.parts;
        editor.advanced.set_text(&mut engine, "*/5 9 * * *");
        editor.advanced_changed(&mut engine);
        assert!(!editor.advanced_valid);
        assert_eq!(editor.parts, before);
        assert!(editor.rule().is_none());

        editor.advanced.set_text(&mut engine, "30 8 * * *");
        editor.advanced_changed(&mut engine);
        assert!(editor.advanced_valid);
        assert_eq!(editor.rule().unwrap().cron().as_str(), "30 8 * * *");
    }

    #[test]
    fn advanced_minute_rules_submit_without_pretending_a_picker_chip_represents_them() {
        let (mut editor, mut engine) = editor("en-US", HourCycle::H12);
        for (authored, canonical, preview) in [
            ("* * * * *", "*/1 * * * *", "Every minute"),
            ("*/05 * * * *", "*/5 * * * *", "Every 5 minutes"),
            (
                "45,05,30 * * * *",
                "5,30,45 * * * *",
                "Every hour at minutes 5, 30, and 45",
            ),
        ] {
            editor.advanced.set_text(&mut engine, authored);
            editor.advanced_changed(&mut engine);

            assert!(editor.advanced_valid);
            assert_eq!(editor.frequency, RuleShape::Hourly);
            assert!(!editor.picker_active);
            assert_eq!(editor.preview(), preview);
            assert_eq!(editor.rule().unwrap().cron().as_str(), canonical);
            editor.settle_advanced(&mut engine);
            assert_eq!(editor.advanced.value(), canonical);
        }

        editor.set_frequency(&mut engine, RuleShape::Hourly);
        assert!(editor.picker_active);
        assert_eq!(editor.rule().unwrap().cron().as_str(), "5 * * * *");
    }

    #[test]
    fn floating_and_zoned_rules_share_the_same_validated_parts() {
        let (mut editor, _) = editor("en-US", HourCycle::H12);
        assert!(matches!(
            editor.rule(),
            Some(RecurrenceRule::Floating { .. })
        ));
        editor.basis = TimeBasis::Zoned;
        let rule = editor.rule().expect("the detected IANA zone is valid");
        assert!(matches!(rule, RecurrenceRule::Zoned { .. }));
        assert_eq!(rule.cron().parts(), editor.parts);
    }

    #[test]
    fn fixed_zone_preview_names_the_rule_that_will_be_stored() {
        let (mut repeat, mut engine) = editor("en-US", HourCycle::H12);
        repeat.basis = TimeBasis::Zoned;
        repeat.zone.set_text(&mut engine, "America/Los_Angeles");
        assert_eq!(
            repeat.preview(),
            "Every day at 9:00 AM in the America/Los_Angeles time zone"
        );

        let (mut arabic, mut engine) = editor("ar-EG", HourCycle::H12);
        arabic.basis = TimeBasis::Zoned;
        arabic.zone.set_text(&mut engine, "Africa/Cairo");
        let preview = arabic.preview();
        assert!(preview.contains("\u{2068}Africa/Cairo\u{2069}"));
    }
}
