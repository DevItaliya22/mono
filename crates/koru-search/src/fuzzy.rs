// SPDX-License-Identifier: MPL-2.0

//! Typo tolerance: a BK-tree over the normalized vocabulary the index holds.
//!
//! **A copy of the old app search index's `fuzzy.rs`**, for the reason given
//! at the top of `normalize.rs`. Do not fix a bug in one without the other.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

use strsim::damerau_levenshtein;

use crate::{FuzzyIndexState, FuzzyIndexStats};

const MAX_ALTERNATIVES: usize = 8;
const COMPACT_MIN_INACTIVE: usize = 1_000;

#[derive(Clone)]
pub(crate) struct SharedVocabulary(Arc<RwLock<Vocabulary>>);

impl SharedVocabulary {
    pub(crate) fn warming() -> Self {
        Self(Arc::new(RwLock::new(Vocabulary::default())))
    }

    pub(crate) fn ready() -> Self {
        let vocabulary = Vocabulary {
            state: FuzzyIndexState::Ready,
            ..Vocabulary::default()
        };
        Self(Arc::new(RwLock::new(vocabulary)))
    }

    pub(crate) fn read(&self) -> RwLockReadGuard<'_, Vocabulary> {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, Vocabulary> {
        self.0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn stats(&self) -> FuzzyIndexStats {
        self.read().stats()
    }

    /// Build a compact replacement without holding the read/write lock, then
    /// publish only if no partition delta changed the active vocabulary while
    /// it was being built. Queries keep using the previous complete tree.
    pub(crate) fn compact_if_needed(&self, built_at_ms: i64) -> bool {
        let counts = {
            let vocabulary = self.read();
            if !vocabulary.should_compact() {
                return false;
            }
            vocabulary.counts.clone()
        };
        let mut replacement = Vocabulary {
            state: FuzzyIndexState::Rebuilding,
            ..Vocabulary::default()
        };
        replacement.replace_all(counts.clone(), built_at_ms);
        let mut vocabulary = self.write();
        if vocabulary.counts != counts || !vocabulary.should_compact() {
            return false;
        }
        *vocabulary = replacement;
        true
    }
}

#[derive(Default)]
pub(crate) struct Vocabulary {
    root: Option<Box<BkNode>>,
    counts: HashMap<String, u64>,
    tree_nodes: usize,
    inactive_nodes: usize,
    state: FuzzyIndexState,
    last_build_ms: Option<u64>,
    last_built_at_ms: Option<i64>,
}

impl Vocabulary {
    pub(crate) fn candidates(&self, query: &str) -> Vec<FuzzyCandidate> {
        if self.state != FuzzyIndexState::Ready {
            return Vec::new();
        }
        let max_distance = allowed_distance(query);
        if max_distance == 0 {
            return Vec::new();
        }
        let mut found = Vec::new();
        if let Some(root) = &self.root {
            root.search(query, max_distance, &self.counts, &mut found);
        }
        found.retain(|candidate| candidate.term != query);
        found.sort_by(|left, right| {
            left.distance
                .cmp(&right.distance)
                .then_with(|| right.frequency.cmp(&left.frequency))
                .then_with(|| left.term.cmp(&right.term))
        });
        found.truncate(MAX_ALTERNATIVES);
        found
    }

    pub(crate) fn replace_all(&mut self, counts: HashMap<String, u64>, built_at_ms: i64) {
        let started = Instant::now();
        let mut terms: Vec<_> = counts.keys().cloned().collect();
        terms.sort_by_key(|term| std::cmp::Reverse(counts.get(term).copied().unwrap_or(0)));
        self.root = None;
        self.tree_nodes = 0;
        for term in terms {
            self.insert_tree_term(term);
        }
        self.counts = counts;
        self.inactive_nodes = 0;
        self.state = FuzzyIndexState::Ready;
        self.last_build_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
        self.last_built_at_ms = Some(built_at_ms);
    }

    pub(crate) fn apply_delta(&mut self, delta: HashMap<String, i64>) {
        for (term, change) in delta {
            if change == 0 {
                continue;
            }
            let previous = self.counts.get(&term).copied().unwrap_or(0);
            let next = if change.is_negative() {
                previous.saturating_sub(change.unsigned_abs())
            } else {
                previous.saturating_add(change as u64)
            };
            if next == 0 {
                if self.counts.remove(&term).is_some() && previous > 0 {
                    self.inactive_nodes = self.inactive_nodes.saturating_add(1);
                }
            } else {
                if previous == 0 {
                    self.insert_tree_term(term.clone());
                }
                self.counts.insert(term, next);
            }
        }
    }

    pub(crate) fn should_compact(&self) -> bool {
        self.inactive_nodes >= COMPACT_MIN_INACTIVE
            && self.inactive_nodes.saturating_mul(5) >= self.tree_nodes
    }

    fn insert_tree_term(&mut self, term: String) {
        if let Some(root) = &mut self.root {
            if root.insert(term) {
                self.tree_nodes += 1;
            }
        } else {
            self.root = Some(Box::new(BkNode::new(term)));
            self.tree_nodes = 1;
        }
    }

    fn stats(&self) -> FuzzyIndexStats {
        FuzzyIndexStats {
            state: self.state,
            active_terms: self.counts.len() as u64,
            tree_nodes: self.tree_nodes as u64,
            inactive_nodes: self.inactive_nodes as u64,
            last_build_ms: self.last_build_ms,
            last_built_at_ms: self.last_built_at_ms,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FuzzyCandidate {
    pub term: String,
    pub distance: usize,
    pub frequency: u64,
}

struct BkNode {
    term: String,
    children: HashMap<usize, Box<BkNode>>,
}

impl BkNode {
    fn new(term: String) -> Self {
        Self {
            term,
            children: HashMap::new(),
        }
    }

    fn insert(&mut self, term: String) -> bool {
        let distance = damerau_levenshtein(&self.term, &term);
        if distance == 0 {
            return false;
        }
        if let Some(child) = self.children.get_mut(&distance) {
            child.insert(term)
        } else {
            self.children.insert(distance, Box::new(Self::new(term)));
            true
        }
    }

    fn search(
        &self,
        query: &str,
        max_distance: usize,
        active: &HashMap<String, u64>,
        found: &mut Vec<FuzzyCandidate>,
    ) {
        let distance = damerau_levenshtein(&self.term, query);
        if distance <= max_distance
            && let Some(frequency) = active.get(&self.term)
        {
            found.push(FuzzyCandidate {
                term: self.term.clone(),
                distance,
                frequency: *frequency,
            });
        }
        let lower = distance.saturating_sub(max_distance);
        let upper = distance.saturating_add(max_distance);
        for (_, child) in self
            .children
            .iter()
            .filter(|(edge, _)| **edge >= lower && **edge <= upper)
        {
            child.search(query, max_distance, active, found);
        }
    }
}

fn allowed_distance(term: &str) -> usize {
    if !term.chars().all(char::is_alphabetic) {
        return 0;
    }
    match term.chars().count() {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocabulary(words: &[&str]) -> Vocabulary {
        let counts = words.iter().map(|word| ((*word).into(), 1)).collect();
        let mut vocabulary = Vocabulary::default();
        vocabulary.replace_all(counts, 1);
        vocabulary
    }

    #[test]
    fn deletion_and_adjacent_transposition_cost_one_edit() {
        let vocabulary = vocabulary(&["should", "would", "shoulder"]);
        assert!(
            vocabulary
                .candidates("hould")
                .iter()
                .any(|candidate| candidate.term == "should")
        );
        assert!(
            vocabulary
                .candidates("shoudl")
                .iter()
                .any(|candidate| candidate.term == "should")
        );
    }

    #[test]
    fn short_and_non_word_terms_are_not_expanded() {
        let vocabulary = vocabulary(&["cat", "1234", "should"]);
        assert!(vocabulary.candidates("cat").is_empty());
        assert!(vocabulary.candidates("1234").is_empty());
    }

    #[test]
    fn inactive_terms_never_return_from_the_tree() {
        let mut vocabulary = vocabulary(&["should"]);
        vocabulary.apply_delta(HashMap::from([("should".into(), -1)]));
        assert!(vocabulary.candidates("hould").is_empty());
    }
}
