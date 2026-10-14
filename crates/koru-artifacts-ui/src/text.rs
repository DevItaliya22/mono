// SPDX-License-Identifier: MPL-2.0

//! Every word this surface says, and the one place each is assembled.
//!
//! Three rules hold here and are tested next door:
//!
//! - **Nothing is a literal.** Every sentence comes from `koru-i18n`'s
//!   table, so it exists in all eight languages or it does not compile.
//! - **Every time goes through `koru_i18n::time`** and every size through
//!   `koru_i18n::number::bytes`, so a column of results reads the way every
//!   other column in the app does.
//! - **Provenance is a sentence, not a graph.** [`provenance_clauses`]
//!   builds it out of clauses — where the version came from, what it was
//!   built on, who used it since — and the column draws one clause per
//!   line. A person reads *"Version 3 came from run 9f2c11ab, 2 hours ago.
//!   It was built on version 2. Run 41d0e7c2 used it as input."*, which is
//!   the same information a diagram would have carried and none of the
//!   work of reading one.
//!
//! **A run is named by the first characters of its id, never by its
//! objective.** An objective is content (standing requirement 7); an id
//! prefix is an id. A version is named by its ordinal for the same reason a
//! person says *version 3* and not a UUID.

use koru_artifact_wire::run_folder::UnregisteredFile;
use koru_artifact_wire::{ArtifactVersion, ArtifactVersionId, LinkRole, Provenance, RunId};
use koru_i18n::strings::{Key, t};
use koru_i18n::{Lang, fill, number, time};

/// How many characters of a run's id name it. Eight is what every other
/// `Debug` in the delegation vocabulary shows, so one habit reads the same
/// in a log and on screen.
pub const RUN_LABEL_CHARS: usize = 8;

/// How many input links are named one at a time before the rest are
/// counted. A sentence stops being one at the fourth clause.
pub const NAMED_INPUT_RUNS: usize = 2;

/// A run, as a person reads one: the first characters of its id.
pub fn run_label(run: RunId) -> String {
    run.to_storage().chars().take(RUN_LABEL_CHARS).collect()
}

/// A size, in the person's language.
pub fn size(lang: Lang, bytes: u64) -> String {
    number::bytes(lang, bytes)
}

/// How long ago, in the person's language.
pub fn when(lang: Lang, then_ms: i64, now_ms: i64) -> String {
    time::relative_long(lang, then_ms, now_ms)
}

/// A version's row label — the ordinal, never the id.
pub fn version_label(lang: Lang, version: &ArtifactVersion) -> String {
    fill(
        t(lang, Key::ArtifactsVersion),
        &[&number::decimal(lang, u64::from(version.ordinal.get()))],
    )
}

/// A version's row, as it reads without being opened: the ordinal and how
/// long ago it was made.
///
/// **The moment is on the row and not only in the sentence**, because *when*
/// is half of what a person scanning a list of versions is asking; the run
/// that made it and what it was built on are the other half, and those are
/// what the sentence is for.
///
/// The separator lives here rather than in the table: it is punctuation, not
/// a word, and a language does not translate a middle dot.
pub fn version_row(lang: Lang, version: &ArtifactVersion, now_ms: i64) -> String {
    format!(
        "{} · {}",
        version_label(lang, version),
        when(lang, version.created_at_ms, now_ms)
    )
}

/// The primary artifact row: its human title and latest version identifier.
/// The `.0` is presentation, not storage or semantic-version state; artifact
/// ordinals remain the single integer the immutable version model stores.
pub fn artifact_row(lang: Lang, title: &str, version: &ArtifactVersion) -> String {
    format!(
        "{} - {}.0",
        title,
        number::decimal(lang, u64::from(version.ordinal.get()))
    )
}

/// A family's trailing count.
pub fn version_count(lang: Lang, count: usize) -> String {
    if count == 1 {
        t(lang, Key::ArtifactsVersionsOne).to_owned()
    } else {
        fill(
            t(lang, Key::ArtifactsVersionsMany),
            &[&number::decimal(lang, count as u64)],
        )
    }
}

/// The row that reveals the next page of a family's history.
pub fn earlier_versions(lang: Lang, count: usize) -> String {
    fill(
        t(lang, Key::ArtifactsEarlierVersions),
        &[&number::decimal(lang, count as u64)],
    )
}

/// A run's heading.
pub fn run_heading(lang: Lang, run: RunId) -> String {
    fill(t(lang, Key::ArtifactsRun), &[&run_label(run)])
}

/// What a run's heading says underneath it — **D29 in as many words**, and
/// where to look next.
///
/// Empty for a run that delivered and merely left something behind: that is
/// not news, and a note under every heading is a note nobody reads.
pub fn run_notes(lang: Lang, entry: &crate::model::RunEntry) -> Vec<String> {
    let mut notes = Vec::new();
    if entry.registered == 0 {
        notes.push(t(lang, Key::ArtifactsRunRegisteredNothing).to_owned());
        if !entry.folder_present {
            notes.push(t(lang, Key::ArtifactsRunFolderGone).to_owned());
        } else if entry.files.is_empty() {
            notes.push(t(lang, Key::ArtifactsRunLeftNothing).to_owned());
        } else if entry.files.len() == 1 {
            notes.push(t(lang, Key::ArtifactsRunLeftOne).to_owned());
        } else {
            notes.push(fill(
                t(lang, Key::ArtifactsRunLeftMany),
                &[&number::decimal(lang, entry.files.len() as u64)],
            ));
        }
    }
    if entry.truncated {
        notes.push(t(lang, Key::ArtifactsMoreFiles).to_owned());
    }
    notes
}

/// One unregistered file's row label: the name it has where it was
/// written. The folders above it are not drawn — a column is narrow, and
/// the last segment is what a person recognises.
pub fn file_label(file: &UnregisteredFile) -> &str {
    file.path.file_name()
}

/// The provenance sentence, clause by clause (D27 first when it applies).
///
/// `holder` is the device that holds the content when this one does not —
/// `None` when the version is here, and `Some(None)` when it is elsewhere
/// and no device can be named.
pub fn provenance_clauses(
    lang: Lang,
    version: &ArtifactVersion,
    provenance: Option<&Provenance>,
    holder: Option<Option<&str>>,
    now_ms: i64,
) -> Vec<String> {
    let mut clauses = Vec::new();
    // **D27 first.** Where the content is outranks where it came from: a
    // person who cannot reach a result needs that before its history.
    if let Some(device) = holder {
        clauses.push(match device {
            Some(name) => fill(t(lang, Key::ArtifactsHeldElsewhere), &[name]),
            None => t(lang, Key::ArtifactsHeldElsewhereUnknown).to_owned(),
        });
    }
    let ordinal = number::decimal(lang, u64::from(version.ordinal.get()));
    let moment = when(lang, version.created_at_ms, now_ms);
    // **A result outlives the run that made it.** A person may delete a run
    // and keep what it produced; the run's id then resolves to nothing, and
    // printing it would teach nothing. The walk says which runs are gone.
    let deleted = |run: RunId| {
        provenance
            .map(|walk| walk.deleted_runs.contains(&run))
            .unwrap_or(false)
    };
    clauses.push(match version.produced_by {
        Some(by) if deleted(by.run) => fill(
            t(lang, Key::ArtifactsProvenanceProducedDeleted),
            &[&ordinal, &moment],
        ),
        Some(by) => fill(
            t(lang, Key::ArtifactsProvenanceProduced),
            &[&ordinal, &run_label(by.run), &moment],
        ),
        None => fill(
            t(lang, Key::ArtifactsProvenanceRegistered),
            &[&ordinal, &moment],
        ),
    });
    // The chain, one link. The ancestor's *ordinal* is what a person reads,
    // and the walk is the only thing that knows it.
    match (version.derived_from, provenance) {
        (Some(parent), Some(walk)) => {
            if let Some(ancestor) = ancestor(walk, parent) {
                clauses.push(fill(
                    t(lang, Key::ArtifactsProvenanceBuiltOn),
                    &[&number::decimal(lang, u64::from(ancestor.ordinal.get()))],
                ));
            }
        }
        (None, _) => clauses.push(t(lang, Key::ArtifactsProvenanceFirst).to_owned()),
        // Derived from something the walk has not arrived with yet: say
        // nothing rather than say *first*, which would be false.
        (Some(_), None) => {}
    }
    if let Some(walk) = provenance {
        let inputs: Vec<RunId> = walk
            .links(version.id, LinkRole::Input)
            .into_iter()
            .map(|link| link.run)
            .collect();
        for run in inputs.iter().take(NAMED_INPUT_RUNS) {
            clauses.push(match walk.deleted_runs.contains(run) {
                true => t(lang, Key::ArtifactsProvenanceUsedByDeleted).to_owned(),
                false => fill(t(lang, Key::ArtifactsProvenanceUsedBy), &[&run_label(*run)]),
            });
        }
        let rest = inputs.len().saturating_sub(NAMED_INPUT_RUNS);
        if rest > 0 {
            clauses.push(fill(
                t(lang, Key::ArtifactsProvenanceUsedByMore),
                &[&number::decimal(lang, rest as u64)],
            ));
        }
    }
    clauses
}

fn ancestor(walk: &Provenance, id: ArtifactVersionId) -> Option<&ArtifactVersion> {
    walk.versions().find(|version| version.id == id)
}

/// A refusal, as a sentence.
///
/// **The code is never drawn.** It is stable so that this table can be
/// written against it and so a log can carry it; what a person reads is
/// what to do about it. A code with no row of its own still gets a
/// sentence, because a button that stops responding teaches nothing.
pub fn refusal(lang: Lang, code: &str) -> &'static str {
    let key = match code {
        "file_changed" => Key::ArtifactsRefusedFileChanged,
        "path_outside_run_folder" | "path_traversal" | "invalid_request" => {
            Key::ArtifactsRefusedOutside
        }
        "run_folder_missing" => Key::ArtifactsRefusedFolderMissing,
        "file_not_found" => Key::ArtifactsRefusedNotFound,
        "not_a_file" => Key::ArtifactsRefusedNotAFile,
        "path_symlink" | "path_junction" => Key::ArtifactsRefusedLink,
        "file_unreadable" => Key::ArtifactsRefusedUnreadable,
        "artifact_family_retired" => Key::ArtifactsRefusedRetired,
        "artifact_family_exists" | "artifact_registration_conflict" => Key::ArtifactsRefusedExists,
        "artifacts_unavailable"
        | "artifact_storage_failed"
        | "gateway_unavailable"
        | "store_unavailable" => Key::ArtifactsRefusedUnavailable,
        _ => Key::ArtifactsRefusedOther,
    };
    t(lang, key)
}

/// Whether a failure means the store is not answering at all, rather than
/// this one request being wrong. A store that is down is not an empty
/// column.
pub fn unavailable(code: &str) -> bool {
    matches!(
        code,
        "artifacts_unavailable"
            | "gateway_unavailable"
            | "artifact_storage_failed"
            | "unauthenticated"
            | "gateway_failure"
            | "invalid_response"
    )
}
