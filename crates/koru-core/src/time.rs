// SPDX-License-Identifier: MPL-2.0

//! The three temporal types, and the only conversions between them.
//!
//! **Koru has three temporal types, not one with a nullable timezone**
//! ([ADR-183], and `docs/architecture/TIME.md` is the reader's version of all of this):
//!
//! | Type | The question it answers | Carries a zone |
//! | --- | --- | --- |
//! | [`DateOnly`] | *which day?* | never, at any layer |
//! | [`FloatingDateTime`] | *what does the clock on the wall say?* | never |
//! | [`ZonedDateTime`] | *which instant, and where was that meant?* | always |
//!
//! The shape this replaces was `{ instant_ms, tz: Option<String> }`, where the
//! *absence* of a zone decided what the value meant. That is the failure this
//! module exists to make impossible: **the type carries the meaning, never a
//! missing field** — a value that could be read two ways cannot be built.
//! `ZonedDateTime`'s fields are private and its zone is a validated
//! [`TimeZoneName`], so there is no way to spell "an instant with no zone";
//! `FloatingDateTime` has no zone field to leave empty; and `DateOnly` holds a
//! `NaiveDate`, which has neither a time nor an offset to lose.
//!
//! ## Nothing converts implicitly
//!
//! Foundation invariant 23. There is no `From`, no `Deref`, no
//! `impl Into` between the three. Every conversion that needs a zone takes
//! one — as a [`ZoneRules`] argument, so the call site names *which* zone and
//! a reviewer can see whether it is the value's own (semantic) or the
//! reader's (display). The two that do **not** need a zone are still named
//! rather than free: [`DateOnly::at`] adds a wall-clock time and stays
//! floating, and [`FloatingDateTime::date`] drops one.
//!
//! `koru-core` deliberately carries **no timezone database**. It can hold a
//! zone's *name* and compare instants; it cannot tell you what 09:00 in
//! `America/New_York` is. That is why [`ZoneRules`] is a trait implemented
//! above — by the layer that has the tz data — rather than a table here.
//!
//! ## The storage tokens are versioned
//!
//! [`TemporalKind::token`] is the stable storage spelling, and it is
//! deliberately a *different set* from what R0 briefly wrote:
//!
//! | Type | Token |
//! | --- | --- |
//! | `DateOnly` | `date` |
//! | `FloatingDateTime` | `datetime_floating` |
//! | `ZonedDateTime` | `datetime_zoned` |
//!
//! The retired token is `datetime`, which meant *instant plus optional zone* —
//! the ambiguous shape. It is not read back: an unknown token reads as
//! **absent** everywhere attributes are decoded, so the ambiguous spelling
//! simply has no meaning in this build, and nothing can revive it by writing
//! one. R0 never shipped, so there is no data under it and there never has to
//! be a migration ([ADR-183]).
//!
//! [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

/// The longest an IANA zone name may be. Real ones are far shorter; this is a
/// sanity bound so a stored name cannot be a payload.
const MAX_ZONE_CHARS: usize = 64;

/// A calendar date. **No timezone, at any layer, ever.**
///
/// `2026-08-30` means August 30 to every collaborator on the planet.
/// Converting one through UTC midnight — the classic date-only bug — makes it
/// August 29 for a third of them, which is why there is no method here that
/// produces an instant. To get one you must first say what time of day you
/// mean ([`at`](Self::at)) and then in which zone ([`FloatingDateTime::in_zone`]),
/// and both of those are decisions somebody has to make on purpose.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DateOnly(NaiveDate);

impl DateOnly {
    pub const fn new(date: NaiveDate) -> Self {
        Self(date)
    }

    pub const fn date(self) -> NaiveDate {
        self.0
    }

    /// This day at a wall-clock time — still **floating**, because naming an
    /// hour does not name a place. The zone comes later, explicitly.
    pub const fn at(self, time: NaiveTime) -> FloatingDateTime {
        FloatingDateTime(self.0.and_time(time))
    }

    /// The canonical storage spelling, `YYYY-MM-DD` — zero-padded, so
    /// lexicographic order *is* date order and a projection can compare the
    /// strings.
    pub fn to_storage(self) -> String {
        self.0.format("%Y-%m-%d").to_string()
    }

    /// Strictly `YYYY-MM-DD`. Strict because the string is also compared
    /// lexicographically: an unpadded `2026-9-1` would parse and then sort
    /// before every August.
    pub fn parse(s: &str) -> Option<Self> {
        if s.len() != 10 {
            return None;
        }
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(Self)
    }
}

impl std::fmt::Display for DateOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_storage())
    }
}

/// A wall-clock date and time with **no zone**: it means the same clock
/// reading wherever it is read.
///
/// "Take the pills at 08:00" is a floating value — it does not move when you
/// fly to Tokyo, and it is not an instant until somebody says where they are.
/// This is the default for **personal** timed values (`koru.plan`), which
/// follow the person rather than a place.
///
/// There is no zone field, so no code path can leave one unset and no reader
/// can infer meaning from its absence.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FloatingDateTime(NaiveDateTime);

impl FloatingDateTime {
    pub const fn new(wall: NaiveDateTime) -> Self {
        Self(wall)
    }

    /// The wall-clock reading. Not an instant, and never comparable to one.
    pub const fn wall(self) -> NaiveDateTime {
        self.0
    }

    /// The calendar day this wall clock falls on. **No zone is involved** —
    /// dropping a time component is not a conversion between places — which is
    /// why this one is free of [`ZoneRules`].
    pub const fn date(self) -> DateOnly {
        DateOnly(self.0.date())
    }

    /// Resolve this wall clock **in a named zone**, producing an instant that
    /// stays put when the reader travels.
    ///
    /// Explicit and named, per invariant 23: the zone is an argument, the
    /// caller chose it, and DST gaps and folds are decided by whatever
    /// implements [`ZoneRules`] — the layer that actually has the rules.
    pub fn in_zone<R: ZoneRules + ?Sized>(self, rules: &R) -> ZonedDateTime {
        ZonedDateTime {
            instant_ms: rules.instant_of(self.0),
            zone: rules.zone().clone(),
        }
    }

    /// The canonical storage spelling, `YYYY-MM-DDTHH:MM:SS` — no offset, no
    /// `Z`, because there is nothing zone-shaped to write. Sorts
    /// chronologically as a string.
    pub fn to_storage(self) -> String {
        self.0.format("%Y-%m-%dT%H:%M:%S").to_string()
    }

    pub fn parse(s: &str) -> Option<Self> {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
            .ok()
            .map(Self)
    }
}

impl std::fmt::Display for FloatingDateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_storage())
    }
}

/// An IANA timezone **name** — `America/New_York`, `Europe/Zurich`, `UTC`.
///
/// A name, never an offset: daylight-saving rules change what a future offset
/// will be, and a `+02:00` written today is a wrong answer next winter. The
/// newtype is what makes [`ZonedDateTime`]'s zone impossible to omit — there
/// is no way to build one from an unvalidated string except [`parse`](Self::parse),
/// which refuses offset shapes.
///
/// This crate carries no timezone *database*, so the check is shape, not
/// existence: a name that looks right but names no zone is caught by the layer
/// that tries to resolve it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TimeZoneName(String);

impl TimeZoneName {
    /// `UTC` — a real IANA name, and the one honest answer when a device
    /// cannot say where it is.
    pub fn utc() -> Self {
        Self("UTC".to_owned())
    }

    /// A plausible IANA name, or `None`.
    ///
    /// Refused: anything empty or over-long; anything starting with `+` or `-`;
    /// and `UTC`/`GMT` followed by a sign or a digit — `UTC+2`, `GMT-3`, which
    /// are offsets wearing a name. Accepted: bare `UTC` and `GMT`, and the
    /// `Etc/GMT+5` family, which *are* zone names whose rules are fixed.
    pub fn parse(name: &str) -> Option<Self> {
        if name.is_empty() || name.chars().count() > MAX_ZONE_CHARS {
            return None;
        }
        let ok = name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'+' | b'-'));
        if !ok {
            return None;
        }
        if name.starts_with('+') || name.starts_with('-') {
            return None;
        }
        for prefix in ["UTC", "GMT"] {
            if let Some(rest) = name.strip_prefix(prefix)
                && rest
                    .chars()
                    .next()
                    .is_some_and(|c| c == '+' || c == '-' || c.is_ascii_digit())
            {
                return None;
            }
        }
        Some(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TimeZoneName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A stable instant **plus the zone it was meant in**.
///
/// The form a **shared** timed value takes (`koru.due`), because collaborators
/// must agree on the same moment: an instant is the same moment for everyone,
/// and the zone name is what lets a reader in another country be told *when
/// that was, where it was set*. Travelling changes only how it is displayed —
/// the deadline itself does not move (invariant 24).
///
/// The fields are private on purpose. There is no constructor that omits the
/// zone, no setter that clears it, and no `Option` anywhere in the type — so
/// "an instant whose zone we forgot" is not a state this program can be in.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ZonedDateTime {
    instant_ms: i64,
    zone: TimeZoneName,
}

impl ZonedDateTime {
    /// The only constructor. A [`TimeZoneName`] is already validated, so this
    /// cannot fail and cannot be handed nothing.
    pub const fn new(instant_ms: i64, zone: TimeZoneName) -> Self {
        Self { instant_ms, zone }
    }

    /// Epoch milliseconds, UTC. **This is the comparable number** — two zoned
    /// values order by it, whatever zones they name.
    pub const fn instant_ms(&self) -> i64 {
        self.instant_ms
    }

    /// The **semantic** zone: where this deadline was meant. Not the zone a
    /// reader sees it in, which is presentation's choice and lives nowhere in
    /// this value.
    pub const fn zone(&self) -> &TimeZoneName {
        &self.zone
    }

    /// The wall clock this instant reads as **in the zone `rules` describes**.
    ///
    /// The argument is the whole point: pass the value's own zone to ask what
    /// it meant, or the reader's to ask what it looks like from here. Nothing
    /// answers "the" wall clock, because there is no such thing.
    pub fn wall_in<R: ZoneRules + ?Sized>(&self, rules: &R) -> FloatingDateTime {
        FloatingDateTime(rules.wall_of(self.instant_ms))
    }

    /// The calendar day this instant falls on **in the zone `rules`
    /// describes**. Deliberately not a bare `date()`: the UTC day is a
    /// different day for a third of the planet, and that is exactly the bug
    /// this module exists to prevent.
    pub fn date_in<R: ZoneRules + ?Sized>(&self, rules: &R) -> DateOnly {
        self.wall_in(rules).date()
    }
}

/// One temporal value, whichever of the three it is.
///
/// A tagged union rather than a common supertype: there is no field they
/// share, no comparison that spans them, and nothing here flattens to a
/// number. Code that must handle all three matches and says what it does for
/// each — which is what makes a missing case a compile error instead of a
/// silent coercion.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Temporal {
    Date(DateOnly),
    Floating(FloatingDateTime),
    Zoned(ZonedDateTime),
}

impl Temporal {
    pub fn kind(&self) -> TemporalKind {
        match self {
            Self::Date(_) => TemporalKind::Date,
            Self::Floating(_) => TemporalKind::Floating,
            Self::Zoned(_) => TemporalKind::Zoned,
        }
    }

    /// The stable storage token — also what a projection files the value's
    /// `kind` under, and what a query filters on.
    pub fn token(&self) -> &'static str {
        self.kind().token()
    }

    /// The calendar day, **when it can be answered without a timezone**:
    /// a date is itself, a floating value drops its time. A zoned value
    /// answers `None` — its day depends on where you read it, so the caller
    /// asks [`ZonedDateTime::date_in`] with a zone in hand.
    ///
    /// This is the type-honest replacement for the old "a timed due is just
    /// `None`" shrug: the two `None`s meant different things and now only one
    /// exists.
    pub fn zone_free_date(&self) -> Option<DateOnly> {
        match self {
            Self::Date(d) => Some(*d),
            Self::Floating(f) => Some(f.date()),
            Self::Zoned(_) => None,
        }
    }
}

/// Which of the three a value is, without the value. The storage vocabulary.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum TemporalKind {
    Date,
    Floating,
    Zoned,
}

impl TemporalKind {
    pub const ALL: [Self; 3] = [Self::Date, Self::Floating, Self::Zoned];

    /// The stable storage token. Never renamed — renaming a storage key
    /// orphans every value already written under it.
    pub const fn token(self) -> &'static str {
        match self {
            Self::Date => "date",
            Self::Floating => "datetime_floating",
            Self::Zoned => "datetime_zoned",
        }
    }

    /// A token back to its kind. `None` for anything else — **including the
    /// retired `datetime`**, whose meaning was "instant plus optional zone"
    /// and is therefore not a thing this build can represent. An unknown token
    /// reads as absent, which is the whole compatibility story.
    pub fn parse(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.token() == token)
    }
}

/// A timezone's rules, supplied by the layer that has them.
///
/// `koru-core` holds names and instants; it cannot tell you that 09:00 on the
/// last Sunday in March does not exist in `Europe/Zurich`. Whoever implements
/// this trait owns two decisions this crate must not fake:
///
/// - **the gap**: a wall clock that DST skipped names no instant. An
///   implementation picks one (the convention here is the instant the clock
///   jumps *to*) rather than failing a person's typing.
/// - **the fold**: a wall clock that DST repeated names two. The convention is
///   the first — the earlier instant — so "01:30" on that morning is the one
///   that arrives sooner.
///
/// Both conventions are written down in `docs/architecture/TIME.md`; an implementation that
/// differs must say so.
pub trait ZoneRules {
    /// The name of the zone these rules describe.
    fn zone(&self) -> &TimeZoneName;

    /// The instant a wall-clock reading names in this zone, epoch-ms UTC.
    fn instant_of(&self, wall: NaiveDateTime) -> i64;

    /// The wall-clock reading an instant shows as in this zone.
    fn wall_of(&self, instant_ms: i64) -> NaiveDateTime;
}

/// Which zone a **new** shared timed value is resolved in: the workspace's
/// default when it has declared one, otherwise this device's.
///
/// The one seam rule 6 of the temporal decision names, in one function so
/// there is nowhere else to decide it. Two things it deliberately is not:
///
/// - **It never touches a value that already exists.** Changing a workspace's
///   default re-answers the next deadline somebody sets, never a deadline
///   already written — a stored [`ZonedDateTime`] carries its own zone and
///   nothing rewrites it (invariant 24).
/// - **It is not a display choice.** What zone a reader *sees* a due in is
///   presentation's, decided per surface, and never read from here.
pub fn zone_for_new_shared_value(
    workspace_default: Option<&TimeZoneName>,
    device: &TimeZoneName,
) -> TimeZoneName {
    workspace_default.cloned().unwrap_or_else(|| device.clone())
}

#[cfg(test)]
mod tests;
