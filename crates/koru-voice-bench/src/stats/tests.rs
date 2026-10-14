// SPDX-License-Identifier: MPL-2.0

use super::*;

fn one_to(n: usize) -> Vec<f64> {
    (1..=n).map(|i| i as f64).collect()
}

#[test]
fn n_300_at_p95_gives_the_protocols_ranks() {
    // The number the protocol states, and the reason n = 300 is the decision
    // run's size: ranks 278 and 293 is a usable width.
    let q = quantile(&one_to(300), 0.95, Z_90).expect("300 values");
    assert_eq!(q.ranks, (278, 293));
    assert!(!q.unbounded_above);
    assert_eq!(q.value, 285.0);
}

#[test]
fn n_100_at_p95_has_an_upper_end_that_is_the_largest_observation() {
    // Which is why the screen can only screen: the interval is unbounded above
    // and the figure prints as `>= x`.
    let q = quantile(&one_to(100), 0.95, Z_90).expect("100 values");
    assert_eq!(q.ranks, (91, 100));
    assert!(q.unbounded_above);
}

#[test]
fn a_quantile_sorts_its_input_rather_than_trusting_it() {
    let shuffled = vec![5.0, 1.0, 4.0, 2.0, 3.0];
    let q = quantile(&shuffled, 0.5, Z_90).expect("five values");
    assert_eq!(q.value, 3.0);
}

#[test]
fn an_empty_input_has_no_quantile_rather_than_a_zero() {
    assert!(quantile(&[], 0.95, Z_90).is_none());
}

#[test]
fn a_censored_observation_is_counted_in_the_quantile_and_never_dropped() {
    // Dropping them would flatter precisely the recognisers that are worst at
    // producing useful partials.
    let uncensored = one_to(100);
    let censored = vec![50.0, 60.0, 70.0]; // ~2.9%, below the limit
    let result =
        censored_quantile(&uncensored, &censored, 0.95, Z_90).expect("there are observations");
    let Censored::Estimable(with) = result else {
        panic!("2.9% is below the 5% limit");
    };
    let without = quantile(&uncensored, 0.95, Z_90).expect("100 values");
    // The censored three sort above every uncensored one, so the p95 moves up.
    assert!(with.value >= without.value);
}

#[test]
fn above_five_percent_the_p95_is_not_estimable_and_the_candidate_fails() {
    // A recogniser that produces no useful partial on more than one utterance
    // in twenty is not a dictation engine.
    let uncensored = one_to(90);
    let censored = vec![0.0; 10]; // 10%
    let result = censored_quantile(&uncensored, &censored, 0.95, Z_90).expect("observations");
    match result {
        Censored::NotEstimable {
            largest_uncensored,
            fraction,
        } => {
            assert_eq!(largest_uncensored, Some(90.0));
            assert!((fraction - 0.1).abs() < 1e-12);
        }
        Censored::Estimable(_) => panic!("10% censored is above the limit"),
    }
}

#[test]
fn exactly_five_percent_is_still_estimable() {
    // The rule is "≤ 5%", and a boundary that behaved as though it were "< 5%"
    // would fail a candidate the protocol passes.
    let result = censored_quantile(&one_to(95), &vec![0.0; 5], 0.95, Z_90).expect("observations");
    assert!(matches!(result, Censored::Estimable(_)));
}

#[test]
fn every_censored_and_the_answer_says_so_rather_than_dividing_by_nothing() {
    let result = censored_quantile(&[], &vec![100.0; 10], 0.95, Z_90).expect("observations");
    match result {
        Censored::NotEstimable {
            largest_uncensored,
            fraction,
        } => {
            assert_eq!(largest_uncensored, None);
            assert_eq!(fraction, 1.0);
        }
        Censored::Estimable(_) => panic!("all censored is not estimable"),
    }
}

#[test]
fn the_bootstrap_is_seeded_so_two_runs_print_one_interval() {
    // An unseeded bootstrap makes a reported interval move between runs of a
    // program over identical data, which is the kind of irreproducibility
    // nobody looks for because nobody expects it.
    let values = one_to(50);
    let a = bootstrap_mean(&values, 7).expect("values");
    let b = bootstrap_mean(&values, 7).expect("values");
    assert_eq!(a, b);
    let c = bootstrap_mean(&values, 8).expect("values");
    assert_ne!(a.1, c.1);
}

#[test]
fn the_bootstrap_interval_brackets_the_point_estimate() {
    let values = one_to(200);
    let (point, interval) = bootstrap_mean(&values, 1).expect("values");
    assert!(
        interval.low <= point && point <= interval.high,
        "{interval:?}"
    );
}

#[test]
fn an_error_rate_bootstrap_resamples_utterances_rather_than_tokens() {
    // The interval it produces has to be wider than a binomial one over tokens,
    // because errors cluster inside an utterance. Constructed so that is
    // visible: one utterance is entirely wrong and the rest are perfect.
    let mut counts = vec![
        crate::metrics::ErrorCount {
            edits: 0,
            reference_units: 10
        };
        50
    ];
    counts[0] = crate::metrics::ErrorCount {
        edits: 10,
        reference_units: 10,
    };
    let (rate, interval) = bootstrap_rate(&counts, 3).expect("counts");
    assert!((rate - 0.02).abs() < 1e-12, "{rate}");
    // A resample that happens to draw the bad utterance several times, or none,
    // moves the rate a long way — which is the width a token-level binomial
    // would have hidden.
    assert!(interval.low < rate);
    assert!(interval.high > rate);
    assert!(interval.high - interval.low > 0.02, "{interval:?}");
}

#[test]
fn a_rate_bootstrap_over_no_counts_is_absent() {
    assert!(bootstrap_rate(&[], 1).is_none());
    assert!(bootstrap_mean(&[], 1).is_none());
}

#[test]
fn the_minimum_n_is_the_protocols() {
    // A cell whose n is below this prints `n/a (n=…)` rather than a number.
    assert_eq!(MINIMUM_N, 30);
    assert_eq!(BOOTSTRAP_RESAMPLES, 1000);
    assert_eq!(CENSORED_LIMIT, 0.05);
}
