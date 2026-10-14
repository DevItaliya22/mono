// SPDX-License-Identifier: MPL-2.0

//! Where a committed catalog fact leaves the Store.
//!
//! **A port, for the same reason as search, the outline and replication.** The
//! journal an event lands in is the daemon's — `koru-events-server` owns it,
//! and this crate is the one that knows HTTP exists, not the one that knows
//! where a journal lives. The daemon hands an implementation down once, before
//! anything is served ([`crate::state::AppState::set_catalog_event_sink`]);
//! a build that opened no journal leaves it empty and every publish is a
//! silent no-op.
//!
//! **Every call site is after the commit.** The handler has already returned
//! from the store call that persisted the mutation and released the lock, so
//! the ordering the manifest broadcast obeys — persist, then tell anyone else —
//! holds here too. An event is a fact: nothing downstream may read one as an
//! instruction to do the work, because the work is already done.
//!
//! **Never a reason to fail a request.** A journal that will not accept an
//! observation is a store whose menus refresh late, not a store that lost a
//! workspace, so a failed publish is logged and dropped — the same posture the
//! commit feed takes one layer up.

/// The daemon's journal, as this crate is allowed to see it.
pub trait CatalogEventSink: Send + Sync + 'static {
    /// Record one committed catalog fact. Must not block the caller for long
    /// and must not fail the request it belongs to: an implementation that
    /// cannot record swallows the observation.
    fn publish(&self, event: koru_events::Draft);
}
