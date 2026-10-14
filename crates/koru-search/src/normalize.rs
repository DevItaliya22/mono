// SPDX-License-Identifier: MPL-2.0

//! Search form, and the map back to the authored bytes.
//!
//! **A copy of the old app search index's `normalize.rs`, on purpose.** The app's own
//! search has to keep working until stage 4 of the store milestone removes it,
//! and pointing the dying crate at this one creates an edge that would have to
//! be unwound then. Two copies for two stages;
//! [TECHDEBT](../../../docs/contributing/techdebt/README.md) says what discharges it. Do not fix a
//! bug in one without the other.

use std::collections::HashMap;

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::MatchSpan;

/// Search form plus a byte map back into the original string.  The map is what
/// lets a diacritic-insensitive match still bold the exact authored text.
pub(crate) struct Normalized {
    pub text: String,
    original_byte_for_normalized_byte: Vec<usize>,
    original_len: usize,
}

pub(crate) fn normalize(original: &str) -> Normalized {
    let mut text = String::new();
    let mut map = Vec::new();
    for (original_byte, ch) in original.char_indices() {
        for decomposed in ch.to_string().nfkd() {
            if is_combining_mark(decomposed) {
                continue;
            }
            for lower in decomposed.to_lowercase() {
                let mut encoded = [0; 4];
                let bytes = lower.encode_utf8(&mut encoded).as_bytes();
                text.push(lower);
                map.extend(std::iter::repeat_n(original_byte, bytes.len()));
            }
        }
    }
    Normalized {
        text,
        original_byte_for_normalized_byte: map,
        original_len: original.len(),
    }
}

impl Normalized {
    pub fn original_span(&self, start: usize, end: usize) -> Option<MatchSpan> {
        if start >= end || end > self.text.len() {
            return None;
        }
        let original_start = *self.original_byte_for_normalized_byte.get(start)?;
        let original_end = if end == self.text.len() {
            self.original_len
        } else {
            *self.original_byte_for_normalized_byte.get(end)?
        };
        Some(MatchSpan {
            start: original_start,
            end: original_end.max(original_start),
        })
    }
}

fn all_spans(original: &str, query: &str, whole_token: bool) -> Vec<MatchSpan> {
    let haystack = normalize(original);
    let needle = normalize(query).text;
    if needle.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(relative) = haystack.text[from..].find(&needle) {
        let start = from + relative;
        let end = start + needle.len();
        let needs_word_boundary = needle
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric());
        let starts_token = !needs_word_boundary
            || start == 0
            || haystack.text[..start]
                .chars()
                .next_back()
                .is_some_and(|ch| !ch.is_alphanumeric());
        let ends_token = !whole_token
            || end == haystack.text.len()
            || haystack.text[end..]
                .chars()
                .next()
                .is_some_and(|ch| !ch.is_alphanumeric());
        if starts_token
            && ends_token
            && let Some(span) = haystack.original_span(start, end)
        {
            spans.push(span);
        }
        from = end;
    }
    spans
}

/// Canonical word boundaries shared by query planning and the fuzzy
/// vocabulary. FTS5's unicode tokenizer follows the same alphanumeric runs;
/// punctuation and whitespace never become typo candidates.
pub(crate) fn query_terms(normalized: &str) -> Vec<String> {
    normalized
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn term_counts<'a>(texts: impl IntoIterator<Item = &'a str>) -> HashMap<String, u64> {
    let mut counts = HashMap::new();
    for text in texts {
        for term in query_terms(&normalize(text).text) {
            *counts.entry(term).or_insert(0) += 1;
        }
    }
    counts
}

/// A term selected by the query planner and whether it must explain a whole
/// authored token (fuzzy alternatives) rather than an ordinary prefix.
pub(crate) struct EvidenceTerm<'a> {
    pub text: &'a str,
    pub whole_token: bool,
}

pub(crate) fn matching_evidence_spans<'a>(
    original: &str,
    terms: impl IntoIterator<Item = EvidenceTerm<'a>>,
) -> Vec<MatchSpan> {
    let mut spans: Vec<_> = terms
        .into_iter()
        .flat_map(|term| all_spans(original, term.text, term.whole_token))
        .collect();
    merge_spans(&mut spans);
    spans
}

/// Every authored range that explains why the AND query matched this fragment.
/// Terms use the same boundaries as the FTS query builder, while the returned
/// offsets always address the original UTF-8 text.
#[cfg(test)]
pub(crate) fn matching_spans(original: &str, query: &str) -> Vec<MatchSpan> {
    let normalized = normalize(query).text;
    let terms = query_terms(&normalized);
    matching_evidence_spans(
        original,
        terms.iter().map(|term| EvidenceTerm {
            text: term,
            whole_token: false,
        }),
    )
}

fn merge_spans(spans: &mut Vec<MatchSpan>) {
    spans.sort_by_key(|span| (span.start, span.end));
    let mut merged: Vec<MatchSpan> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        if let Some(previous) = merged.last_mut()
            && span.start <= previous.end
        {
            previous.end = previous.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    *spans = merged;
}

pub(crate) fn snippet(
    original: &str,
    spans: &[MatchSpan],
    radius: usize,
) -> (String, Vec<MatchSpan>) {
    let Some(first) = spans.first() else {
        return (original.to_owned(), Vec::new());
    };
    let last = spans.last().unwrap();
    let mut start = first.start.saturating_sub(radius);
    while start > 0 && !original.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (last.end + radius).min(original.len());
    while end < original.len() && !original.is_char_boundary(end) {
        end += 1;
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let prefix = out.len();
    out.push_str(&original[start..end]);
    if end < original.len() {
        out.push('…');
    }
    let translated = spans
        .iter()
        .filter(|span| span.start >= start && span.end <= end)
        .map(|span| MatchSpan {
            start: prefix + span.start - start,
            end: prefix + span.end - start,
        })
        .collect();
    (out, translated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_diacritic_insensitive_match_to_authored_bytes() {
        let span = matching_spans("Crème brûlée", "creme").remove(0);
        assert_eq!(&"Crème brûlée"[span.start..span.end], "Crème");
    }

    #[test]
    fn maps_cjk_without_splitting_utf8() {
        let span = matching_spans("四半期の計画", "計画").remove(0);
        assert_eq!(&"四半期の計画"[span.start..span.end], "計画");
    }

    #[test]
    fn every_query_term_and_occurrence_gets_an_authored_span() {
        let text = "Specifications and decisions connect decisions to execution";
        let spans = matching_spans(text, "spec and decision");
        let matches: Vec<_> = spans
            .iter()
            .map(|span| &text[span.start..span.end])
            .collect();
        assert_eq!(matches, ["Spec", "and", "decision", "decision"]);
    }

    #[test]
    fn a_term_is_not_highlighted_inside_an_unmatched_token() {
        let text = "Standards and decisions";
        let spans = matching_spans(text, "and decision");
        let matches: Vec<_> = spans
            .iter()
            .map(|span| &text[span.start..span.end])
            .collect();
        assert_eq!(matches, ["and", "decision"]);
    }

    #[test]
    fn snippet_translates_every_visible_match() {
        let text = "A long prefix. Specifications and decisions connect to execution. A suffix.";
        let spans = matching_spans(text, "spec and decision");
        let (snippet, spans) = snippet(text, &spans, 4);
        let matches: Vec<_> = spans
            .iter()
            .map(|span| &snippet[span.start..span.end])
            .collect();
        assert_eq!(matches, ["Spec", "and", "decision"]);
    }
}
