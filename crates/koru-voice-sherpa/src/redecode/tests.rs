// SPDX-License-Identifier: MPL-2.0

use super::*;

/// One 20 ms frame at 16 kHz, which is what the paced pass pushes.
const FRAME: usize = 320;

#[test]
fn a_500ms_interval_fires_every_twenty_five_frames() {
    let mut counter = Redecode::every(Duration::from_millis(500));
    assert_eq!(counter.interval_samples(), 8_000);
    let fired: Vec<usize> = (0..100)
        .filter(|_| counter.push(FRAME))
        .enumerate()
        .map(|(i, _)| i)
        .collect();
    // 100 frames is exactly 2 s of audio, so four redecodes and not five.
    assert_eq!(fired.len(), 4);
    assert_eq!(counter.elapsed_audio(), Duration::from_secs(2));
}

#[test]
fn the_remainder_carries_so_the_schedule_does_not_drift() {
    // 300-sample pushes do not divide 8000. Resetting `since` to zero on every
    // fire would lose up to a frame each time and the fifty-first redecode
    // would be most of a second late.
    let mut counter = Redecode::every(Duration::from_millis(500));
    let mut fires = 0;
    for _ in 0..1000 {
        if counter.push(300) {
            fires += 1;
        }
    }
    // 300_000 samples is 18.75 s; 18.75 / 0.5 = 37.5, so 37 redecodes.
    assert_eq!(fires, 37);
}

#[test]
fn one_push_carrying_several_intervals_is_one_redecode() {
    // The redecode is over the *whole* buffer, so firing twice would decode the
    // same audio twice and charge the candidate for it.
    let mut counter = Redecode::every(Duration::from_millis(500));
    assert!(counter.push(16_000 * 5));
    assert_eq!(counter.elapsed_audio(), Duration::from_secs(5));
    // And the next interval still lands where it should rather than
    // immediately.
    assert!(!counter.push(FRAME));
}

#[test]
fn a_disabled_counter_never_fires_but_still_counts_audio() {
    // `never()` is the honest configuration for measuring an offline model
    // *without* the policy — the RTF and accuracy rows are still wanted.
    let mut counter = Redecode::never();
    assert!(!counter.is_enabled());
    for _ in 0..1000 {
        assert!(!counter.push(FRAME));
    }
    assert_eq!(counter.elapsed_audio(), Duration::from_secs(20));
    assert_eq!(counter.total_samples(), 320_000);
}

#[test]
fn a_sub_sample_interval_means_never_rather_than_every_sample() {
    // Rounding up would produce a redecode per frame, which reads as a hang
    // rather than as a misconfiguration.
    let mut counter = Redecode::every(Duration::from_nanos(1));
    assert!(!counter.is_enabled());
    assert!(!counter.push(FRAME));
}

#[test]
fn restart_keeps_the_interval_and_forgets_the_session() {
    let mut counter = Redecode::every(Duration::from_millis(500));
    counter.push(16_000);
    counter.restart();
    assert_eq!(counter.total_samples(), 0);
    assert_eq!(counter.interval_samples(), 8_000);
    // And the first interval of the new session is a full one.
    assert!(!counter.push(7_999));
    assert!(counter.push(1));
}

#[test]
fn the_schedule_is_audio_time_and_owns_no_clock() {
    // Stated as a property of the type rather than as prose: nothing here reads
    // `Instant::now`, so the paced and unpaced passes fire the same number of
    // redecodes over the same utterance. A wall-clock schedule would make the
    // two passes disagree about the final transcript, and the protocol would
    // report that as `drift` and blame the model.
    let mut paced = Redecode::every(Duration::from_millis(500));
    let mut unpaced = Redecode::every(Duration::from_millis(500));
    let mut paced_fires = 0;
    let mut unpaced_fires = 0;
    for _ in 0..250 {
        if paced.push(FRAME) {
            paced_fires += 1;
        }
    }
    if unpaced.push(FRAME * 250) {
        unpaced_fires += 1;
    }
    assert_eq!(paced.elapsed_audio(), unpaced.elapsed_audio());
    // The counts differ — one push cannot fire ten times — which is exactly
    // why the unpaced pass is documented as measuring RTF and not latency.
    assert_eq!(paced_fires, 10);
    assert_eq!(unpaced_fires, 1);
}
