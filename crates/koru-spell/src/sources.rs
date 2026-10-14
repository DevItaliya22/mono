// SPDX-License-Identifier: MPL-2.0

//! **Where a missing dictionary is downloaded from, byte for byte.**
//!
//! The spell-check plan's delivery decision (D2, Q5) is English bundled and the
//! other languages as signed packs on Koru's own download server. Until those
//! packs are published, Settings offers a download straight from the source
//! `scripts/setup/fetch-dictionaries` already uses — the LibreOffice dictionaries
//! tree — **pinned to one release and to the SHA-256 of every file**, so the
//! app installs exactly the bytes the corpus was tested against or nothing at
//! all. Swapping the source for Koru-hosted packs later changes this table and
//! the fetcher, not the Settings row.
//!
//! Pure data: this crate does no I/O over the network; the fetcher lives in
//! `koru-workspace-host`'s `spelling` module, off the thread that answers a
//! person.
//!
//! **A dictionary never travels without its license.** Each language also
//! names its [`notices`] — the upstream README and license files, verbatim,
//! from the same pinned release — and its [`License`]: what upstream declares,
//! what Koru takes where upstream offers a choice, and whether it is GPL
//! whatever is chosen. The fetch scripts carry the same table row for row (a
//! test holds them to it), so a directory written by either path holds the
//! pair *and* the files that say who may use it.
//!
//! The shipping rule this exists for (`licensing/THIRD-PARTY.md`, ⚑2):
//! dictionaries are separate files, one directory per language, never
//! compiled into a binary (a test walks the tree for that) and never merged
//! with Koru's own word lists — which is what lets a GPL dictionary sit
//! beside an MPL program as mere aggregation.

use crate::SpellLang;

/// The LibreOffice release every file below comes from. Bump deliberately:
/// every hash changes with it, and the corpus must be re-run after.
pub const SOURCE_REF: &str = "libreoffice-24.8.2.1";

const BASE: &str = "https://raw.githubusercontent.com/LibreOffice/dictionaries/";

/// One file of a language's pair, as the source publishes it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    /// `dictionary.aff` or `dictionary.dic` — the name it is installed under.
    pub installed_as: &'static str,
    /// The path inside the source tree.
    pub path: &'static str,
    /// Its exact size, checked before the hash so a truncated body fails fast.
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the exact bytes.
    pub sha256: &'static str,
}

impl SourceFile {
    pub fn url(&self) -> String {
        format!("{BASE}{SOURCE_REF}/{}", self.path)
    }
}

/// The two files a language needs: affix rules first, then the word list.
pub fn files(lang: SpellLang) -> [SourceFile; 2] {
    let (aff, aff_bytes, aff_sha, dic, dic_bytes, dic_sha) = match lang {
        SpellLang::EnUs => (
            "en/en_US.aff",
            3_205,
            "e746c882dd6f303c2c46e7452804b9201115a6942cfeb15f18f8edf774d2e24e",
            "en/en_US.dic",
            551_762,
            "f0b1a234bd178bdd01875b2a392a9647f888b8fe879f79c52aae62c2759b3647",
        ),
        SpellLang::EnGb => (
            "en/en_GB.aff",
            35_311,
            "04755f317851defd8b9f77fd7e5d60f495221fad926d0df346cd9b61b646bfc2",
            "en/en_GB.dic",
            1_226_510,
            "e070c64007d0f6a3423d028a53da099934b5f967dc0ef6d01b631ec2119659d4",
        ),
        SpellLang::Es => (
            "es/es_ES.aff",
            167_135,
            "892039d59db11d747061b81c3a92a52d48ba0e055b5835af1943cbd3c2fba85b",
            "es/es_ES.dic",
            703_244,
            "21cbea1e258f1fa98a035efd4e700ef41d9e6a8f07ed9023c4aa3915e5eb1362",
        ),
        SpellLang::Fr => (
            "fr_FR/fr.aff",
            201_591,
            "4730775ff4488c4a9aa4833393c8b2187126336eed766866300a58a75fe874f0",
            "fr_FR/fr.dic",
            1_227_095,
            "b99447ab2e655c1e4b64509cfd2e6c24821242bff1f971b49eac723e388a2d19",
        ),
        SpellLang::PtBr => (
            "pt_BR/pt_BR.aff",
            979_792,
            "21d8ad2a769a60e17e2b5ea4ef11d4d593a58b9e2a82d642ef82d6a4c5523865",
            "pt_BR/pt_BR.dic",
            4_477_695,
            "a38bfb26b68ece2834e79fe83e48d5792652970ace12db89d1b9674bf9933183",
        ),
        SpellLang::De => (
            "de/de_DE_frami.aff",
            19_067,
            "646bf3333ac69c23e9d794533ee5241d6f755c359e8fe10a648f87613743d594",
            "de/de_DE_frami.dic",
            4_356_858,
            "52d2484a70681386d979e958f2f828a976f0dcdaa680038f371bc70abcf7463a",
        ),
        SpellLang::Ar => (
            "ar/ar.aff",
            86_949,
            "cec30b8621001e49618feb05aec1984c5fcfbf7d2ec309901d5cbf66585217a3",
            "ar/ar.dic",
            7_217_161,
            "2a3e5367f61c1583734db9d66734f5603e6be5c2d227cf5c5cd7e4ca586e34fe",
        ),
    };
    [
        SourceFile {
            installed_as: "dictionary.aff",
            path: aff,
            bytes: aff_bytes,
            sha256: aff_sha,
        },
        SourceFile {
            installed_as: "dictionary.dic",
            path: dic,
            bytes: dic_bytes,
            sha256: dic_sha,
        },
    ]
}

/// The upstream README and license files a language's directory carries,
/// verbatim, installed under their upstream file names beside the pair.
///
/// Most come from the language's own upstream directory. Where that directory
/// names a license without carrying its text (en-GB's LGPL-3.0, the MPL-1.1
/// Koru takes for pt-BR and ar), the text is the copy another directory of the
/// **same pinned release** carries — `es/` has all three — so every byte still
/// comes from one revision.
pub fn notices(lang: SpellLang) -> &'static [SourceFile] {
    const fn notice(
        path: &'static str,
        installed_as: &'static str,
        bytes: u64,
        sha256: &'static str,
    ) -> SourceFile {
        SourceFile {
            installed_as,
            path,
            bytes,
            sha256,
        }
    }
    const MPL_1_1: SourceFile = notice(
        "es/MPL-1.1.txt",
        "MPL-1.1.txt",
        25_755,
        "53692a2ed6c6a2c6ec9b32dd0b820dfae91e0a1fcdf625ca9ed0bdf8705fcc4f",
    );
    match lang {
        SpellLang::EnUs => {
            const N: &[SourceFile] = &[notice(
                "en/README_en_US.txt",
                "README_en_US.txt",
                15_732,
                "168b4c01cb841f766a72f310b065c04d09cff46376c37a81d799593ce751c371",
            )];
            N
        }
        SpellLang::EnGb => {
            const N: &[SourceFile] = &[
                notice(
                    "en/README_en_GB.txt",
                    "README_en_GB.txt",
                    10_063,
                    "8a736381614885b601c20168d0fbb2df7564fa43a239dca3723cb0a4cce60eef",
                ),
                notice(
                    "es/LGPLv3.txt",
                    "LGPLv3.txt",
                    7_639,
                    "a853c2ffec17057872340eee242ae4d96cbf2b520ae27d903e1b2fef1a5f9d1c",
                ),
                notice(
                    "es/GPLv3.txt",
                    "GPLv3.txt",
                    35_147,
                    "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
                ),
            ];
            N
        }
        SpellLang::Es => {
            const N: &[SourceFile] = &[
                notice(
                    "es/README_hunspell_es.txt",
                    "README_hunspell_es.txt",
                    2_615,
                    "0b5c39a863ecb61f5c5631ec1cd977667ef9bda6da6f90e80e9fc0edcbfadd97",
                ),
                notice(
                    "es/LICENSE.md",
                    "LICENSE.md",
                    991,
                    "2b70fd5cb8d3b34cc6ae322c969557a2270b99017594addb2531ef5ffd9e1b17",
                ),
                MPL_1_1,
            ];
            N
        }
        SpellLang::Fr => {
            const N: &[SourceFile] = &[notice(
                "fr_FR/README_fr.txt",
                "README_fr.txt",
                2_758,
                "41af48742037f80ece5338a2a1bbb01111610339af4686ab93f468045659ba6b",
            )];
            N
        }
        SpellLang::PtBr => {
            const N: &[SourceFile] = &[
                notice(
                    "pt_BR/README_pt_BR.txt",
                    "README_pt_BR.txt",
                    16_348,
                    "9974ce691fdc1fe731717d7a2dc668244405fdc2bf9bf3367eb9b29e85177c88",
                ),
                notice(
                    "pt_BR/README_en.txt",
                    "README_en.txt",
                    1_461,
                    "bb064f3db7ba5f5536da7d079773f692c5f928822e8aebc3d78259ff6cceb9e5",
                ),
                MPL_1_1,
            ];
            N
        }
        SpellLang::De => {
            const N: &[SourceFile] = &[
                notice(
                    "de/README_de_DE_frami.txt",
                    "README_de_DE_frami.txt",
                    752,
                    "c141f4f79c428b7348b5012836f4ad3db4d124f288f15effc22696dc876512ae",
                ),
                notice(
                    "de/COPYING_GPLv2",
                    "COPYING_GPLv2",
                    17_992,
                    "e7431ac20e815b6797369e802f9e4bb1b0082485060d8b096235f035d741bd89",
                ),
                notice(
                    "de/COPYING_GPLv3",
                    "COPYING_GPLv3",
                    35_147,
                    "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
                ),
            ];
            N
        }
        SpellLang::Ar => {
            const N: &[SourceFile] = &[
                notice(
                    "ar/README_ar.txt",
                    "README_ar.txt",
                    420,
                    "5ead684ae5211ccf615686959e24bd21ef6eebbb6195997529fda08b993f615e",
                ),
                notice(
                    "ar/COPYING.txt",
                    "COPYING.txt",
                    532,
                    "73480d918cd78d655508e80aa88c022cbd53634524a0f714b5f2519f2cf19124",
                ),
                notice(
                    "ar/AUTHORS.txt",
                    "AUTHORS.txt",
                    200,
                    "bade1d2d0d47dff5956c08732cdb9c02432d9b0b8016a56e27f58b8d7e842462",
                ),
                MPL_1_1,
            ];
            N
        }
    }
}

/// Who may use a language's dictionary, and on what terms Koru does.
///
/// Expressions are SPDX (`MPL-1.1+` is SPDX's "or later" operator;
/// `LicenseRef-SCOWL` names SCOWL's own notices, which have no SPDX id).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct License {
    /// What the upstream README or COPYING file declares.
    pub declared: &'static str,
    /// What Koru distributes the dictionary under: `declared` itself, or —
    /// where upstream offers alternatives — the one Koru chose.
    pub koru_takes: &'static str,
    /// GPL whichever alternative is taken. Such a dictionary may only ever
    /// ship as separate, unmodified data beside the app, never compiled in or
    /// merged into anything of Koru's.
    pub gpl_only: bool,
    /// What a reader of the notices needs that the expression cannot say.
    pub note: &'static str,
}

impl License {
    /// Whether `koru_takes` is a choice among upstream's alternatives.
    pub fn chosen(&self) -> bool {
        self.declared != self.koru_takes
    }
}

/// The license of a language's dictionary, as its notices state it.
pub fn license(lang: SpellLang) -> License {
    let (declared, koru_takes, gpl_only, note) = match lang {
        SpellLang::EnUs => (
            "LicenseRef-SCOWL",
            "LicenseRef-SCOWL",
            false,
            "BSD-style notices (Atkinson, Titze, Kuenning, UKACD) in README_en_US.txt, which UKACD requires verbatim",
        ),
        SpellLang::EnGb => (
            "LGPL-3.0-only",
            "LGPL-3.0-only",
            false,
            "README_en_GB.txt: LGPL (v3 per its changelog), credits (c) OpenStreetMap contributors (ODbL), which must be kept; license texts from es/",
        ),
        SpellLang::Es => (
            "GPL-3.0-or-later OR LGPL-3.0-or-later OR MPL-1.1+",
            "MPL-1.1+",
            false,
            "Koru takes MPL-1.1 or later (LICENSE.md, MPL-1.1.txt)",
        ),
        SpellLang::Fr => (
            "MPL-2.0",
            "MPL-2.0",
            false,
            "Same license as Koru; the upstream directory carries no license text of its own",
        ),
        SpellLang::PtBr => (
            "LGPL-3.0-only OR MPL-1.1+",
            "MPL-1.1+",
            false,
            "The README names the Mozilla Public License without a version; MPL-1.1 or later is our reading; text from es/",
        ),
        SpellLang::De => (
            "GPL-2.0-only OR GPL-3.0-only",
            "GPL-2.0-only OR GPL-3.0-only",
            true,
            "GPL only: ship as separate unmodified data beside the app, or replace; the OASIS-0.1 alternative covers only ODF-primary programs, which Koru is not",
        ),
        SpellLang::Ar => (
            "GPL-2.0-or-later OR LGPL-2.1-or-later OR MPL-1.1+",
            "MPL-1.1+",
            false,
            "Koru takes MPL-1.1 or later (COPYING.txt); text from es/",
        ),
    };
    License {
        declared,
        koru_takes,
        gpl_only,
        note,
    }
}

/// Where a person can see the upstream directory a language comes from, at
/// the pinned release — the source location its notices entry names.
pub fn source_url(lang: SpellLang) -> String {
    let [aff, _] = files(lang);
    let dir = aff.path.rsplit_once('/').map_or("", |(dir, _)| dir);
    format!("https://github.com/LibreOffice/dictionaries/tree/{SOURCE_REF}/{dir}")
}

/// Everything a language's directory receives: the pair, then its notices.
pub fn download_files(lang: SpellLang) -> Vec<SourceFile> {
    let mut all = files(lang).to_vec();
    all.extend_from_slice(notices(lang));
    all
}

/// The whole download for a language, for the row that offers it.
pub fn download_bytes(lang: SpellLang) -> u64 {
    download_files(lang).iter().map(|file| file.bytes).sum()
}

#[cfg(test)]
mod tests;
