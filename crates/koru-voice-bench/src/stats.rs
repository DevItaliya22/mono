// SPDX-License-Identifier: MPL-2.0

//! **Every figure is printed with its `n`, and every quantile and rate with an
//! interval.**
//!
//! No parametric assumption is made anywhere. Latency distributions are not
//! normal, and a confidence interval that assumed they were would be narrower
//! than the truth in exactly the tail the gate is about.
//!
//! ## Quantiles — a distribution-free order-statistic interval
//!
//! For quantile `q` from `n` samples at two-sided confidence `1 − α`, with
//! `z = z_{α/2}`:
//!
//! ```text
//! r = floor( n·q − z·sqrt(n·q·(1−q)) )
//! s = ceil ( n·q + z·sqrt(n·q·(1−q)) ) + 1
//! ```
//!
//! clipped to `[1, n]`; the interval is `[x_(r), x_(s)]`. At 90% confidence and
//! `q = 0.95`: **n = 300 gives ranks 278 and 293**, a usable width; **n = 100
//! gives 91 and 100**, whose upper end *is* the largest observation — which is
//! why the screen stage prints `p95 ≥ x` and is labelled a screen rather than a
//! decision.
//!
//! ## Error rates — a bootstrap over utterances, not a binomial over tokens
//!
//! **Errors cluster within an utterance**: one badly recognised utterance
//! contributes a run of adjacent errors, not independent draws, so a binomial
//! interval on tokens understates the width badly. Resample utterances with
//! replacement 1000 times, recompute the *corpus-level* rate on each resample,
//! take the 2.5th and 97.5th percentiles.
//!
//! The resampling is seeded, so two runs of the harness over one set of
//! utterances print the same interval. An unseeded bootstrap would make a
//! reported interval move between runs of a program over identical data, which
//! is the kind of irreproducibility nobody looks for because nobody expects it.

use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};

use crate::metrics::ErrorCount;

/// `z_{α/2}` for 90% two-sided confidence. The protocol's value for quantile
/// intervals.
pub const Z_90: f64 = 1.645;

/// How many resamples the bootstrap takes.
pub const BOOTSTRAP_RESAMPLES: usize = 1000;

/// A cell whose `n` is below this prints `n/a (n=…)` rather than a number.
pub const MINIMUM_N: usize = 30;

/// An interval, as printed.
#[derive(Copy, Clone, PartialEq, Debug, serde::Serialize)]
pub struct Interval {
    pub low: f64,
    pub high: f64,
}

/// One order-statistic quantile with its interval.
#[derive(Copy, Clone, PartialEq, Debug, serde::Serialize)]
pub struct Quantile {
    pub value: f64,
    pub interval: Interval,
    /// The 1-based ranks the interval came from, so a reader can check the
    /// arithmetic against the protocol's table.
    pub ranks: (usize, usize),
    /// True when the upper rank is `n` — the interval is unbounded above and
    /// the figure can screen but not decide. The screen stage prints `≥ x`.
    pub unbounded_above: bool,
}

/// The `q`th quantile of `values` with a distribution-free interval.
///
/// `values` need not be sorted; it is sorted here. `None` for an empty input,
/// which a caller must print as `n/a` rather than as zero.
pub fn quantile(values: &[f64], q: f64, z: f64) -> Option<Quantile> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN reaches a quantile"));
    let n = sorted.len();
    let nf = n as f64;

    // The order statistic itself: the nearest-rank definition, which is what
    // "the 278th value" means and what the interval's ranks are relative to.
    let rank = ((nf * q).ceil() as usize).clamp(1, n);
    let spread = z * (nf * q * (1.0 - q)).sqrt();
    let low = ((nf * q - spread).floor() as isize).clamp(1, n as isize) as usize;
    let high = (((nf * q + spread).ceil() as isize) + 1).clamp(1, n as isize) as usize;

    Some(Quantile {
        value: sorted[rank - 1],
        interval: Interval {
            low: sorted[low - 1],
            high: sorted[high - 1],
        },
        ranks: (low, high),
        unbounded_above: high >= n,
    })
}

/// What a censored quantile turned out to be.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Censored {
    /// The censored fraction is at or below the threshold; the quantile is an
    /// ordinary order statistic drawn from the uncensored region.
    Estimable(Quantile),
    /// Above the threshold. Not estimable: report `>= largest_uncensored`, print
    /// the fraction, and **the candidate fails that language**.
    NotEstimable {
        largest_uncensored: Option<f64>,
        fraction: f64,
    },
}

/// The censored fraction above which a `TTFUP` p95 is not estimable.
///
/// A recogniser that produces no useful partial on more than one utterance in
/// twenty is not a dictation engine.
pub const CENSORED_LIMIT: f64 = 0.05;

/// A quantile over observations of which some are censored — known only to be
/// at least their recorded value.
///
/// Censored observations are **counted, never dropped**: dropping them would
/// flatter precisely the recognisers that are worst at producing useful
/// partials. They sort above every uncensored one, which is what makes the
/// order statistic honest when the fraction is small.
pub fn censored_quantile(uncensored: &[f64], censored: &[f64], q: f64, z: f64) -> Option<Censored> {
    let total = uncensored.len() + censored.len();
    if total == 0 {
        return None;
    }
    let fraction = censored.len() as f64 / total as f64;
    let largest_uncensored = uncensored
        .iter()
        .copied()
        .fold(None::<f64>, |acc, v| Some(acc.map_or(v, |a: f64| a.max(v))));

    if fraction > CENSORED_LIMIT {
        return Some(Censored::NotEstimable {
            largest_uncensored,
            fraction,
        });
    }

    // Censored values sort above every uncensored one. Their *magnitudes* are
    // lower bounds, so placing them at the top is the only ordering that cannot
    // understate the quantile.
    let mut all: Vec<f64> = uncensored.to_vec();
    all.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let ceiling = all.last().copied().unwrap_or(0.0);
    for value in censored {
        all.push(value.max(ceiling));
    }
    quantile(&all, q, z).map(Censored::Estimable)
}

/// The mean of a set of values, with a bootstrap interval.
///
/// Used for rates the protocol reports as means — revisions per utterance, CPU.
pub fn bootstrap_mean(values: &[f64], seed: u64) -> Option<(f64, Interval)> {
    if values.is_empty() {
        return None;
    }
    let mean = |sample: &[f64]| sample.iter().sum::<f64>() / sample.len() as f64;
    let point = mean(values);
    let interval = bootstrap(values.len(), seed, |indices| {
        let sample: Vec<f64> = indices.iter().map(|&i| values[i]).collect();
        mean(&sample)
    })?;
    Some((point, interval))
}

/// A corpus-level error rate with a bootstrap interval over **utterances**.
///
/// See the module note: resampling tokens instead would understate the width,
/// because errors cluster inside an utterance.
pub fn bootstrap_rate(counts: &[ErrorCount], seed: u64) -> Option<(f64, Interval)> {
    let point = crate::metrics::corpus_rate(counts)?;
    let interval = bootstrap(counts.len(), seed, |indices| {
        let sample: Vec<ErrorCount> = indices.iter().map(|&i| counts[i]).collect();
        crate::metrics::corpus_rate(&sample).unwrap_or(0.0)
    })?;
    Some((point, interval))
}

/// The shared bootstrap: 1000 resamples of `n` indices with replacement, the
/// statistic recomputed on each, the 2.5th and 97.5th percentiles taken.
fn bootstrap(n: usize, seed: u64, statistic: impl Fn(&[usize]) -> f64) -> Option<Interval> {
    if n == 0 {
        return None;
    }
    let mut rng = ChaCha20Rng::seed_from_u64(seed);
    let mut estimates = Vec::with_capacity(BOOTSTRAP_RESAMPLES);
    let mut indices = vec![0usize; n];
    for _ in 0..BOOTSTRAP_RESAMPLES {
        for slot in indices.iter_mut() {
            *slot = below(&mut rng, n);
        }
        estimates.push(statistic(&indices));
    }
    estimates.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let low = percentile(&estimates, 0.025);
    let high = percentile(&estimates, 0.975);
    Some(Interval { low, high })
}

/// A percentile of an already-sorted slice, nearest-rank.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

/// A uniform index below `n`, without modulo bias.
///
/// `next_u64() % n` is biased whenever `n` does not divide `2^64`, and with
/// n = 300 and 300,000 draws per bootstrap that bias is small and entirely
/// avoidable. Rejection sampling costs one extra draw about never.
fn below(rng: &mut ChaCha20Rng, n: usize) -> usize {
    let n = n as u64;
    let limit = u64::MAX - (u64::MAX % n);
    loop {
        let draw = rng.next_u64();
        if draw < limit {
            return (draw % n) as usize;
        }
    }
}

#[cfg(test)]
mod tests;
