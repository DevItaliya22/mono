// SPDX-License-Identifier: MPL-2.0

//! Personal attributes — per-profile facts about shared entities.
//!
//! Shared attributes (tags, color, due, priority) live in the document beside
//! the node they describe, because collaborators must agree they are true.
//! Personal attributes describe how **one person** intends to deal with a
//! shared thing — `koru.plan` is the first — and must never overwrite another
//! collaborator's answer to the same question. They therefore live in that
//! person's [`ProfileManifest`](crate::profile_manifest::ProfileManifest),
//! which replicates between that person's devices and nobody else's
//! ([ADR-200](../../../docs/decisions/README.md)).
//!
//! ## The shapes
//!
//! - [`EntityRef`] — the stable address an attribute attaches to. A node id
//!   alone means nothing outside its document, so the reference carries the
//!   workspace, the document and the node **together**, plus a kind token.
//! - [`PersonalAttributeValue`] — a typed value. **Three temporal types, not
//!   one with a nullable timezone** ([ADR-183]): `date` is a calendar date
//!   string (`2026-08-30`) and never midnight UTC; `datetime_floating` is a
//!   wall-clock reading with no zone at all — the default here, because a
//!   personal plan follows the person; `datetime_zoned` is an instant plus the
//!   IANA zone it was meant in, for the rare personal value that is genuinely
//!   pinned to a place. There is deliberately no shape meaning "an instant
//!   whose zone we forgot", so nothing downstream has to infer meaning from an
//!   absent field.
//!
//! [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
//!
//! ## Absent means unset
//!
//! A cleared attribute is a **deleted key**, never a stored "none" — the
//! `set_workspace_flag` pattern. An entity with no attributes reads as an
//! empty map, and an entry whose stored type this build does not recognise
//! reads as absent rather than as an error, which is what keeps the container
//! additive for older builds.

use std::collections::BTreeMap;
use std::collections::HashMap;

use loro::LoroValue;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::{Code, Error, Result};
use crate::ids::{DocumentId, WorkspaceId};
use crate::temporal::{
    CalendarDate, FloatingDateTime, T_DATE, T_DATETIME_FLOATING, T_DATETIME_ZONED, TemporalValue,
    ZonedDateTime, temporal_from_loro, temporal_to_loro, validate_calendar_date,
    validate_timezone_name, validate_wall_clock,
};

/// The most characters an attribute key may hold. Counted in characters, not
/// bytes, though a key is ASCII so the two agree.
const MAX_KEY_CHARS: usize = 200;
/// The most characters a text value may hold. Counted in **characters**, so
/// the limit means the same thing in every language.
const MAX_TEXT_CHARS: usize = 2000;
/// The most characters a timezone name may hold. IANA names are short ASCII.
/// The most characters an entity kind token may hold.
const MAX_KIND_CHARS: usize = 32;

// Storage tokens. Stable ASCII, written once and never renamed — renaming a
// storage key orphans every value already written under it.
const T_TEXT: &str = "text";
const T_NUMBER: &str = "number";
const T_BOOL: &str = "bool";
// Temporal storage tokens come from `temporal`; these two are non-temporal.
const F_TYPE: &str = "t";
const F_VALUE: &str = "v";

/// The stable address a personal attribute attaches to.
///
/// Spelled `<kind>:<workspace>:<document>:<node>` — a lowercase ASCII kind
/// token and three hyphenated lowercase UUIDs. The document and node travel
/// **together** because a node id alone means nothing outside its document,
/// and the workspace is included because one profile spans many workspaces.
/// Never an offset, a line number or a tree position.
///
/// ## The kind is preserved, never canonicalized away
///
/// The kind is **part of the address**, not a hint about it: `task:` and
/// `block:` over one triple are two entities here, they key two different
/// entries in the personal-attribute map, and nothing in this crate folds one
/// into the other ([ADR-285]). A note filed against a task therefore does not
/// leak onto the block it sits on and back. [`same_target`](Self::same_target)
/// is the one comparison allowed to cross kinds, named so a call site has to
/// say it means to; `==` answers the ordinary question and says `false`.
///
/// ## The conversion is the storage string, in both directions
///
/// `koru_core::EntityRef` is the same address in the domain, and **neither
/// crate depends on the other** — `koru-store-core` names nothing from
/// `koru-core` on purpose. So the conversion is
/// `koru_core::EntityRef::to_storage()` → [`parse`](Self::parse), and
/// [`to_storage`](Self::to_storage) → `koru_core::EntityRef::parse` coming
/// back, byte-identically in both directions. This side stores the kind as an
/// opaque token and validates shape only, which is why `task` needed no change
/// here at all: a kind this build has never heard of still files and still
/// reads back. The *domain* side is the one that knows the closed set, and a
/// build that has never heard of `task:` follows the existing unknown-kind
/// rule there — `koru_core::EntityRef::parse` answers `None` and the reader
/// skips the reference rather than failing.
///
/// [ADR-285]: ../../../docs/decisions/ADR-276-300.md#adr-285-taskmodel--a-task-is-a-marked-node-and-the-marker-or-a-due-date-is-the-whole-derivation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityRef {
    kind: String,
    workspace: WorkspaceId,
    document: DocumentId,
    node: Uuid,
}

impl EntityRef {
    /// Build a reference, validating the kind token.
    pub fn new(
        kind: &str,
        workspace: WorkspaceId,
        document: DocumentId,
        node: Uuid,
    ) -> Result<Self> {
        if !is_kind_token(kind) {
            return Err(Error::new(
                Code::InvalidRequest,
                "an entity kind is a short lowercase ASCII token",
            ));
        }
        Ok(Self {
            kind: kind.to_owned(),
            workspace,
            document,
            node,
        })
    }

    /// Parse the storage spelling. `None` for anything that is not exactly a
    /// kind token and three UUIDs — an address that is not understood must
    /// not be filed under, or it can never be read back.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split(':');
        let kind = parts.next()?;
        let workspace = WorkspaceId::parse(parts.next()?)?;
        let document = DocumentId::parse(parts.next()?)?;
        let node = Uuid::try_parse(parts.next()?).ok()?;
        if parts.next().is_some() || !is_kind_token(kind) {
            return None;
        }
        Some(Self {
            kind: kind.to_owned(),
            workspace,
            document,
            node,
        })
    }

    /// The canonical spelling — the map key it is stored under.
    pub fn to_storage(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.kind,
            self.workspace.to_storage(),
            self.document.to_storage(),
            self.node.as_hyphenated()
        )
    }

    /// The kind token — `document`, `block`, `task`, or one this build has
    /// never heard of. Answered so a caller can tell two references apart
    /// without re-parsing either, and so nothing has to slice the storage
    /// spelling by hand to find out what it is addressing.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The triple, **ignoring the kind**. The one comparison allowed to cross
    /// kinds, named so a call site says it means to: `==` is the ordinary
    /// question, and it answers `false` for a task and the block it sits on.
    ///
    /// The mirror of `koru_core::EntityRef::same_target`, restated here
    /// because this crate deliberately names nothing from `koru-core`.
    pub fn same_target(&self, other: &Self) -> bool {
        self.workspace == other.workspace
            && self.document == other.document
            && self.node == other.node
    }

    pub fn workspace(&self) -> WorkspaceId {
        self.workspace
    }

    pub fn document(&self) -> DocumentId {
        self.document
    }

    pub fn node(&self) -> Uuid {
        self.node
    }
}

impl serde::Serialize for EntityRef {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_storage())
    }
}

impl<'de> serde::Deserialize<'de> for EntityRef {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value)
            .ok_or_else(|| serde::de::Error::custom("not a canonical entity reference"))
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for EntityRef {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("EntityRef")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"string"})
    }
}

fn is_kind_token(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= MAX_KIND_CHARS
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A typed personal attribute value.
///
/// **The three temporal types are three variants**, and none of them is
/// "an instant with an optional zone" ([ADR-183]). The meaning is carried by
/// which variant it is, never by whether a field happens to be filled in, so
/// there is no reading of a stored value that could go two ways.
///
/// [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersonalAttributeValue {
    /// Free text.
    Text { text: String },
    /// A finite number.
    Number { number: f64 },
    /// A boolean. Stored only when set — "false" is a value, "unset" is an
    /// absent key.
    Bool { value: bool },
    /// A calendar date, `YYYY-MM-DD`. Deliberately **not** an instant: it
    /// names a day, the same day in every timezone.
    Date { date: String },
    /// A wall-clock date and time with **no zone**, `YYYY-MM-DDTHH:MM:SS`.
    ///
    /// The normal shape for a personal timed value, and why it is: `koru.plan`
    /// answers *when do I intend to work on this*, and 09:00 means 09:00
    /// wherever the person wakes up. Pinning it to the zone they were sitting
    /// in when they typed it would make a plan move overnight on a flight —
    /// a decision nobody made.
    ///
    /// There is no `tz` field, so nothing can half-fill one.
    DatetimeFloating { wall: String },
    /// An instant in epoch milliseconds UTC **plus** the IANA zone it was
    /// meant in (`America/New_York`) — a name rather than an offset, because
    /// daylight-saving rules change what a future offset will be.
    ///
    /// The zone is **not optional**: a personal value that is genuinely tied
    /// to a place says which, and one that is not is
    /// [`DatetimeFloating`](Self::DatetimeFloating) instead.
    DatetimeZoned { ms: i64, tz: String },
}

impl PersonalAttributeValue {
    /// Refuse a value that must never reach storage.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Text { text } => {
                if text.chars().count() > MAX_TEXT_CHARS {
                    return Err(Error::new(
                        Code::InvalidRequest,
                        format!("a text value holds at most {MAX_TEXT_CHARS} characters"),
                    ));
                }
            }
            Self::Number { number } => {
                if !number.is_finite() {
                    return Err(Error::new(
                        Code::InvalidRequest,
                        "a number value must be finite",
                    ));
                }
            }
            Self::Bool { .. } => {}
            Self::Date { date } => {
                validate_calendar_date(date)?;
            }
            Self::DatetimeFloating { wall } => {
                validate_wall_clock(wall)?;
            }
            Self::DatetimeZoned { tz, .. } => {
                validate_timezone_name(tz)?;
            }
        }
        Ok(())
    }

    /// The storage encoding: a small value map, written and replaced
    /// atomically so two concurrent writes to one key resolve to one whole
    /// value rather than a splice of both.
    pub(crate) fn to_loro(&self) -> LoroValue {
        let mut map: HashMap<String, LoroValue> = HashMap::new();
        match self {
            Self::Text { text } => {
                map.insert(F_TYPE.into(), T_TEXT.into());
                map.insert(F_VALUE.into(), text.as_str().into());
            }
            Self::Number { number } => {
                map.insert(F_TYPE.into(), T_NUMBER.into());
                map.insert(F_VALUE.into(), (*number).into());
            }
            Self::Bool { value } => {
                map.insert(F_TYPE.into(), T_BOOL.into());
                map.insert(F_VALUE.into(), (*value).into());
            }
            Self::Date { date } => {
                return temporal_to_loro(&TemporalValue::Date {
                    date: CalendarDate::parse(date).expect("validated calendar date"),
                });
            }
            Self::DatetimeFloating { wall } => {
                return temporal_to_loro(&TemporalValue::Floating {
                    wall: FloatingDateTime::parse(wall).expect("validated floating datetime"),
                });
            }
            Self::DatetimeZoned { ms, tz } => {
                return temporal_to_loro(&TemporalValue::Zoned {
                    value: ZonedDateTime::new(*ms, tz.clone()).expect("validated zoned datetime"),
                });
            }
        }
        LoroValue::from(map)
    }

    /// Decode a stored value. `None` for a shape this build does not
    /// recognise — an unknown type token reads as absent, never as an error,
    /// so a newer build's value does not make an older build refuse the map.
    pub(crate) fn from_loro(value: &LoroValue) -> Option<Self> {
        let LoroValue::Map(map) = value else {
            return None;
        };
        let tag = match map.get(F_TYPE) {
            Some(LoroValue::String(s)) => s.to_string(),
            _ => return None,
        };
        match tag.as_str() {
            T_TEXT => match map.get(F_VALUE) {
                Some(LoroValue::String(s)) => Some(Self::Text {
                    text: s.to_string(),
                }),
                _ => None,
            },
            T_NUMBER => match map.get(F_VALUE) {
                Some(LoroValue::Double(n)) => Some(Self::Number { number: *n }),
                Some(LoroValue::I64(n)) => Some(Self::Number { number: *n as f64 }),
                _ => None,
            },
            T_BOOL => match map.get(F_VALUE) {
                Some(LoroValue::Bool(b)) => Some(Self::Bool { value: *b }),
                _ => None,
            },
            T_DATE | T_DATETIME_FLOATING | T_DATETIME_ZONED => match temporal_from_loro(value)? {
                TemporalValue::Date { date } => Some(Self::Date {
                    date: date.as_str().to_owned(),
                }),
                TemporalValue::Floating { wall } => Some(Self::DatetimeFloating {
                    wall: wall.as_str().to_owned(),
                }),
                TemporalValue::Zoned { value } => Some(Self::DatetimeZoned {
                    ms: value.instant_ms(),
                    tz: value.zone().to_owned(),
                }),
            },
            // Anything else — including the retired `datetime`, which meant
            // instant-plus-optional-zone — reads as absent.
            _ => None,
        }
    }
}

/// Whether a key may name a personal attribute: `koru.plan`, or a custom
/// definition's generated id. ASCII, so it means the same bytes everywhere.
pub fn validate_attribute_key(key: &str) -> Result<()> {
    let ok = !key.is_empty()
        && key.len() <= MAX_KEY_CHARS
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(Error::new(
            Code::InvalidRequest,
            "an attribute key is a short ASCII token such as koru.plan",
        ))
    }
}

/// The attributes one entity carries, decoded. Ordered so an answer is stable.
pub type PersonalAttributes = BTreeMap<String, PersonalAttributeValue>;

/// `YYYY-MM-DD`, with a real month and a real day of that month.
/// `YYYY-MM-DDTHH:MM:SS`, with a real date and a real clock reading — and
/// **no zone marker of any kind**. A trailing `Z` or `+02:00` is refused
/// rather than trimmed: a floating value that arrived carrying a zone claim is
/// a caller who meant something else, and silently dropping the claim is how
/// one type becomes another.
/// A plausible IANA timezone name. The store does not carry the timezone
/// database, so this checks shape, not existence: ASCII, no whitespace,
/// bounded — and **never an offset**, because daylight-saving rules change
/// what a future offset will be. The same rule
/// `koru_core::time::TimeZoneName` enforces above, restated here because
/// `koru-store-core` deliberately names nothing from `koru-core`.
#[cfg(test)]
mod tests;
