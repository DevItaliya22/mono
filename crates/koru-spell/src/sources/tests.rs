// SPDX-License-Identifier: MPL-2.0

use super::*;
use sha2::{Digest, Sha256};

/// The two fetch scripts, compiled in so the comparison walks no disk.
const FETCH_PS1: &str = include_str!("../../../../scripts/setup/fetch-dictionaries.ps1");
const FETCH_SH: &str = include_str!("../../../../scripts/setup/fetch-dictionaries.sh");

// ------------------------------------------------------------------ the table

#[test]
fn every_language_has_a_pinned_https_pair_installed_under_the_reader_names() {
    for lang in SpellLang::ALL {
        let [aff, dic] = files(lang);
        assert_eq!(aff.installed_as, "dictionary.aff");
        assert_eq!(dic.installed_as, "dictionary.dic");
        for file in [aff, dic] {
            assert_pinned(lang, &file);
        }
    }
}

/// Every language carries at least one upstream notice, pinned like the pair,
/// installed under its upstream name — never under a name the reader takes for
/// half a dictionary, and never two notices under one name.
#[test]
fn every_language_carries_pinned_notices_under_their_upstream_names() {
    for lang in SpellLang::ALL {
        let notices = notices(lang);
        assert!(!notices.is_empty(), "{} has no license notice", lang.tag());
        let mut names = Vec::new();
        for notice in notices {
            assert_pinned(lang, notice);
            assert_eq!(
                Some(notice.installed_as),
                notice.path.rsplit('/').next(),
                "{}: a notice keeps its upstream file name",
                lang.tag()
            );
            assert!(
                !matches!(notice.installed_as, "dictionary.aff" | "dictionary.dic"),
                "{}: a notice must not shadow the pair",
                lang.tag()
            );
            assert!(
                !names.contains(&notice.installed_as),
                "{}: two notices install as {}",
                lang.tag(),
                notice.installed_as
            );
            names.push(notice.installed_as);
        }
        assert_eq!(
            download_files(lang).len(),
            2 + notices.len(),
            "a download installs the pair and every notice"
        );
    }
}

/// What every language's license entry must say, and the choices
/// `licensing/THIRD-PARTY.md` (⚑2) records, held so a table edit cannot
/// quietly change one.
#[test]
fn every_language_names_its_license_the_choice_koru_takes_and_the_gpl_flag() {
    for lang in SpellLang::ALL {
        let license = license(lang);
        let tag = lang.tag();
        assert!(!license.declared.is_empty(), "{tag}: no declared license");
        assert!(!license.koru_takes.is_empty(), "{tag}: no license taken");
        assert!(!license.note.is_empty(), "{tag}: no note");
        let alternatives: Vec<&str> = license.declared.split(" OR ").collect();
        if license.chosen() {
            assert!(
                alternatives.contains(&license.koru_takes),
                "{tag}: Koru takes {} but upstream offers only {}",
                license.koru_takes,
                license.declared
            );
        }
        // GPL whatever is taken ⇔ every alternative Koru could take is GPL.
        let all_gpl = license
            .koru_takes
            .split(" OR ")
            .all(|id| id.starts_with("GPL-"));
        assert_eq!(license.gpl_only, all_gpl, "{tag}: the GPL-only flag");
        // A license Koru takes has its text beside the dictionary, except
        // MPL-2.0 (Koru's own LICENSE) and SCOWL (whose notices are the README).
        for id in license.koru_takes.split(" OR ") {
            let text: &[&str] = match id {
                "MPL-1.1+" => &["MPL-1.1.txt"],
                "LGPL-3.0-only" => &["LGPLv3.txt", "GPLv3.txt"],
                "GPL-2.0-only" => &["COPYING_GPLv2"],
                "GPL-3.0-only" => &["COPYING_GPLv3"],
                "MPL-2.0" | "LicenseRef-SCOWL" => &[],
                other => panic!("{tag}: no rule for which file carries {other}'s text"),
            };
            for name in text {
                assert!(
                    notices(lang).iter().any(|n| n.installed_as == *name),
                    "{tag} takes {id} but does not carry {name}"
                );
            }
        }
        // The scripts write these into TOML strings and shell/PowerShell
        // literals; none of those quotes may appear, and .ps1 is ASCII.
        for text in [license.declared, license.koru_takes, license.note] {
            assert!(
                text.is_ascii() && !text.contains(['"', '\'', '\\', '$', '`', '|']),
                "{tag}: {text:?} cannot be written by the fetch scripts"
            );
        }
    }
    let takes = |lang| license(lang).koru_takes;
    assert_eq!(takes(SpellLang::Es), "MPL-1.1+");
    assert_eq!(takes(SpellLang::PtBr), "MPL-1.1+");
    assert_eq!(takes(SpellLang::Ar), "MPL-1.1+");
    assert_eq!(takes(SpellLang::Fr), "MPL-2.0");
    let gpl_only: Vec<SpellLang> = SpellLang::ALL
        .into_iter()
        .filter(|lang| license(*lang).gpl_only)
        .collect();
    assert_eq!(
        gpl_only,
        [SpellLang::De],
        "German is the one GPL-only dictionary"
    );
}

#[test]
fn the_source_url_is_the_upstream_directory_at_the_pinned_release() {
    assert_eq!(
        source_url(SpellLang::PtBr),
        format!("https://github.com/LibreOffice/dictionaries/tree/{SOURCE_REF}/pt_BR")
    );
    for lang in SpellLang::ALL {
        let [aff, dic] = files(lang);
        let dir = source_url(lang).rsplit('/').next().unwrap().to_owned();
        assert!(aff.path.starts_with(&format!("{dir}/")));
        assert!(dic.path.starts_with(&format!("{dir}/")));
    }
}

// ---------------------------------------------------------- the fetch scripts

/// **The scripts and this table are one table.** Each script's language rows
/// must be exactly the rows this table renders, in order, and both must pin
/// the same release — so a language, a notice or a license added in one place
/// and not the others fails here rather than in a package.
#[test]
fn the_fetch_scripts_carry_this_table_row_for_row() {
    let expected: Vec<String> = SpellLang::ALL.into_iter().map(script_row).collect();
    for (name, script, quote) in [("ps1", FETCH_PS1, '\''), ("sh", FETCH_SH, '"')] {
        assert_eq!(
            script_rows(script, quote),
            expected,
            "fetch-dictionaries.{name}"
        );
    }
    let release = SOURCE_REF.strip_prefix("libreoffice-").unwrap();
    assert!(FETCH_PS1.contains(&format!("$Ref = \"{release}\"")));
    assert!(FETCH_SH.contains(&format!("ref=\"{release}\"")));
    assert!(
        FETCH_PS1.is_ascii(),
        "fetch-dictionaries.ps1 must stay ASCII"
    );
}

/// One script row: `tag|aff|dic|notices|declared|takes|GPL only|note`.
fn script_row(lang: SpellLang) -> String {
    let [aff, dic] = files(lang);
    let notices: Vec<&str> = notices(lang).iter().map(|n| n.path).collect();
    let license = license(lang);
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}",
        lang.tag(),
        aff.path,
        dic.path,
        notices.join(" "),
        license.declared,
        license.koru_takes,
        if license.gpl_only { "yes" } else { "no" },
        license.note
    )
}

/// The quoted, pipe-separated language rows of a script, in order.
fn script_rows(script: &str, quote: char) -> Vec<String> {
    script
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix(quote)?.strip_suffix(quote))
        .filter(|row| row.matches('|').count() == 7)
        .map(str::to_owned)
        .collect()
}

// --------------------------------------------------------------- the manifest

/// `manifest.toml` exactly as both scripts write it (they write the same
/// bytes; the fetched-tree test below compares a real one to this).
fn manifest() -> String {
    let mut out = String::from(
        "# Written by scripts/setup/fetch-dictionaries (.ps1 or .sh; both write these exact bytes).\n\
         # A record, not a license review: redistribution stays \"unreviewed\" until plan D2/P8.\n\
         \n\
         format = 2\n\
         \n",
    );
    for lang in SpellLang::ALL {
        let [aff, dic] = files(lang);
        let license = license(lang);
        let notices: Vec<String> = notices(lang)
            .iter()
            .map(|n| format!("\"{}\"", n.path))
            .collect();
        out += &format!(
            "[[dictionary]]\n\
             language = \"{}\"\n\
             source = \"{}\"\n\
             revision = \"{SOURCE_REF}\"\n\
             aff = \"{}\"\n\
             dic = \"{}\"\n\
             notices = [{}]\n\
             license_declared = \"{}\"\n\
             license = \"{}\"\n\
             license_chosen = {}\n\
             gpl_only = {}\n\
             note = \"{}\"\n\
             redistribution = \"unreviewed\"\n\
             \n",
            lang.tag(),
            source_url(lang),
            aff.path,
            dic.path,
            notices.join(", "),
            license.declared,
            license.koru_takes,
            license.chosen(),
            license.gpl_only,
            license.note,
        );
    }
    out
}

/// The manifest's schema: every field required, nothing unknown, so a
/// manifest that loses a license or a source fails to parse at all.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    dictionary: Vec<Entry>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    language: String,
    source: String,
    revision: String,
    aff: String,
    dic: String,
    notices: Vec<String>,
    license_declared: String,
    license: String,
    license_chosen: bool,
    gpl_only: bool,
    note: String,
    redistribution: String,
}

/// The manifest names a source, a revision, notices and a license for every
/// language the scripts fetch — once each — and agrees with this table.
#[test]
fn the_manifest_names_a_license_and_a_source_for_every_language() {
    let manifest: Manifest = toml::from_str(&manifest()).expect("the manifest is TOML");
    assert_eq!(manifest.format, 2);
    let tags: Vec<&str> = manifest
        .dictionary
        .iter()
        .map(|e| e.language.as_str())
        .collect();
    let expected: Vec<&str> = SpellLang::ALL.into_iter().map(SpellLang::tag).collect();
    assert_eq!(tags, expected, "one entry per language, in table order");
    for (lang, entry) in SpellLang::ALL.into_iter().zip(&manifest.dictionary) {
        let [aff, dic] = files(lang);
        let license = license(lang);
        assert!(entry.source.starts_with("https://") && entry.source.contains(SOURCE_REF));
        assert_eq!(entry.revision, SOURCE_REF);
        assert_eq!(
            (entry.aff.as_str(), entry.dic.as_str()),
            (aff.path, dic.path)
        );
        let paths: Vec<&str> = notices(lang).iter().map(|n| n.path).collect();
        assert_eq!(entry.notices, paths);
        assert!(!entry.notices.is_empty());
        assert_eq!(entry.license_declared, license.declared);
        assert_eq!(entry.license, license.koru_takes);
        assert!(!entry.license.is_empty());
        assert_eq!(entry.license_chosen, license.chosen());
        assert_eq!(entry.gpl_only, license.gpl_only);
        assert!(!entry.note.is_empty());
        assert_eq!(entry.redistribution, "unreviewed");
    }
}

/// **A fetched tree, checked file by file.** Needs a tree written by
/// `scripts/setup/fetch-dictionaries` at `KORU_DICTIONARIES`; skips, saying
/// so, when there is none. Every language directory must hold the pinned pair
/// and every notice, byte for byte, and the manifest must be the one this
/// table renders — an older fetch, without notices, fails with the fix.
#[test]
fn a_fetched_tree_carries_every_notice_and_its_manifest() {
    let Some(root) = koru_env::value(koru_env::name::KORU_DICTIONARIES) else {
        eprintln!(
            "KORU_DICTIONARIES is not set; fetched-tree license check skipped (run scripts/setup/fetch-dictionaries and point it there)"
        );
        return;
    };
    let root = std::path::PathBuf::from(root);
    let refetch = "re-run scripts/setup/fetch-dictionaries into it";
    let written = std::fs::read_to_string(root.join("manifest.toml"))
        .unwrap_or_else(|e| panic!("{}: no manifest.toml ({e}); {refetch}", root.display()));
    assert!(
        written == manifest(),
        "{}/manifest.toml is not the manifest koru_spell::sources describes; {refetch}",
        root.display()
    );
    for lang in SpellLang::ALL {
        let dir = root.join(lang.tag());
        for file in download_files(lang) {
            let path = dir.join(file.installed_as);
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("{}: {e}; {refetch}", path.display()));
            assert_eq!(bytes.len() as u64, file.bytes, "{}", path.display());
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                file.sha256,
                "{} is not the pinned {}",
                path.display(),
                file.path
            );
        }
    }
}

fn assert_pinned(lang: SpellLang, file: &SourceFile) {
    assert!(file.url().starts_with("https://"), "{lang:?}");
    assert!(file.url().contains(SOURCE_REF));
    assert_eq!(file.sha256.len(), 64);
    assert!(
        file.sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert!(file.bytes > 0);
}
