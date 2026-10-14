// SPDX-License-Identifier: MPL-2.0

//! Who is subscribed to what, and how a committed update reaches them.
//!
//! ## Room names
//!
//! ```text
//! workspace:<workspace-id>:manifest    the list of a workspace's documents
//! document:<document-id>               one document
//! ```
//!
//! A document room names **only the document**, because a document id is
//! globally unique and a client should not have to know which workspace
//! something is in to open it. Resolving that is the store's job, through the
//! location index in `catalog.db`.
//!
//! ## Broadcast happens after the commit, never before
//!
//! A subscriber receiving an update the store has not yet persisted would see
//! work that a power cut could still erase — and would go on to build on it.
//! The order is: persist, acknowledge the sender, then tell everyone else.
//!
//! ## A sender is never sent its own update
//!
//! It already has it — that is where it came from — and returning it would make
//! every edit a round trip that ends in `frame_is_known`. Subscriptions are
//! keyed by connection so the origin can be excluded.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use koru_store_core::ids::{DocumentId, WorkspaceId};
use tokio::sync::mpsc;

/// What a room is about.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Room {
    Manifest(WorkspaceId),
    Document(DocumentId),
}

impl Room {
    /// Parse a room id as it appears on the wire.
    ///
    /// Unknown shapes are `None` rather than an error type: a room id arrives
    /// from a client, so it is untrusted input and "I do not know that room" is
    /// the whole of the answer.
    pub fn parse(id: &str) -> Option<Self> {
        if let Some(rest) = id.strip_prefix("workspace:")
            && let Some(workspace) = rest.strip_suffix(":manifest")
        {
            return WorkspaceId::parse(workspace).map(Room::Manifest);
        }
        if let Some(document) = id.strip_prefix("document:") {
            return DocumentId::parse(document).map(Room::Document);
        }
        None
    }

    pub fn to_id(&self) -> String {
        match self {
            Room::Manifest(workspace) => format!("workspace:{}:manifest", workspace.to_storage()),
            Room::Document(document) => format!("document:{}", document.to_storage()),
        }
    }
}

/// A connection's handle, for addressing it from elsewhere.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ConnectionId(pub u64);

/// One outgoing frame.
pub type Frame = Vec<u8>;

/// Every subscription the store is currently holding.
#[derive(Default)]
pub struct Rooms {
    inner: Mutex<RoomState>,
    next: AtomicU64,
}

#[derive(Default)]
struct RoomState {
    members: HashMap<Room, HashMap<ConnectionId, mpsc::UnboundedSender<Frame>>>,
    closers: HashMap<ConnectionId, mpsc::UnboundedSender<()>>,
    blocked: HashSet<Room>,
}

impl Rooms {
    /// Only viewer subscriptions, never the Workspace's stored document list.
    pub fn documents(&self) -> Vec<DocumentId> {
        self.inner
            .lock()
            .map(|inner| {
                inner
                    .members
                    .keys()
                    .filter_map(|room| match room {
                        Room::Document(id) => Some(*id),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn next_connection(&self) -> ConnectionId {
        ConnectionId(self.next.fetch_add(1, Ordering::Relaxed))
    }

    pub fn register(&self, who: ConnectionId, closer: mpsc::UnboundedSender<()>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.closers.insert(who, closer);
        }
    }

    /// How many sync sockets are open right now.
    ///
    /// **The store's own answer to "is anybody still using me".** A window
    /// holds one of these for its manifest and one per open document, and
    /// drops them all as it exits; an ordinary HTTP request holds none. So a
    /// client that has already closed its sockets can ask this — through
    /// `POST /v1/shutdown?only_if_idle=true` — and get an answer that counts
    /// every *other* window without counting the asking.
    ///
    /// It is a count of connections rather than of clients, and that is the
    /// honest reading: what matters is whether anything would be cut off.
    pub fn live_connections(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.closers.len())
            .unwrap_or(0)
    }

    /// Join unless workspace deletion has blocked this room. The check and
    /// insertion share one lock, closing the otherwise tiny race between a
    /// successful Store join and the HTTP delete that follows it.
    pub fn join(
        &self,
        room: Room,
        who: ConnectionId,
        sender: mpsc::UnboundedSender<Frame>,
    ) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.blocked.contains(&room) {
            return false;
        }
        inner.members.entry(room).or_default().insert(who, sender);
        true
    }

    pub fn leave(&self, room: &Room, who: ConnectionId) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(members) = inner.members.get_mut(room) {
                members.remove(&who);
                if members.is_empty() {
                    inner.members.remove(room);
                }
            }
        }
    }

    /// Drop every subscription a connection holds. Called when it closes,
    /// however it closes — a room that kept dead senders would grow forever.
    pub fn leave_all(&self, who: ConnectionId) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.members.retain(|_, members| {
                members.remove(&who);
                !members.is_empty()
            });
            inner.closers.remove(&who);
        }
    }

    /// Number of distinct sockets subscribed to any of these rooms.
    pub fn occupants(&self, rooms: &[Room]) -> usize {
        let Ok(inner) = self.inner.lock() else {
            return 0;
        };
        rooms
            .iter()
            .filter_map(|room| inner.members.get(room))
            .flat_map(|members| members.keys().copied())
            .collect::<HashSet<_>>()
            .len()
    }

    /// Refuse future joins and close every socket currently using any room.
    /// Returns the number of distinct connections asked to close.
    pub fn block_and_disconnect(&self, rooms: &[Room]) -> usize {
        let Ok(mut inner) = self.inner.lock() else {
            return 0;
        };
        inner.blocked.extend(rooms.iter().cloned());
        let connections: HashSet<_> = rooms
            .iter()
            .filter_map(|room| inner.members.get(room))
            .flat_map(|members| members.keys().copied())
            .collect();
        inner.members.retain(|_, members| {
            members.retain(|who, _| !connections.contains(who));
            !members.is_empty()
        });
        for who in &connections {
            if let Some(close) = inner.closers.get(who) {
                let _ = close.send(());
            }
        }
        connections.len()
    }

    pub fn unblock(&self, rooms: &[Room]) {
        if let Ok(mut inner) = self.inner.lock() {
            for room in rooms {
                inner.blocked.remove(room);
            }
        }
    }

    /// Send a frame to everyone in a room **except** `except`.
    ///
    /// Returns how many were reached. A send that fails means the receiver has
    /// gone; the subscription is dropped rather than retried, because a
    /// connection that cannot take a frame will not take the next one either.
    pub fn broadcast(&self, room: &Room, except: ConnectionId, frame: Frame) -> usize {
        self.broadcast_many(room, except, &[frame])
    }

    /// Send an ordered set of frames to one stable subscriber set.
    ///
    /// Fragment headers and their chunks must not be interleaved with a join
    /// that would receive only the tail, so the room lock covers the complete
    /// set rather than being reacquired for every frame.
    pub fn broadcast_many(&self, room: &Room, except: ConnectionId, frames: &[Frame]) -> usize {
        let Ok(mut inner) = self.inner.lock() else {
            return 0;
        };
        let Some(members) = inner.members.get_mut(room) else {
            return 0;
        };
        let mut gone = Vec::new();
        let mut reached = 0;
        for (who, sender) in members.iter() {
            if *who == except {
                continue;
            }
            if frames
                .iter()
                .all(|frame| sender.send(frame.clone()).is_ok())
            {
                reached += 1;
            } else {
                gone.push(*who);
            }
        }
        for who in gone {
            members.remove(&who);
        }
        reached
    }

    /// How many connections are in a room. For the debug routes.
    pub fn subscribers(&self, room: &Room) -> usize {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.members.get(room).map(|m| m.len()))
            .unwrap_or(0)
    }

    /// Every room with at least one subscriber, for the debug routes.
    pub fn occupied(&self) -> Vec<(String, usize)> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        let mut out: Vec<(String, usize)> = inner
            .members
            .iter()
            .map(|(room, members)| (room.to_id(), members.len()))
            .collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests;
