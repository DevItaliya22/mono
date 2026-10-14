// SPDX-License-Identifier: MPL-2.0

//! **Three normalisers with a shared core, applied to the reference and the
//! hypothesis by the same function.**
//!
//! And to the partial-prefix test by the same function again — that third use is
//! the one it is easy to forget, and forgetting it flatters exactly the
//! recognisers that revise least usefully, because a raw byte-prefix test
//! reports *no useful partial* on an utterance whose displayed words never
//! moved.
//!
//! ## The four shared rules, in order
//!
//! 1. Unicode **NFKC**.
//! 2. Unicode **simple case folding**.
//! 3. Remove every character in general categories **P\*** and **S\***, with one
//!    exception: an apostrophe (`U+0027` or `U+2019`) **between two letters** is
//!    mapped to `U+0027` and kept.
//! 4. Collapse runs of whitespace to a single space; trim.
//!
//! `En`, `Es`, `PtBr`, `De` and `Fr` use exactly these and nothing else — no
//! stemming, no German compound splitting, no lexicon. Case and diacritics are
//! the only orthography rules applied.
//!
//! **Rule 3's exception is not cosmetic.** *don't* and *dont* are different
//! words in English, and French elision — *l'eau* — is not punctuation. The
//! between-two-letters test is what separates those from a quotation mark, and
//! it is why this cannot be `retain(|c| !c.is_ascii_punctuation())`.
//!
//! ## The categories, and the one place we do not use a table
//!
//! Rule 3 says P\* and S\*, and Rust's standard library has no Unicode general
//! category. Pulling a full category table in for one predicate would be a
//! dependency whose data is a fourth opinion about Unicode beside
//! `unicode-normalization`'s, `unicode-segmentation`'s and the platform's. So
//! [`is_punctuation_or_symbol`] is an explicit range list, written from the
//! Unicode blocks these eight languages are actually written in, and its
//! coverage is stated rather than implied: **it covers every P\* and S\*
//! codepoint reachable from Latin, Arabic and CJK text, and it does not claim
//! to cover the whole of Unicode.** A codepoint outside those scripts surviving
//! into a word is a fact about a recogniser emitting something none of these
//! corpora contains, and that is worth seeing rather than silently deleting.
//!
//! ## What each script adds
//!
//! **Arabic** strips tatweel and the harakat, and folds the alef variants and
//! alef maqsura. **Teh marbuta is left alone and is not merged with `ه`** — some
//! published Arabic ASR normalisers merge them; we do not, because it changes
//! word identity and a person dictating would have to fix it. That is a decision
//! and it makes our Arabic WER slightly higher than a paper's.
//!
//! **Simplified Chinese** removes whitespace entirely rather than collapsing it,
//! and converts Traditional forms to Simplified. That conversion is **not
//! implemented here**: it is a table the corpus step places, and
//! [`opencc`] says at length why writing our own would produce a number that is
//! neither the protocol's nor an honestly unconverted one. A [`Normaliser`] with
//! no table converts nothing and the run header says so.
//!
//! **Japanese** removes whitespace entirely and does **nothing else**. No
//! kana↔kanji normalisation and no conversion to readings: a recogniser that
//! writes かんじ where the reference has 漢字 has produced text the person would
//! have to correct, so for a dictation product that is an error. This makes our
//! Japanese CER pessimistic relative to published numbers scored on kana
//! readings, **and the comparison to those numbers is therefore invalid.**
//! Stated loudly because somebody will cite it.

use koru_voice::VoiceLanguage;
use unicode_normalization::UnicodeNormalization;

pub mod opencc;

pub use opencc::{T2S_FILE, T2S_PAIRS, T2sTable, t2s};

/// **The one door.** Every reference, every hypothesis and every partial goes
/// through the same instance, which is what makes the prefix test and the error
/// rate agree about what a string is.
///
/// It exists as a struct rather than a free function for one reason: rule 5 for
/// Simplified Chinese needs a table this crate does not own, and a process
/// global holding it would be a lock every test had to take — the hazard
/// `THEME_LOCK` already documents.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Normaliser {
    t2s: T2sTable,
}

impl Normaliser {
    /// With a Traditional-to-Simplified table.
    pub fn new(t2s: T2sTable) -> Normaliser {
        Normaliser { t2s }
    }

    /// Without one. Correct for the seven languages rule 5 does not apply to,
    /// and **reported** for the eighth.
    pub fn without_t2s() -> Normaliser {
        Normaliser::default()
    }

    /// Whether a `zh-Hans` figure from this normaliser is the protocol's.
    pub fn has_t2s(&self) -> bool {
        !self.t2s.is_empty()
    }

    /// The table, for a run header that has to state its size.
    pub fn t2s_table(&self) -> &T2sTable {
        &self.t2s
    }

    /// Normalise one string for one language.
    pub fn normalise(&self, text: &str, language: VoiceLanguage) -> String {
        let shared = shared_rules(text);
        match language {
            VoiceLanguage::En
            | VoiceLanguage::Es
            | VoiceLanguage::PtBr
            | VoiceLanguage::De
            | VoiceLanguage::Fr => shared,
            VoiceLanguage::Ar => arabic(&shared),
            VoiceLanguage::ZhHans => simplified_chinese(&shared, &self.t2s),
            VoiceLanguage::Ja => japanese(&shared),
        }
    }
}

/// Normalise with no Traditional-to-Simplified table.
///
/// A convenience for the seven languages where that is the whole rule anyway;
/// `zh-Hans` through this function is **not** the protocol's normalisation, and
/// the doc says so where somebody would reach for it.
pub fn normalise(text: &str, language: VoiceLanguage) -> String {
    Normaliser::without_t2s().normalise(text, language)
}

/// Whether this language's error rate is counted in words or in grapheme
/// clusters, which is also whether whitespace survives normalisation.
pub fn is_whitespace_separated(language: VoiceLanguage) -> bool {
    !matches!(language, VoiceLanguage::ZhHans | VoiceLanguage::Ja)
}

/// Rules 1 to 4.
pub fn shared_rules(text: &str) -> String {
    // 1 and 2. `to_lowercase` is Rust's full Unicode lowercase mapping, which
    // is what "simple case folding" means for every script here — and a no-op
    // for the CJK and Arabic ones, as the protocol says.
    let folded: String = text.nfkc().collect::<String>().to_lowercase();

    // 3. The apostrophe exception needs to look at neighbours, so this is an
    // indexed walk rather than a `retain`.
    let chars: Vec<char> = folded.chars().collect();
    let mut kept = String::with_capacity(folded.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '\u{0027}' || c == '\u{2019}' {
            let before = i > 0 && chars[i - 1].is_alphabetic();
            let after = chars.get(i + 1).is_some_and(|n| n.is_alphabetic());
            if before && after {
                kept.push('\u{0027}');
            }
            continue;
        }
        if is_punctuation_or_symbol(c) {
            continue;
        }
        kept.push(c);
    }

    // 4.
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Unicode general categories P\* and S\*, over the scripts these eight
/// languages are written in. See the module note for the scope this claims.
pub fn is_punctuation_or_symbol(c: char) -> bool {
    // ASCII first, because it is most of what arrives and the ranges below are
    // a linear scan.
    if c.is_ascii() {
        return c.is_ascii_punctuation();
    }
    matches!(c,
        // Latin-1 supplement symbols and punctuation.
        '\u{00A1}'..='\u{00BF}' | '\u{00D7}' | '\u{00F7}'
        // General punctuation, superscripts, currency, letterlike, arrows,
        // maths, technical, box drawing, geometric, miscellaneous symbols,
        // dingbats.
        | '\u{2000}'..='\u{2BFF}'
        // Supplemental punctuation.
        | '\u{2E00}'..='\u{2E7F}'
        // CJK symbols and punctuation — 。、「」〜 and the rest.
        | '\u{3000}'..='\u{303F}'
        // Small form variants and the fullwidth ASCII punctuation NFKC did not
        // already fold (it folds most of it, but not all fonts' variants).
        | '\u{FE10}'..='\u{FE6F}'
        | '\u{FF01}'..='\u{FF0F}' | '\u{FF1A}'..='\u{FF20}'
        | '\u{FF3B}'..='\u{FF40}' | '\u{FF5B}'..='\u{FF65}'
        // Arabic punctuation: comma, semicolon, question mark, full stop,
        // and the ornate parentheses.
        | '\u{060C}' | '\u{061B}' | '\u{061F}' | '\u{06D4}'
        | '\u{FD3E}' | '\u{FD3F}'
        // Emoji and pictographs, which a recogniser will not emit and a
        // reference should not carry.
        | '\u{1F000}'..='\u{1FAFF}'
    )
}

/// Rules 5 to 7 for Arabic.
pub fn arabic(text: &str) -> String {
    text.chars()
        .filter(|c| {
            // Tatweel, and the harakat. Optional in writing: a recogniser that
            // emits them and a reference that omits them are not disagreeing.
            !matches!(c, '\u{0640}' | '\u{064B}'..='\u{0652}' | '\u{0670}')
        })
        .map(|c| match c {
            // Alef variants.
            '\u{0623}' | '\u{0625}' | '\u{0622}' | '\u{0671}' => '\u{0627}',
            // Alef maqsura to yeh.
            '\u{0649}' => '\u{064A}',
            // Teh marbuta is **not** here. See the module note: merging it with
            // `ه` changes word identity, and a person dictating would have to
            // fix it.
            other => other,
        })
        .collect()
}

/// Rule 5 for Simplified Chinese, plus the whitespace rule.
pub fn simplified_chinese(text: &str, table: &T2sTable) -> String {
    t2s(&remove_whitespace(text), table)
}

/// The whitespace rule for Japanese, and nothing else.
pub fn japanese(text: &str) -> String {
    remove_whitespace(text)
}

/// Whitespace **removed**, not collapsed: Chinese and Japanese reference text
/// has none and a recogniser may insert some.
pub fn remove_whitespace(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests;
