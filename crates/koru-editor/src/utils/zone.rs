// SPDX-License-Identifier: MPL-2.0

//! Editor compatibility edge for the shared timezone-rule mechanism.
//!
//! The database and OS detection moved to `koru-time-zone`; this module keeps
//! existing editor call sites stable while making the dependency direction
//! explicit. Domain decisions stay in `utils::when` and the due commit.

pub use koru_time_zone::{IanaZoneRules, SystemZone, ZoneError};

#[cfg(test)]
mod tests;
