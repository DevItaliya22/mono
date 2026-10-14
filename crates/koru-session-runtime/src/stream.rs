// SPDX-License-Identifier: MPL-2.0

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Why a non-blocking stream read produced no item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamReadError {
    Empty,
    Closed,
}

struct State<T> {
    items: VecDeque<T>,
    closed: bool,
    receiver_alive: bool,
}

struct Shared<T> {
    capacity: usize,
    state: Mutex<State<T>>,
    ready: Condvar,
}

/// The receiving half of a fixed-capacity in-process stream.
///
/// Producers never wait for a client. A domain owner either coalesces or emits
/// an explicit gap when [`Queue::try_push`] reports a full queue.
pub struct BoundedStream<T> {
    shared: Arc<Shared<T>>,
}

/// A producer-independent way for an outer transport to stop a blocked stream
/// bridge when its client connection disappears.
#[derive(Clone)]
pub struct StreamCancel<T> {
    shared: Arc<Shared<T>>,
}

pub(crate) struct Queue<T> {
    shared: Arc<Shared<T>>,
}

#[derive(Debug)]
pub(crate) enum PushError<T> {
    Full(T),
    Closed(T),
}

impl<T> Queue<T> {
    pub(crate) fn new(capacity: usize) -> (Self, BoundedStream<T>) {
        assert!(capacity > 0, "a bounded stream needs capacity");
        let shared = Arc::new(Shared {
            capacity,
            state: Mutex::new(State {
                items: VecDeque::with_capacity(capacity),
                closed: false,
                receiver_alive: true,
            }),
            ready: Condvar::new(),
        });
        (
            Self {
                shared: Arc::clone(&shared),
            },
            BoundedStream { shared },
        )
    }

    pub(crate) fn try_push(&self, item: T) -> Result<usize, PushError<T>> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if state.closed || !state.receiver_alive {
            return Err(PushError::Closed(item));
        }
        if state.items.len() == self.shared.capacity {
            return Err(PushError::Full(item));
        }
        state.items.push_back(item);
        let depth = state.items.len();
        drop(state);
        self.shared.ready.notify_one();
        Ok(depth)
    }

    pub(crate) fn replace_with(&self, item: T) -> Result<(), T> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if state.closed || !state.receiver_alive {
            return Err(item);
        }
        state.items.clear();
        state.items.push_back(item);
        drop(state);
        self.shared.ready.notify_one();
        Ok(())
    }

    pub(crate) fn clear(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .items
            .clear();
    }

    pub(crate) fn first(&self) -> Option<T>
    where
        T: Clone,
    {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .items
            .front()
            .cloned()
    }

    pub(crate) fn is_alive(&self) -> bool {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        state.receiver_alive && !state.closed
    }

    pub(crate) fn close(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .closed = true;
        self.shared.ready.notify_all();
    }
}

impl<T> Clone for Queue<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> BoundedStream<T> {
    pub fn cancel_handle(&self) -> StreamCancel<T> {
        StreamCancel {
            shared: Arc::clone(&self.shared),
        }
    }

    pub fn try_recv(&self) -> Result<T, StreamReadError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        state.items.pop_front().ok_or_else(|| {
            if state.closed {
                StreamReadError::Closed
            } else {
                StreamReadError::Empty
            }
        })
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, StreamReadError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if state.items.is_empty() && !state.closed {
            let (next, _) = self
                .shared
                .ready
                .wait_timeout(state, timeout)
                .unwrap_or_else(|held| held.into_inner());
            state = next;
        }
        state.items.pop_front().ok_or_else(|| {
            if state.closed {
                StreamReadError::Closed
            } else {
                StreamReadError::Empty
            }
        })
    }

    /// Wait until an item arrives or the producer/outer transport closes the
    /// stream. There is no periodic wake on an idle terminal.
    pub fn recv(&self) -> Result<T, StreamReadError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        while state.items.is_empty() && !state.closed && state.receiver_alive {
            state = self
                .shared
                .ready
                .wait(state)
                .unwrap_or_else(|held| held.into_inner());
        }
        state.items.pop_front().ok_or(StreamReadError::Closed)
    }
}

impl<T> StreamCancel<T> {
    pub fn cancel(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .receiver_alive = false;
        self.shared.ready.notify_all();
    }
}

impl<T> Drop for BoundedStream<T> {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .receiver_alive = false;
        self.shared.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_queue_is_explicit_and_can_be_replaced_by_one_gap() {
        let (queue, stream) = Queue::new(2);
        assert_eq!(queue.try_push(1).unwrap(), 1);
        assert_eq!(queue.try_push(2).unwrap(), 2);
        assert!(matches!(queue.try_push(3), Err(PushError::Full(3))));
        queue.replace_with(99).unwrap();
        assert_eq!(stream.try_recv(), Ok(99));
        assert_eq!(stream.try_recv(), Err(StreamReadError::Empty));
    }

    #[test]
    fn dropping_a_receiver_closes_its_producer_without_waiting() {
        let (queue, stream) = Queue::new(1);
        drop(stream);
        assert!(matches!(queue.try_push(1), Err(PushError::Closed(1))));
    }

    #[test]
    fn outer_transport_cancellation_wakes_a_blocked_receiver_without_a_timer() {
        let (_queue, stream) = Queue::<u8>::new(1);
        let cancel = stream.cancel_handle();
        let join = std::thread::spawn(move || stream.recv());
        cancel.cancel();
        assert_eq!(join.join().unwrap(), Err(StreamReadError::Closed));
    }
}
