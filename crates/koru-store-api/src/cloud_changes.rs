// SPDX-License-Identifier: MPL-2.0

//! Content-free, coalesced wakes for live Cloud datasets and local viewers.
//! No durable backlog: reconnect/open reads state before relying on a wake.
use koru_store_core::{DocumentId, ProfileId, WorkspaceId};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dataset {
    Workspace(WorkspaceId),
    Profile(ProfileId),
    Document(WorkspaceId, DocumentId),
    /// Local viewers can watch an ID before its bytes have arrived. This is
    /// only an invalidation; every subsequent read still checks Store scope.
    DocumentView(DocumentId),
}

#[derive(Clone, Default)]
pub struct Changes(Arc<Mutex<HashMap<Dataset, Weak<watch::Sender<u64>>>>>);

impl koru_store_core::commit_signal::CommitSignal for Changes {
    fn committed(&self, workspace: WorkspaceId, document: DocumentId) {
        // Wake only. Never read a document here: this runs under the Store's
        // commit lock, before projection workers have drained their feed.
        self.changed(Dataset::Document(workspace, document));
        self.changed(Dataset::DocumentView(document));
        self.changed(Dataset::Workspace(workspace));
    }
}

pub struct Subscription {
    // Keeping the sender here bounds the registry by live subscriptions.
    _owner: Arc<watch::Sender<u64>>,
    receiver: watch::Receiver<u64>,
}

impl Changes {
    pub fn subscribe(&self, dataset: Dataset) -> Subscription {
        let mut slots = self.0.lock().unwrap_or_else(|e| e.into_inner());
        slots.retain(|_, sender| sender.strong_count() > 0);
        let sender = slots
            .get(&dataset)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let sender = Arc::new(watch::channel(0).0);
                slots.insert(dataset, Arc::downgrade(&sender));
                sender
            });
        Subscription {
            receiver: sender.subscribe(),
            _owner: sender,
        }
    }

    pub fn changed(&self, dataset: Dataset) {
        if let Some(sender) = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&dataset)
            .and_then(Weak::upgrade)
        {
            sender.send_modify(|generation| *generation = generation.wrapping_add(1));
        }
    }

    /// Profile API mutations already use the peer replication wake. Their
    /// callers do not all carry a unit id, so only live Profile datasets wake.
    pub fn profiles_changed(&self) {
        let slots = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for (dataset, sender) in slots.iter() {
            if matches!(dataset, Dataset::Profile(_))
                && let Some(sender) = sender.upgrade()
            {
                sender.send_modify(|generation| *generation = generation.wrapping_add(1));
            }
        }
    }
}

impl Subscription {
    pub fn begin_pass(&mut self) {
        self.receiver.borrow_and_update();
    }
    pub fn pending(&self) -> bool {
        self.receiver.has_changed().unwrap_or(false)
    }
    pub async fn changed(&mut self) {
        let _ = self.receiver.changed().await;
    }
}

#[cfg(test)]
mod tests;
