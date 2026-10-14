// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn capacity_is_rounded_up_to_a_power_of_two() {
    assert_eq!(Ring::new(100).capacity(), 128);
    assert_eq!(Ring::new(128).capacity(), 128);
    assert_eq!(Ring::new(1).capacity(), 2);
}

#[test]
fn what_goes_in_comes_out_in_order() {
    let ring = Ring::new(8);
    for step in 0..5 {
        ring.push(step as f32);
    }
    let mut out = Vec::new();
    ring.drain(&mut out);
    assert_eq!(out, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
    assert_eq!(ring.dropped(), 0);

    // A second drain with nothing new answers nothing, rather than replaying.
    out.clear();
    ring.drain(&mut out);
    assert!(out.is_empty());
}

/// **The plan's overrun test.** A full ring drops the *oldest*, keeps the
/// newest, moves the counter, and — the half that matters — the pushes return
/// rather than waiting for a consumer that is not there.
#[test]
fn an_overrun_drops_the_oldest_and_moves_the_counter() {
    let ring = Ring::new(8);
    let capacity = ring.capacity() as i32;
    let overrun = 5;

    let started = std::time::Instant::now();
    for step in 0..capacity + overrun {
        ring.push(step as f32);
    }
    // Not a wall-clock budget on the machine: the assertion is that a
    // hundred-odd pushes with no consumer at all completed, which a blocking
    // ring could not do however fast the machine was. The generous bound is
    // there so a loaded CI box does not fail a test about semantics.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "pushing into a full ring waited"
    );

    let mut out = Vec::new();
    ring.drain(&mut out);
    // **The count appears at the drain**, because that is where it is exact —
    // the producer cannot know whether the consumer got to a slot first, and a
    // producer-side count over-reports. See `Ring::push`.
    assert_eq!(ring.dropped(), overrun as u64);
    assert_eq!(out.len(), capacity as usize);
    // The newest `capacity` samples, which is to say the beginning is gone.
    assert_eq!(out.first(), Some(&(overrun as f32)));
    assert_eq!(out.last(), Some(&((capacity + overrun - 1) as f32)));
}

/// The consumer clamps rather than reading slots the producer has already
/// overwritten — without it a lapped ring hands back a wrapped buffer, which
/// sounds like a burst of noise instead of a gap.
#[test]
fn a_lapped_reader_skips_forward_instead_of_replaying_garbage() {
    let ring = Ring::new(4);
    for step in 0..(ring.capacity() * 3) {
        ring.push(step as f32);
    }
    let mut out = Vec::new();
    ring.drain(&mut out);
    assert_eq!(out, vec![8.0, 9.0, 10.0, 11.0]);
    assert_eq!(ring.dropped(), 8);
}

/// `Effect::Release` clears the ring, so the next dictation does not begin with
/// the tail of the one before it — and that is a decision rather than a fault,
/// so it must not be reported as an overrun.
#[test]
fn clearing_throws_away_what_is_waiting_without_counting_it() {
    let ring = Ring::new(8);
    for step in 0..4 {
        ring.push(step as f32);
    }
    assert_eq!(ring.pending(), 4);
    ring.clear();
    assert_eq!(ring.pending(), 0);
    let mut out = Vec::new();
    ring.drain(&mut out);
    assert!(out.is_empty());
    assert_eq!(
        ring.dropped(),
        0,
        "a deliberate discard was called an overrun"
    );
}

/// **The producer and the consumer really are on two threads**, which is the
/// only arrangement the atomics are for — and the accounting is exact:
/// everything pushed was either drained or counted as dropped, with no sample
/// on both sides of that sum.
///
/// This is the test that found the original bug. A producer-side counter passed
/// every single-threaded case above and over-reported by 623 in 50,000 here,
/// because it counted slots the consumer had already read.
#[test]
fn a_producer_thread_and_a_consumer_thread_account_for_every_sample() {
    const PRODUCED: u64 = 50_000;
    let ring = std::sync::Arc::new(Ring::new(1024));
    let producer = {
        let ring = ring.clone();
        std::thread::spawn(move || {
            for step in 0..PRODUCED {
                ring.push(step as f32);
            }
        })
    };
    let mut seen = 0u64;
    let mut out = Vec::new();
    while !producer.is_finished() || ring.pending() > 0 {
        out.clear();
        ring.drain(&mut out);
        seen += out.len() as u64;
    }
    producer.join().unwrap();
    out.clear();
    ring.drain(&mut out);
    seen += out.len() as u64;
    assert_eq!(
        seen + ring.dropped(),
        PRODUCED,
        "{seen} drained and {} dropped does not account for {PRODUCED}",
        ring.dropped()
    );
}
