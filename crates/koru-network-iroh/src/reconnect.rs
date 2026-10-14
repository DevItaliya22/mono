// SPDX-License-Identifier: MPL-2.0

//! When a desired peer is dialed again, and when its refusal is worth a line.
//!
//! Pure policy, with the clock passed in: the runtime owns the timers and the
//! connections, this module owns the numbers. Two schedules, chosen by why
//! the last connection ended:
//!
//! - **Transient** — a dial that failed, or a connection that died on first
//!   use for no reason this side can name. Quarter of a second doubling to
//!   thirty: a network blip between compatible devices heals in about the
//!   time it takes to notice it.
//! - **Incompatible** — a connection that died on first use while the peer
//!   advertises a peer protocol this build refuses to sync with. Nothing
//!   about that changes on a timescale of seconds: one side has to install
//!   an update. Thirty seconds doubling to ten minutes. The updated side
//!   still reconnects at once, because inbound connections are never held
//!   back and a changed build record resets the schedule.
//!
//! The runtime holds **one** pending retry per peer; scheduling a retry
//! replaces the previous one rather than adding a second timer beside it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use koru_device::{BuildPayload, DeviceId};

/// How long a connection must live to count as useful. A peer that accepts
/// the handshake and dies inside this window — an old build refusing what it
/// does not understand, a runtime restarting underneath — is a failure and
/// accumulates backoff; a connection that proved itself and then dropped is
/// a blip and starts fresh. Five seconds comfortably covers the first
/// catalog exchange that proves a connection works.
pub(crate) const EARLY_DEATH: Duration = Duration::from_secs(5);

/// The incompatible-protocol schedule, indexed by consecutive early deaths.
///
/// Thirty seconds first, because the trust gossip that tells the older side
/// to update has already crossed on the connection that just died, and a
/// person who updates the other device is still looking at it a minute
/// later. Ten minutes last, because an update is a deliberate act that can
/// be days away, and a pair of devices left in that state for a week then
/// costs 144 handshakes a day rather than one every few milliseconds.
pub(crate) const INCOMPATIBLE_BACKOFF: [Duration; 5] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
    Duration::from_secs(300),
    Duration::from_secs(600),
];

/// Why the next dial is being scheduled. Decides the schedule, nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetryReason {
    Transient,
    Incompatible,
}

/// The wait before the next dial after `failures` consecutive failures.
pub(crate) fn retry_delay(reason: RetryReason, failures: u8, device: DeviceId) -> Duration {
    match reason {
        RetryReason::Transient => reconnect_delay(failures, device),
        RetryReason::Incompatible => incompatible_delay(failures, device),
    }
}

/// The transient schedule: 250 ms doubling, capped at 30 s, plus up to a
/// quarter more seeded by the device so a mesh does not redial in lockstep.
pub(crate) fn reconnect_delay(failures: u8, device: DeviceId) -> Duration {
    let exponent = u32::from(failures.min(7));
    let base_ms = (250u64.saturating_mul(1u64 << exponent)).min(30_000);
    let uuid = device.as_uuid();
    let bytes = uuid.as_bytes();
    let seed = u16::from_be_bytes([bytes[0], bytes[1]]) as u64;
    let jitter = seed % (base_ms / 4 + 1);
    Duration::from_millis((base_ms + jitter).min(30_000))
}

/// The incompatible schedule: [`INCOMPATIBLE_BACKOFF`], less up to a tenth
/// seeded by the device *and* the attempt. Subtracted rather than added, so
/// the ten-minute cap is a cap and still jitters.
pub(crate) fn incompatible_delay(failures: u8, device: DeviceId) -> Duration {
    let step = usize::from(failures).min(INCOMPATIBLE_BACKOFF.len() - 1);
    let base = INCOMPATIBLE_BACKOFF[step];
    let bytes = device.as_uuid().into_bytes();
    let seed = u64::from(u16::from_be_bytes([bytes[2], bytes[3]]))
        .wrapping_mul(31)
        .wrapping_add(u64::from(failures).wrapping_mul(0x9e37));
    let base_ms = base.as_millis() as u64;
    Duration::from_millis(base_ms - seed % (base_ms / 10 + 1))
}

/// What to say about a peer the runtime is refusing to sync with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IncompatibleReport {
    /// First sight in this state, or of a peer build not seen before.
    Entered,
    /// The backoff window since the last line has passed; say it again.
    StillRefused,
    /// Already said within this window.
    Quiet,
}

#[derive(Clone, Debug)]
struct IncompatibleState {
    peer_build: Option<BuildPayload>,
    next_report_at: Instant,
}

/// Which peers are in the incompatible state and when each may next be
/// reported: the `incompatible_protocol` error is news once when the state is
/// entered and then once per backoff window, never once per connection.
#[derive(Debug, Default)]
pub(crate) struct IncompatibleReports {
    peers: BTreeMap<DeviceId, IncompatibleState>,
}

impl IncompatibleReports {
    /// A connection to a refused peer came up.
    pub(crate) fn connected(
        &mut self,
        device: DeviceId,
        peer_build: Option<&BuildPayload>,
        now: Instant,
    ) -> IncompatibleReport {
        match self.peers.get(&device) {
            Some(state) if state.peer_build.as_ref() == peer_build => IncompatibleReport::Quiet,
            _ => {
                self.peers.insert(
                    device,
                    IncompatibleState {
                        peer_build: peer_build.cloned(),
                        next_report_at: now + window(INCOMPATIBLE_BACKOFF[0]),
                    },
                );
                IncompatibleReport::Entered
            }
        }
    }

    /// A connection to a refused peer died on first use and the next dial
    /// waits `delay`.
    pub(crate) fn backing_off(
        &mut self,
        device: DeviceId,
        peer_build: Option<&BuildPayload>,
        now: Instant,
        delay: Duration,
    ) -> IncompatibleReport {
        let Some(state) = self.peers.get_mut(&device) else {
            self.peers.insert(
                device,
                IncompatibleState {
                    peer_build: peer_build.cloned(),
                    next_report_at: now + window(delay),
                },
            );
            return IncompatibleReport::Entered;
        };
        if now < state.next_report_at {
            return IncompatibleReport::Quiet;
        }
        state.next_report_at = now + window(delay);
        IncompatibleReport::StillRefused
    }

    /// The peer left the state — it speaks our protocol now, or is no longer
    /// linked. True when it had been in it, which is when that is news.
    pub(crate) fn clear(&mut self, device: DeviceId) -> bool {
        self.peers.remove(&device).is_some()
    }

    pub(crate) fn devices(&self) -> Vec<DeviceId> {
        self.peers.keys().copied().collect()
    }
}

/// The report window for a backoff of `delay`: a tenth short of it, the most
/// [`incompatible_delay`] jitters, so a retry that came back early still
/// gets its line.
fn window(delay: Duration) -> Duration {
    delay - delay / 10
}

#[cfg(test)]
mod tests;
