// SPDX-License-Identifier: MPL-2.0

//! **The one module that knows `spellbook` exists.**
//!
//! The pin (`=0.4.2`) and this confinement are the same decision (plan D1):
//! the crate calls itself alpha and allows breaking API changes, so the
//! surface it can break is kept to this file. Nothing here is `pub` beyond
//! the crate, and `the_public_api_speaks_only_koru_types` in `lib.rs`'s tests
//! is what notices if that stops being true.

use crate::SpellProvider;

/// One parsed Hunspell dictionary, answering through [`SpellProvider`].
///
/// Immutable once built — custom words are Koru-owned overlays checked *before*
/// the engine (spec §22), never `add()`ed into it, so one shared instance
/// serves every checker without a lock.
pub(crate) struct SpellbookDictionary {
    dictionary: spellbook::Dictionary,
}

impl SpellbookDictionary {
    /// Parse `.aff` + `.dic` contents. Costs tens to hundreds of milliseconds
    /// for a real language — the reason `DictionaryManager` only ever calls
    /// this off the thread that answers a person.
    pub(crate) fn parse(aff: &str, dic: &str) -> Result<SpellbookDictionary, String> {
        spellbook::Dictionary::new(aff, dic)
            .map(|dictionary| SpellbookDictionary { dictionary })
            .map_err(|error| error.to_string())
    }
}

impl SpellProvider for SpellbookDictionary {
    fn check(&self, word: &str) -> bool {
        // Fail open on the engine's own length cap: a token past it is not
        // prose anybody typed by hand, and "valid" costs nothing while a
        // squiggle on it would be noise.
        if word.len() > spellbook::MAX_WORD_LEN {
            return true;
        }
        self.dictionary.check(word)
    }

    fn suggest(&self, word: &str, limit: usize) -> Vec<String> {
        let mut suggestions = Vec::new();
        self.dictionary.suggest(word, &mut suggestions);
        suggestions.truncate(limit);
        suggestions
    }
}
