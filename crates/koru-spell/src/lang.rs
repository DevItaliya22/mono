// SPDX-License-Identifier: MPL-2.0

//! **The seven spell languages, and why they are not `koru_i18n::Lang`.**
//!
//! Spelling is a finer partition than UI strings: `en-US` and `en-GB` disagree
//! about `color`, and one `Lang::En` cannot say which. So this is its own
//! closed enum — the `VoiceLanguage` pattern — with the overlap *derived* from
//! `koru_i18n::Lang` where the two sets agree, so there is never a second
//! opinion about how Brazilian Portuguese is spelled as a tag.
//!
//! The other deliberate difference: [`SpellLang::from_locale_tag`] returns
//! `None` for a locale none of the seven fit. `Lang::for_system` answers
//! English for the unmatched because a UI must draw *something*; a spell
//! checker must not — checking a `ko-KR` person's prose against English is
//! red squiggles over everything they write (plan D6). Absence is the honest
//! answer, and the caller turns it into "spell checking quietly off".

use koru_i18n::Lang;

/// A language spelling can be checked in. V1's seven (plan §P1); Chinese and
/// Japanese are deliberately absent — they get their own path later rather
/// than being forced through a Hunspell engine (spec §1).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum SpellLang {
    EnUs,
    EnGb,
    Es,
    Fr,
    PtBr,
    De,
    Ar,
}

impl SpellLang {
    /// Every spell language, English variants first, then `Lang::ALL`'s order
    /// for the rest, so a picker built beside a UI-language picker reads
    /// familiarly.
    pub const ALL: [SpellLang; 7] = [
        SpellLang::EnUs,
        SpellLang::EnGb,
        SpellLang::Es,
        SpellLang::Fr,
        SpellLang::PtBr,
        SpellLang::De,
        SpellLang::Ar,
    ];

    /// The BCP 47 tag. For the languages `koru_i18n` also knows, the tag is
    /// borrowed from there rather than restated; only the regional English
    /// split is this crate's own spelling, because `koru-i18n` deliberately
    /// has no regional English.
    pub fn tag(self) -> &'static str {
        match self {
            SpellLang::EnUs => "en-US",
            SpellLang::EnGb => "en-GB",
            SpellLang::Es => Lang::Es.tag(),
            SpellLang::Fr => Lang::Fr.tag(),
            SpellLang::PtBr => Lang::PtBr.tag(),
            SpellLang::De => Lang::De.tag(),
            SpellLang::Ar => Lang::Ar.tag(),
        }
    }

    /// The language's name in itself, for a picker somebody may be using
    /// precisely because the current language is not theirs.
    pub fn endonym(self) -> &'static str {
        match self {
            SpellLang::EnUs => "English (US)",
            SpellLang::EnGb => "English (UK)",
            SpellLang::Es => Lang::Es.endonym(),
            SpellLang::Fr => Lang::Fr.endonym(),
            SpellLang::PtBr => Lang::PtBr.endonym(),
            SpellLang::De => Lang::De.endonym(),
            SpellLang::Ar => Lang::Ar.endonym(),
        }
    }

    /// The spell language a person's locale asks for, or `None` when spelling
    /// is not available for it — which the caller must treat as "quietly off",
    /// never as English (plan D6).
    ///
    /// Resolution is deliberately simple: exact tag first, then the primary
    /// subtag with a regional default for the two languages that have variants
    /// here (`en` → `en-US`, `pt` → `pt-BR` — the same pt call `koru-i18n`
    /// made). A subtag with a *different* region still gets the base language:
    /// `en-AU` reads as `en-GB`-adjacent but Hunspell has no `en-AU` here, and
    /// `en-US` squiggles on "colour" would be wrong for them — so `en-AU`,
    /// like every explicit non-US English region, resolves to `en-GB`.
    pub fn from_locale_tag(tag: &str) -> Option<SpellLang> {
        let tag = tag.trim();
        if tag.is_empty() {
            return None;
        }
        for lang in SpellLang::ALL {
            if lang.tag().eq_ignore_ascii_case(tag) {
                return Some(lang);
            }
        }
        let primary = tag
            .split(['-', '_'])
            .next()
            .unwrap_or(tag)
            .to_ascii_lowercase();
        match primary.as_str() {
            // A named non-US region ("en-AU", "en-NZ", "en-IN") follows UK
            // spelling conventions; bare "en" is the US default.
            "en" if tag.len() > 2 => Some(SpellLang::EnGb),
            "en" => Some(SpellLang::EnUs),
            "es" => Some(SpellLang::Es),
            "fr" => Some(SpellLang::Fr),
            "pt" => Some(SpellLang::PtBr),
            "de" => Some(SpellLang::De),
            "ar" => Some(SpellLang::Ar),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
