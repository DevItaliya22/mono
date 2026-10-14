// SPDX-License-Identifier: MPL-2.0

//! **A bounded lossy ring between the device callback and the inference
//! thread.**
//!
//! One producer — the `cpal` data callback — and one consumer — the inference
//! thread. The rule that shapes every line of this file is
//! [ADR-249](../../../docs/decisions/ADR-226-250.md)'s: *the real-time
//! obligation belongs to the ring-buffer write, not to the engine.* So
//! [`Ring::push`] **allocates nothing, locks nothing and never waits**, and
//! when the consumer has fallen behind it overwrites the oldest sample rather
//! than blocking the audio device.
//!
//! **The drop is the producer's act and the count is the consumer's
//! observation**, which is not where anybody puts it the first time. See
//! [`Ring::push`] for the measurement that moved it.
//!
//! ## Why dropping the oldest, and not the newest
//!
//! Because the newest audio is the audio a person is still speaking. A ring
//! that refused the newest sample would transcribe the beginning of a sentence
//! and lose its end, which is the half that carries the meaning — and it would
//! do it silently, because nothing downstream can tell a dropped tail from a
//! person who stopped talking. Dropping the oldest loses the beginning of a
//! backlog that only exists because the engine is behind, and
//! [`Ring::dropped`] is what says so out loud in `koru-voice-worker status`.
//!
//! ## Why the slots are atomics rather than a `Box<[f32]>`
//!
//! Because the producer overwrites a slot the consumer may be reading, which is
//! a data race — undefined behaviour in Rust, not merely a torn sample. Storing
//! `f32::to_bits` in `AtomicU32` makes the race defined: the consumer reads
//! either the old value or the new one, and a single sample from the wrong side
//! of a wrap is inaudible against the drop that caused it. The alternative — a
//! mutex — would put a lock on the device callback, which is the one thing this
//! file exists to avoid.
//!
//! The consumer never trusts its own read cursor: it clamps to
//! `write - capacity` first, so an overwritten range is skipped rather than
//! replayed as garbage. That clamp is what makes the producer's unconditional
//! write correct.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// A fixed-capacity lossy SPSC ring of `f32` samples.
///
/// Capacity is rounded up to a power of two so the index is a mask rather than
/// a division — the callback runs a few thousand times a second and a modulo
/// per sample is a cost with nothing to buy.
#[derive(Debug)]
pub struct Ring {
    slots: Box<[AtomicU32]>,
    mask: u64,
    /// Written only by the producer, read by both.
    write: AtomicU64,
    /// Written only by the consumer, read by both.
    read: AtomicU64,
    dropped: AtomicU64,
}

impl Ring {
    /// A ring holding at least `capacity` samples.
    ///
    /// The default the worker uses is [`Ring::DEFAULT_CAPACITY`]; a test builds
    /// a tiny one so that overrun is reachable in a few hundred pushes rather
    /// than half a million.
    pub fn new(capacity: usize) -> Ring {
        let capacity = capacity.max(2).next_power_of_two();
        let slots = (0..capacity).map(|_| AtomicU32::new(0)).collect::<Vec<_>>();
        Ring {
            slots: slots.into_boxed_slice(),
            mask: capacity as u64 - 1,
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// **Two seconds at [`koru_voice::SAMPLE_RATE`]**, and the number is a
    /// judgement rather than a measurement.
    ///
    /// The inference thread drains every few milliseconds while listening, so
    /// two seconds is not a latency budget — it is how long the engine may
    /// stall before a person loses words. A whole-utterance decoder asked to
    /// finalise can hold the thread for a good fraction of a second, and a
    /// laptop that has just swapped can hold it longer; anything shorter turns
    /// an ordinary hiccup into a gap in the transcript. It costs 128 KiB.
    pub const DEFAULT_CAPACITY: usize = 32_768;

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// **The producer's half. Called from the device callback.**
    ///
    /// Two atomic stores and nothing else: no allocation, no lock, and no
    /// branch that can wait. It always succeeds — when the ring is full the
    /// oldest sample is simply overwritten — because a device callback that
    /// waited would glitch the whole system's audio rather than only ours.
    ///
    /// **It does not count the drop, and that is not an oversight.** The
    /// obvious version compares the write cursor to the consumer's published
    /// read cursor and increments when they are a whole capacity apart. That
    /// number is an *upper bound*, not a count: the consumer may have read the
    /// slot in the moment between the load and the store, in which case nothing
    /// was lost and the counter says otherwise. Measured on this ring with one
    /// producer and one consumer at 50,000 samples, it over-reported by 623 —
    /// so `seen + dropped` did not equal `produced`, and the diagnostic could
    /// not be reconciled with anything. The consumer knows exactly what it
    /// skipped; see [`Ring::drain`].
    pub fn push(&self, sample: f32) {
        // Relaxed is right for our own cursor: this is the only writer.
        let write = self.write.load(Ordering::Relaxed);
        self.slots[(write & self.mask) as usize].store(sample.to_bits(), Ordering::Relaxed);
        // Release, so a consumer that sees this index also sees the sample.
        self.write.store(write.wrapping_add(1), Ordering::Release);
    }

    /// Every sample the producer has written and this consumer has not taken,
    /// appended to `out`.
    ///
    /// **The clamp is load-bearing, and it is also where an overrun is
    /// counted.** When the producer has lapped us the oldest slots hold newer
    /// audio than our cursor claims, so the cursor is moved forward to the
    /// oldest sample that still exists — reading them anyway would replay a
    /// wrapped buffer as a burst of noise, which is worse than the gap it
    /// stands in for. The distance it moves is exactly what was lost, which is
    /// why the count lives here rather than in [`Ring::push`]: the producer
    /// cannot know whether the consumer got there first, and the consumer
    /// cannot be wrong about what it skipped.
    pub fn drain(&self, out: &mut Vec<f32>) {
        let write = self.write.load(Ordering::Acquire);
        let mut read = self.read.load(Ordering::Relaxed);
        let capacity = self.slots.len() as u64;
        let behind = write.wrapping_sub(read);
        if behind > capacity {
            self.dropped.fetch_add(behind - capacity, Ordering::Relaxed);
            read = write.wrapping_sub(capacity);
        }
        while read != write {
            let bits = self.slots[(read & self.mask) as usize].load(Ordering::Relaxed);
            out.push(f32::from_bits(bits));
            read = read.wrapping_add(1);
        }
        self.read.store(read, Ordering::Release);
    }

    /// Throw away whatever is waiting, without reading it.
    ///
    /// What [`koru_voice::Effect::Release`] means for the ring: the session is
    /// over and the audio still in flight belongs to it, so the next dictation
    /// must not begin with the tail of the last one.
    ///
    /// **This is not an overrun and does not move [`Ring::dropped`].** Audio
    /// discarded because a dictation ended is a decision; audio lost because
    /// nothing drained the ring is a fault, and a counter that folded the two
    /// together would report a fault after every cancelled session.
    pub fn clear(&self) {
        let write = self.write.load(Ordering::Acquire);
        self.read.store(write, Ordering::Release);
    }

    /// **Samples the callback overwrote because nothing had drained them.**
    ///
    /// Observed by the consumer rather than the producer — see [`Ring::drain`]
    /// — so it is exact: `drained + dropped` is always what was pushed.
    ///
    /// Cumulative for the life of the process and never reset, because
    /// `koru-voice-worker status` is read after the fact: a counter that a new session
    /// zeroed would answer *"none"* to somebody investigating the dictation
    /// before it. Divide by [`koru_voice::SAMPLE_RATE`] for seconds lost.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// How many samples are waiting. A diagnostic; the drain does not use it.
    pub fn pending(&self) -> u64 {
        let write = self.write.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Relaxed);
        write.wrapping_sub(read).min(self.slots.len() as u64)
    }
}

#[cfg(test)]
mod tests;
