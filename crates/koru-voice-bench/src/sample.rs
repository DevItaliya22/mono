// SPDX-License-Identifier: MPL-2.0

//! **Deterministic, seeded, and reproducible by a second person** — otherwise
//! "comparable numbers" means nothing.
//!
//! ```text
//! seed = u64::from_le_bytes(
//!     sha256("koru-voice-bench" || protocol_rev || source_id || language)[0..8]
//! )
//! ```
//!
//! Then: collect every eligible utterance id, **sort the ids
//! lexicographically**, and take a Fisher–Yates shuffle under ChaCha20 seeded as
//! above.
//!
//! **The sort is load-bearing and is the step somebody skips.** Dataset row
//! order is not stable across downloads or library versions, so a shuffle over
//! the order a directory listing happened to produce is a different sample on
//! every machine — and it would look reproducible, because the seed is fixed.
//!
//! ## Eligibility, and why every exclusion is counted
//!
//! - duration after trimming in `[1.5 s, 20.0 s]` for accuracy, `[1.5 s,
//!   10.0 s]` for latency. Below 1.5 s there is no room for a partial to be
//!   useful before the utterance ends; above the caps the utterance stops being
//!   dictation-shaped.
//! - **the reference contains no digit.** Number formatting is a genuine
//!   confound — references carry digits, recognisers emit either digits or
//!   number words, and the choice is not an error — and writing a bidirectional
//!   number normaliser in eight languages is a project. Excluding is mechanical,
//!   costs nothing, and biases toward shorter less numeric sentences, which is
//!   *toward* dictation. The alternative was silently scoring *2019* against
//!   *twenty nineteen* as three edits.
//! - Common Voice only: at most 3 utterances per `client_id`, so n = 300 draws
//!   from at least 100 speakers.
//! - AISHELL-1 only: at least 30 utterances from each of the 20 test speakers,
//!   so the per-speaker spread has a floor under every cell.
//!
//! Every exclusion is counted and printed, and a language whose exclusion rate
//! exceeds 25% gets a line in the report — at that point the sample is
//! describing a subset nobody chose.

use std::collections::HashMap;
use std::time::Duration;

use koru_voice::VoiceLanguage;
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use sha2::{Digest, Sha256};

/// Which of the two corpora an utterance is being sampled for, which decides
/// the duration cap.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Purpose {
    /// FLEURS and AISHELL-1: read prose, up to 20 s.
    Accuracy,
    /// Common Voice: single crowd-read sentences, up to 10 s.
    Latency,
}

impl Purpose {
    pub fn bounds(self) -> (Duration, Duration) {
        match self {
            Purpose::Accuracy => (Duration::from_millis(1500), Duration::from_secs(20)),
            Purpose::Latency => (Duration::from_millis(1500), Duration::from_secs(10)),
        }
    }
}

/// Why an utterance was not sampled.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Excluded {
    TooShort,
    TooLong,
    /// The reference carries a digit. See the module note.
    Numeric,
    /// Common Voice's three-per-speaker cap.
    SpeakerCap,
    /// The audio would not read, or trimmed to nothing.
    Unreadable,
}

impl Excluded {
    pub fn as_str(self) -> &'static str {
        match self {
            Excluded::TooShort => "too_short",
            Excluded::TooLong => "too_long",
            Excluded::Numeric => "numeric",
            Excluded::SpeakerCap => "speaker_cap",
            Excluded::Unreadable => "unreadable",
        }
    }
}

/// The exclusion rate above which the report says the sample describes a subset
/// nobody chose.
pub const EXCLUSION_WARNING: f64 = 0.25;

/// The seed for one `(protocol_rev, source, language)`.
///
/// The concatenation is unambiguous because every part is length-delimited by a
/// separator that cannot appear in a source id or a tag — a plain concatenation
/// of `"fleurs"` and `"en"` and of `"fleur"` and `"sen"` would be the same
/// bytes and therefore the same sample.
pub fn seed(protocol_rev: u32, source_id: &str, language: VoiceLanguage) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"koru-voice-bench");
    hasher.update(b"\x1f");
    hasher.update(protocol_rev.to_string().as_bytes());
    hasher.update(b"\x1f");
    hasher.update(source_id.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(language.tag().as_bytes());
    let digest = hasher.finalize();
    u64::from_le_bytes(digest[0..8].try_into().expect("sha256 is 32 bytes"))
}

/// Whether a reference contains a digit, in any script.
///
/// `char::is_numeric` rather than `is_ascii_digit`: Arabic-Indic digits are
/// digits, and so are the fullwidth forms a Japanese reference may carry.
pub fn contains_digit(reference: &str) -> bool {
    reference.chars().any(char::is_numeric)
}

/// One candidate for the sample, before eligibility is applied.
#[derive(Clone, PartialEq, Debug)]
pub struct Eligible {
    pub id: String,
    pub reference: String,
    /// **Trimmed** duration. See [`crate::trim`] — the file's own duration would
    /// make two utterances with the same speech score differently.
    pub duration: Duration,
    /// `None` where the source carries no speaker id, which is FLEURS.
    pub speaker: Option<String>,
}

/// What a sampling run decided.
#[derive(Clone, PartialEq, Debug)]
pub struct Sampled {
    /// The chosen ids, in the order they will be measured.
    pub ids: Vec<String>,
    /// Every exclusion, counted by reason.
    pub excluded: HashMap<Excluded, usize>,
    /// How many were considered at all.
    pub considered: usize,
    /// How many distinct speakers the sample drew from, where the source knows.
    pub speakers: usize,
}

impl Sampled {
    /// The total excluded.
    pub fn excluded_total(&self) -> usize {
        self.excluded.values().sum()
    }

    /// The exclusion rate, for the 25% line in the report.
    pub fn exclusion_rate(&self) -> f64 {
        if self.considered == 0 {
            return 0.0;
        }
        self.excluded_total() as f64 / self.considered as f64
    }

    /// Whether the sample reached the size it was asked for.
    pub fn short_by(&self, target: usize) -> usize {
        target.saturating_sub(self.ids.len())
    }
}

/// Draw a sample.
///
/// `per_speaker_cap` is `Some(3)` for Common Voice and `None` elsewhere; the
/// AISHELL-1 floor is applied by [`with_speaker_floor`] afterwards, because a
/// floor and a cap are different operations and doing both in one pass makes
/// neither readable.
pub fn draw(
    mut candidates: Vec<Eligible>,
    purpose: Purpose,
    target: usize,
    per_speaker_cap: Option<usize>,
    seed: u64,
) -> Sampled {
    let considered = candidates.len();
    let (min, max) = purpose.bounds();
    let mut excluded: HashMap<Excluded, usize> = HashMap::new();
    let mut count = |reason: Excluded| {
        *excluded.entry(reason).or_insert(0) += 1;
    };

    candidates.retain(|c| {
        if c.duration.is_zero() {
            count(Excluded::Unreadable);
            return false;
        }
        if c.duration < min {
            count(Excluded::TooShort);
            return false;
        }
        if c.duration > max {
            count(Excluded::TooLong);
            return false;
        }
        if contains_digit(&c.reference) {
            count(Excluded::Numeric);
            return false;
        }
        true
    });

    // **The sort.** See the module note: without it the seed is fixed and the
    // sample is not.
    candidates.sort_by(|a, b| a.id.cmp(&b.id));

    let order = shuffled(candidates.len(), seed);

    let mut per_speaker: HashMap<String, usize> = HashMap::new();
    let mut ids = Vec::with_capacity(target);
    let mut speakers: HashMap<String, ()> = HashMap::new();
    for index in order {
        if ids.len() == target {
            break;
        }
        let candidate = &candidates[index];
        if let (Some(cap), Some(speaker)) = (per_speaker_cap, candidate.speaker.as_ref()) {
            let taken = per_speaker.entry(speaker.clone()).or_insert(0);
            if *taken >= cap {
                count(Excluded::SpeakerCap);
                continue;
            }
            *taken += 1;
        }
        if let Some(speaker) = candidate.speaker.as_ref() {
            speakers.insert(speaker.clone(), ());
        }
        ids.push(candidate.id.clone());
    }

    Sampled {
        ids,
        excluded,
        considered,
        speakers: speakers.len(),
    }
}

/// AISHELL-1's floor: at least `floor` utterances from each speaker, the
/// remainder drawn at random.
///
/// Applied over the same seeded order, so the result is still reproducible. The
/// point of the floor is the per-speaker CER spread in Table 4: the
/// decision-relevant question about a broad-coverage-tier language is not "is it
/// worse on average" but "is it uniformly a little worse, or catastrophic for
/// some speakers", and a mean cannot tell those apart.
pub fn with_speaker_floor(
    candidates: &[Eligible],
    already: &Sampled,
    floor: usize,
    seed: u64,
) -> Sampled {
    let mut by_speaker: HashMap<&str, Vec<&Eligible>> = HashMap::new();
    for candidate in candidates {
        if let Some(speaker) = candidate.speaker.as_deref() {
            by_speaker.entry(speaker).or_default().push(candidate);
        }
    }

    let mut chosen: Vec<String> = Vec::new();
    let mut taken: HashMap<&str, usize> = HashMap::new();
    let mut speakers: Vec<&str> = by_speaker.keys().copied().collect();
    speakers.sort_unstable();

    for speaker in &speakers {
        let mut pool = by_speaker[speaker].clone();
        pool.sort_by(|a, b| a.id.cmp(&b.id));
        // A seed per speaker, derived from the run's, so adding a speaker to the
        // corpus does not reshuffle everybody else's draw.
        let order = shuffled(pool.len(), seed ^ hash_str(speaker));
        for index in order.into_iter().take(floor) {
            chosen.push(pool[index].id.clone());
            *taken.entry(speaker).or_insert(0) += 1;
        }
    }

    // Then the remainder, from the seeded order the ordinary draw produced,
    // skipping what the floor already took.
    for id in &already.ids {
        if chosen.len() >= already.ids.len().max(chosen.len()) {
            break;
        }
        if !chosen.contains(id) {
            chosen.push(id.clone());
        }
    }

    Sampled {
        ids: chosen,
        excluded: already.excluded.clone(),
        considered: already.considered,
        speakers: taken.len(),
    }
}

/// A Fisher–Yates shuffle of `0..n` under ChaCha20.
pub fn shuffled(n: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    if n < 2 {
        return order;
    }
    let mut rng = ChaCha20Rng::seed_from_u64(seed);
    // Downward, which is the version that is uniform. The upward variant with
    // an unrestricted index is the classic wrong one and produces n^n orderings
    // over n! permutations.
    for i in (1..n).rev() {
        let j = below(&mut rng, i + 1);
        order.swap(i, j);
    }
    order
}

/// A uniform index below `n`, without modulo bias. See [`crate::stats`].
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

/// A stable hash of a speaker id, for the per-speaker sub-seed.
///
/// SHA-256 rather than `DefaultHasher`: `std`'s hasher is explicitly not
/// stable across releases, and a seed that changed with the Rust version would
/// make two runs of the same harness draw different samples.
fn hash_str(value: &str) -> u64 {
    let digest = Sha256::digest(value.as_bytes());
    u64::from_le_bytes(digest[0..8].try_into().expect("sha256 is 32 bytes"))
}

#[cfg(test)]
mod tests;
