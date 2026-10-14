// SPDX-License-Identifier: MPL-2.0

use std::fmt;
use std::str::FromStr;

use cron::Schedule;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// The four rule shapes Koru stores in the first recurrence release.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RuleShape {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

/// The minute selection for an hourly recurrence.
///
/// Daily, weekly and monthly rules always use [`Self::At`]. The other two
/// variants are deliberately limited to otherwise-wildcard rules, keeping
/// Koru's advanced language smaller than the dependency's cron grammar.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MinutePattern {
    At(u8),
    Every(u8),
    List(MinuteSet),
}

impl MinutePattern {
    /// The first minute selected within an hour.
    pub const fn first(self) -> u8 {
        match self {
            Self::At(minute) => minute,
            Self::Every(_) => 0,
            Self::List(minutes) => minutes.first(),
        }
    }
}

/// A non-empty set of minutes in ascending order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MinuteSet(u64);

impl MinuteSet {
    const VALID_BITS: u64 = (1_u64 << 60) - 1;

    fn from_bits(bits: u64) -> Result<Self, DialectError> {
        if bits == 0 || bits & !Self::VALID_BITS != 0 {
            Err(DialectError::InvalidMinutePattern)
        } else {
            Ok(Self(bits))
        }
    }

    pub const fn contains(self, minute: u8) -> bool {
        minute < 60 && self.0 & (1_u64 << minute) != 0
    }

    pub fn iter(self) -> impl Iterator<Item = u8> {
        (0..60).filter(move |minute| self.contains(*minute))
    }

    const fn first(self) -> u8 {
        self.0.trailing_zeros() as u8
    }
}

/// One semantic weekday, independent of cron's storage spelling.
///
/// The order is canonical and deliberate: every set iterates Monday through
/// Sunday, whatever order an advanced cron expression used. A surface maps
/// these values to its own localized names; the three-letter cron tokens stay
/// private to this module.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    pub const ALL: [Self; 7] = [
        Self::Monday,
        Self::Tuesday,
        Self::Wednesday,
        Self::Thursday,
        Self::Friday,
        Self::Saturday,
        Self::Sunday,
    ];

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A non-empty set of weekdays in canonical Monday-to-Sunday order.
///
/// A weekly picker cannot represent "no day". Keeping that invariant here
/// means neither the picker nor a formatter can accidentally construct a rule
/// the stored dialect would refuse.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct WeekdaySet(u8);

impl WeekdaySet {
    const VALID_BITS: u8 = 0b0111_1111;

    /// Build a semantic set. Repeated days are naturally idempotent; an empty
    /// iterator is rejected because it is not a weekly recurrence.
    pub fn from_days(days: impl IntoIterator<Item = Weekday>) -> Result<Self, DialectError> {
        let bits = days
            .into_iter()
            .fold(0, |selected, day| selected | day.bit());
        Self::from_bits(bits)
    }

    /// Build from the seven low bits, Monday first. This is useful to a picker
    /// whose selected chips already live as a bit set.
    pub const fn from_bits(bits: u8) -> Result<Self, DialectError> {
        if bits == 0 || bits & !Self::VALID_BITS != 0 {
            Err(DialectError::InvalidWeekday)
        } else {
            Ok(Self(bits))
        }
    }

    /// The picker's compact representation: bit 0 is Monday and bit 6 Sunday.
    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, day: Weekday) -> bool {
        self.0 & day.bit() != 0
    }

    pub fn iter(self) -> impl Iterator<Item = Weekday> {
        Weekday::ALL
            .into_iter()
            .filter(move |day| self.contains(*day))
    }
}

/// The semantic fields shared by a structured picker and the advanced cron
/// field.
///
/// Fields are private and the constructors enforce the same bounds as
/// [`CronExpression::parse`]. Consequently every `CronParts` value maps to one
/// accepted canonical expression. [`Self::picker_compatible`] tells a surface
/// whether all of that value can also be projected through the simple picker.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CronParts {
    minutes: MinutePattern,
    pattern: Pattern,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum Pattern {
    Hourly,
    Daily { hour: u8 },
    Weekly { hour: u8, weekdays: WeekdaySet },
    Monthly { hour: u8, day_of_month: u8 },
}

impl CronParts {
    pub fn hourly(minute: u8) -> Result<Self, DialectError> {
        checked_number(minute, 0, 59, "minute")?;
        Ok(Self {
            minutes: MinutePattern::At(minute),
            pattern: Pattern::Hourly,
        })
    }

    /// Run at every `step`th minute of each hour, starting at minute zero.
    pub fn every_minutes(step: u8) -> Result<Self, DialectError> {
        checked_number(step, 1, 59, "minute step")?;
        Ok(Self {
            minutes: MinutePattern::Every(step),
            pattern: Pattern::Hourly,
        })
    }

    /// Run at each named minute of every hour.
    ///
    /// At least two distinct minutes are required; one minute is the ordinary
    /// [`Self::hourly`] shape. Repeated values are rejected so one semantic
    /// rule always has one canonical spelling.
    pub fn hourly_list(minutes: impl IntoIterator<Item = u8>) -> Result<Self, DialectError> {
        let mut bits = 0_u64;
        let mut count = 0_u8;
        for minute in minutes {
            checked_number(minute, 0, 59, "minute")?;
            let bit = 1_u64 << minute;
            if bits & bit != 0 {
                return Err(DialectError::InvalidMinutePattern);
            }
            bits |= bit;
            count += 1;
        }
        if count < 2 {
            return Err(DialectError::InvalidMinutePattern);
        }
        Ok(Self {
            minutes: MinutePattern::List(MinuteSet::from_bits(bits)?),
            pattern: Pattern::Hourly,
        })
    }

    pub fn daily(hour: u8, minute: u8) -> Result<Self, DialectError> {
        checked_number(minute, 0, 59, "minute")?;
        checked_number(hour, 0, 23, "hour")?;
        Ok(Self {
            minutes: MinutePattern::At(minute),
            pattern: Pattern::Daily { hour },
        })
    }

    pub fn weekly(weekdays: WeekdaySet, hour: u8, minute: u8) -> Result<Self, DialectError> {
        checked_number(minute, 0, 59, "minute")?;
        checked_number(hour, 0, 23, "hour")?;
        Ok(Self {
            minutes: MinutePattern::At(minute),
            pattern: Pattern::Weekly { hour, weekdays },
        })
    }

    pub fn monthly(day_of_month: u8, hour: u8, minute: u8) -> Result<Self, DialectError> {
        checked_number(minute, 0, 59, "minute")?;
        checked_number(hour, 0, 23, "hour")?;
        checked_number(day_of_month, 1, 31, "day-of-month")?;
        Ok(Self {
            minutes: MinutePattern::At(minute),
            pattern: Pattern::Monthly { hour, day_of_month },
        })
    }

    pub const fn shape(self) -> RuleShape {
        match self.pattern {
            Pattern::Hourly => RuleShape::Hourly,
            Pattern::Daily { .. } => RuleShape::Daily,
            Pattern::Weekly { .. } => RuleShape::Weekly,
            Pattern::Monthly { .. } => RuleShape::Monthly,
        }
    }

    /// The first selected minute. For picker-backed rules this is the one
    /// selected minute; advanced interval/list rules use it only as the
    /// picker's hidden reset value.
    pub const fn minute(self) -> u8 {
        self.minutes.first()
    }

    pub const fn minute_pattern(self) -> MinutePattern {
        self.minutes
    }

    pub const fn picker_compatible(self) -> bool {
        matches!(self.minutes, MinutePattern::At(_))
    }

    pub const fn hour(self) -> Option<u8> {
        match self.pattern {
            Pattern::Hourly => None,
            Pattern::Daily { hour }
            | Pattern::Weekly { hour, .. }
            | Pattern::Monthly { hour, .. } => Some(hour),
        }
    }

    pub const fn day_of_month(self) -> Option<u8> {
        match self.pattern {
            Pattern::Monthly { day_of_month, .. } => Some(day_of_month),
            _ => None,
        }
    }

    pub const fn weekdays(self) -> Option<WeekdaySet> {
        match self.pattern {
            Pattern::Weekly { weekdays, .. } => Some(weekdays),
            _ => None,
        }
    }

    fn canonical(self) -> String {
        let minutes = match self.minutes {
            MinutePattern::At(minute) => minute.to_string(),
            MinutePattern::Every(step) => format!("*/{step}"),
            MinutePattern::List(minutes) => minutes
                .iter()
                .map(|minute| minute.to_string())
                .collect::<Vec<_>>()
                .join(","),
        };
        match self.pattern {
            Pattern::Hourly => format!("{minutes} * * * *"),
            Pattern::Daily { hour } => format!("{minutes} {hour} * * *"),
            Pattern::Weekly { hour, weekdays } => {
                let weekdays = weekdays
                    .iter()
                    .map(storage_weekday)
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{minutes} {hour} * * {weekdays}")
            }
            Pattern::Monthly { hour, day_of_month } => {
                format!("{minutes} {hour} {day_of_month} * *")
            }
        }
    }
}

/// A validated expression in Koru's stored five-field cron dialect.
///
/// The canonical spelling is `minute hour day-of-month month day-of-week`.
/// See [`CronExpression::parse`] for the intentionally small language.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CronExpression {
    canonical: String,
    parts: CronParts,
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for CronExpression {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CronExpression".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "A canonical expression in Koru's bounded five-field cron dialect."
        })
    }
}

impl CronExpression {
    /// Parse, validate, and canonicalize Koru's bounded cron dialect.
    ///
    /// Supported shapes are:
    ///
    /// - hourly: `M * * * *`, `*/N * * * *`, or `M1,M2,... * * * *`;
    /// - daily: `M H * * *`;
    /// - weekly: `M H * * WEEKDAYS`, where `WEEKDAYS` is one or more
    ///   three-letter names (`MON`..`SUN`) joined by commas, or one inclusive
    ///   range such as `MON-FRI`;
    /// - monthly: `M H D * *`.
    ///
    /// Literal `M`, `H`, and `D` values are single decimal values in their
    /// valid ranges. An hourly minute may instead be an every-minute step or
    /// a list of two or more distinct minutes.
    /// Seconds, years, month restrictions, numeric weekdays, other lists and
    /// steps, macros, and a simultaneous day-of-month and day-of-week are
    /// rejected rather than delegated to `cron`'s wider language. Minute steps
    /// and lists require every other field to be `*`.
    pub fn parse(input: &str) -> Result<Self, DialectError> {
        let fields: Vec<&str> = input.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(DialectError::FieldCount);
        }

        let minutes = minute_pattern(fields[0])?;
        let hour = optional_number(fields[1], 0, 23, "hour")?;
        let day = optional_number(fields[2], 1, 31, "day-of-month")?;
        if fields[3] != "*" {
            return Err(DialectError::Unsupported(
                "month must be `*` in the first-release dialect",
            ));
        }
        let weekdays = weekday_selector(fields[4])?;

        let parts = match (minutes, hour, day, weekdays) {
            (MinutePattern::Every(step), None, None, None) => CronParts::every_minutes(step)?,
            (MinutePattern::List(minutes), None, None, None) => CronParts {
                minutes: MinutePattern::List(minutes),
                pattern: Pattern::Hourly,
            },
            (MinutePattern::At(minute), None, None, None) => CronParts::hourly(minute)?,
            (MinutePattern::At(minute), Some(hour), None, None) => CronParts::daily(hour, minute)?,
            (MinutePattern::At(minute), Some(hour), None, Some(weekdays)) => {
                CronParts::weekly(weekdays, hour, minute)?
            }
            (MinutePattern::At(minute), Some(hour), Some(day), None) => {
                CronParts::monthly(day, hour, minute)?
            }
            (MinutePattern::Every(_) | MinutePattern::List(_), _, _, _) => {
                return Err(DialectError::Unsupported(
                    "minute steps and lists require wildcard hour, day-of-month, month, and day-of-week",
                ));
            }
            (MinutePattern::At(_), None, _, _) => {
                return Err(DialectError::Unsupported(
                    "day and weekday constraints require one literal hour",
                ));
            }
            (MinutePattern::At(_), _, Some(_), Some(_)) => {
                return Err(DialectError::Unsupported(
                    "day-of-month and day-of-week cannot both be constrained",
                ));
            }
        };

        let canonical = parts.canonical();
        dependency_schedule(&canonical)?;
        Ok(Self { canonical, parts })
    }

    /// Construct from validated semantic fields. Both input directions cross
    /// the same dependency check before an expression can be stored.
    pub fn from_parts(parts: CronParts) -> Result<Self, DialectError> {
        let canonical = parts.canonical();
        dependency_schedule(&canonical)?;
        Ok(Self { canonical, parts })
    }

    pub fn as_str(&self) -> &str {
        &self.canonical
    }

    pub const fn shape(&self) -> RuleShape {
        self.parts.shape()
    }

    /// The complete semantic state represented by this expression.
    pub const fn parts(&self) -> CronParts {
        self.parts
    }

    pub(crate) fn schedule(&self) -> Schedule {
        dependency_schedule(&self.canonical)
            .expect("a CronExpression is constructed only after dependency validation")
    }
}

impl fmt::Display for CronExpression {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CronExpression {
    type Err = DialectError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl TryFrom<CronParts> for CronExpression {
    type Error = DialectError;

    fn try_from(parts: CronParts) -> Result<Self, Self::Error> {
        Self::from_parts(parts)
    }
}

impl Serialize for CronExpression {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CronExpression {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialectError {
    FieldCount,
    InvalidNumber {
        field: &'static str,
    },
    OutOfRange {
        field: &'static str,
        minimum: u8,
        maximum: u8,
    },
    InvalidWeekday,
    InvalidMinutePattern,
    Unsupported(&'static str),
    DependencyRejected,
}

impl fmt::Display for DialectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FieldCount => formatter.write_str("a recurrence cron has exactly five fields"),
            Self::InvalidNumber { field } => write!(formatter, "{field} must be one decimal value"),
            Self::OutOfRange {
                field,
                minimum,
                maximum,
            } => write!(formatter, "{field} must be between {minimum} and {maximum}"),
            Self::InvalidWeekday => formatter.write_str(
                "day-of-week must be `*`, named weekdays joined by commas, or one named range",
            ),
            Self::InvalidMinutePattern => formatter.write_str(
                "minute must be one value, `*/N`, or a comma-separated list of distinct values",
            ),
            Self::Unsupported(reason) => formatter.write_str(reason),
            Self::DependencyRejected => formatter.write_str("the cron explorer rejected this rule"),
        }
    }
}

impl std::error::Error for DialectError {}

fn number(value: &str, minimum: u8, maximum: u8, field: &'static str) -> Result<u8, DialectError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DialectError::InvalidNumber { field });
    }
    let value = value
        .parse::<u8>()
        .map_err(|_| DialectError::InvalidNumber { field })?;
    checked_number(value, minimum, maximum, field)?;
    Ok(value)
}

fn minute_pattern(value: &str) -> Result<MinutePattern, DialectError> {
    // The ordinary five-field cron spelling for every minute is the same
    // semantic value as `*/1`. Keep one typed representation and therefore
    // one stored canonical spelling rather than teaching every consumer two.
    if value == "*" {
        return Ok(MinutePattern::Every(1));
    }
    if let Some(step) = value.strip_prefix("*/") {
        if step.contains('/') || step.contains(',') {
            return Err(DialectError::InvalidMinutePattern);
        }
        return number(step, 1, 59, "minute step").map(MinutePattern::Every);
    }
    if value.contains(',') {
        let values = value.split(',').collect::<Vec<_>>();
        if values.len() < 2 || values.len() > 60 || values.iter().any(|part| part.is_empty()) {
            return Err(DialectError::InvalidMinutePattern);
        }
        let minutes = values
            .into_iter()
            .map(|part| number(part, 0, 59, "minute"))
            .collect::<Result<Vec<_>, _>>()?;
        return CronParts::hourly_list(minutes).map(|parts| parts.minute_pattern());
    }
    number(value, 0, 59, "minute").map(MinutePattern::At)
}

fn checked_number(
    value: u8,
    minimum: u8,
    maximum: u8,
    field: &'static str,
) -> Result<(), DialectError> {
    if (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(DialectError::OutOfRange {
            field,
            minimum,
            maximum,
        })
    }
}

fn optional_number(
    value: &str,
    minimum: u8,
    maximum: u8,
    field: &'static str,
) -> Result<Option<u8>, DialectError> {
    if value == "*" {
        Ok(None)
    } else {
        number(value, minimum, maximum, field).map(Some)
    }
}

fn weekday_selector(value: &str) -> Result<Option<WeekdaySet>, DialectError> {
    if value == "*" {
        return Ok(None);
    }
    if value.contains('/') || value.contains('#') || value.contains('L') {
        return Err(DialectError::InvalidWeekday);
    }

    let mut selected = 0_u8;
    if let Some((start, end)) = value.split_once('-') {
        if value.matches('-').count() != 1 || value.contains(',') {
            return Err(DialectError::InvalidWeekday);
        }
        let start = weekday(start)?;
        let end = weekday(end)?;
        let start = start as u8;
        let end = end as u8;
        if start > end {
            return Err(DialectError::InvalidWeekday);
        }
        for day in Weekday::ALL
            .into_iter()
            .skip(usize::from(start))
            .take(usize::from(end - start + 1))
        {
            selected |= day.bit();
        }
    } else {
        let parts: Vec<&str> = value.split(',').collect();
        if parts.is_empty() || parts.len() > 7 || parts.iter().any(|part| part.is_empty()) {
            return Err(DialectError::InvalidWeekday);
        }
        for part in parts {
            let day = weekday(part)?;
            if selected & day.bit() != 0 {
                return Err(DialectError::InvalidWeekday);
            }
            selected |= day.bit();
        }
    }
    WeekdaySet::from_bits(selected).map(Some)
}

fn weekday(value: &str) -> Result<Weekday, DialectError> {
    match value.to_ascii_uppercase().as_str() {
        "MON" => Ok(Weekday::Monday),
        "TUE" => Ok(Weekday::Tuesday),
        "WED" => Ok(Weekday::Wednesday),
        "THU" => Ok(Weekday::Thursday),
        "FRI" => Ok(Weekday::Friday),
        "SAT" => Ok(Weekday::Saturday),
        "SUN" => Ok(Weekday::Sunday),
        _ => Err(DialectError::InvalidWeekday),
    }
}

fn storage_weekday(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    }
}

fn dependency_schedule(canonical: &str) -> Result<Schedule, DialectError> {
    // The dependency's language is seconds + our five fields + year. It never
    // crosses this module, and only receives a spelling our parser accepted.
    Schedule::from_str(&format!("0 {canonical} *")).map_err(|_| DialectError::DependencyRejected)
}
