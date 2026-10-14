// SPDX-License-Identifier: MPL-2.0

//! **Whether a word is spelled correctly, and what might replace it.**
//!
//! The engine crate of the spell-check feature.
//! It owns the four things that are true of spelling anywhere — which languages
//! exist ([`SpellLang`]), how prose becomes checkable words ([`tokenizer`]),
//! where dictionaries live and how they load ([`DictionaryManager`]), and the
//! provider boundary that seals the engine ([`SpellProvider`]) — and deliberately
//! nothing that is knowledge about the editor: no nodes, no rows, no draw calls.
//!
//! ## The one architectural rule
//!
//! ```text
//! Koru owns:      text semantics · tokenization · languages · ranges ·
//!                 dictionaries · scheduling · UI · collaboration semantics
//! the engine owns: "is this word valid?" · "what spellings might replace it?"
//! ```
//!
//! `spellbook` is pinned exactly and confined to `engine.rs`; no spellbook type
//! crosses this crate's public API (`the_public_api_speaks_only_koru_types`
//! asserts it), so a language can later get a different engine — or the whole
//! engine can be swapped — without the editor changing.
//!
//! ## Fail open
//!
//! A missing dictionary, a parse failure, an unresolvable locale: every one of
//! them answers "no spell checking", never an error the editor must handle to
//! keep editing. Losing underlines is the acceptable failure; blocking a
//! document is not (spec §31).

mod dictionaries;
mod engine;
mod lang;
pub mod tokenizer;

pub use dictionaries::{DictionaryManager, DictionaryPaths, DictionaryState, SpellError};
pub mod sources;
pub use lang::SpellLang;

#[cfg(test)]
mod tests;

/// A loaded language's answers: the two questions the engine owns.
///
/// One provider per *loaded language*, handed out by [`DictionaryManager`], so
/// a future language can ship a different implementation (the CJK plan in the
/// spec) without a registry of engines: the manager is the registry.
///
/// `check` is designed to be called hot — once per token of every re-checked
/// block — while `suggest` is deliberately lazy: it is comparatively expensive
/// and most underlines are never right-clicked (plan D13), so nothing calls it
/// until a person opens the menu.
pub trait SpellProvider: Send + Sync {
    /// Is this word spelled correctly in this language?
    fn check(&self, word: &str) -> bool;

    /// Up to `limit` replacement spellings, best first.
    fn suggest(&self, word: &str, limit: usize) -> Vec<String>;
}
