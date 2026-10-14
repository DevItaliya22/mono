// SPDX-License-Identifier: MPL-2.0

//! The execution side: how far *this machine* got (D3, §7.1).
//!
//! An [`AttemptState`] is what `runs.db` carries about one attempt — the
//! generation, the provider session key, the process facts recovery reasons
//! from (§7.6) and the publications not yet acknowledged. It never carries a
//! second canonical status (§15) and it never syncs.
//!
//! There is no `From` between this and [`crate::RunHeader`], in either
//! direction. They share a [`RunId`]; a field that belongs to one cannot
//! reach the other by conversion, which is the boundary the roadmap asks for
//! in types rather than in discipline.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AttemptGeneration, EventId, IdempotencyKey, RunAttemptId, RunId, SessionId};

/// The session host's incarnation — `HostHello.instance_id`, a fresh v4
/// UUID per host process. It separates "the same host that might still hold
/// my session" from "a new host, so nothing survived" (§7.6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostInstanceId(Uuid);

impl HostInstanceId {
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

/// Written by hand: the workspace's `schemars` carries no `uuid`
/// integration, and the value crosses as a string.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for HostInstanceId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("HostInstanceId")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "format": "uuid" })
    }
}

impl fmt::Debug for HostInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let full = self.0.simple().to_string();
        write!(f, "host({})", &full[..8])
    }
}

/// How the attempt's provider session is found again (§7.6).
///
/// The idempotency key is reserved in the host's registry before the process
/// is spawned and retired when a fresh attempt is authorised; the session id
/// is known once creation completes. A non-creating lookup by the key is how
/// recovery asks "did it start?" — never a retried create.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderSessionKey {
    pub idempotency: IdempotencyKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
}

/// What this machine observed about the attempt's processes. Facts, not
/// health: recovery combines them with the host's answer (§7.6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProcessFacts {
    /// The host incarnation the session was created under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_instance: Option<HostInstanceId>,
    /// The delegate worker's own process id when it last wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
}

/// What a pending publication is (§7.1's list).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PublicationKind {
    /// A `delegate.run.*` / `delegate.turn.*` lifecycle fact.
    Lifecycle,
    /// An artifact registration whose reply was lost.
    ArtifactRegistration,
    /// A usage fact (§17.1: committed first, then published).
    Usage,
}

/// One publication not yet acknowledged. Re-publication reuses the same
/// [`EventId`], which the journal dedupes — a retry queue, not a second
/// audit service.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PendingPublication {
    pub event: EventId,
    pub kind: PublicationKind,
    /// Delivery attempts so far. Bounded by the caller's policy, counted
    /// here.
    pub attempts: u32,
}

/// What `runs.db` holds about one attempt of a run.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttemptState {
    pub run: RunId,
    pub attempt: RunAttemptId,
    /// The fence (§7.5, §7.6): an event or call carrying an older generation
    /// may not touch status, control or authority.
    pub generation: AttemptGeneration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session: Option<ProviderSessionKey>,
    #[serde(default)]
    pub process: ProcessFacts,
    #[serde(default)]
    pub pending_publications: Vec<PendingPublication>,
    /// The projection cursor the Run document has acknowledged (§7.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_cursor: Option<u64>,
}

impl AttemptState {
    /// The first attempt of a run: generation one, nothing observed yet.
    pub fn first(run: RunId) -> Self {
        Self::new(run, AttemptGeneration::FIRST)
    }

    /// The attempt a resume mints: the next generation, a fresh attempt id,
    /// and none of the previous attempt's session or process facts — those
    /// belong to the attempt they were observed on.
    pub fn successor(&self) -> Self {
        Self::new(self.run, self.generation.next())
    }

    fn new(run: RunId, generation: AttemptGeneration) -> Self {
        Self {
            run,
            attempt: RunAttemptId::mint(),
            generation,
            started_at_ms: None,
            provider_session: None,
            process: ProcessFacts::default(),
            pending_publications: Vec::new(),
            projection_cursor: None,
        }
    }
}

/// A generation the current attempt refuses (§7.3: "a call carrying an
/// older attempt generation is refused with a stable code"). Both numbers,
/// no text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StaleAttempt {
    pub current: AttemptGeneration,
    pub offered: AttemptGeneration,
}

impl fmt::Display for StaleAttempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stale attempt: generation {} offered, {} current",
            self.offered.get(),
            self.current.get()
        )
    }
}

impl std::error::Error for StaleAttempt {}
