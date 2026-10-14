// SPDX-License-Identifier: MPL-2.0

use super::*;

const SR: usize = 16_000;

fn silence(seconds: f64) -> Vec<f32> {
    vec![0.0; (SR as f64 * seconds) as usize]
}

/// A loud block, well above the floor.
fn loud(seconds: f64) -> Vec<f32> {
    (0..(SR as f64 * seconds) as usize)
        .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
        .collect()
}

fn joined(parts: &[Vec<f32>]) -> Vec<f32> {
    parts.iter().flatten().copied().collect()
}

#[test]
fn digital_silence_is_negative_infinity_rather_than_a_special_case() {
    assert_eq!(window_dbfs(&[0.0; 320]), f64::NEG_INFINITY);
    assert!(window_dbfs(&[0.5; 320]) > FLOOR_DBFS);
}

#[test]
fn a_full_scale_square_wave_is_zero_dbfs() {
    let db = window_dbfs(&[1.0, -1.0, 1.0, -1.0]);
    assert!(db.abs() < 1e-9, "{db}");
}

#[test]
fn the_floor_is_where_the_protocol_puts_it() {
    // -45 dBFS is an RMS of about 0.00562. Either side of it decides whether a
    // window is speech, and every latency figure moves with it.
    let just_below = vec![0.005_f32; 320];
    let just_above = vec![0.007_f32; 320];
    assert!(window_dbfs(&just_below) < FLOOR_DBFS);
    assert!(window_dbfs(&just_above) > FLOOR_DBFS);
}

#[test]
fn two_hundred_milliseconds_of_the_original_lead_is_put_back() {
    let audio = joined(&[silence(1.0), loud(1.0), silence(1.0)]);
    let trimmed = trim(&audio);
    assert_eq!(trimmed.lead_in, LEAD_IN);
    // 200 ms of lead plus a second of speech, to within one 20 ms window.
    let seconds = trimmed.duration().as_secs_f64();
    assert!((1.18..=1.23).contains(&seconds), "{seconds}");
}

#[test]
fn a_short_lead_is_reported_rather_than_padded() {
    // A recording's silence is its room and its microphone's noise floor;
    // feeding a VAD generated zeros and then real audio is a step change
    // nothing in the world produces.
    let audio = joined(&[silence(0.05), loud(1.0)]);
    let trimmed = trim(&audio);
    assert!(trimmed.lead_in < LEAD_IN);
    assert!(trimmed.lead_in <= Duration::from_millis(60));
}

#[test]
fn leading_silence_does_not_reach_the_duration_every_figure_divides_by() {
    // The point of the whole module: two utterances with the same speech and
    // different leading silence must score the same.
    let short_lead = trim(&joined(&[silence(0.2), loud(2.0), silence(0.1)]));
    let long_lead = trim(&joined(&[silence(3.0), loud(2.0), silence(0.1)]));
    let difference = short_lead.duration().as_secs_f64() - long_lead.duration().as_secs_f64();
    assert!(difference.abs() < 0.05, "{difference}");
}

#[test]
fn an_all_silent_file_trims_to_nothing_and_is_then_excluded() {
    // A file that is silence all the way through is a corpus problem and has to
    // be counted as one rather than measured.
    let trimmed = trim(&silence(4.0));
    assert!(trimmed.is_empty());
    assert_eq!(trimmed.duration(), Duration::ZERO);
}

#[test]
fn trailing_silence_goes_entirely() {
    // Unlike the lead, none of it is put back: nothing downstream needs it, and
    // every millisecond of it would land in `t_final − t_last_push`.
    let with_tail = trim(&joined(&[silence(0.3), loud(1.0), silence(5.0)]));
    let without = trim(&joined(&[silence(0.3), loud(1.0)]));
    let difference = with_tail.duration().as_secs_f64() - without.duration().as_secs_f64();
    assert!(difference.abs() < 0.05, "{difference}");
}

#[test]
fn the_placeholder_fixture_trims_to_something_eligible() {
    // The fixture exists so the harness can be run end to end, and the first
    // thing that would stop it is an utterance the eligibility filter throws
    // away.
    let root = crate::corpus::SmokeFixture::default_root();
    for language in koru_voice::VoiceLanguage::ALL {
        let samples = crate::wav::read_wav(&root.join(format!("{}.wav", language.tag())))
            .expect("the fixture reads");
        let trimmed = trim(&samples);
        let seconds = trimmed.duration().as_secs_f64();
        assert!(
            (1.5..=20.0).contains(&seconds),
            "{} trims to {seconds:.2} s, outside the accuracy corpus bounds",
            language.tag()
        );
    }
}
