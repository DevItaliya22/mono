// SPDX-License-Identifier: MPL-2.0

//! The text every suite in the store is tested against.
//!
//! One fixture, shared by the sqlite, API, protocol, recovery and
//! export/import suites, so "it round-trips" means the same thing everywhere.
//! Compiled in rather than test-only: the API and daemon crates need it too,
//! and a `dev-dependency` cannot cross crates.
//!
//! **Why these eight.** They are the supported languages, and between them they
//! exercise every property that breaks a text path:
//!
//! | | what it catches |
//! | --- | --- |
//! | English | the ASCII fast path — the one that always works, so it proves nothing alone |
//! | Spanish, Portuguese (BR), German, French | Latin with diacritics: composition, and byte length ≠ character count |
//! | Arabic | RTL, and — in [`BIDI_LINE`] — mixed direction, the case that a naive "reverse it for display" gets wrong in a way that survives to storage |
//! | Simplified Chinese | CJK: three bytes per character, no spaces to tokenise on |
//! | Japanese | three scripts in one string, plus half-width kana that width-folding would silently rewrite |
//!
//! Every string here is also a **trap for a specific bug**, not decoration.
//! Nothing is truncated by byte offset without hitting a character boundary;
//! nothing is sorted without a locale; nothing is folded.

use crate::text::Locale;

/// One language's sample text.
pub struct Sample {
    /// BCP 47.
    pub tag: &'static str,
    /// A short title — the shape a workspace or document name takes.
    pub title: &'static str,
    /// A longer line, the shape document content takes.
    pub body: &'static str,
}

impl Sample {
    pub fn locale(&self) -> Locale {
        Locale::parse(self.tag).expect("fixture tags are valid BCP 47")
    }
}

/// The eight supported languages.
pub const SAMPLES: &[Sample] = &[
    Sample {
        tag: "en",
        title: "Quarterly plan",
        body: "The quick brown fox jumps over the lazy dog.",
    },
    Sample {
        tag: "es",
        title: "Plan trimestral",
        body: "El veloz murciélago hindú comía feliz cardillo y kiwi.",
    },
    Sample {
        tag: "pt-BR",
        title: "Planejamento trimestral",
        body: "À noite, vovô Kowalsky vê o ímã cair no pé do pinguim queixoso.",
    },
    Sample {
        tag: "de",
        title: "Quartalsplanung",
        body: "Falsches Üben von Xylophonmusik quält jeden größeren Zwerg.",
    },
    Sample {
        tag: "fr",
        title: "Plan trimestriel",
        body: "Portez ce vieux whisky au juge blond qui fume: à l’élève éveillé.",
    },
    Sample {
        tag: "ar",
        title: "الخطة الفصلية",
        body: "نص حكيم له سر قاطع وذو شأن عظيم مكتوب على ثوب أخضر ومغلف بجلد أزرق.",
    },
    Sample {
        tag: "zh-Hans",
        title: "季度计划",
        body: "视野无限宽，前途无限远，我们的工作区支持所有语言。",
    },
    Sample {
        tag: "ja",
        title: "四半期計画",
        body: "いろはにほへと ちりぬるを — 漢字とカタカナとひらがな、ﾊﾝｶｸ も。",
    },
];

/// A line that is genuinely mixed-direction: Arabic, a Latin product name, an
/// Arabic-Indic numeral, and explicit bidi controls.
///
/// The store must carry this **byte for byte**. It is the single string most
/// likely to be quietly damaged — by stripping controls "for safety", by
/// reordering to visual order, or by a display-oriented sanitiser that has no
/// business running on stored content.
pub const BIDI_LINE: &str = "مرحبا \u{202B}Koru Store\u{202C} — الإصدار ٣ (v3) جاهز.";

/// The same text in two Unicode spellings: composed, then decomposed.
///
/// Used to prove that keys compare equal after NFC and that **content is not
/// normalised** — both halves must survive storage unchanged.
pub const COMPOSED: &str = "Précis — Grüße — Água";
pub const DECOMPOSED: &str = "Pre\u{0301}cis — Gru\u{0308}ße — A\u{0301}gua";

/// A filename in a non-Latin script, with a space and an extension.
///
/// Blob identity is the content hash; this is metadata. It exists so a test can
/// prove it is stored and returned intact and **never used to build a path**.
pub const UNICODE_FILENAME: &str = "الميزانية النهائية ٢٠٢٦.pdf";

/// Every fixture string, for suites that just want to sweep all of them.
pub fn all_strings() -> Vec<&'static str> {
    let mut out = Vec::with_capacity(SAMPLES.len() * 2 + 4);
    for s in SAMPLES {
        out.push(s.title);
        out.push(s.body);
    }
    out.push(BIDI_LINE);
    out.push(COMPOSED);
    out.push(DECOMPOSED);
    out.push(UNICODE_FILENAME);
    out
}

#[cfg(test)]
mod tests;
