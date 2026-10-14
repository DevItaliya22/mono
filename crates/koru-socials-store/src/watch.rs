// SPDX-License-Identifier: MPL-2.0

//! Notification-only Store room subscriptions, driven by the caller's worker.

use std::{collections::HashMap, time::Duration};

use koru_api_client::{Client, Connection, Event};
use koru_socials::POST_KIND;
use koru_store_core::WorkspaceId;
use loro::VersionVector;
use uuid::Uuid;

use crate::{CHANNEL_KIND, Error};

/// A local cache may be stale. Reload through the Store before acting on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Manifest(WorkspaceId),
    Post(WorkspaceId, Uuid),
    Channel(WorkspaceId, Uuid),
}

impl Change {
    fn workspace(self) -> WorkspaceId {
        match self {
            Self::Manifest(workspace) | Self::Post(workspace, _) | Self::Channel(workspace, _) => {
                workspace
            }
        }
    }
}

/// One multiplexed socket watches manifests and their current Socials documents.
///
/// Room updates are invalidations, not a local replica: initial catch-up updates
/// intentionally invalidate the cache too. The caller owns reconnection and must
/// refresh a workspace's subscriptions after receiving [`Change::Manifest`].
/// No worker, timer or filesystem access is created here.
pub struct Watch {
    connection: Connection,
    rooms: HashMap<String, Change>,
}

impl Watch {
    /// Subscribe to manifests before listing documents, so changes during that
    /// initial listing are caught by the subscription or its empty-vector catch-up.
    pub fn open(client: &Client, workspaces: &[WorkspaceId]) -> Result<Self, Error> {
        let mut connection =
            Connection::open(&client.store().discovery()).map_err(|_| Error::Unavailable)?;
        connection
            .set_read_timeout(Some(Duration::from_millis(10)))
            .map_err(|_| Error::Unavailable)?;
        let mut watch = Self {
            connection,
            rooms: HashMap::new(),
        };
        for &workspace in workspaces {
            watch.join_manifest(workspace)?;
        }
        for &workspace in workspaces {
            watch.refresh_workspace(client, workspace)?;
        }
        Ok(watch)
    }

    /// Join newly listed Socials documents and leave removed or trashed ones.
    /// A previously unwatched workspace is subscribed before it is listed.
    pub fn refresh_workspace(
        &mut self,
        client: &Client,
        workspace: WorkspaceId,
    ) -> Result<(), Error> {
        self.join_manifest(workspace)?;
        let entries = client
            .store()
            .documents(workspace)
            .map_err(|_| Error::Unavailable)?;
        let desired: HashMap<_, _> = entries
            .into_iter()
            .filter(|entry| entry.trashed_at.is_none())
            .filter_map(|entry| {
                let change = match entry.kind.as_str() {
                    POST_KIND => Change::Post(workspace, entry.id.as_uuid()),
                    CHANNEL_KIND => Change::Channel(workspace, entry.id.as_uuid()),
                    _ => return None,
                };
                Some((format!("document:{}", entry.id.to_storage()), change))
            })
            .collect();
        let removed: Vec<_> = self
            .rooms
            .iter()
            .filter(|(room, change)| {
                change.workspace() == workspace
                    && !matches!(change, Change::Manifest(_))
                    && !desired.contains_key(*room)
            })
            .map(|(room, _)| room.clone())
            .collect();
        for room in removed {
            self.connection
                .leave(&room)
                .map_err(|_| Error::Unavailable)?;
            self.rooms.remove(&room);
        }
        for (room, change) in desired {
            if !self.rooms.contains_key(&room) {
                self.connection
                    .join(&room, &VersionVector::default())
                    .map_err(|_| Error::Unavailable)?;
            }
            self.rooms.insert(room, change);
        }
        Ok(())
    }

    /// Consume at most one protocol event. `None` means timeout or an event
    /// without an invalidation (for example Joined or an empty update).
    /// A refused/broken room or socket requires the caller to reconnect.
    pub fn poll(&mut self) -> Result<Option<Change>, Error> {
        let event = self
            .connection
            .next_event()
            .map_err(|_| Error::Unavailable)?;
        classify(&self.rooms, event)
    }

    fn join_manifest(&mut self, workspace: WorkspaceId) -> Result<(), Error> {
        let room = format!("workspace:{}:manifest", workspace.to_storage());
        if !self.rooms.contains_key(&room) {
            self.connection
                .join(&room, &VersionVector::default())
                .map_err(|_| Error::Unavailable)?;
            self.rooms.insert(room, Change::Manifest(workspace));
        }
        Ok(())
    }
}

fn classify(
    rooms: &HashMap<String, Change>,
    event: Option<Event>,
) -> Result<Option<Change>, Error> {
    match event {
        Some(Event::Updates { room, updates })
            if updates.iter().any(|update| !update.is_empty()) =>
        {
            Ok(rooms.get(&room).copied())
        }
        Some(Event::JoinRefused { .. } | Event::RoomProblem { .. }) => Err(Error::Unavailable),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests;
