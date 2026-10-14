// SPDX-License-Identifier: MPL-2.0

//! **The eight launch languages, and the difference between asking for one and
//! being told one.**
//!
//! Two types that look like one and are not. [`Requested`] is what a *request*
//! carries and it has a ninth answer — `Auto`, "work it out from the audio".
//! [`VoiceLanguage`] is what a *result* reports, and there are only ever eight
//! of those, because "auto" is not a language anybody speaks.
//!
//! Rev 1 of the roadmap asked that every variant round-trip through BCP 47,
//! which was unsatisfiable while `Auto` was a member: `auto` is not a tag, and
//! there is no tag it could honestly be given. Splitting the two types is what
//! makes the round-trip test true rather than relaxed.
//!
//! ## The tags are `koru-i18n`'s, deliberately
//!
//! `koru_i18n::Lang` already declares all eight and their tags, and those tags
//! are also how `koru_store_core::fixtures` spells the eight test languages. A
//! table here would be a second opinion about whether Brazilian Portuguese is
//! `pt-BR` or `pt-br`, and the two would drift the first time one was edited.
//! So [`VoiceLanguage::lang`] is the whole mapping and everything else is
//! derived from it — including serde, which a test pins to `tag()` so the wire
//! spelling and the displayed spelling cannot come apart.

use koru_i18n::Lang;
use serde::{Deserialize, Serialize};

/// A language a speech engine can be asked to transcribe.
///
/// The same eight as `koru_i18n::Lang`, and the correspondence is exhaustive in
/// both directions — [`VoiceLanguage::lang`] and [`VoiceLanguage::from_lang`]
/// are `match`es with no wildcard, so adding a ninth language to either crate is
/// a compile error here rather than a language that silently transcribes as
/// English.
///
/// It serializes as its BCP 47 tag rather than as a Rust-shaped `zh_hans`,
/// because the tag is what every other boundary in this product speaks.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum VoiceLanguage {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "es")]
    Es,
    #[serde(rename = "pt-BR")]
    PtBr,
    #[serde(rename = "de")]
    De,
    #[serde(rename = "fr")]
    Fr,
    #[serde(rename = "ar")]
    Ar,
    #[serde(rename = "zh-Hans")]
    ZhHans,
    #[serde(rename = "ja")]
    Ja,
}

impl VoiceLanguage {
    /// Every language, in `koru_i18n::Lang::ALL`'s order so a picker built from
    /// either list reads the same.
    pub const ALL: [VoiceLanguage; 8] = [
        VoiceLanguage::En,
        VoiceLanguage::Es,
        VoiceLanguage::PtBr,
        VoiceLanguage::De,
        VoiceLanguage::Fr,
        VoiceLanguage::Ar,
        VoiceLanguage::ZhHans,
        VoiceLanguage::Ja,
    ];

    /// The `koru-i18n` language this one is. The single point of contact
    /// between the two crates.
    pub fn lang(self) -> Lang {
        match self {
            VoiceLanguage::En => Lang::En,
            VoiceLanguage::Es => Lang::Es,
            VoiceLanguage::PtBr => Lang::PtBr,
            VoiceLanguage::De => Lang::De,
            VoiceLanguage::Fr => Lang::Fr,
            VoiceLanguage::Ar => Lang::Ar,
            VoiceLanguage::ZhHans => Lang::ZhHans,
            VoiceLanguage::Ja => Lang::Ja,
        }
    }

    /// The other direction. Total, because the two lists are the same eight —
    /// and it is a `match` rather than an index so that stops being true
    /// loudly.
    pub fn from_lang(lang: Lang) -> VoiceLanguage {
        match lang {
            Lang::En => VoiceLanguage::En,
            Lang::Es => VoiceLanguage::Es,
            Lang::PtBr => VoiceLanguage::PtBr,
            Lang::De => VoiceLanguage::De,
            Lang::Fr => VoiceLanguage::Fr,
            Lang::Ar => VoiceLanguage::Ar,
            Lang::ZhHans => VoiceLanguage::ZhHans,
            Lang::Ja => VoiceLanguage::Ja,
        }
    }

    /// The BCP 47 tag — `koru_i18n::Lang`'s, never a copy of it.
    pub fn tag(self) -> &'static str {
        self.lang().tag()
    }

    /// The language a tag names, or `None` when none of the eight fits.
    ///
    /// `koru_i18n::Lang::matching`'s rules apply unchanged, including the one
    /// worth knowing: `zh-Hant` is Traditional Chinese and does **not** quietly
    /// become Simplified, because two scripts are not two dialects. A model
    /// trained on Simplified handed a Traditional request would produce
    /// confident output in the wrong script.
    pub fn matching(tag: &str) -> Option<VoiceLanguage> {
        Lang::matching(tag).map(VoiceLanguage::from_lang)
    }

    /// The language's name in itself, for a picker somebody has to use while
    /// already stuck in a language they cannot read.
    pub fn endonym(self) -> &'static str {
        self.lang().endonym()
    }
}

/// What a [`crate::SessionRequest`] carries: a language, or the instruction to
/// work one out.
///
/// `Auto` is **deliberately not** a member of the BCP 47 mapping. [`tag`] is an
/// `Option` for exactly that reason, and a test asserts the `None` rather than
/// leaving it as an absence somebody later fills in with `"und"` or `""`.
///
/// Whether an engine can answer `Auto` at all is [`crate::EngineInfo`]'s
/// `detects_language`; an engine that cannot refuses in
/// [`crate::SpeechEngine::begin`] with [`crate::ErrorCode::LanguageUnsupported`]
/// rather than silently transcribing as English.
///
/// [`tag`]: Requested::tag
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requested {
    Auto,
    Fixed(VoiceLanguage),
}

impl Requested {
    /// The BCP 47 tag, when there is one. `Auto` has none and never will.
    pub fn tag(self) -> Option<&'static str> {
        match self {
            Requested::Auto => None,
            Requested::Fixed(language) => Some(language.tag()),
        }
    }

    /// The language, when the request named one.
    pub fn fixed(self) -> Option<VoiceLanguage> {
        match self {
            Requested::Auto => None,
            Requested::Fixed(language) => Some(language),
        }
    }
}

impl From<VoiceLanguage> for Requested {
    fn from(language: VoiceLanguage) -> Requested {
        Requested::Fixed(language)
    }
}

#[cfg(test)]
mod tests;
