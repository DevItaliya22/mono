// SPDX-License-Identifier: MPL-2.0

//! The telemetry wire shapes, and the one projection onto them.
//!
//! **Two envelopes, deliberately.** The local [`crate::Envelope`] carries
//! references and identity so rules can act on them; the [`TelemetryEvent`]
//! carries build dimensions and no identity at all. The exporter *constructs*
//! the second from the first through [`project`] — it never serializes the
//! internal event verbatim, so a field added locally cannot leak by default.
//!
//! What `project` strips, unconditionally and regardless of the event's
//! declared policy: every `Id`-valued attr and every attr whose class is
//! `LocalReference`; the actor, origin (bar the ephemeral session id), scope,
//! subject, correlation and causation. What the batch adds at export time,
//! from `koru_channel::build_info()` on the exporter's side: app version,
//! channel, platform. Nothing here reads the environment — this crate stays a
//! leaf, so the exporter passes those in.
//!
//! The endpoint's host and path are deliberately NOT pinned here — the body
//! shape is the contract; the URL is a config value. See
//! `docs/architecture/eventbus/README.md`.

use serde::{Deserialize, Serialize};

use crate::envelope::{AttrValue, Envelope, EventId, SessionId};
use crate::registry;

/// Which consent copy is in force. Carried on every batch, so a server can
/// drop events consented under a version that was later withdrawn.
///
/// **Bump it when what is collected changes**, or when the basis of the
/// consent does. `"1"` was ADR-176's *default-on* era, when an installation
/// nobody had asked counted as granted; `"2"` is the opt-in contract of
/// 2026-10-11 (ADR-176-consenton's amendment),
/// where only an explicit yes on this device counts. A server can therefore
/// tell a batch a person agreed to from one an old build sent by default.
pub const DEFAULT_CONSENT_VERSION: &str = "2";

/// The consent gate the outbox and exporter obey.
///
/// **Default is off, and only an explicit yes turns it on.** The opt-in
/// contract (maintainers, 2026-10-11) amends ADR-176: each device's own recorded
/// answer is the only thing that allows an export. A release channel or a
/// build marker classifies a binary and never stands in for consent, and an
/// installation nobody has asked — including one whose old default said
/// "granted" — exports nothing.
///
/// Two values are not granted and mean different things to a surface:
/// [`undecided`](Consent::undecided) (nobody has answered, so the person is
/// asked) and [`none`](Consent::none) (an explicit refusal, so nobody asks
/// again). To the exporter they are the same answer — no. Whether a person
/// has answered is the journal's record, not a field here, because it is a
/// fact about the device rather than about a batch.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Consent {
    pub granted: bool,
    /// Which consent copy the person saw — carried on every batch so the
    /// server can drop events consented under a withdrawn version.
    pub consent_version: String,
}

impl Consent {
    /// An explicit yes, at the current version.
    pub fn granted() -> Self {
        Self {
            granted: true,
            consent_version: DEFAULT_CONSENT_VERSION.to_string(),
        }
    }

    /// An explicit refusal. Not a default; a decision.
    pub fn none() -> Self {
        Self {
            granted: false,
            consent_version: String::new(),
        }
    }

    /// Nobody has answered on this device. Exports nothing.
    pub fn undecided() -> Self {
        Self::none()
    }
}

impl Default for Consent {
    /// **Not granted.** What an installation nobody has asked is treated as.
    fn default() -> Self {
        Self::undecided()
    }
}

/// A server never accepts more than this many events in one batch.
pub const MAX_BATCH_EVENTS: usize = 500;

/// One exportable observation. Constructed only by [`project`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TelemetryEvent {
    /// The journal `EventId` — the dedupe key on the server too, so a
    /// re-uploaded batch is idempotent.
    pub event_id: EventId,
    pub name: String,
    pub version: u16,
    pub occurred_at_ms: i64,
    /// Ephemeral, per application run — the one identity-adjacent field that
    /// survives export, because it cannot outlive a process.
    pub session_id: SessionId,
    /// Exportable-class attrs only, as plain JSON values.
    pub properties: serde_json::Map<String, serde_json::Value>,
}

/// One upload. Build dimensions ride the batch, not each event.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TelemetryBatch {
    /// Batch-level id for logs and retries; v4, minted per attempt.
    pub batch_id: String,
    pub sent_at_ms: i64,
    pub app_version: String,
    pub build_channel: String,
    pub platform: String,
    pub consent_version: String,
    /// Only after consent; random, rotating, revocable. `None` means the
    /// batch may carry `AnonymousUsage` events only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anonymous_installation_id: Option<String>,
    pub events: Vec<TelemetryEvent>,
}

/// The server's answer. Duplicates are success, not error — that is what
/// makes re-upload after a lost ack safe.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TelemetryAck {
    pub accepted: u32,
    pub duplicates: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<TelemetryRejection>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TelemetryRejection {
    pub event_id: String,
    /// A stable code (`unknown_event`, `unknown_version`, `undeclared_field`,
    /// `policy_violation`, `consent_missing`), never prose.
    pub reason: String,
}

/// The projection: `Some` only when consent is granted, the event's registry
/// policy is exportable, and then only the exportable attrs survive.
pub fn project(envelope: &Envelope, consent: &Consent) -> Option<TelemetryEvent> {
    if !consent.granted {
        return None;
    }
    let spec = registry::spec(envelope.name.as_str())?;
    if !spec.telemetry.exportable() {
        return None;
    }
    let mut properties = serde_json::Map::new();
    for (key, value) in &envelope.attrs {
        let Some(attr) = spec.attr(key) else { continue };
        if !attr.class.exportable() {
            continue;
        }
        // Belt beside braces: an id-shaped value never exports even if a
        // registry row misclassified it.
        let json = match value {
            AttrValue::Id(_) => continue,
            AttrValue::Time(ms) => serde_json::Value::from(*ms),
            AttrValue::Token(token) => serde_json::Value::from(token.clone()),
            AttrValue::Count(count) => serde_json::Value::from(*count),
            AttrValue::Int(value) => serde_json::Value::from(*value),
            AttrValue::Version(version) => serde_json::Value::from(version.clone()),
            AttrValue::Bucket(bucket) => serde_json::Value::from(*bucket),
            AttrValue::Flag(flag) => serde_json::Value::from(*flag),
        };
        properties.insert(key.clone(), json);
    }
    Some(TelemetryEvent {
        event_id: envelope.event_id,
        name: envelope.name.as_str().to_string(),
        version: envelope.version,
        occurred_at_ms: envelope.occurred_at_ms,
        session_id: envelope.origin.session_id,
        properties,
    })
}

/// **Server-side validation of an exported batch — the mirror of [`project`].**
/// What the client strips by construction, the server refuses by check, from
/// the same registry and never a second vocabulary: defence in depth at the
/// one boundary where the client is not ours. Pure over registry data, so this
/// crate stays a leaf.
///
/// The answer is one of the wire's own rejection codes
/// ([`TelemetryRejection::reason`]), never prose:
///
/// - `unknown_event` — the name is not registered.
/// - `unknown_version` — the name is registered at a different version.
/// - `policy_violation` — the event's policy is not exportable (a `Never`
///   event arriving at all), or a property key names an attribute whose class
///   never leaves the device (`LocalReference`).
/// - `undeclared_field` — a property key the event does not declare, a JSON
///   shape that does not match the attribute's kind, or a required exportable
///   attribute that is absent.
/// - `consent_missing` — a `PseudonymousUsage` event in a batch with no
///   `anonymous_installation_id`; such a batch may carry `AnonymousUsage` (and
///   `Operational`) events only.
///
/// **`required` is checked only for attributes whose class is exportable.** The
/// client strips `LocalReference` attributes by design, so requiring them here
/// would reject every valid event that carries one. Token values are held to
/// their JSON *shape* (a string), not re-checked against their vocabulary: the
/// version check above already pins the registry generation, and a second
/// vocabulary drift point is exactly what this function exists not to be.
pub fn validate_event(event: &TelemetryEvent, batch: &TelemetryBatch) -> Result<(), &'static str> {
    let Some(spec) = registry::spec(event.name.as_str()) else {
        return Err("unknown_event");
    };
    validate_against(spec, event, batch.anonymous_installation_id.is_some())
}

/// The checks, given the registry row. Split from [`validate_event`] so the
/// tests can hold the edges the live registry does not currently exercise — a
/// `PseudonymousUsage` event, an exportable event with a required
/// `LocalReference` attr — without inventing registry rows for them.
fn validate_against(
    spec: &registry::EventSpec,
    event: &TelemetryEvent,
    identified: bool,
) -> Result<(), &'static str> {
    if event.version != spec.version {
        return Err("unknown_version");
    }
    if !spec.telemetry.exportable() {
        return Err("policy_violation");
    }
    if !identified && spec.telemetry == registry::TelemetryPolicy::PseudonymousUsage {
        return Err("consent_missing");
    }
    for (key, value) in &event.properties {
        let Some(attr) = spec.attr(key) else {
            return Err("undeclared_field");
        };
        if !attr.class.exportable() {
            return Err("policy_violation");
        }
        if !shape_matches(attr.kind, value) {
            return Err("undeclared_field");
        }
    }
    for attr in spec
        .attrs
        .iter()
        .filter(|attr| attr.required && attr.class.exportable())
    {
        if !event.properties.contains_key(attr.key) {
            return Err("undeclared_field");
        }
    }
    Ok(())
}

/// Whether a JSON value has the shape an attribute kind serializes to under
/// [`project`]: counts and buckets are unsigned numbers, times and ints are
/// signed numbers, flags are booleans, and tokens, versions and ids are
/// strings — a version additionally held to the dotted-digits rule both
/// boundaries share. An `Id` attr is always `LocalReference` (the registry
/// tests hold that), so its arm is unreachable in practice and exists only to
/// keep the match total.
fn shape_matches(kind: registry::AttrKind, value: &serde_json::Value) -> bool {
    match (kind, value) {
        (registry::AttrKind::Id, serde_json::Value::String(_)) => true,
        (registry::AttrKind::Time, serde_json::Value::Number(number)) => number.is_i64(),
        (registry::AttrKind::Token, serde_json::Value::String(_)) => true,
        (registry::AttrKind::Count, serde_json::Value::Number(number)) => number.is_u64(),
        (registry::AttrKind::Int, serde_json::Value::Number(number)) => number.is_i64(),
        (registry::AttrKind::Version, serde_json::Value::String(version)) => {
            crate::validate::version_is_well_formed(version)
        }
        (registry::AttrKind::Bucket, serde_json::Value::Number(number)) => number.is_u64(),
        (registry::AttrKind::Flag, serde_json::Value::Bool(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
