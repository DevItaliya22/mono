// SPDX-License-Identifier: MPL-2.0

//! A link with no socket, for testing what a session does when the answer is
//! not "fine".
//!
//! **This is the reason [`Link`] is a trait.** The interesting behaviour of a
//! sync session is all in the failures — an acknowledgement that refuses, a
//! connection that dies with edits in flight, a room that asks to be rejoined —
//! and none of those can be provoked reliably against a real store. Here each
//! is one line.
//!
//! It records what was sent so a test can assert on the *bytes*, which is what
//! makes "the update was re-sent" checkable by importing them into a fresh
//! replica rather than by counting calls. A count passes while both sides are
//! empty; a replica that shows the row does not.

use std::sync::Mutex;

use koru_api_client::{Ack, Event};
use koru_store_core::errors::Result;
use loro_protocol::{BatchId, UpdateStatusCode};

use super::{Inbound, Link, LinkState, Waker};

struct Recorded {
    joined: Vec<(String, loro::VersionVector)>,
    sent: Vec<(String, Vec<u8>)>,
    left: Vec<String>,
    /// Waiting to be handed to the next `take`.
    queued: Vec<Inbound>,
    next_batch: u64,
    state: LinkState,
    waker: Option<Waker>,
}

impl Default for Recorded {
    fn default() -> Self {
        Self {
            joined: Vec::new(),
            sent: Vec::new(),
            left: Vec::new(),
            queued: Vec::new(),
            next_batch: 0,
            state: LinkState::Live,
            waker: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct FakeLink {
    inner: Mutex<Recorded>,
}

impl FakeLink {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.inner.lock().expect("a test never poisons this")
    }

    // ---------------------------------------------------------- what a test says

    /// The store answers a batch. `durable` mirrors the real client: `true`
    /// only for a status of `Ok`.
    pub(crate) fn acks(&self, batch: BatchId, status: UpdateStatusCode) {
        let durable = status == UpdateStatusCode::Ok;
        self.push(Event::Acked {
            room: String::new(),
            ack: Ack {
                batch,
                durable,
                status,
            },
        });
    }

    /// Another client's edit arrives.
    pub(crate) fn delivers(&self, updates: Vec<Vec<u8>>) {
        self.push(Event::Updates {
            room: String::new(),
            updates,
        });
    }

    pub(crate) fn push(&self, event: Event) {
        let mut inner = self.lock();
        inner.queued.push(Inbound::Event(event));
        // The real link wakes whoever will collect, so this one does too —
        // otherwise a test could not tell a wake path that works from one that
        // was never wired.
        if let Some(waker) = &inner.waker {
            waker.wake();
        }
    }

    /// The socket is in this state from now on.
    ///
    /// Separate from [`drops_and_returns`](Self::drops_and_returns), which
    /// queues the *events* a reconnect produces: this sets what
    /// [`Link::state`] answers, which is the half a caller reading health —
    /// rather than draining frames — sees. A real link moves both together, so
    /// a test that needs one usually needs the other.
    pub(crate) fn is(&self, state: LinkState) {
        self.lock().state = state;
    }

    /// The socket died, and then a fresh one came up — which is what a store
    /// being restarted looks like from here.
    pub(crate) fn drops_and_returns(&self) {
        let mut inner = self.lock();
        inner.queued.push(Inbound::Lost {
            why: "the store went away".to_owned(),
        });
        inner.queued.push(Inbound::Ready);
    }

    // --------------------------------------------------------- what a test reads

    /// Every update handed over, in order, as bytes.
    pub(crate) fn sent(&self) -> Vec<Vec<u8>> {
        self.lock().sent.iter().map(|(_, b)| b.clone()).collect()
    }

    /// The version vector each `join` carried. A rejoin has to name what this
    /// replica actually holds, so this is the assertion that a reconnect asked
    /// for the right thing.
    pub(crate) fn joins(&self) -> Vec<loro::VersionVector> {
        self.lock().joined.iter().map(|(_, v)| v.clone()).collect()
    }

    pub(crate) fn left(&self) -> Vec<String> {
        self.lock().left.clone()
    }
}

impl Link for FakeLink {
    fn join(&self, room: &str, version: &loro::VersionVector) -> Result<()> {
        self.lock().joined.push((room.to_owned(), version.clone()));
        Ok(())
    }

    fn send_update(&self, room: &str, bytes: Vec<u8>) -> Result<BatchId> {
        let mut inner = self.lock();
        inner.next_batch += 1;
        let batch = BatchId(inner.next_batch.to_le_bytes());
        inner.sent.push((room.to_owned(), bytes));
        Ok(batch)
    }

    fn leave(&self, room: &str) -> Result<()> {
        self.lock().left.push(room.to_owned());
        Ok(())
    }

    fn take(&self, max: usize) -> Result<Vec<Inbound>> {
        let mut inner = self.lock();
        // The real link rearms here, so this one must too — a fake that is
        // easier to satisfy than the thing it stands in for is a test that
        // passes for the wrong reason.
        if let Some(waker) = &inner.waker {
            waker.rearm();
        }
        let n = max.min(inner.queued.len());
        let taken: Vec<_> = inner.queued.drain(..n).collect();
        if !inner.queued.is_empty()
            && let Some(waker) = &inner.waker
        {
            waker.wake();
        }
        Ok(taken)
    }

    fn state(&self) -> LinkState {
        self.lock().state.clone()
    }

    fn set_waker(&self, waker: Waker) {
        let mut inner = self.lock();
        inner.waker = Some(waker.clone());
        if !inner.queued.is_empty() {
            waker.wake();
        }
    }
}
