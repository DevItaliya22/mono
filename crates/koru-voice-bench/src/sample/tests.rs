// SPDX-License-Identifier: MPL-2.0

use super::*;

use koru_voice::VoiceLanguage;

fn candidate(id: &str, seconds: f64, reference: &str, speaker: Option<&str>) -> Eligible {
    Eligible {
        id: id.to_owned(),
        reference: reference.to_owned(),
        duration: Duration::from_secs_f64(seconds),
        speaker: speaker.map(str::to_owned),
    }
}

fn plain(n: usize) -> Vec<Eligible> {
    (0..n)
        .map(|i| candidate(&format!("utt-{i:04}"), 4.0, "a sentence with words", None))
        .collect()
}

#[test]
fn the_seed_is_stable_and_differs_per_language_and_source() {
    let en = seed(1, "fleurs", VoiceLanguage::En);
    assert_eq!(en, seed(1, "fleurs", VoiceLanguage::En));
    assert_ne!(en, seed(1, "fleurs", VoiceLanguage::De));
    assert_ne!(en, seed(1, "commonvoice", VoiceLanguage::En));
    assert_ne!(en, seed(2, "fleurs", VoiceLanguage::En));
}

#[test]
fn the_seeds_parts_cannot_run_together() {
    // A plain concatenation of "fleur"+"sen" and "fleurs"+"en" would be the
    // same bytes and therefore the same sample.
    assert_ne!(
        seed(1, "fleurs", VoiceLanguage::En),
        seed(1, "fleur", VoiceLanguage::En)
    );
    assert_ne!(
        seed(1, "fleursen", VoiceLanguage::En),
        seed(1, "fleurs", VoiceLanguage::En)
    );
}

#[test]
fn the_same_seed_draws_the_same_sample_from_the_same_ids() {
    let a = draw(plain(200), Purpose::Accuracy, 50, None, 42);
    let b = draw(plain(200), Purpose::Accuracy, 50, None, 42);
    assert_eq!(a.ids, b.ids);
    assert_eq!(a.ids.len(), 50);
}

#[test]
fn the_sample_does_not_depend_on_the_order_the_candidates_arrived_in() {
    // **The load-bearing one.** Dataset row order is not stable across
    // downloads or library versions, so a shuffle over whatever order a
    // directory listing produced is a different sample on every machine — and
    // it would look reproducible, because the seed is fixed.
    let forwards = plain(200);
    let mut backwards = forwards.clone();
    backwards.reverse();
    let a = draw(forwards, Purpose::Accuracy, 50, None, 42);
    let b = draw(backwards, Purpose::Accuracy, 50, None, 42);
    assert_eq!(a.ids, b.ids);
}

#[test]
fn a_different_seed_draws_a_different_sample() {
    let a = draw(plain(200), Purpose::Accuracy, 50, None, 1);
    let b = draw(plain(200), Purpose::Accuracy, 50, None, 2);
    assert_ne!(a.ids, b.ids);
}

#[test]
fn the_duration_bounds_differ_between_the_two_purposes() {
    // FLEURS utterances are read news prose about twelve seconds long;
    // dictation is two to eight seconds of one person's own words.
    let pool = vec![
        candidate("a", 1.0, "short", None),
        candidate("b", 5.0, "fine", None),
        candidate("c", 15.0, "long", None),
    ];
    // Compared as a set: `draw` answers in the seeded shuffle's order, which is
    // the order the utterances will be measured in and is not alphabetical.
    let mut accuracy = draw(pool.clone(), Purpose::Accuracy, 10, None, 1).ids;
    accuracy.sort();
    assert_eq!(accuracy, ["b", "c"]);
    let latency = draw(pool, Purpose::Latency, 10, None, 1);
    assert_eq!(latency.ids, ["b"]);
}

#[test]
fn a_reference_with_a_digit_is_excluded_in_any_script() {
    // 2019 against "twenty nineteen" is three edits and not an error, and
    // writing a bidirectional number normaliser in eight languages is a
    // project.
    assert!(contains_digit("in 2019 we"));
    assert!(contains_digit("في ٢٠١٩"));
    assert!(contains_digit("２０１９年"));
    assert!(!contains_digit("in twenty nineteen we"));

    let pool = vec![
        candidate("a", 4.0, "no numbers here", None),
        candidate("b", 4.0, "in 2019 we shipped", None),
    ];
    let sampled = draw(pool, Purpose::Accuracy, 10, None, 1);
    assert_eq!(sampled.ids, ["a"]);
    assert_eq!(sampled.excluded[&Excluded::Numeric], 1);
}

#[test]
fn an_unreadable_utterance_is_counted_rather_than_ignored() {
    let pool = vec![candidate("a", 0.0, "trimmed to nothing", None)];
    let sampled = draw(pool, Purpose::Accuracy, 10, None, 1);
    assert!(sampled.ids.is_empty());
    assert_eq!(sampled.excluded[&Excluded::Unreadable], 1);
}

#[test]
fn the_common_voice_cap_makes_n_300_draw_from_at_least_a_hundred_speakers() {
    // Three per `client_id`. Built here with 40 speakers and 10 utterances
    // each, so the cap is what limits the draw rather than the pool.
    let pool: Vec<Eligible> = (0..40)
        .flat_map(|s| {
            (0..10).map(move |u| {
                candidate(
                    &format!("s{s:02}-u{u:02}"),
                    4.0,
                    "a sentence",
                    Some(&format!("speaker-{s:02}")),
                )
            })
        })
        .collect();
    let sampled = draw(pool, Purpose::Latency, 120, Some(3), 9);
    assert_eq!(sampled.ids.len(), 120);
    assert_eq!(sampled.speakers, 40);
    // Every speaker is at their cap, which is what "at least n/3 speakers"
    // means in the limit.
    assert!(sampled.excluded.contains_key(&Excluded::SpeakerCap));
}

#[test]
fn the_exclusion_rate_is_reportable_and_the_warning_line_is_at_a_quarter() {
    let pool = vec![
        candidate("a", 4.0, "fine", None),
        candidate("b", 0.5, "short", None),
        candidate("c", 4.0, "in 2019", None),
        candidate("d", 4.0, "fine too", None),
    ];
    let sampled = draw(pool, Purpose::Accuracy, 10, None, 1);
    assert_eq!(sampled.considered, 4);
    assert_eq!(sampled.excluded_total(), 2);
    assert!(sampled.exclusion_rate() > EXCLUSION_WARNING);
}

#[test]
fn a_short_sample_reports_how_short_rather_than_silently_returning_fewer() {
    // Common Voice does not split Portuguese by variety, so `pt-BR` can come up
    // short; the protocol says the shortfall is reported and European
    // Portuguese is not substituted.
    let sampled = draw(plain(10), Purpose::Accuracy, 300, None, 1);
    assert_eq!(sampled.ids.len(), 10);
    assert_eq!(sampled.short_by(300), 290);
}

#[test]
fn the_shuffle_is_a_permutation_and_not_an_identity() {
    let order = shuffled(100, 5);
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..100).collect::<Vec<_>>());
    assert_ne!(order, (0..100).collect::<Vec<_>>());
}

#[test]
fn the_shuffle_handles_the_degenerate_sizes() {
    assert!(shuffled(0, 1).is_empty());
    assert_eq!(shuffled(1, 1), vec![0]);
}

#[test]
fn the_aishell_floor_takes_at_least_n_from_every_speaker() {
    // The point of the floor is the per-speaker CER spread: the
    // decision-relevant question about a broad-coverage-tier language is not
    // "is it worse on average" but "is it uniformly a little worse, or
    // catastrophic for some speakers".
    let pool: Vec<Eligible> = (0..20)
        .flat_map(|s| {
            (0..60).map(move |u| {
                candidate(
                    &format!("S{s:04}W{u:04}"),
                    4.0,
                    "一个句子",
                    Some(&format!("S{s:04}")),
                )
            })
        })
        .collect();
    let base = draw(pool.clone(), Purpose::Accuracy, 600, None, 11);
    let floored = with_speaker_floor(&pool, &base, 30, 11);
    assert_eq!(floored.speakers, 20);
    let mut per_speaker = std::collections::HashMap::new();
    for id in &floored.ids {
        *per_speaker.entry(&id[..5]).or_insert(0usize) += 1;
    }
    for (speaker, count) in &per_speaker {
        assert!(*count >= 30, "{speaker} contributed only {count}");
    }
}
