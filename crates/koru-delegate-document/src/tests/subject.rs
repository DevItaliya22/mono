// SPDX-License-Identifier: MPL-2.0

//! **The subject gate for P1.8** (ROADMAP §7.5, §9). A run is
//! manifest-registered, so its events use `SubjectKind::Document` with the
//! Run document's id — no change to `SubjectKind`. This pins the id shape:
//! the `DocumentId` the store minted for the run is what `Subject::document`
//! carries, and it round-trips through the subject's text spelling.

use koru_events::{Subject, SubjectKind};

use super::*;

#[test]
fn a_runs_document_id_is_what_a_document_subject_carries() {
    // The manifest id the store mints in `create_document(ws, title,
    // RUN_KIND, ..)`; the run's own `RunId` is a different id, carried as a
    // typed attribute rather than as the subject.
    let run_document = DocumentId::new();
    let subject = Subject::document(run_document.to_storage());
    assert_eq!(subject.kind, SubjectKind::Document);
    assert_eq!(DocumentId::parse(&subject.id), Some(run_document));
    assert_eq!(subject.version, None);

    let header = header();
    assert_ne!(
        header.run.to_storage(),
        run_document.to_storage(),
        "the RunId is not the subject; the manifest entry's DocumentId is"
    );
}

#[test]
fn a_document_subject_kind_still_has_no_run_variant() {
    // The closed set P1.8 registers rows against. A `Run` variant here
    // would be a schema change the roadmap refused (§7.5).
    assert_eq!(
        SubjectKind::from_token("document"),
        Some(SubjectKind::Document)
    );
    assert_eq!(SubjectKind::from_token("run"), None);
}
