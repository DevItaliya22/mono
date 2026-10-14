// SPDX-License-Identifier: MPL-2.0

//! **The metrics, and the two that a naive implementation gets wrong.**
//!
//! Everything here operates on *normalised* strings. Nothing in this module
//! normalises: [`crate::normalise::Normaliser`] is the one door, and a function
//! here that also normalised would be a second answer to what a string is.
//!
//! ## First useful partial, which is the metric the product lives on
//!
//! A streaming recogniser revises case, punctuation, whitespace and numerals as
//! it goes, so a raw byte-prefix test reports *no useful partial* on exactly the
//! utterances whose displayed words never moved — which flatters the recognisers
//! that revise least usefully. So the comparison is normalised, and it is
//! against **the engine's own final transcript** rather than against the
//! reference: this metric measures whether text appears and stops moving, and
//! accuracy is measured separately. A fast-and-wrong engine is penalised in WER,
//! not here.
//!
//! A partial qualifies when both hold:
//!
//! 1. it is a grapheme-cluster prefix of the final, **and** — for the
//!    whitespace-separated languages — the prefix ends on a token boundary.
//!    Without that, *"Creat"* counts as a useful prefix of *"Create a"*.
//! 2. it is at least [`minimum_graphemes`] long.
//!
//! ## Censoring, which is the other half of the same metric
//!
//! An utterance that never produces a qualifying partial is **recorded as
//! censored and counted in the p95, never dropped** — dropping it would flatter
//! precisely the recognisers that are worst at producing useful partials. Above
//! a 5% censored fraction the p95 is *not estimable* and the candidate fails
//! that language. [`crate::stats::censored_quantile`] is where that rule lives,
//! because it is a fact about the distribution rather than about the engine.
//!
//! ## Error rates are corpus-level, never a mean of rates
//!
//! Total edits over total reference units. A mean of per-utterance rates lets a
//! three-word utterance weigh as much as a thirty-word one, which is not what
//! anybody means by "the error rate on this corpus".
//!
//! **WER and CER are not comparable to each other and neither is comparable
//! across languages.** Only across candidates within one language. Nothing in
//! this crate averages an error rate over languages, and
//! [`crate::report`] has no cell that could.

use koru_voice::VoiceLanguage;
use unicode_segmentation::UnicodeSegmentation;

use crate::normalise::is_whitespace_separated;

/// Which unit an error rate counts.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Unit {
    /// Whitespace-separated tokens — `En`, `Es`, `PtBr`, `De`, `Fr`, `Ar`.
    Word,
    /// Extended grapheme clusters — `ZhHans`, `Ja`.
    Grapheme,
}

impl Unit {
    /// What the column header says.
    pub fn metric(self) -> &'static str {
        match self {
            Unit::Word => "WER",
            Unit::Grapheme => "CER",
        }
    }

    /// The unit a language's error rate counts.
    pub fn of(language: VoiceLanguage) -> Unit {
        if is_whitespace_separated(language) {
            Unit::Word
        } else {
            Unit::Grapheme
        }
    }
}

/// Split a normalised string into the units its language is scored in.
pub fn units(text: &str, unit: Unit) -> Vec<String> {
    match unit {
        Unit::Word => text.split_whitespace().map(str::to_owned).collect(),
        Unit::Grapheme => text.graphemes(true).map(str::to_owned).collect(),
    }
}

/// Levenshtein distance over any sequence.
///
/// Two rows rather than a full matrix: n = 300 utterances of up to a few hundred
/// units each, times 1000 bootstrap resamples, is enough arithmetic that the
/// O(n) space version is worth having and cheap to write correctly.
pub fn levenshtein<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(left != right);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// One utterance's contribution to a corpus-level error rate.
///
/// Kept as *edits and units* rather than as a ratio, which is what makes the
/// aggregate a corpus-level rate and makes a bootstrap resample of utterances
/// mean anything.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ErrorCount {
    pub edits: usize,
    pub reference_units: usize,
}

/// Score one hypothesis against one reference. Both must already be normalised.
pub fn score(reference: &str, hypothesis: &str, unit: Unit) -> ErrorCount {
    let reference = units(reference, unit);
    let hypothesis = units(hypothesis, unit);
    ErrorCount {
        edits: levenshtein(&reference, &hypothesis),
        reference_units: reference.len(),
    }
}

/// Total edits over total reference units.
///
/// `None` when there are no reference units at all — a rate with a zero
/// denominator is not zero, and printing 0.0% for an empty corpus is how a
/// broken sample looks like a perfect score.
pub fn corpus_rate(counts: &[ErrorCount]) -> Option<f64> {
    let units: usize = counts.iter().map(|c| c.reference_units).sum();
    if units == 0 {
        return None;
    }
    let edits: usize = counts.iter().map(|c| c.edits).sum();
    Some(edits as f64 / units as f64)
}

/// `L(language)` — the minimum length, **in graphemes**, at which a partial
/// counts as useful.
///
/// In graphemes and not characters, because *N* characters is a word in English
/// and a sentence in Chinese; the target is the same amount of *useful text* in
/// each language, roughly two words.
///
/// **These are provisional and are checked rather than assumed.** The screen
/// stage prints the mean number of words in a qualifying prefix per language and
/// the check is that every language falls in `[1.5, 3.0]`; a language outside
/// that band has its `L` adjusted once, the screen rerun for it, and the
/// adjusted table recorded in `ENGINE_DECISION.md`. Adjusting after seeing
/// decision-run results is forbidden, which is why the check lives in the
/// screen — see [`crate::report`].
pub fn minimum_graphemes(language: VoiceLanguage) -> usize {
    match language {
        VoiceLanguage::En
        | VoiceLanguage::Es
        | VoiceLanguage::PtBr
        | VoiceLanguage::De
        | VoiceLanguage::Fr => 8,
        VoiceLanguage::Ar => 6,
        VoiceLanguage::Ja => 4,
        VoiceLanguage::ZhHans => 3,
    }
}

/// Whether a normalised partial qualifies as a first useful partial of a
/// normalised final.
pub fn qualifies(partial: &str, final_text: &str, language: VoiceLanguage) -> bool {
    let partial_graphemes: Vec<&str> = partial.graphemes(true).collect();
    if partial_graphemes.len() < minimum_graphemes(language) {
        return false;
    }
    let final_graphemes: Vec<&str> = final_text.graphemes(true).collect();
    if partial_graphemes.len() > final_graphemes.len() {
        return false;
    }
    if partial_graphemes[..] != final_graphemes[..partial_graphemes.len()] {
        return false;
    }
    if !is_whitespace_separated(language) {
        // Chinese and Japanese have no whitespace, so the grapheme prefix
        // stands alone.
        return true;
    }
    // The token-boundary rule: either the partial *is* the final, or the next
    // grapheme of the final is a space.
    partial_graphemes.len() == final_graphemes.len()
        || final_graphemes[partial_graphemes.len()] == " "
}

/// Whether `later` extends `earlier` rather than revising it.
///
/// The definition of a revision, and it is a grapheme prefix test on the
/// **normalised** strings — an engine that only changed the capitalisation of a
/// word it had already emitted has not revised anything a person would see move.
pub fn extends(earlier: &str, later: &str) -> bool {
    let earlier: Vec<&str> = earlier.graphemes(true).collect();
    let later: Vec<&str> = later.graphemes(true).collect();
    earlier.len() <= later.len() && earlier[..] == later[..earlier.len()]
}

/// How many of a session's partials revised rather than extended.
pub fn revisions(partials: &[String]) -> usize {
    partials
        .windows(2)
        .filter(|pair| !extends(&pair[0], &pair[1]))
        .count()
}

/// How many words a qualifying prefix held, for the `L` check the screen runs.
///
/// Counted in whitespace-separated tokens for every language, **including the
/// two that have no whitespace** — where it is therefore zero or one and the
/// check does not apply. Reported rather than silently skipped, because a
/// missing cell is a question and a `0.0` is an answer.
pub fn words_in(prefix: &str) -> usize {
    prefix.split_whitespace().count()
}

#[cfg(test)]
mod tests;
