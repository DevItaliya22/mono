// SPDX-License-Identifier: MPL-2.0

//! The immediate, content-free half of the durable commit feed.
//! Projection still drains the coalesced records; latency-sensitive consumers
//! only receive a wake and read authoritative state after the Store releases it.
use crate::{Committed, DocumentId, WorkspaceId};
use std::{collections::HashMap, sync::Arc};

/// A host-owned signal, not a derived-state callback. Implementations must only
/// enqueue/coalesce a wake: no I/O, document reads or re-entry into the Store.
pub trait CommitSignal: Send + Sync {
    fn committed(&self, workspace: WorkspaceId, document: DocumentId);
}

#[derive(Default)]
pub(crate) struct Commits {
    records: HashMap<(WorkspaceId, DocumentId), Committed>,
    pub signal: Option<Arc<dyn CommitSignal>>,
}

impl Commits {
    pub fn insert(&mut self, key: (WorkspaceId, DocumentId), committed: Committed) {
        self.records.insert(key, committed);
        if let Some(signal) = &self.signal {
            // An observer cannot turn already durable content into a failed
            // acknowledgement or poison the Store lock.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                signal.committed(key.0, key.1);
            }));
        }
    }

    pub fn take(&mut self) -> Vec<Committed> {
        self.records.drain().map(|(_, record)| record).collect()
    }
}

#[cfg(test)]
mod tests;
