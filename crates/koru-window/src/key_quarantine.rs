// SPDX-License-Identifier: MPL-2.0

//! Physical keys held across a window focus handoff.
//!
//! Windows may deliver a key repeat to the window that receives focus rather
//! than the one that consumed the original press.  Search is the conspicuous
//! case: the global `K` can arrive in the new input, and Enter can arrive in the
//! document search just opened.  Quarantine is deliberately physical-key based
//! so keyboard layout and text composition do not change its meaning.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use winit::keyboard::PhysicalKey;

const MISSED_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct KeyQuarantine {
    keys: HashMap<PhysicalKey, Instant>,
}

impl KeyQuarantine {
    pub fn suppress(&mut self, key: PhysicalKey, now: Instant) {
        self.keys.insert(key, now + MISSED_RELEASE_TIMEOUT);
    }

    pub fn suppress_all(&mut self, keys: impl IntoIterator<Item = PhysicalKey>, now: Instant) {
        for key in keys {
            self.suppress(key, now);
        }
    }

    /// Whether this press belongs to the chord/focus handoff rather than the
    /// receiving view. Expired entries are removed so a missing OS release can
    /// never permanently disable a key.
    pub fn consumes(&mut self, key: PhysicalKey, now: Instant) -> bool {
        self.keys.retain(|_, until| *until > now);
        self.keys.contains_key(&key)
    }

    pub fn released(&mut self, key: PhysicalKey) {
        self.keys.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    fn key() -> PhysicalKey {
        PhysicalKey::Code(KeyCode::Enter)
    }

    #[test]
    fn repeats_are_consumed_until_release() {
        let now = Instant::now();
        let mut q = KeyQuarantine::default();
        q.suppress(key(), now);
        assert!(q.consumes(key(), now));
        assert!(q.consumes(key(), now + Duration::from_millis(50)));
        q.released(key());
        assert!(!q.consumes(key(), now + Duration::from_millis(51)));
    }

    #[test]
    fn a_missed_release_expires() {
        let now = Instant::now();
        let mut q = KeyQuarantine::default();
        q.suppress(key(), now);
        assert!(!q.consumes(key(), now + MISSED_RELEASE_TIMEOUT));
    }
}
