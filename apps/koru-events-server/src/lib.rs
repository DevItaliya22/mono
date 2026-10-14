// SPDX-License-Identifier: MPL-2.0

//! **The Koru events worker** — the journal, the fan-out, and the
//! `/v1/events` API.
//!
//! The same worker-beside-the-gateway shape as `koru-store-server` and the
//! ai-server: the API gateway embeds one ([`EventsHandle`]) and routes
//! `/v1/events*` into it; a thin unshipped binary hosts the same router
//! standalone for integration and diagnostics; a deployed service later is a
//! change of host, not a rewrite. The store's commit feed reaches it through
//! [`EventsHandle::feed`]; everything else — koru.exe, the Agent, koru-mcp,
//! external tools — reaches it as a gateway client.
//!
//! What an event *is* lives in `koru-events`; this crate is only where one
//! goes: `events.db` under the worker's directory, appended after validation,
//! read back by cursor, streamed over SSE, and spooled to the consent-gated
//! telemetry outbox. The architecture and milestones:
//! `docs/architecture/eventbus/README.md`.

pub mod forward;
pub mod journal;
pub mod routes;
pub mod state;

pub use journal::{BaselineEntry, FeedNote, IdentityWake, JournalError, ReminderWake};
pub use state::{EventsHandle, EventsState, WorkerConfig};
