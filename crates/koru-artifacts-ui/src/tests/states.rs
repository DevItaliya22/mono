// SPDX-License-Identifier: MPL-2.0

//! **The deterministic states the roadmap names**, one test each: empty,
//! one family, many versions, unregistered-only, a mix, a retired family,
//! and a version this device does not hold.

use super::*;
use crate::model::Load;
use crate::rows::Notice;

/// Nothing here, and it says what would put something here — never a blank
/// column, and never a list of nothing.
#[test]
fn an_empty_workspace_says_what_would_fill_it() {
    let mut surface = showing(Catalog::default());
    assert_eq!(surface.load(), Load::Ready);
    assert_eq!(rows_of(&mut surface), vec![Row::Notice(Notice::Empty)]);

    // Before the first answer it says it is looking — a zero mid-fetch is
    // not a claim about somebody's work.
    let mut cold = ArtifactsSurface::without_reader();
    assert_eq!(cold.load(), Load::Cold);
    assert_eq!(rows_of(&mut cold), vec![Row::Notice(Notice::Loading)]);

    // And a store that will not answer is not an empty column: what was on
    // screen stays, with the state above it.
    let one = family(1, "Findings", 500);
    let versions = history(&one, 2);
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one, versions)],
        Vec::new(),
        Some(present),
    ));
    surface.take_catalog(Err(koru_artifact_wire::Failure::new(
        "artifacts_unavailable",
        "",
        true,
    )));
    assert_eq!(surface.load(), Load::Unavailable);
    let rows = rows_of(&mut surface);
    assert_eq!(rows[0], Row::Notice(Notice::Unavailable));
    assert!(rows.contains(&Row::Family(0)), "{rows:?}");
}

/// One family is its latest artifact row. Opening its caret reveals only the
/// older versions; the latest is never duplicated underneath itself.
#[test]
fn one_family_is_one_row_until_it_is_opened() {
    let one = family(1, "Quarterly findings", 500);
    let versions = history(&one, 3);
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), versions)],
        Vec::new(),
        Some(present),
    ));
    assert_eq!(
        rows_of(&mut surface),
        vec![Row::ResultsHeading, Row::Family(0)]
    );
    assert_eq!(
        crate::text::artifact_row(
            surface.lang(),
            "Quarterly findings",
            surface.catalog().families[0].latest().unwrap(),
        ),
        "Quarterly findings - 3.0"
    );

    surface.toggle_family(one.id);
    assert_eq!(
        rows_of(&mut surface),
        vec![
            Row::ResultsHeading,
            Row::Family(0),
            Row::Version {
                family: 0,
                version: 1
            },
            Row::Version {
                family: 0,
                version: 2
            },
        ]
    );
}

/// **Newest first.** The store answers by ordinal; a person opening a
/// result wants the newest one, and a column that puts version 1 at the top
/// makes them scroll past the history to find it.
#[test]
fn versions_read_newest_first_and_a_selection_shows_its_sentence() {
    let one = family(1, "Findings", 500);
    let versions = history(&one, 3);
    let selected = versions[1].clone();
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), versions.clone())],
        Vec::new(),
        Some(present),
    ));
    surface.toggle_family(one.id);
    let shown = surface.catalog().families[0].versions.clone();
    assert_eq!(
        shown.iter().map(|v| v.ordinal.get()).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );

    // Selecting one reveals its provenance, clause by clause.
    surface.select_version(selected.id);
    surface.take_provenance(
        selected.id,
        walk(&selected, vec![versions[0].clone()], vec![]),
    );
    let rows = rows_of(&mut surface);
    let clauses: Vec<_> = rows
        .iter()
        .filter(|row| matches!(row, Row::Provenance { .. }))
        .collect();
    assert_eq!(clauses.len(), 2, "{rows:?}");
    let sentence = crate::text::provenance_clauses(
        surface.lang(),
        &selected,
        Some(&walk(&selected, vec![versions[0].clone()], vec![])),
        None,
        10_000_000,
    )
    .join(" ");
    assert_eq!(
        sentence,
        "Version 2 came from run 00001001, 2 hours ago. It was built on version 1."
    );

    // Selecting it again puts the sentence away — one gesture, both ways.
    surface.select_version(selected.id);
    assert!(surface.selected().is_none());
}

/// A first version says so rather than saying nothing, and a version a
/// person registered by hand names no run.
#[test]
fn a_first_version_and_a_hand_registered_one_each_read_as_themselves() {
    let one = family(1, "Findings", 500);
    let first = version(&one, 1, Some(run(1)), None);
    let by_hand = version(&one, 2, None, None);
    let lang = koru_i18n::Lang::En;
    assert_eq!(
        crate::text::provenance_clauses(
            lang,
            &first,
            Some(&walk(&first, vec![], vec![])),
            None,
            10_000_000
        )
        .join(" "),
        "Version 1 came from run 00001001, 2 hours ago. It is the first version."
    );
    assert_eq!(
        crate::text::provenance_clauses(
            lang,
            &by_hand,
            Some(&walk(&by_hand, vec![], vec![])),
            None,
            10_000_000
        )
        .join(" "),
        "Version 2 was registered 2 hours ago. It is the first version."
    );
}

/// A later run took a version as input; the sentence says so, and stops
/// naming them after two.
#[test]
fn input_links_are_named_then_counted() {
    let one = family(1, "Findings", 500);
    let subject = version(&one, 1, Some(run(1)), None);
    let inputs = vec![run(2), run(3), run(4), run(5)];
    let clauses = crate::text::provenance_clauses(
        koru_i18n::Lang::En,
        &subject,
        Some(&walk(&subject, vec![], inputs)),
        None,
        10_000_000,
    );
    // Where it came from, that it is the first, two named inputs, and the
    // rest counted — a sentence stops being one at the fourth clause.
    assert_eq!(clauses.len(), 5, "{clauses:?}");
    assert_eq!(clauses[2], "Run 00001002 used it as input.");
    assert_eq!(clauses[3], "Run 00001003 used it as input.");
    assert_eq!(clauses[4], "2 more runs used it as input.");
}

/// **A retired family is retired, not gone** — its versions stay readable
/// and the row says which it is.
#[test]
fn a_retired_family_is_listed_and_marked() {
    let mut one = family(1, "Findings", 500);
    one.retired_at_ms = Some(9_000);
    let versions = history(&one, 2);
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), versions)],
        Vec::new(),
        Some(present),
    ));
    assert!(surface.catalog().families[0].family.is_retired());
    surface.toggle_family(one.id);
    let rows = rows_of(&mut surface);
    assert!(rows.contains(&Row::Version {
        family: 0,
        version: 1
    }));
}

/// **D19, in the column**: registered and unregistered side by side, in
/// their own sections, and a run that registered nothing says so (D29).
#[test]
fn registered_and_unregistered_sit_side_by_side_in_their_own_sections() {
    let one = family(1, "Findings", 500);
    let versions = history(&one, 1);
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one, versions)],
        vec![
            folder(run(1), 1, vec![unregistered("scratch.txt", 40)]),
            folder(run(2), 0, vec![unregistered("out.md", 900)]),
        ],
        Some(present),
    ));
    let rows = rows_of(&mut surface);
    let results = rows.iter().position(|row| *row == Row::ResultsHeading);
    let not_yet = rows.iter().position(|row| *row == Row::UnregisteredHeading);
    assert!(results < not_yet, "{rows:?}");
    assert!(rows.contains(&Row::File { run: 0, file: 0 }));
    assert!(rows.contains(&Row::File { run: 1, file: 0 }));

    // D29: the run that registered nothing carries the sentence; the one
    // that delivered carries none.
    let entries = &surface.catalog().runs;
    assert!(crate::text::run_notes(surface.lang(), &entries[0]).is_empty());
    assert_eq!(
        crate::text::run_notes(surface.lang(), &entries[1]),
        vec![
            "This run registered nothing.".to_owned(),
            "It left 1 file you can register.".to_owned(),
        ]
    );
}

/// A run that registered nothing **and** wrote nothing has nowhere to
/// point, and says that instead of pointing.
#[test]
fn a_run_with_nothing_at_all_says_so_and_a_gone_folder_says_something_else() {
    let surface = showing(Catalog::build(
        Vec::new(),
        vec![folder(run(1), 0, Vec::new())],
        Some(Default::default()),
    ));
    assert_eq!(
        crate::text::run_notes(surface.lang(), &surface.catalog().runs[0]),
        vec![
            "This run registered nothing.".to_owned(),
            "It left no files either.".to_owned(),
        ]
    );

    let mut gone = folder(run(1), 0, Vec::new());
    gone.folder_present = false;
    let surface = showing(Catalog::build(
        Vec::new(),
        vec![gone],
        Some(Default::default()),
    ));
    assert_eq!(
        crate::text::run_notes(surface.lang(), &surface.catalog().runs[0])[1],
        "Its results folder is no longer there."
    );

    // A run that delivered and left nothing behind is not news, and gets no
    // heading at all.
    let quiet = showing(Catalog::build(
        Vec::new(),
        vec![folder(run(1), 2, Vec::new())],
        Some(Default::default()),
    ));
    assert!(quiet.catalog().runs.is_empty());
}

/// Unregistered only: no results at all, and the column is still useful.
#[test]
fn a_workspace_with_only_unregistered_files_shows_them_and_no_results_heading() {
    let mut surface = showing(Catalog::build(
        Vec::new(),
        vec![folder(
            run(1),
            0,
            vec![unregistered("a.md", 10), unregistered("out/b.csv", 20)],
        )],
        Some(Default::default()),
    ));
    let rows = rows_of(&mut surface);
    assert!(!rows.contains(&Row::ResultsHeading), "{rows:?}");
    assert!(rows.contains(&Row::UnregisteredHeading));
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, Row::File { .. }))
            .count(),
        2
    );
}

/// **D27.** A version whose bytes this device does not hold is named,
/// sized and dated all the same, and says the content is elsewhere.
#[test]
fn a_version_this_device_does_not_hold_names_the_device_that_does() {
    let one = family(1, "Findings", 500);
    let versions = history(&one, 1);
    let only = versions[0].clone();
    // The blob listing answered, and this hash was not in it.
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), versions)],
        Vec::new(),
        Some(Default::default()),
    ));
    surface.toggle_family(one.id);
    surface.select_version(only.id);

    let clauses = crate::text::provenance_clauses(
        surface.lang(),
        &only,
        Some(&walk(&only, vec![], vec![])),
        surface.holder(&only),
        10_000_000,
    );
    assert_eq!(clauses[0], "The content is on another device.");

    // With a device name pushed, D27 is answered in full.
    surface.set_run_devices([(run(1), "Ada's laptop".to_owned())].into_iter().collect());
    let clauses = crate::text::provenance_clauses(
        surface.lang(),
        &only,
        Some(&walk(&only, vec![], vec![])),
        surface.holder(&only),
        10_000_000,
    );
    assert_eq!(clauses[0], "The content is on Ada's laptop.");
    // And the size and the moment are still there, which is the rest of D27.
    assert!(clauses[1].contains("2 hours ago"));
    assert_eq!(crate::text::size(surface.lang(), only.size_bytes), "1.0 KB");
}

/// **Unknown is not elsewhere.** Before the blob listing has landed nothing
/// is claimed to be on another device.
#[test]
fn a_blob_listing_that_never_answered_does_not_turn_every_result_into_a_not_here() {
    let one = family(1, "Findings", 500);
    let versions = history(&one, 1);
    let only = versions[0].clone();
    let surface = showing(Catalog::build(vec![(one, versions)], Vec::new(), None));
    assert!(surface.holder(&only).is_none());
}

/// Families read newest first, so the result you just came back for is at
/// the top.
#[test]
fn families_read_newest_first() {
    let old = family(1, "Old", 100);
    let new = family(2, "New", 900);
    let surface = showing(Catalog::build(
        vec![(old, Vec::new()), (new, Vec::new())],
        Vec::new(),
        Some(Default::default()),
    ));
    assert_eq!(
        surface
            .catalog()
            .families
            .iter()
            .map(|entry| entry.family.title.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec!["New".to_owned(), "Old".to_owned()]
    );
}

/// A refresh must not leave the surface holding ids the store has
/// forgotten — one leak per refresh is a window that grows all day.
#[test]
fn a_refresh_drops_what_the_store_no_longer_has() {
    let one = family(1, "Findings", 500);
    let versions = history(&one, 2);
    let newest = versions.last().unwrap().clone();
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), versions)],
        vec![folder(run(1), 0, vec![unregistered("a.md", 1)])],
        Some(present),
    ));
    surface.toggle_family(one.id);
    surface.select_version(newest.id);
    surface.register(run(1), path("a.md"));
    assert!(surface.is_expanded(one.id));
    assert!(surface.selected().is_some());
    assert!(surface.registering(run(1), &path("a.md")).is_some());

    surface.take_catalog(Ok(Catalog::default()));
    assert!(!surface.is_expanded(one.id));
    assert!(surface.selected().is_none());
    assert!(surface.registering(run(1), &path("a.md")).is_none());
}

/// **Revealing a version opens what has to be open to show it, selects it and
/// leaves the arrows on it** — what `Ctrl+Shift+E` on a result tab means
/// (ADR-373). The family is shut and the version is buried past the first page
/// of history, which is the case a plain *select* cannot answer: a row that is
/// not laid out cannot be focused.
#[test]
fn revealing_a_version_opens_its_family_and_focuses_its_row() {
    let one = family(1, "Quarterly findings", 500);
    // Newest first is how the column holds them, so ordinal 1 — the oldest —
    // is the last row of a long history and sits behind *N earlier versions*.
    let versions = history(&one, (crate::VERSIONS_SHOWN as u32) + 4);
    let oldest = versions[0].id;
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), versions.clone())],
        Vec::new(),
        Some(present),
    ));
    assert!(!surface.is_expanded(one.id), "it starts shut");

    assert!(surface.reveal_version(oldest, tall(), TOP), "it was found");
    assert!(surface.is_expanded(one.id), "its family opened");
    assert_eq!(surface.selected(), Some(oldest), "and it is selected");
    let index = versions.len() - 1;
    assert_eq!(
        surface.focus(),
        Some(Row::Version {
            family: 0,
            version: index
        }),
        "with the keyboard on its row"
    );
    assert!(
        rows_of(&mut surface).contains(&Row::Version {
            family: 0,
            version: index
        }),
        "and the row is actually laid out, not merely focused"
    );
}

/// **Revealing is not a toggle.** `select_version` deselects when asked twice,
/// which is right for a click and wrong for a reveal: asking the same question
/// again must not close the answer.
#[test]
fn revealing_the_same_version_twice_leaves_it_selected() {
    let one = family(1, "Quarterly findings", 500);
    let versions = history(&one, 2);
    let newest = versions[1].id;
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one, versions)],
        Vec::new(),
        Some(present),
    ));
    assert!(surface.reveal_version(newest, tall(), TOP));
    assert!(surface.reveal_version(newest, tall(), TOP));
    assert_eq!(surface.selected(), Some(newest));
}

/// **A version this catalog has never heard of selects nothing.** The column
/// has not answered yet, or it is about another workspace; either way the
/// honest answer is `false` and an untouched column, so the caller can put the
/// keyboard somewhere real instead.
#[test]
fn revealing_a_version_the_catalog_does_not_hold_changes_nothing() {
    let one = family(1, "Quarterly findings", 500);
    let versions = history(&one, 2);
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), versions)],
        Vec::new(),
        Some(present),
    ));
    let stranger = ArtifactVersionId::from_uuid(v4(0xdead));
    assert!(!surface.reveal_version(stranger, tall(), TOP));
    assert_eq!(surface.selected(), None);
    assert_eq!(surface.focus(), None);
    assert!(!surface.is_expanded(one.id));
}

/// **A result the person kept says its run was deleted**, rather than
/// printing an id that resolves to nothing (ADR-374). The producer and an
/// input link both get the sentence; a run that is still there keeps its id.
#[test]
fn a_deleted_run_is_named_as_deleted_rather_than_by_its_id() {
    use crate::text::{provenance_clauses, run_label};

    let one = family(1, "Quarterly findings", 500);
    let versions = history(&one, 2);
    let subject = versions[1].clone();
    let maker = subject.produced_by.expect("a producer").run;
    let reader = run(9);
    let mut walk = walk(&subject, vec![versions[0].clone()], vec![reader]);
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one, versions)],
        Vec::new(),
        Some(present),
    ));
    surface.take_provenance(subject.id, walk.clone());

    let still_here =
        provenance_clauses(koru_i18n::Lang::En, &subject, Some(&walk), None, 10_000_000);
    assert!(
        still_here
            .iter()
            .any(|line| line.contains(&run_label(maker))),
        "a run that is still there is named by its id: {still_here:?}"
    );

    walk.deleted_runs = std::collections::BTreeSet::from([maker, reader]);
    let after = provenance_clauses(koru_i18n::Lang::En, &subject, Some(&walk), None, 10_000_000);
    assert!(
        after.iter().any(|line| line.contains("has been deleted")),
        "the producer's clause says the run is gone: {after:?}"
    );
    assert!(
        !after.iter().any(|line| line.contains(&run_label(maker))),
        "and never prints an id that resolves to nothing: {after:?}"
    );
    assert!(
        !after.iter().any(|line| line.contains(&run_label(reader))),
        "nor for the run that read it: {after:?}"
    );
}
