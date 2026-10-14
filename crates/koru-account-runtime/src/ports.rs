// SPDX-License-Identifier: MPL-2.0

//! Time and entropy are explicit inputs to account and enrollment effects.
use rand_core::{OsRng, RngCore};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    fn now_millis(&self) -> Option<i64>;
    fn monotonic(&self) -> Instant;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_millis(&self) -> Option<i64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
    }
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
}

pub trait Entropy: Send + Sync {
    fn fill_bytes(&self, bytes: &mut [u8]);
}

pub struct SystemEntropy;
impl Entropy for SystemEntropy {
    fn fill_bytes(&self, bytes: &mut [u8]) {
        OsRng.fill_bytes(bytes);
    }
}
