// SPDX-License-Identifier: MPL-2.0

//! The request and response bodies the events API carries.
//!
//! They live in this leaf beside the [`crate::Envelope`] they are made of, so
//! the events worker, the SDK, and any external client name one declaration —
//! and the OpenAPI document is generated from the same types that produce the
//! bytes (the koru-store-wire lesson).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::envelope::{ActorKind, AttrKey, AttrValue, Envelope, EventId, EventName};

/// `POST /v1/events` — publish a batch. Order is preserved; each event is
/// accepted, deduplicated or rejected on its own.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventBatch {
    pub events: Vec<Envelope>,
}

/// A batch may not carry more events than this.
pub const MAX_PUBLISH_EVENTS: usize = 500;

/// What `POST /v1/events` answers. Duplicates are success — that is what
/// makes retrying under the same `event_id` idempotent.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AppendReceipt {
    pub accepted: u32,
    pub duplicates: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<RejectedEvent>,
    /// The journal's latest sequence after this batch — a subscriber's
    /// starting cursor.
    pub last_seq: u64,
}

/// One refused event, by position and stable code (`Violation::code`).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RejectedEvent {
    /// Index into the batch as sent.
    pub index: u32,
    pub event_id: String,
    /// A stable ASCII code, never prose (`unknown_event`, `undeclared_attr`,
    /// `events_disabled`, …).
    pub code: String,
    /// Untranslated developer text. Not for people.
    pub message: String,
}

/// One journaled event: the envelope plus what only the journal knows.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StoredEvent {
    /// The delivery cursor. Totally ordered per journal, never reused.
    pub seq: u64,
    /// The journal's own clock at append — `recorded - occurred` is the
    /// delivery latency the debug flag prints.
    pub recorded_at_ms: i64,
    pub envelope: Envelope,
}

/// `GET /v1/events` — one page of the journal after a cursor.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventPage {
    pub events: Vec<StoredEvent>,
    /// Pass this back as `after` to continue; equal to the request's `after`
    /// when the page is empty.
    pub next_after: u64,
}

/// One line of a subject's activity — **a projection of an envelope, not the
/// envelope.**
///
/// A pane asking "what happened to this task" wants what happened, when, who
/// did it and which lines belong to one gesture. It does not want the device
/// id, the profile id, the emitting session, the payload version or the
/// journal's own append clock, all of which a [`StoredEvent`] carries. Sending
/// the whole envelope would mean handing a rendering surface a device identity
/// it has no use for, and every later field added to the envelope with it — so
/// the row is enumerated here and the envelope stays the journal's shape.
///
/// **No content, structurally.** The only free-form thing an envelope holds is
/// its attributes, and [`AttrValue`] has no free-string variant: a value is a
/// reference, a time, a count, a flag, or a token from the vocabulary the
/// registry declared for that key. So passing the map through whole cannot leak
/// a title, a due date, a tag name or a line of text, and the pane gets the
/// tokens it actually needs — `reason` on a lifecycle event, `change` and
/// `temporal_kind` on a Due change, `child` on a structural one — without this
/// type learning a field per event.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActivityEntry {
    /// The journal position. The page cursor, and the **only** ordering: two
    /// entries are compared by this and never by `occurred_at_ms`.
    pub seq: u64,
    pub name: EventName,
    /// Wall clock, epoch ms UTC. **Display only** — clocks skew and devices
    /// sleep, so a surface prints this and orders by `seq`.
    pub occurred_at_ms: i64,
    /// Who acted, as a kind: a person, an agent, or a part of the system.
    pub actor: ActorKind,
    /// The actor's registered name token when it has one — an agent's name, a
    /// system component (`store`, `importer`). Lowercase and from a closed
    /// vocabulary, never prose, and absent for a person at this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_token: Option<String>,
    /// **The causal group.** A cascade — completing a parent completes every
    /// task under it — is a set of events sharing one of these, not one event
    /// with a count, so a pane folds a group into one line by this key. It is
    /// [`Envelope::chain_root`] already resolved: an event that started its own
    /// chain reports its own id, so this is never absent and a reader never
    /// has to know the `None`-means-self rule.
    pub correlation_id: EventId,
    /// The event's declared attributes, whole — see the type doc for why that
    /// cannot be content.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<AttrKey, AttrValue>,
}

impl ActivityEntry {
    /// The `reason` token, when the event declares one — `explicit_complete`,
    /// `cascade_from_ancestor`, `due`. The lifecycle enum is the single source
    /// these are spelled from (roadmap §3.10), so a surface matches on the
    /// token rather than reading it as prose.
    pub fn reason(&self) -> Option<&str> {
        match self.attrs.get("reason") {
            Some(AttrValue::Token(token)) => Some(token),
            _ => None,
        }
    }
}

impl From<&StoredEvent> for ActivityEntry {
    fn from(stored: &StoredEvent) -> Self {
        Self {
            seq: stored.seq,
            name: stored.envelope.name.clone(),
            occurred_at_ms: stored.envelope.occurred_at_ms,
            actor: stored.envelope.actor.kind,
            actor_token: stored.envelope.actor.token.clone(),
            correlation_id: stored.envelope.chain_root(),
            attrs: stored.envelope.attrs.clone(),
        }
    }
}

/// `GET /v1/events/activity` — one bounded page of one subject's activity.
///
/// Paged exactly like [`EventPage`]: a `seq` cursor, oldest first, `next_after`
/// to continue, and `next_after` equal to the request's `after` when the page
/// is empty. One cursor convention across the whole journal is what keeps a
/// page from being silently skipped.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActivityPage {
    pub entries: Vec<ActivityEntry>,
    /// Pass back as `after` to continue.
    pub next_after: u64,
    /// **How far back this answer can possibly reach.** Activity is
    /// device-local and the journal keeps nothing beyond this window, so an
    /// empty page has two readings — nothing happened, or it happened longer
    /// ago than this — and a surface can only say the honest one if it is told
    /// the number. It is carried on every page rather than only on an empty
    /// one, because a *full* page is equally horizonless without it.
    pub retention_days: u32,
}

/// The default page size for [`ActivityPage`] when the caller names none, and
/// the ceiling it is clamped to. A pane draws a screenful and pages; a caller
/// asking for more than the ceiling is answered with the ceiling rather than
/// refused, which is what [`EventPage`]'s limit already does.
pub const DEFAULT_ACTIVITY_LIMIT: u32 = 50;
pub const MAX_ACTIVITY_LIMIT: u32 = 200;

/// `GET /v1/events/status` — the worker's answer to "what are you doing?"
/// (PRINCIPLES: every component can be asked). Always-live counters — a
/// handful of integers, never content.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventsStatus {
    /// True when `KORU_EVENTS_DISABLED` silenced the worker: appends are
    /// no-ops and every read answers empty.
    #[serde(default)]
    pub disabled: bool,
    pub total_events: u64,
    pub domain_events: u64,
    pub instrumentation_events: u64,
    pub last_seq: u64,
    pub pruned_events: u64,
    pub outbox_pending: u64,
    /// Whether this device's recorded answer allows an export. False both
    /// when the person said no and when nobody has asked yet.
    pub telemetry_consent: bool,
    /// Whether this device holds an answer under the opt-in contract at all.
    /// `false` is what makes a surface ask — on a fresh install and on an
    /// upgrade from the old default-granted state alike. Defaulted so a newer
    /// client reading an older worker asks rather than assumes.
    #[serde(default)]
    pub telemetry_consent_answered: bool,
    pub baseline_seeded: bool,
    pub tracked_documents: u64,
    pub sse_subscribers: u64,
    pub journal_bytes: u64,
    /// Per-subject activity reads served since this worker started, and rows
    /// answered across them — the counters behind "is a pane scanning?".
    /// Defaulted so a newer client still parses an older worker's status.
    #[serde(default)]
    pub activity_queries: u64,
    #[serde(default)]
    pub activity_rows: u64,
}

/// `PUT /v1/events/telemetry/consent` — the one consent switch, and the only
/// way an export becomes allowed. Either answer is recorded as this device's
/// decision. Turning it off deletes the outbox and the installation id;
/// turning it on from any other state also starts from an empty outbox, so
/// nothing recorded before the yes is ever uploaded.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConsentBody {
    pub granted: bool,
    #[serde(default)]
    pub consent_version: String,
}
