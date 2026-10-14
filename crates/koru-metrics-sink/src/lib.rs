// SPDX-License-Identifier: MPL-2.0

//! The sink port: one trait, one event shape, one outcome.
//!
//! **Charter — what a delivery attempt to an analytics system looks like, and
//! nothing about where events are stored or when they are sent.** A forwarder
//! links this crate and never links an HTTP client or the name of a vendor; an
//! adapter links this crate and never links a database. Adding a second sink is
//! then a new crate and one line at a construction site.
//!
//! ## Two rules that are easy to break later
//!
//! **A [`SinkId`] is stable storage.** Today the only delivery state is *the
//! row is still in the outbox*; the moment delivery state becomes per
//! `(event, sink)` — which is the hosted service's design — this string is a
//! primary-key column. Renaming one then orphans the delivery state of every
//! event that sink had already taken, exactly as renaming `NodeMeta.colour`
//! would orphan every document written with it.
//!
//! **A code is a stable ASCII token, never prose.** It is stored, compared and
//! read by a person in a log line; a sentence there is a message that will be
//! reworded and silently stop matching.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Which sink. Stable ASCII — see the module note before renaming one.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SinkId(pub &'static str);

impl SinkId {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for SinkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// One event as a sink receives it: the outbox row plus the build dimensions
/// the exporter adds, and no delivery state at all.
///
/// **There is no identity here beyond the two anonymous ids.** `session_id` is
/// ephemeral by construction — it cannot outlive a process — and
/// `installation_id` is a client-minted v4 UUID that is deleted the moment
/// consent is withdrawn. Nothing derived from a device, a hostname, a user
/// name, a MAC address or a serial number may ever be added to this struct;
/// what reaches a sink is what leaves the machine.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SinkEvent {
    /// The journal's `EventId`. The dedupe key downstream too, which is what
    /// makes a duplicate delivery harmless and therefore makes retrying safe.
    pub event_id: String,
    pub name: String,
    pub version: u16,
    /// When it happened, not when it was sent — an adapter maps this to the
    /// vendor's timestamp field, or a day of backlog arrives as a spike.
    pub occurred_at_ms: i64,
    pub session_id: String,
    pub installation_id: Option<String>,
    pub app_version: String,
    pub build_channel: String,
    pub platform: String,
    pub consent_version: String,
    /// Exportable-class attributes only, as `koru_events::telemetry::project`
    /// produced them. An adapter passes them through; it never adds to them
    /// from its own knowledge.
    pub properties: serde_json::Map<String, serde_json::Value>,
}

/// What became of one delivery attempt.
///
/// The distinction that matters is **whether time fixes it**. `Retry` says the
/// same bytes may work later, so nothing may be deleted; `Permanent` says they
/// never will, so a caller must stop resending them — and, because a batch is
/// refused as a whole, must find out which event caused it before dropping
/// anything.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Delivery {
    Ok,
    /// The whole batch failed and should be tried again. `after` is the
    /// sink's own instruction when it gave one — honour it rather than a
    /// local backoff, since it is the only party that knows.
    Retry {
        after: Option<Duration>,
        code: &'static str,
    },
    /// The whole batch was refused for a reason time will not fix.
    Permanent {
        code: &'static str,
    },
}

impl Delivery {
    /// The stable token, whatever the outcome — what a log line and a stored
    /// `last_error_code` carry.
    pub fn code(&self) -> &'static str {
        match self {
            Delivery::Ok => code::OK,
            Delivery::Retry { code, .. } | Delivery::Permanent { code } => code,
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Delivery::Ok)
    }
}

/// Every code a sink may answer with. **Stable ASCII, never prose** — these are
/// stored and compared, and a reworded sentence would silently stop matching.
pub mod code {
    pub const OK: &str = "ok";
    /// The request did not complete: no connection, DNS, TLS, or a read that
    /// ran out of time.
    pub const TIMEOUT: &str = "timeout";
    /// The sink asked us to slow down.
    pub const RATE_LIMITED: &str = "rate_limited";
    /// The sink is unwell — 5xx.
    pub const SERVER_ERROR: &str = "server_error";
    /// The sink refused the bytes and will refuse them again.
    pub const BAD_REQUEST: &str = "bad_request";
    /// The credential is wrong. Retryable on purpose: a bad token is fixed by
    /// a config change, and discarding a queue over it is unrecoverable.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// The sink answered something no rule here covers.
    pub const UNEXPECTED: &str = "unexpected";
}

/// One analytics destination.
///
/// Blocking, deliberately: every implementation is an HTTP request, the tree's
/// HTTP client is `reqwest`'s blocking one, and a forwarder that owns a thread
/// has nothing to gain from a future here. A caller that lives on an async
/// runtime hands this to `spawn_blocking`.
pub trait Sink: Send + Sync + 'static {
    fn id(&self) -> SinkId;

    /// The most events this sink will take in one request.
    ///
    /// **The sink knows its own limit; a forwarder never guesses one.** That
    /// is the difference between adding a sink and editing the sender.
    fn max_batch(&self) -> usize;

    /// Deliver a batch, all or nothing.
    ///
    /// `events` is never empty. An implementation must be safe to call again
    /// with the same events: every sink here dedupes on `event_id`, which is
    /// what makes an aggressive retry cheap rather than a duplication bug.
    fn deliver(&self, events: &[SinkEvent]) -> Delivery;
}

/// Accepts everything, delivers nothing.
///
/// It is what the forwarder's tests run against, and it is what makes *"the
/// event bus does not depend on a sink"* a fact a test asserts rather than a
/// claim in a document. It counts, so a test can say how many events a drain
/// actually handed over.
#[derive(Default, Debug)]
pub struct Blackhole {
    delivered: std::sync::atomic::AtomicUsize,
    batches: std::sync::atomic::AtomicUsize,
}

impl Blackhole {
    pub const ID: SinkId = SinkId("blackhole");

    pub fn new() -> Self {
        Self::default()
    }

    /// How many events it has been handed.
    pub fn delivered(&self) -> usize {
        self.delivered.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many calls that took — the number that catches a forwarder
    /// ignoring `max_batch`.
    pub fn batches(&self) -> usize {
        self.batches.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Sink for Blackhole {
    fn id(&self) -> SinkId {
        Self::ID
    }

    fn max_batch(&self) -> usize {
        500
    }

    fn deliver(&self, events: &[SinkEvent]) -> Delivery {
        self.delivered
            .fetch_add(events.len(), std::sync::atomic::Ordering::Relaxed);
        self.batches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Delivery::Ok
    }
}

#[cfg(test)]
mod tests;
