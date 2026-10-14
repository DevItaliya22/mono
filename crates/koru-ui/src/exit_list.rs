// SPDX-License-Identifier: MPL-2.0

//! Retains removed list items just long enough to explain their departure.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::time::{Duration, Instant};

pub const EXIT_DURATION: Duration = Duration::from_millis(470);

pub struct ExitList<K> {
    exiting: HashMap<K, Instant>,
}

impl<K: Clone + Eq + Hash> ExitList<K> {
    pub fn new() -> Self {
        Self {
            exiting: HashMap::new(),
        }
    }

    pub fn begin(&mut self, before: impl IntoIterator<Item = K>, after: &[K], now: Instant) {
        let live: HashSet<_> = after.iter().cloned().collect();
        for key in before {
            if !live.contains(&key) {
                self.exiting.insert(key, now);
            }
        }
    }

    pub fn progress(&self, key: &K, now: Instant) -> Option<f32> {
        self.exiting.get(key).map(|started| {
            now.saturating_duration_since(*started).as_secs_f32() / EXIT_DURATION.as_secs_f32()
        })
    }

    pub fn maintain(&mut self, now: Instant) -> bool {
        let before = self.exiting.len();
        self.exiting
            .retain(|_, started| now.saturating_duration_since(*started) < EXIT_DURATION);
        before != self.exiting.len()
    }

    pub fn animating(&self) -> bool {
        !self.exiting.is_empty()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.exiting
            .values()
            .map(|started| *started + EXIT_DURATION)
            .min()
    }
}

impl<K: Clone + Eq + Hash> Default for ExitList<K> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_removed_keys_exit_and_expire() {
        let now = Instant::now();
        let mut list = ExitList::new();
        list.begin([1, 2, 3], &[1, 3], now);
        assert!(list.progress(&2, now).is_some());
        assert!(list.progress(&1, now).is_none());
        assert!(list.maintain(now + EXIT_DURATION));
        assert!(!list.animating());
    }
}
