// SPDX-License-Identifier: MPL-2.0

//! Timezone rule mechanisms for Koru's temporal values.
//!
//! `koru-core` owns the three temporal values, canonical zone-name shape, and
//! the [`ZoneRules`] seam. This crate is the layer above it that owns the IANA
//! database and OS zone detection. It knows no document, reminder, Store,
//! setting, process clock, or UI type.
//!
//! Both implementations use Koru's documented DST policy: a fold chooses the
//! earlier instant; a gap moves the skipped wall reading forward by the size
//! of the ordinary one-hour transition. Resolution never guesses an ambiguous
//! abbreviation such as `EST` or `CST`.

use std::str::FromStr;

use chrono::{LocalResult, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use koru_core::time::{
    DateOnly, FloatingDateTime, TimeZoneName, ZoneRules, ZonedDateTime, zone_for_new_shared_value,
};

mod canonical;

/// Why an explicit timezone could not become IANA rules.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ZoneError {
    /// A short alphabetic abbreviation can name several places and is never
    /// guessed. `UTC` and `GMT` are the two unambiguous exceptions.
    AmbiguousAbbreviation,
    /// The value is not an IANA name known by the compiled database.
    UnknownZone,
    /// A canonical Koru wall value was malformed. Callers normally receive
    /// this only when an older or newer storage shape crossed the mechanism
    /// boundary; authored values are validated before persistence.
    InvalidWall,
}

/// The content-free inputs that make a floating wall resolution stable.
///
/// This is deliberately the canonical IANA name plus the compiled database
/// version, rather than the current UTC offset. The target date's rules decide
/// the instant, and a rules database update must make an old native
/// registration stale even when today's offset happens to be unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RulesFingerprint {
    pub canonical_zone: String,
    pub database_version: &'static str,
}

/// A canonical wall clock resolved for a device without importing the Koru
/// temporal model into the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedWall {
    pub instant_ms: i64,
    pub rules: RulesFingerprint,
}

/// Rules for one validated, database-backed IANA zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IanaZoneRules {
    zone: TimeZoneName,
    rules: Tz,
}

impl IanaZoneRules {
    /// Parse and validate a zone against the compiled IANA database.
    ///
    /// IANA Link names are first mapped through the table derived from this
    /// exact-pinned database; the returned [`TimeZoneName`] therefore persists
    /// the canonical target rather than the caller's alias. Matching is
    /// deliberately case-sensitive.
    pub fn parse(name: &str) -> Result<Self, ZoneError> {
        if is_ambiguous_abbreviation(name) {
            return Err(ZoneError::AmbiguousAbbreviation);
        }
        let canonical = canonical::iana_link_target(name).unwrap_or(name);
        let rules = Tz::from_str(canonical).map_err(|_| ZoneError::UnknownZone)?;
        let zone = TimeZoneName::parse(rules.name()).ok_or(ZoneError::UnknownZone)?;
        Ok(Self { zone, rules })
    }

    /// Resolve an already shape-validated Koru name against the database.
    pub fn from_name(name: &TimeZoneName) -> Result<Self, ZoneError> {
        Self::parse(name.as_str())
    }

    /// The database-normalized IANA name returned at the resolver edge.
    pub fn canonical_name(&self) -> &TimeZoneName {
        &self.zone
    }

    /// Resolve the Store's canonical `YYYY-MM-DDTHH:MM:SS` spelling.
    ///
    /// The string boundary is intentional: `koru-store-core` and
    /// `koru-core` keep separate temporal values, while this mechanism crate
    /// is the one shared place that owns timezone rules.
    pub fn resolve_canonical_wall(&self, wall: &str) -> Result<ResolvedWall, ZoneError> {
        let wall = NaiveDateTime::parse_from_str(wall, "%Y-%m-%dT%H:%M:%S")
            .map_err(|_| ZoneError::InvalidWall)?;
        Ok(ResolvedWall {
            instant_ms: self.instant_of(wall),
            rules: self.rules_fingerprint(),
        })
    }

    pub fn rules_fingerprint(&self) -> RulesFingerprint {
        RulesFingerprint {
            canonical_zone: self.zone.as_str().to_owned(),
            database_version: chrono_tz::IANA_TZDB_VERSION,
        }
    }
}

impl ZoneRules for IanaZoneRules {
    fn zone(&self) -> &TimeZoneName {
        &self.zone
    }

    fn instant_of(&self, wall: NaiveDateTime) -> i64 {
        match self.rules.from_local_datetime(&wall) {
            LocalResult::Single(value) => value.timestamp_millis(),
            LocalResult::Ambiguous(earlier, _) => earlier.timestamp_millis(),
            LocalResult::None => self
                .rules
                .from_local_datetime(
                    &wall
                        .checked_add_signed(chrono::TimeDelta::hours(1))
                        .unwrap_or(wall),
                )
                .earliest()
                .map(|value| value.timestamp_millis())
                .unwrap_or_else(|| wall.and_utc().timestamp_millis()),
        }
    }

    fn wall_of(&self, instant_ms: i64) -> NaiveDateTime {
        chrono::DateTime::from_timestamp_millis(instant_ms)
            .map(|utc| utc.with_timezone(&self.rules).naive_local())
            .unwrap_or(chrono::DateTime::<Utc>::UNIX_EPOCH.naive_utc())
    }
}

/// This device's detected IANA rules, with an honest UTC fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemZone {
    rules: IanaZoneRules,
}

impl SystemZone {
    /// Ask the OS for its IANA zone name, then validate it against the same
    /// database arbitrary-zone resolution uses. Detection failure falls back
    /// to UTC, so the instant and persisted name never disagree.
    pub fn detect() -> Self {
        iana_time_zone::get_timezone()
            .ok()
            .and_then(|name| IanaZoneRules::parse(&name).ok())
            .map(Self::from_rules)
            .unwrap_or_else(Self::utc)
    }

    pub fn utc() -> Self {
        Self::from_rules(IanaZoneRules::parse("UTC").expect("UTC is in the IANA database"))
    }

    /// Construct a system-zone mechanism from an injected name. Tests and
    /// device adapters use this instead of changing process-global OS state.
    pub fn from_name(name: &TimeZoneName) -> Result<Self, ZoneError> {
        IanaZoneRules::from_name(name).map(Self::from_rules)
    }

    fn from_rules(rules: IanaZoneRules) -> Self {
        Self { rules }
    }

    /// Resolve a new shared value in the workspace default, or this device
    /// when no default exists. Arbitrary defaults now use their own rules;
    /// the former local-rules/foreign-label mismatch is gone.
    pub fn try_resolve_for_shared(
        &self,
        workspace_default: Option<&TimeZoneName>,
        wall: FloatingDateTime,
    ) -> Result<ZonedDateTime, ZoneError> {
        let chosen = zone_for_new_shared_value(workspace_default, self.zone());
        let rules = IanaZoneRules::from_name(&chosen)?;
        Ok(wall.in_zone(&rules))
    }

    /// Compatibility path for existing `/due` callers. Stored workspace zone
    /// names have already passed Koru validation; if an old syntactically valid
    /// but database-unknown value is encountered, the honest device rules are
    /// used and named rather than attaching the unknown label to a local instant.
    pub fn resolve_for_shared(
        &self,
        workspace_default: Option<&TimeZoneName>,
        wall: FloatingDateTime,
    ) -> ZonedDateTime {
        self.try_resolve_for_shared(workspace_default, wall)
            .unwrap_or_else(|_| wall.in_zone(self))
    }

    pub fn display_wall(&self, zoned: &ZonedDateTime) -> FloatingDateTime {
        zoned.wall_in(self)
    }

    pub fn display_date(&self, zoned: &ZonedDateTime) -> DateOnly {
        zoned.date_in(self)
    }

    pub fn resolve_canonical_wall(&self, wall: &str) -> Result<ResolvedWall, ZoneError> {
        self.rules.resolve_canonical_wall(wall)
    }

    pub fn rules_fingerprint(&self) -> RulesFingerprint {
        self.rules.rules_fingerprint()
    }
}

impl ZoneRules for SystemZone {
    fn zone(&self) -> &TimeZoneName {
        self.rules.zone()
    }

    fn instant_of(&self, wall: NaiveDateTime) -> i64 {
        self.rules.instant_of(wall)
    }

    fn wall_of(&self, instant_ms: i64) -> NaiveDateTime {
        self.rules.wall_of(instant_ms)
    }
}

fn is_ambiguous_abbreviation(name: &str) -> bool {
    !matches!(name, "UTC" | "GMT")
        && !name.contains('/')
        && (2..=6).contains(&name.chars().count())
        && name.chars().all(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests;
