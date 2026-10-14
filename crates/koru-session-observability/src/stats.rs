// SPDX-License-Identifier: MPL-2.0

use std::array;
use std::sync::atomic::{AtomicU64, Ordering};

const COUNTER_COUNT: usize = 17;
const TIMING_COUNT: usize = 6;

/// Fixed counter slots from the Shell System observability contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Counter {
    InputBytes,
    OutputBytes,
    ParsedBatches,
    EmulatorDamagedRows,
    ViewportRowsSent,
    SnapshotPageRows,
    SnapshotPageBytes,
    RowsShaped,
    CachedRowHits,
    RowsAppendedToScene,
    Deltas,
    DroppedSubscribers,
    GappedSubscribers,
    QueueHighWater,
    UiWakes,
    RenderRuns,
    StreamGaps,
}

/// Exact local diagnostic timings. Telemetry receives only the separately
/// reviewed buckets produced by `ExperienceAggregator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Timing {
    FirstPresent,
    SessionSwitchToPresent,
    DeltaToPresent,
    RenderCpu,
    SnapshotEncode,
    ScrollbackPage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimingSnapshot {
    pub samples: u64,
    pub total_micros: u64,
    pub max_micros: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatsSnapshot {
    counters: [u64; COUNTER_COUNT],
    timings: [TimingSnapshot; TIMING_COUNT],
}

impl StatsSnapshot {
    pub fn counter(&self, counter: Counter) -> u64 {
        self.counters[counter as usize]
    }

    pub fn timing(&self, timing: Timing) -> TimingSnapshot {
        self.timings[timing as usize]
    }
}

/// Atomic so the PTY worker, encoder, and view may share one instance. When
/// disabled, record calls return before touching an atomic. Construction and
/// recording allocate no content strings and cannot perform I/O or wake work.
pub struct SessionStats {
    enabled: bool,
    counters: [AtomicU64; COUNTER_COUNT],
    timing_samples: [AtomicU64; TIMING_COUNT],
    timing_total_micros: [AtomicU64; TIMING_COUNT],
    timing_max_micros: [AtomicU64; TIMING_COUNT],
}

impl SessionStats {
    pub fn from_env() -> Self {
        Self::new(koru_env::switch(koru_env::name::KORU_SESSION_STATS))
    }

    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            counters: array::from_fn(|_| AtomicU64::new(0)),
            timing_samples: array::from_fn(|_| AtomicU64::new(0)),
            timing_total_micros: array::from_fn(|_| AtomicU64::new(0)),
            timing_max_micros: array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[inline]
    pub fn add(&self, counter: Counter, amount: u64) {
        if !self.enabled {
            return;
        }
        self.counters[counter as usize].fetch_add(amount, Ordering::Relaxed);
    }

    #[inline]
    pub fn observe_high_water(&self, counter: Counter, value: u64) {
        if !self.enabled {
            return;
        }
        self.counters[counter as usize].fetch_max(value, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_timing_micros(&self, timing: Timing, micros: u64) {
        if !self.enabled {
            return;
        }
        let index = timing as usize;
        self.timing_samples[index].fetch_add(1, Ordering::Relaxed);
        self.timing_total_micros[index].fetch_add(micros, Ordering::Relaxed);
        self.timing_max_micros[index].fetch_max(micros, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Option<StatsSnapshot> {
        self.enabled.then(|| StatsSnapshot {
            counters: array::from_fn(|index| self.counters[index].load(Ordering::Relaxed)),
            timings: array::from_fn(|index| TimingSnapshot {
                samples: self.timing_samples[index].load(Ordering::Relaxed),
                total_micros: self.timing_total_micros[index].load(Ordering::Relaxed),
                max_micros: self.timing_max_micros[index].load(Ordering::Relaxed),
            }),
        })
    }
}
