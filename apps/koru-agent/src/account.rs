// SPDX-License-Identifier: MPL-2.0

//! Desktop adapters for the shared account runtime.
pub use koru_account_runtime::account::*;
pub mod cloud_sync;
pub mod delivery;
pub mod inventory;

/// Preserve the execution owner across the desktop event-loop boundary.
/// Lifecycle publication, when needed, has already completed before this map.
pub(crate) fn account_event(event: WorkerEvent) -> crate::event::AgentEvent {
    use crate::event::AgentEvent;
    match event {
        WorkerEvent::Changed => AgentEvent::AccountChanged,
        WorkerEvent::Browser {
            owner_scope,
            requester,
            generation,
            attempt,
            url,
        } => AgentEvent::AccountBrowser {
            owner_scope,
            requester: crate::ConnectionId(requester),
            generation,
            attempt,
            url,
        },
        WorkerEvent::Result {
            owner_scope,
            requester,
            generation,
            result,
        } => AgentEvent::AccountResult {
            owner_scope,
            requester: crate::ConnectionId(requester),
            generation,
            result,
        },
    }
}
