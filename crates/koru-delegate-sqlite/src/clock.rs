// SPDX-License-Identifier: MPL-2.0

//! Where the ledger's times come from.
//!
//! Every row carries an epoch-millisecond time with the unit in its name, and
//! every one of them is read from a [`Clock`] the ledger was opened with.
//! Production opens with [`SystemClock`]; a test opens with a clock it moves
//! by hand (§9: *fake clock, injected crash points*), so "the oldest open
//! publication is forty seconds old" is asserted rather than raced.

use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send {
    /// Milliseconds since the Unix epoch, UTC. Display converts; storage
    /// never does (PRINCIPLES, *timestamps are structural*).
    fn now_ms(&self) -> i64;
}

/// The machine's wall clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }
}
