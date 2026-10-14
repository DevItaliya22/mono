// SPDX-License-Identifier: MPL-2.0

//! **Traditional → Simplified, from a table somebody else produced.**
//!
//! The protocol's rule 5 for Simplified Chinese is *convert any Traditional
//! forms to Simplified with OpenCC `t2s`*. This module does not reimplement
//! OpenCC and deliberately does not pretend to.
//!
//! ## Why there is no built-in table
//!
//! `guide/contributing/PRINCIPLES.md` is explicit that rolling our own needs a written reason,
//! and there is none here — OpenCC exists, it is the tool the protocol names,
//! and its `t2s` mapping is thousands of entries maintained by people who read
//! both scripts. A hand-written subset would be worse than either alternative in
//! the way that matters: it would convert the common characters, miss the rest,
//! and produce a CER that is neither the protocol's number nor an honestly
//! unconverted one — and nothing would say which.
//!
//! So the table is **data the corpus step places**, beside `MANIFEST.json`:
//!
//! ```text
//! <corpus>/opencc-t2s.tsv     <traditional>\t<simplified>\n
//! ```
//!
//! generated once, by OpenCC, at corpus-acquisition time — the same step that
//! already runs `ffmpeg` and records what version of it did the converting. The
//! converter's version belongs in the manifest for the same reason ffmpeg's
//! does.
//!
//! ## When it is absent
//!
//! [`T2sTable::empty`] converts nothing, and the run header says so in as many
//! words. That is the third option and it is the honest one: a `zh-Hans` figure
//! produced without the table is a figure that scores a model's Traditional
//! output as wrong, which is a real difference from the protocol's number and
//! must not be invisible. `--stage decision` refuses to run without it; the
//! screen and the smoke fixture do not, because neither produces a number
//! anybody cites.
//!
//! ## The mapping is per character, which is what `t2s` is
//!
//! OpenCC's `t2s` config is `TSCharacters` plus `TSPhrases`; the phrase table
//! handles the cases where one Traditional character maps to different
//! Simplified ones depending on context — 乾 as 干 or as 乾, and so on. A TSV of
//! character pairs cannot carry those. **What the corpus step should dump is the
//! output of OpenCC's own converter over the reference and hypothesis strings**,
//! and where that is impractical, the character table is a documented
//! approximation whose remaining error is in the few dozen context-sensitive
//! characters. [`T2sTable::phrases`] exists so a longer key in the same file
//! wins over its characters, which is how a phrase table is expressed in this
//! format; entries are matched longest-first.

use std::collections::HashMap;

/// The mapping, longest-key-first.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct T2sTable {
    entries: HashMap<String, String>,
    /// The longest key in [`T2sTable::entries`], in `char`s. Cached because it
    /// bounds the lookahead of every step of the conversion, and recomputing it
    /// per character would make the conversion quadratic in the table.
    longest: usize,
}

impl T2sTable {
    /// Convert nothing, and mean it. See the module note.
    pub fn empty() -> T2sTable {
        T2sTable::default()
    }

    /// Whether anything was loaded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many mappings there are — printed in the run header, because "a t2s
    /// table was loaded" and "a t2s table with four entries was loaded" are
    /// different facts.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// How many of the entries are multi-character, i.e. phrases.
    pub fn phrases(&self) -> usize {
        self.entries
            .keys()
            .filter(|k| k.chars().count() > 1)
            .count()
    }

    /// Parse `<traditional>\t<simplified>` lines.
    ///
    /// Blank lines and lines beginning `#` are skipped. A line with no tab, an
    /// empty key, or a duplicate key is an error rather than a skip: a
    /// normalisation table that silently ignored half of itself would produce a
    /// number nobody could reproduce, which is the whole failure the pinning
    /// section of the protocol exists to prevent.
    pub fn parse(source: &str) -> Result<T2sTable, String> {
        let mut entries: HashMap<String, String> = HashMap::new();
        let mut longest = 0usize;
        for (number, line) in source.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (from, to) = line
                .split_once('\t')
                .ok_or_else(|| format!("line {} has no tab", number + 1))?;
            if from.is_empty() {
                return Err(format!("line {} has an empty key", number + 1));
            }
            longest = longest.max(from.chars().count());
            if entries.insert(from.to_owned(), to.to_owned()).is_some() {
                return Err(format!("line {} repeats the key {from:?}", number + 1));
            }
        }
        Ok(T2sTable { entries, longest })
    }

    /// Convert, matching the longest key at each position.
    pub fn convert(&self, text: &str) -> String {
        if self.entries.is_empty() {
            return text.to_owned();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            let mut matched = false;
            let limit = self.longest.min(chars.len() - i);
            for length in (1..=limit).rev() {
                let key: String = chars[i..i + length].iter().collect();
                if let Some(value) = self.entries.get(&key) {
                    out.push_str(value);
                    i += length;
                    matched = true;
                    break;
                }
            }
            if !matched {
                out.push(chars[i]);
                i += 1;
            }
        }
        out
    }
}

/// The file the corpus step places, beside `MANIFEST.json`.
pub const T2S_FILE: &str = "opencc-t2s.tsv";

/// The format, quoted in the one place a script author would look for it.
pub const T2S_PAIRS: &str = "<traditional>\\t<simplified>, one per line, UTF-8, LF";

/// Convert with a table.
///
/// A free function so [`super::simplified_chinese`] reads the way the protocol
/// writes rule 5.
pub fn t2s(text: &str, table: &T2sTable) -> String {
    table.convert(text)
}

#[cfg(test)]
mod tests;
