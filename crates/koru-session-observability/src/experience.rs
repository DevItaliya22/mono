// SPDX-License-Identifier: MPL-2.0

use koru_events::{TerminalExperienceSummary, bucket};

pub const EXPERIENCE_INTERVAL_MS: u64 = 15 * 60 * 1_000;

// Millisecond ceilings. The last bucket is a deliberate cap: exact large
// values stay in local diagnostics; product metrics only need "5s or worse".
const HISTOGRAM_CEILINGS: [u64; 16] = [
    0, 1, 2, 4, 8, 16, 33, 50, 67, 100, 150, 250, 500, 1_000, 2_500, 5_000,
];

/// A fixed-size histogram: one bounded scan per observation and no allocation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixedHistogram {
    buckets: [u64; HISTOGRAM_CEILINGS.len()],
    samples: u64,
}

impl FixedHistogram {
    #[inline]
    pub fn observe(&mut self, value: u64) {
        let index = HISTOGRAM_CEILINGS
            .iter()
            .position(|ceiling| value <= *ceiling)
            .unwrap_or(HISTOGRAM_CEILINGS.len() - 1);
        self.buckets[index] = self.buckets[index].saturating_add(1);
        self.samples = self.samples.saturating_add(1);
    }

    pub fn samples(&self) -> u64 {
        self.samples
    }

    pub fn p95(&self) -> Option<u64> {
        if self.samples == 0 {
            return None;
        }
        let wanted = self.samples.saturating_mul(95).div_ceil(100);
        let mut seen = 0u64;
        for (index, count) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(*count);
            if seen >= wanted {
                return Some(HISTOGRAM_CEILINGS[index]);
            }
        }
        Some(HISTOGRAM_CEILINGS[HISTOGRAM_CEILINGS.len() - 1])
    }
}

/// One already-visible presentation sample. Callers pass only timings they
/// actually measured; absent remains absent rather than a fabricated zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExperienceSample {
    pub first_present_ms: Option<u64>,
    pub switch_present_ms: Option<u64>,
    pub delta_present_ms: Option<u64>,
    pub render_cpu_ms: Option<u64>,
    pub janky: Option<bool>,
    pub stream_gaps: Option<u64>,
}

/// Local, single-owner aggregation for one visible session surface. It has no
/// clock and cannot schedule work: the view supplies elapsed visible time on
/// an existing presentation sample and publishes a returned payload off-thread.
#[derive(Clone, Debug, Default)]
pub struct ExperienceAggregator {
    visible_ms: u64,
    first_present_ms: Option<u64>,
    switch_present_ms: Option<u64>,
    delta_present: FixedHistogram,
    render_cpu: FixedHistogram,
    sample_count: u64,
    jank_sample_count: u64,
    jank_count: u64,
    stream_gaps: Option<u64>,
}

impl ExperienceAggregator {
    /// Record one existing presentation. A very large elapsed value still
    /// produces at most one summary, preventing a burst after sleep/debugger
    /// suspension; only the remainder contributes to the next interval.
    pub fn presented(
        &mut self,
        visible_elapsed_ms: u64,
        sample: ExperienceSample,
    ) -> Option<TerminalExperienceSummary> {
        self.visible_ms = self.visible_ms.saturating_add(visible_elapsed_ms);
        self.first_present_ms = self.first_present_ms.or(sample.first_present_ms);
        self.switch_present_ms = sample.switch_present_ms.or(self.switch_present_ms);
        if let Some(value) = sample.delta_present_ms {
            self.delta_present.observe(value);
        }
        if let Some(value) = sample.render_cpu_ms {
            self.render_cpu.observe(value);
        }
        self.sample_count = self.sample_count.saturating_add(1);
        if let Some(janky) = sample.janky {
            self.jank_sample_count = self.jank_sample_count.saturating_add(1);
            self.jank_count = self.jank_count.saturating_add(u64::from(janky));
        }
        if let Some(stream_gaps) = sample.stream_gaps {
            self.stream_gaps = Some(
                self.stream_gaps
                    .unwrap_or_default()
                    .saturating_add(stream_gaps),
            );
        }

        if self.visible_ms < EXPERIENCE_INTERVAL_MS {
            return None;
        }
        self.visible_ms %= EXPERIENCE_INTERVAL_MS;
        self.take_summary()
    }

    /// Detach/end flush. No samples means no event.
    pub fn flush(&mut self) -> Option<TerminalExperienceSummary> {
        self.visible_ms = 0;
        self.take_summary()
    }

    fn take_summary(&mut self) -> Option<TerminalExperienceSummary> {
        if self.sample_count == 0 {
            return None;
        }
        let jank_per_thousand = self
            .jank_count
            .saturating_mul(1_000)
            .checked_div(self.jank_sample_count);
        let summary = TerminalExperienceSummary {
            first_present_ms_bucket: self.first_present_ms.map(bucket),
            switch_present_ms_bucket: self.switch_present_ms.map(bucket),
            delta_present_p95_ms_bucket: self.delta_present.p95().map(bucket),
            render_cpu_p95_ms_bucket: self.render_cpu.p95().map(bucket),
            jank_per_thousand_bucket: jank_per_thousand.map(bucket),
            stream_gap_count_bucket: self.stream_gaps.map(bucket),
            sample_count_bucket: Some(bucket(self.sample_count)),
        };
        self.first_present_ms = None;
        self.switch_present_ms = None;
        self.delta_present = FixedHistogram::default();
        self.render_cpu = FixedHistogram::default();
        self.sample_count = 0;
        self.jank_sample_count = 0;
        self.jank_count = 0;
        self.stream_gaps = None;
        Some(summary)
    }
}
