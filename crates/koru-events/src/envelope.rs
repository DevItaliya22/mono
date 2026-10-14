// SPDX-License-Identifier: MPL-2.0

//! The record itself: one event on the local bus.
//!
//! **Profile-scoped and device-local.** The journal an envelope lands in
//! belongs to one device, so actor, device and profile identity may appear
//! here — a future flow rule wants to say "if device = A" — and the privacy
//! line sits at the export boundary instead: [`crate::telemetry::project`]
//! strips every identity mechanically, whatever the event's policy says.
//!
//! **Ordering never comes from this type.** `occurred_at_ms` is a wall clock
//! (display and bucketing only; clocks skew and devices sleep) and the id is
//! an opaque v4 UUID (ADR-39). Order, replay position and "newer than" are
//! all the journal's sequence number, which lives beside a stored event and
//! never inside one — an unsent event has no position yet.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A uuid-string id newtype: serde delegates to `uuid`'s hyphenated string
/// form, and the JSON Schema says `string` / `uuid` — written by hand because
/// the workspace's `schemars` build does not carry the `uuid` integration.
macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Client-minted, never derived from a name, path, machine or
            /// content (ADR-39).
            pub fn mint() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        /// The hyphenated string form [`Display`](fmt::Display) writes, so an
        /// id carried as a `String` — the sink port's `SinkEvent` — parses back
        /// into the newtype rather than being re-wrapped by hand per call site.
        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(text).map(Self)
            }
        }

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed(stringify!($name))
            }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({ "type": "string", "format": "uuid" })
            }
        }
    };
}

uuid_id!(
    /// One event, forever. The dedupe key at every boundary: the journal, the
    /// SDK's retry, the ingest service, a flow rule's execution id.
    EventId
);
uuid_id!(
    /// One application run. Regenerated each start, never persisted across
    /// runs — the same contract as the agent's per-run instance id.
    SessionId
);
uuid_id!(
    /// This installation's device id (the agent's `device.json` identity).
    DeviceId
);
uuid_id!(
    /// The profile the emitting window had open. Never an account id.
    ProfileId
);
uuid_id!(
    /// A workspace, by the store's id.
    WorkspaceId
);
uuid_id!(
    /// Who acted, when the actor has a durable identity of its own.
    ActorId
);
uuid_id!(
    /// An untyped local reference — what an [`AttrValue::Id`] holds.
    RefId
);

/// A registry-validated dotted event name, e.g. `document.created`.
///
/// Construct literals through [`crate::event_name!`], which fails the *build*
/// on a name missing from the registry; a runtime-built name is refused by
/// [`crate::validate`] at the boundary instead.
#[derive(
    Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct EventName(String);

impl EventName {
    /// Unchecked: the boundary validates. Prefer [`crate::event_name!`] for
    /// literals, which checks at compile time.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// What [`crate::event_name!`] expands to.
    pub fn from_static(name: &'static str) -> Self {
        Self(name.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EventName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who caused an event.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Actor {
    pub kind: ActorKind,
    /// The actor's durable identity, when it has one a rule could name.
    /// A person at this profile usually has none; an agent or device does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<ActorId>,
    /// A registered name token — an agent's name, a system component
    /// (`store`, `importer`, `sync`). Lowercase `[a-z0-9_-]`, never prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl Actor {
    pub fn user() -> Self {
        Self {
            kind: ActorKind::User,
            id: None,
            token: None,
        }
    }

    pub fn system(component: &str) -> Self {
        Self {
            kind: ActorKind::System,
            id: None,
            token: Some(component.to_string()),
        }
    }

    pub fn agent(name: &str) -> Self {
        Self {
            kind: ActorKind::Agent,
            id: None,
            token: Some(name.to_string()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    User,
    Agent,
    System,
}

/// Where the event was minted.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Origin {
    /// This installation, when the emitter knows it. Local-only: stripped at
    /// the telemetry boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    /// The application run that emitted it. Ephemeral by construction, so it
    /// is the one origin field a telemetry event may carry.
    pub session_id: SessionId,
}

/// Where in the data model the event applies. The profile is usually implicit
/// (the journal is profile-local today); it is carried so a multi-profile
/// device can tell its own events apart.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Scope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<ProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
}

/// What the event is about, when it is about one thing. A reference, never
/// content: the consumer fetches the thing through the store.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Subject {
    pub kind: SubjectKind,
    /// The store's id for it, as the store spells it.
    pub id: String,
    /// The store sequence of the state referred to, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
}

impl Subject {
    pub fn document(id: impl Into<String>) -> Self {
        Self {
            kind: SubjectKind::Document,
            id: id.into(),
            version: None,
        }
    }

    pub fn workspace(id: impl Into<String>) -> Self {
        Self {
            kind: SubjectKind::Workspace,
            id: id.into(),
            version: None,
        }
    }

    /// A task, addressed **as a task**: the four-segment
    /// `task:<workspace>:<document>:<node>` spelling that
    /// `koru_core::EntityRef::Task` writes (SPEC §3).
    ///
    /// The kind is part of the identity and is never canonicalized away — a
    /// `task:` subject and a `block:` reference to the same triple are
    /// different subjects, which is what keeps a task's activity from being
    /// answered by the block's.
    ///
    /// The parts are `Display` rather than typed ids because this crate is a
    /// leaf: it may not acquire the node model to find out what a `NodeId`
    /// is. The conversion from the domain type is `EntityRef::to_storage`,
    /// and this spelling is its other half.
    pub fn task(
        workspace: impl fmt::Display,
        document: impl fmt::Display,
        node: impl fmt::Display,
    ) -> Self {
        Self {
            kind: SubjectKind::Task,
            id: format!("task:{workspace}:{document}:{node}"),
            version: None,
        }
    }
}

/// Closed on purpose: a new subject kind is an additive, reviewed change,
/// not a string somebody invents at a call site.
///
/// `Task` is one task, by the `task:<workspace>:<document>:<node>` spelling
/// [`Subject::task`] writes. A *node* is the task (ADR-272); the kind exists
/// because addressing that node as a task is a different question from
/// addressing it as content, and the two must not answer each other.
//
// The variants carry no doc comments of their own **on purpose**: schemars
// turns a documented variant into a `oneOf` branch, so one `///` here would
// change this from a flat string enum into a two-branch union in the
// gateway's generated OpenAPI — a published contract shape, changed by a
// comment. Say it above, where it is a description rather than a schema.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Workspace,
    Document,
    Task,
}

impl SubjectKind {
    /// The wire spelling — the same snake_case token serde writes, reachable
    /// without a round trip through `serde_json`.
    ///
    /// It exists because a subject travels as **two query parameters** on the
    /// per-subject activity read (`kind=` and `id=`), so both ends need the
    /// token as a `&str`: the client writes it into a URL and the worker parses
    /// it back. `a_subject_kind_token_is_what_serde_writes` holds the two
    /// spellings equal, so a `rename_all` change cannot quietly desynchronise
    /// the URL from the JSON.
    ///
    /// This is deliberately **not** the journal's storage token. That one lives
    /// beside the SQL that writes it and argues for its own copy: a column
    /// value is storage, and a wire label may be changed in a way a written row
    /// may not.
    pub fn token(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Document => "document",
            Self::Task => "task",
        }
    }

    /// The other direction. `None` for anything not in the closed set — a
    /// caller answers that as a refusal rather than guessing a kind, because
    /// guessing would answer one subject's question with another's history.
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "workspace" => Some(Self::Workspace),
            "document" => Some(Self::Document),
            "task" => Some(Self::Task),
            _ => None,
        }
    }

    /// Every variant, for a caller that has to name the closed set — an error
    /// message listing what it would have accepted, a test walking all three.
    pub const ALL: &'static [Self] = &[Self::Workspace, Self::Document, Self::Task];
}

/// A typed attribute value. **There is deliberately no free-string variant**:
/// content cannot be smuggled through an attribute by construction. A string
/// only exists as [`AttrValue::Token`], whose values come from the vocabulary
/// the registry declares for that key.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum AttrValue {
    /// A reference (`LocalReference` class): valid on the local bus, dropped
    /// mechanically at the telemetry boundary. (`RefId` is only the schema's
    /// name for it — serde carries the bare uuid string.)
    Id(#[schemars(with = "RefId")] Uuid),
    /// Epoch milliseconds UTC.
    Time(i64),
    /// A value from the key's declared closed vocabulary.
    Token(String),
    Count(u64),
    /// A signed quantity — a UTC offset, a delta. `Count` for anything that
    /// cannot be negative.
    Int(i64),
    /// A dotted numeric version — `10.0.26200`. Its own kind rather than a
    /// token because a version has no closed vocabulary, and rather than a
    /// free string because the boundary enforces digits and dots only: it is
    /// structurally incapable of carrying content.
    Version(String),
    /// A client-side bucketed numeric — see [`crate::bucket`].
    Bucket(u32),
    Flag(bool),
}

/// Attribute keys are plain strings on the wire; the registry says which are
/// legal per event.
pub type AttrKey = String;

/// One event on the local bus.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Envelope {
    pub event_id: EventId,
    pub name: EventName,
    /// The `(name, version)` pair identifies the payload shape. Bumped per
    /// event name on breaking change only; additive attrs do not bump it, and
    /// a pair is never reused with a different meaning.
    pub version: u16,
    /// Wall clock, epoch ms UTC. Display and bucketing ONLY — never ordering.
    pub occurred_at_ms: i64,
    pub actor: Actor,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "scope_is_empty")]
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
    /// The chain root. `None` means this event is its own root — readers use
    /// [`Envelope::chain_root`], never the raw field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<EventId>,
    /// The immediate parent event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<EventId>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<AttrKey, AttrValue>,
}

fn scope_is_empty(scope: &Scope) -> bool {
    scope.profile_id.is_none() && scope.workspace_id.is_none()
}

impl Envelope {
    /// The correlation rule, spelled once: a chain's id is the root event's
    /// own id, and only reactions carry it explicitly.
    pub fn chain_root(&self) -> EventId {
        self.correlation_id.unwrap_or(self.event_id)
    }
}
