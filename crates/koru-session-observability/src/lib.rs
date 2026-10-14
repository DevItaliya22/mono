// SPDX-License-Identifier: MPL-2.0

//! Content-free diagnostics and fixed-work experience aggregation for Koru
//! terminal sessions.
//!
//! This crate does no I/O, creates no runtime or timer, and requests no frame.
//! Runtime/view transition owners call it and send the returned typed Koru
//! Event through their already-rooted events client off the interaction path.
//! The protocol crate remains free of logging and product dependencies.

mod experience;
mod stats;
mod trace;

pub use experience::{
    EXPERIENCE_INTERVAL_MS, ExperienceAggregator, ExperienceSample, FixedHistogram,
};
pub use stats::{Counter, SessionStats, StatsSnapshot, Timing, TimingSnapshot};
pub use trace::{
    AttachmentDeliveredFact, InputSubmittedFact, SessionEndedFact, SessionReattachedFact,
    SessionStartedFact, SessionTrace, SurfaceOpenedFact, metric_mode, metric_profile_kind,
    metric_provider_kind,
};

#[cfg(test)]
mod tests;
