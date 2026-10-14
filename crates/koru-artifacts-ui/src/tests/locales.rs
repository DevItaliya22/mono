// SPDX-License-Identifier: MPL-2.0

//! **All eight languages, for every word this column says**, and every
//! time and size through `koru_i18n`'s own formatters.
//!
//! The compiler already refuses a table row with a missing translation.
//! What it cannot see is a call site that assembled a sentence in English
//! by hand, or a placeholder that was never filled — which is what these
//! assert, by walking every sentence in every language and looking for the
//! marks a broken one leaves.

use koru_i18n::strings::{Key, t};
use koru_i18n::{Lang, time};

use super::*;

/// Every key this surface reads, in table order. **Written out** rather
/// than derived, because it is what the completeness check compares
/// against: a list generated from the same source it is checking would
/// prove nothing.
const KEYS: &[Key] = &[
    Key::ArtifactsTitle,
    Key::ArtifactsResults,
    Key::ArtifactsNotYetResults,
    Key::ArtifactsLoading,
    Key::ArtifactsUnavailable,
    Key::ArtifactsEmpty,
    Key::ArtifactsVersion,
    Key::ArtifactsVersionsOne,
    Key::ArtifactsVersionsMany,
    Key::ArtifactsRetired,
    Key::ArtifactsEarlierVersions,
    Key::ArtifactsRegister,
    Key::ArtifactsRegistering,
    Key::ArtifactsRun,
    Key::ArtifactsRunRegisteredNothing,
    Key::ArtifactsRunLeftOne,
    Key::ArtifactsRunLeftMany,
    Key::ArtifactsRunLeftNothing,
    Key::ArtifactsRunFolderGone,
    Key::ArtifactsMoreFiles,
    Key::ArtifactsHeldElsewhere,
    Key::ArtifactsHeldElsewhereUnknown,
    Key::ArtifactsProvenanceProduced,
    Key::ArtifactsProvenanceRegistered,
    Key::ArtifactsProvenanceBuiltOn,
    Key::ArtifactsProvenanceFirst,
    Key::ArtifactsProvenanceUsedBy,
    Key::ArtifactsProvenanceUsedByMore,
    Key::ArtifactsRefusedFileChanged,
    Key::ArtifactsRefusedOutside,
    Key::ArtifactsRefusedFolderMissing,
    Key::ArtifactsRefusedNotFound,
    Key::ArtifactsRefusedNotAFile,
    Key::ArtifactsRefusedLink,
    Key::ArtifactsRefusedUnreadable,
    Key::ArtifactsRefusedRetired,
    Key::ArtifactsRefusedExists,
    Key::ArtifactsRefusedUnavailable,
    Key::ArtifactsRefusedOther,
    Key::SizeBytes,
    Key::SizeKilobytes,
    Key::SizeMegabytes,
    Key::SizeGigabytes,
];

#[test]
fn every_word_this_column_says_exists_in_all_eight() {
    for key in KEYS {
        for lang in Lang::ALL {
            let value = t(lang, *key);
            assert!(
                !value.trim().is_empty(),
                "{key:?} is blank in {}",
                lang.tag()
            );
        }
    }
}

/// Every sentence the column assembles, in one language. One function, so
/// the completeness walk and the *is this actually translated* comparison
/// cannot drift apart.
fn sentences(lang: Lang) -> Vec<String> {
    let one = family(1, "Findings", 500);
    let versions = history(&one, 3);
    let newest = versions.last().unwrap().clone();
    let by_hand = version(&one, 4, None, None);
    let entry = crate::model::RunEntry {
        run: run(1),
        folder_present: true,
        registered: 0,
        files: vec![unregistered("a.md", 10), unregistered("b.md", 20)],
        truncated: true,
    };
    let chain = walk(
        &newest,
        vec![versions[1].clone()],
        vec![run(2), run(3), run(4)],
    );
    let mut out = Vec::new();
    out.extend(crate::text::provenance_clauses(
        lang,
        &newest,
        Some(&chain),
        Some(Some("Ada's laptop")),
        10_000_000,
    ));
    out.extend(crate::text::provenance_clauses(
        lang,
        &by_hand,
        None,
        Some(None),
        10_000_000,
    ));
    out.extend(crate::text::run_notes(lang, &entry));
    out.push(crate::text::version_label(lang, &newest));
    out.push(crate::text::version_row(lang, &newest, 10_000_000));
    out.push(crate::text::artifact_row(lang, "Findings", &newest));
    out.push(crate::text::version_count(lang, 1));
    out.push(crate::text::version_count(lang, 7));
    out.push(crate::text::earlier_versions(lang, 40));
    out.push(crate::text::run_heading(lang, run(1)));
    out.push(crate::text::size(lang, 4_404_019));
    out.push(crate::text::when(lang, 0, 10_000_000));
    for code in REFUSAL_CODES {
        out.push(crate::text::refusal(lang, code).to_owned());
    }
    out
}

/// Every sentence, in every language, with nothing left unfilled and
/// nothing left in English by accident.
#[test]
fn every_assembled_sentence_reads_in_every_language() {
    let english = sentences(Lang::En);
    for lang in Lang::ALL {
        let spoken = sentences(lang);
        assert_eq!(spoken.len(), english.len());
        for sentence in &spoken {
            assert!(
                !sentence.contains("{}"),
                "{}: unfilled placeholder in {sentence}",
                lang.tag()
            );
            assert!(
                !sentence.trim().is_empty(),
                "{}: an empty sentence",
                lang.tag()
            );
        }
        // A language other than English must not read as English: if every
        // sentence matched, something is drawing literals.
        if lang != Lang::En {
            assert!(
                spoken.iter().zip(&english).any(|(a, b)| a != b),
                "{} reads exactly like English",
                lang.tag()
            );
        }
    }
}

/// Every stable code the register route can answer with. **The list is the
/// contract**: a code the route raises and this table has no sentence for
/// still gets one, and this asserts that the fall-back is the only thing
/// that ever reaches it for a code we know about.
pub(crate) const REFUSAL_CODES: &[&str] = &[
    "file_changed",
    "path_outside_run_folder",
    "path_traversal",
    "run_folder_missing",
    "file_not_found",
    "not_a_file",
    "path_symlink",
    "path_junction",
    "file_unreadable",
    "artifact_family_retired",
    "artifact_family_exists",
    "artifact_registration_conflict",
    "artifacts_unavailable",
    "invalid_request",
];

#[test]
fn every_refusal_the_route_can_answer_has_its_own_sentence() {
    for code in REFUSAL_CODES {
        assert_ne!(
            crate::text::refusal(Lang::En, code),
            t(Lang::En, Key::ArtifactsRefusedOther),
            "{code} fell through to the last resort"
        );
    }
    // And a code nobody has written a sentence for still says something.
    assert_eq!(
        crate::text::refusal(Lang::En, "something_new"),
        t(Lang::En, Key::ArtifactsRefusedOther)
    );
}

/// **Every time goes through `koru_i18n::time`.** Asserted by comparing
/// what the column says against what that function says — so a hand-rolled
/// "3h ago" anywhere in this crate would fail here.
#[test]
fn every_time_is_koru_i18n_time() {
    for lang in Lang::ALL {
        for (then, now) in [(0i64, 10_000_000i64), (9_000_000, 10_000_000), (0, 0)] {
            assert_eq!(
                crate::text::when(lang, then, now),
                time::relative_long(lang, then, now)
            );
        }
    }
    // And the sentence carries that exact spelling rather than a second
    // one.
    let one = family(1, "Findings", 500);
    let only = version(&one, 1, Some(run(1)), None);
    for lang in Lang::ALL {
        let clauses = crate::text::provenance_clauses(lang, &only, None, None, 10_000_000);
        let moment = time::relative_long(lang, only.created_at_ms, 10_000_000);
        assert!(
            clauses.iter().any(|clause| clause.contains(&moment)),
            "{}: {clauses:?} does not carry {moment}",
            lang.tag()
        );
    }
}

/// A size is `koru_i18n::number::bytes` and nothing else.
#[test]
fn every_size_is_koru_i18n_number() {
    for lang in Lang::ALL {
        for value in [0u64, 999, 1_024, 4_404_019] {
            assert_eq!(
                crate::text::size(lang, value),
                koru_i18n::number::bytes(lang, value)
            );
        }
    }
}

/// A run is named by its id's first characters — **never by its
/// objective**, which is content.
#[test]
fn a_run_is_named_by_its_id_and_nothing_else() {
    let label = crate::text::run_label(run(7));
    assert_eq!(label.chars().count(), crate::text::RUN_LABEL_CHARS);
    assert!(run(7).to_storage().starts_with(&label));
    for lang in Lang::ALL {
        assert!(crate::text::run_heading(lang, run(7)).contains(&label));
    }
}
