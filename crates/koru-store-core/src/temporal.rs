// SPDX-License-Identifier: MPL-2.0

//! Canonical temporal storage shapes shared by Store-owned domains.
//!
//! This module validates and encodes storage values. It deliberately does not
//! resolve a wall clock: timezone databases and device-zone policy live above
//! Store core.

use std::collections::HashMap;

use loro::LoroValue;
use serde::{Deserialize, Serialize};

use crate::errors::{Code, Error, Result};

const MAX_TZ_CHARS: usize = 64;
const WALL_FORMAT_LEN: usize = "2026-08-30T09:00:00".len();

pub(crate) const T_DATE: &str = "date";
pub(crate) const T_DATETIME_FLOATING: &str = "datetime_floating";
pub(crate) const T_DATETIME_ZONED: &str = "datetime_zoned";
pub(crate) const F_TYPE: &str = "t";
pub(crate) const F_VALUE: &str = "v";
pub(crate) const F_MILLIS: &str = "ms";
pub(crate) const F_TZ: &str = "tz";

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct CalendarDate(String);

impl CalendarDate {
    pub fn parse(value: &str) -> Option<Self> {
        is_calendar_date(value).then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CalendarDate {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        Self::parse(&value).ok_or("a date is a calendar date, YYYY-MM-DD")
    }
}

impl From<CalendarDate> for String {
    fn from(value: CalendarDate) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct FloatingDateTime(String);

impl FloatingDateTime {
    pub fn parse(value: &str) -> Option<Self> {
        is_wall_clock(value).then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for FloatingDateTime {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        Self::parse(&value).ok_or("a floating datetime is YYYY-MM-DDTHH:MM:SS with no zone")
    }
}

impl From<FloatingDateTime> for String {
    fn from(value: FloatingDateTime) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ZonedDateTime {
    instant_ms: i64,
    zone: String,
}

impl<'de> Deserialize<'de> for ZonedDateTime {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Stored {
            instant_ms: i64,
            zone: String,
        }

        let stored = Stored::deserialize(deserializer)?;
        Self::new(stored.instant_ms, stored.zone).map_err(serde::de::Error::custom)
    }
}

impl ZonedDateTime {
    pub fn new(instant_ms: i64, zone: impl Into<String>) -> Result<Self> {
        let zone = zone.into();
        if !is_timezone_name(&zone) {
            return Err(Error::new(
                Code::InvalidRequest,
                "a zoned datetime names an IANA zone such as America/New_York",
            ));
        }
        Ok(Self { instant_ms, zone })
    }

    pub fn instant_ms(&self) -> i64 {
        self.instant_ms
    }

    pub fn zone(&self) -> &str {
        &self.zone
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemporalValue {
    Date { date: CalendarDate },
    Floating { wall: FloatingDateTime },
    Zoned { value: ZonedDateTime },
}

pub(crate) fn validate_calendar_date(value: &str) -> Result<()> {
    CalendarDate::parse(value).map(|_| ()).ok_or_else(|| {
        Error::new(
            Code::InvalidRequest,
            "a date value is a calendar date, YYYY-MM-DD",
        )
    })
}

pub(crate) fn validate_wall_clock(value: &str) -> Result<()> {
    FloatingDateTime::parse(value).map(|_| ()).ok_or_else(|| {
        Error::new(
            Code::InvalidRequest,
            "a floating datetime is a wall clock, YYYY-MM-DDTHH:MM:SS, with no zone",
        )
    })
}

pub(crate) fn validate_timezone_name(value: &str) -> Result<()> {
    ZonedDateTime::new(0, value).map(|_| ())
}

pub(crate) fn temporal_to_loro(value: &TemporalValue) -> LoroValue {
    let mut map = HashMap::<String, LoroValue>::new();
    match value {
        TemporalValue::Date { date } => {
            map.insert(F_TYPE.into(), T_DATE.into());
            map.insert(F_VALUE.into(), date.as_str().into());
        }
        TemporalValue::Floating { wall } => {
            map.insert(F_TYPE.into(), T_DATETIME_FLOATING.into());
            map.insert(F_VALUE.into(), wall.as_str().into());
        }
        TemporalValue::Zoned { value } => {
            map.insert(F_TYPE.into(), T_DATETIME_ZONED.into());
            map.insert(F_MILLIS.into(), value.instant_ms().into());
            map.insert(F_TZ.into(), value.zone().into());
        }
    }
    LoroValue::from(map)
}

pub(crate) fn temporal_from_loro(value: &LoroValue) -> Option<TemporalValue> {
    let LoroValue::Map(map) = value else {
        return None;
    };
    let tag = match map.get(F_TYPE) {
        Some(LoroValue::String(value)) => value.as_ref(),
        _ => return None,
    };
    match tag {
        T_DATE => match map.get(F_VALUE) {
            Some(LoroValue::String(value)) => {
                CalendarDate::parse(value).map(|date| TemporalValue::Date { date })
            }
            _ => None,
        },
        T_DATETIME_FLOATING => match map.get(F_VALUE) {
            Some(LoroValue::String(value)) => {
                FloatingDateTime::parse(value).map(|wall| TemporalValue::Floating { wall })
            }
            _ => None,
        },
        T_DATETIME_ZONED => {
            let instant_ms = match map.get(F_MILLIS) {
                Some(LoroValue::I64(value)) => *value,
                _ => return None,
            };
            let zone = match map.get(F_TZ) {
                Some(LoroValue::String(value)) => value.to_string(),
                _ => return None,
            };
            ZonedDateTime::new(instant_ms, zone)
                .ok()
                .map(|value| TemporalValue::Zoned { value })
        }
        _ => None,
    }
}

fn is_calendar_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        bytes[range].iter().try_fold(0u32, |out, byte| {
            byte.is_ascii_digit()
                .then(|| out * 10 + u32::from(*byte - b'0'))
        })
    };
    let (Some(year), Some(month), Some(day)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    if !(1..=12).contains(&month) || day == 0 {
        return false;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        _ => 28,
    };
    day <= days
}

fn is_wall_clock(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != WALL_FORMAT_LEN
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || !is_calendar_date(&value[..10])
    {
        return false;
    }
    let two = |at: usize| -> Option<u32> {
        let (a, b) = (bytes[at], bytes[at + 1]);
        (a.is_ascii_digit() && b.is_ascii_digit())
            .then(|| u32::from(a - b'0') * 10 + u32::from(b - b'0'))
    };
    matches!((two(11), two(14), two(17)), (Some(h), Some(m), Some(s)) if h <= 23 && m <= 59 && s <= 59)
}

fn is_timezone_name(value: &str) -> bool {
    if value.is_empty()
        || value.chars().count() > MAX_TZ_CHARS
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'+' | b'-'))
        || value.starts_with(['+', '-'])
    {
        return false;
    }
    !["UTC", "GMT"].iter().any(|prefix| {
        value.strip_prefix(prefix).is_some_and(|rest| {
            rest.chars()
                .next()
                .is_some_and(|c| c == '+' || c == '-' || c.is_ascii_digit())
        })
    })
}

#[cfg(test)]
mod tests;
