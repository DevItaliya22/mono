// SPDX-License-Identifier: MPL-2.0

//! **The result tab: reading one version, and saving the next.**
//!
//! Driven through the same seams the worker uses — `take_content` and
//! `take_saved` — so nothing here dials a gateway, opens a socket or touches
//! a disk.

use koru_artifact_wire::ArtifactVersionId;
use koru_markdown_view::{Content, MarkdownDocument};

use super::{ArtifactTab, OpenedArtifact, SavingVersion, TabRow, VersionPlace};
use crate::tests::{family, history, run, v4, workspace};

const FINDINGS: &str = "# Quarterly findings\n\nRevenue held. Two things to watch:\n\n- the second region\n- the renewal in March\n";

fn a_document(text: &str) -> Content {
    Content::Document(Box::new(MarkdownDocument::of(text).expect("a document")))
}

fn version_id(n: u32) -> ArtifactVersionId {
    ArtifactVersionId::from_uuid(v4(0x30000 + n))
}

/// Where version 1 of the one fixture family sits.
fn place(ordinal: u32) -> VersionPlace {
    VersionPlace {
        family: family(1, "Quarterly findings", 500).id,
        workspace: workspace(),
        ordinal,
    }
}

fn opened(held_elsewhere: Option<Option<String>>) -> OpenedArtifact {
    let one = family(1, "Quarterly findings", 500);
    let versions = history(&one, 2);
    OpenedArtifact {
        version: versions[0].id,
        title: one.title.as_str().to_owned(),
        size_bytes: versions[0].size_bytes,
        produced_by: None,
        held_elsewhere,
    }
}

/// A tab that has read version 1 of the fixture family.
fn reading() -> ArtifactTab {
    let mut tab = ArtifactTab::without_reader();
    let open = opened(None);
    let version = open.version;
    tab.open(open);
    tab.take_content(version, Some(place(1)), a_document(FINDINGS));
    tab
}

/// **The tab is the reader, and the document is the host's** (ADR-372).
/// What the result is called, which version it is, and — because it is
/// Markdown this device holds — the offer to revise it. The version's own
/// text is not among the tab's rows: it is handed over as a document, which
/// is what the host opens in Koru's document view.
#[test]
fn a_version_opens_with_its_name_over_a_document_the_host_draws() {
    let mut tab = reading();
    assert_eq!(tab.title(), Some("Quarterly findings"));
    assert_eq!(tab.version(), Some(version_id(1)));

    let rows = tab.rows();
    assert_eq!(rows[0], TabRow::Title);
    assert_eq!(rows[1], TabRow::Meta);
    assert!(rows.contains(&TabRow::Edit), "{rows:?}");
    assert!(
        !rows.contains(&TabRow::Document),
        "a readable document is not this surface's to draw: {rows:?}"
    );
    // **No way back.** A tab is closed the way every other tab is; a *Back
    // to the results* button was the fifth thing to learn about a document
    // that every other document in Koru opens without one (ADR-369).
    assert!(
        rows.iter()
            .all(|row| row.pressable() || matches!(row, TabRow::Title | TabRow::Meta)),
        "{rows:?}"
    );

    let lines = tab.lines();
    assert!(
        lines.iter().any(|line| line == "Quarterly findings"),
        "{lines:?}"
    );
    // The document goes to the host whole, as the snapshot it was built
    // from — the same bytes the composer would open if the person pressed
    // Edit, so what is read and what would be saved cannot differ.
    let document = tab.document_for_host().expect("a document for the host");
    assert!(
        document.markdown().contains("- the second region"),
        "{:?}",
        document
    );
    assert!(!document.snapshot().is_empty());
    // The bottom of the chrome is where the host puts the page.
    let band = tab.document_top();
    let last = tab.laid_rows().last().copied().expect("a chrome row");
    assert!(band > last.1.y + last.1.h, "{band} vs {:?}", last.1);
}

/// The producing run is an icon immediately left of Edit, with its own hit
/// target. When the host says that run was permanently retired, the icon is
/// removed while the artifact remains readable.
#[test]
fn the_producing_run_sits_beside_edit_and_opens_only_while_available() {
    let mut tab = ArtifactTab::without_reader();
    let mut open = opened(None);
    open.produced_by = Some(run(1));
    let version = open.version;
    tab.open(open);
    tab.take_content(version, Some(place(1)), a_document(FINDINGS));
    assert!(tab.set_run_available(true));

    let laid = tab.laid_rows();
    let run_row = laid
        .iter()
        .find(|(row, _)| *row == TabRow::Run)
        .copied()
        .expect("the producing-run icon");
    let edit = laid
        .iter()
        .find(|(row, _)| *row == TabRow::Edit)
        .copied()
        .expect("Edit");
    assert_eq!(run_row.1.y, edit.1.y, "one action line");
    assert!(run_row.1.x + run_row.1.w < edit.1.x, "icon before Edit");

    assert!(tab.mouse_down(
        run_row.1.x + run_row.1.w * 0.5,
        run_row.1.y + run_row.1.h * 0.5,
    ));
    assert_eq!(tab.take_wants_run(), Some(run(1)));
    assert_eq!(tab.take_wants_run(), None, "taken once");

    assert!(tab.set_run_available(false));
    assert!(
        !tab.rows().contains(&TabRow::Run),
        "retired run has no link"
    );
    assert!(tab.rows().contains(&TabRow::Edit), "artifact is unchanged");
}

/// **A version with nothing in it is a state, not a page.** The sentence
/// saying so stays here rather than becoming an empty column under a name.
#[test]
fn an_empty_version_says_so_instead_of_opening_an_empty_page() {
    let mut tab = ArtifactTab::without_reader();
    let open = opened(None);
    let version = open.version;
    tab.open(open);
    tab.take_content(version, Some(place(1)), a_document("   \n"));
    assert!(tab.document_for_host().is_none());
    assert!(tab.rows().contains(&TabRow::Document), "{:?}", tab.rows());
    // **And no edit.** Editing is the page below, and there is no page —
    // an *Edit* that opened nothing would be a control that lies.
    assert!(!tab.rows().contains(&TabRow::Edit), "{:?}", tab.rows());
    tab.begin_editing();
    assert!(
        !tab.is_editing(),
        "and it is refused however it is asked for"
    );
    assert!(
        tab.lines().contains(
            &koru_markdown_view::text::empty_document(koru_i18n::Lang::default()).to_owned()
        ),
        "{:?}",
        tab.lines()
    );
}

/// **D27.** A version this device does not hold says where its content is
/// instead of offering something it cannot give — and offers no edit.
#[test]
fn a_version_this_device_does_not_hold_says_where_it_is() {
    let mut tab = ArtifactTab::without_reader();
    tab.open(opened(Some(Some("the laptop".to_owned()))));
    let lines = tab.lines();
    assert!(
        lines.contains(&koru_markdown_view::text::held_elsewhere(
            koru_i18n::Lang::default(),
            Some("the laptop")
        )),
        "{lines:?}"
    );
    assert!(!tab.rows().contains(&TabRow::Edit), "{:?}", tab.rows());
}

/// Anything that is not Markdown is named rather than drawn, and not edited.
#[test]
fn a_version_that_is_not_markdown_says_what_it_is() {
    let mut tab = ArtifactTab::without_reader();
    let open = opened(None);
    let version = open.version;
    tab.open(open);
    tab.take_content(
        version,
        Some(place(1)),
        Content::NotMarkdown {
            mime: "application/pdf".into(),
            size_bytes: 4_404_019,
            filename: Some("findings.pdf".into()),
        },
    );
    let shown = tab.lines().join("\n");
    assert!(shown.contains("application/pdf"), "{shown}");
    assert!(shown.contains("4.2 MB"), "{shown}");
    assert!(!tab.rows().contains(&TabRow::Edit), "{:?}", tab.rows());
}

/// **A version the store would not describe is read, never edited.** Saving
/// would not know which family it was making the next version of.
#[test]
fn a_version_with_nowhere_to_save_offers_no_edit() {
    let mut tab = ArtifactTab::without_reader();
    let open = opened(None);
    let version = open.version;
    tab.open(open);
    tab.take_content(version, None, a_document(FINDINGS));
    assert!(!tab.rows().contains(&TabRow::Edit), "{:?}", tab.rows());
    assert_eq!(tab.next_ordinal(), None);
}

/// **ADR-344, said before the click.** The control names the version it
/// would make and the line under it says the open one does not change.
#[test]
fn saving_says_it_makes_a_version_rather_than_changing_one() {
    let mut tab = reading();
    tab.begin_editing();

    let label = tab.save_label();
    assert!(label.contains('2'), "the control names version 2: {label}");
    let notes = tab.notes();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains('2'), "{notes:?}");
    assert!(notes[0].contains('1'), "{notes:?}");

    let rows = tab.rows();
    assert!(rows.contains(&TabRow::Save), "{rows:?}");
    assert!(rows.contains(&TabRow::StopEditing), "{rows:?}");
    assert!(!rows.contains(&TabRow::Edit), "{rows:?}");
}

/// A save that landed leaves the tab on the version it made; a save that did
/// not says so and names the version that is still exactly what it was.
#[test]
fn a_save_makes_version_two_and_a_refusal_leaves_version_one() {
    let mut tab = reading();
    tab.begin_editing();

    let next = ArtifactVersionId::from_uuid(v4(0x50001));
    tab.take_saved(Ok((
        next,
        2,
        Box::new(a_document("# Quarterly findings\n\nRevised.\n")),
    )));
    assert_eq!(tab.version(), Some(next));
    assert_eq!(tab.next_ordinal(), Some(3));
    assert!(matches!(tab.saving(), SavingVersion::Idle));
    assert!(
        tab.document_for_host()
            .expect("the version it made")
            .markdown()
            .contains("Revised."),
        "the tab follows the version it made"
    );

    let mut refused = reading();
    let held = refused.version().unwrap();
    refused.begin_editing();
    refused.take_saved(Err(koru_artifact_wire::Failure::new(
        "artifact_family_retired",
        "",
        false,
    )));
    assert!(matches!(refused.saving(), SavingVersion::Refused(_)));
    assert_eq!(
        refused.version(),
        Some(held),
        "nothing about the open version changed"
    );
    let notes = refused.notes();
    assert!(
        notes.iter().any(|note| note.contains(crate::text::refusal(
            koru_i18n::Lang::default(),
            "artifact_family_retired"
        ))),
        "{notes:?}"
    );
}

/// **The rule the roadmap set for P1.13, arrived at from the other side.** A
/// version a thousand times longer costs this tab exactly the same, because
/// it lays out none of it: the chrome is a fixed handful of rows whatever
/// the document says, and the document itself is the host's page.
#[test]
fn a_versions_length_costs_this_tab_nothing() {
    let cost = |count: usize| {
        let text = (0..count)
            .map(|index| format!("Paragraph {index}.\n"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut tab = ArtifactTab::without_reader();
        let open = opened(None);
        let version = open.version;
        tab.open(open);
        tab.take_content(version, Some(place(1)), a_document(&text));
        let rows = tab.rows().len();
        (rows, tab.view().work().blocks)
    };
    assert_eq!(cost(5), cost(5_000));
    assert_eq!(cost(5_000).1, 0, "no block of it is laid out here");
}

/// **Editing keeps the page and swaps the controls** (ADR-378). There is no
/// composer on this tab any more: *Edit* means the document below opens for
/// typing, and the only thing that changes up here is which controls are
/// offered. The tab draws no document either way.
#[test]
fn editing_keeps_the_page_and_swaps_the_controls() {
    let mut tab = reading();
    assert!(!tab.rows().contains(&TabRow::Document), "{:?}", tab.rows());
    let before = tab
        .document_for_host()
        .expect("a page")
        .markdown()
        .to_owned();

    tab.begin_editing();
    assert!(tab.is_editing());
    assert!(
        !tab.rows().contains(&TabRow::Document),
        "the page is still the host's: {:?}",
        tab.rows()
    );
    assert_eq!(
        tab.document_for_host().expect("the same page").markdown(),
        before,
        "the same bytes, now open for typing"
    );
    assert_eq!(tab.view().work().blocks, 0, "nothing is laid out here");

    tab.stop_editing();
    assert!(!tab.is_editing());
    assert!(tab.rows().contains(&TabRow::Edit), "{:?}", tab.rows());
}

/// **A save is asked for, not performed here.** The bytes are the host's
/// document; turning one into Markdown is work proportional to it and would
/// otherwise happen in the frame that answered the click.
#[test]
fn a_press_on_save_asks_the_host_for_the_markdown() {
    let mut tab = reading();
    tab.begin_editing();
    assert!(!tab.take_save_request());

    let save = tab
        .laid_rows()
        .into_iter()
        .find(|(row, _)| *row == TabRow::Save)
        .expect("a Save row");
    assert!(tab.mouse_down(save.1.x + 4.0, save.1.y + save.1.h * 0.5));
    assert!(tab.take_save_request(), "the ask was recorded");
    assert!(!tab.take_save_request(), "and taken exactly once");
    assert!(
        matches!(tab.saving(), SavingVersion::Idle),
        "nothing was sent by the press itself"
    );
}

/// **A save whose text is what the version already holds is not sent.** It
/// would register a second copy of a version rather than a revision of it.
#[test]
fn saving_text_the_version_already_holds_sends_nothing() {
    let mut tab = reading();
    tab.begin_editing();
    let unchanged = tab
        .document_for_host()
        .expect("a page")
        .markdown()
        .to_owned();
    tab.save_as_next_version(unchanged);
    assert!(matches!(tab.saving(), SavingVersion::Idle));
    tab.save_as_next_version("# Quarterly findings\n\nRevised.\n".to_owned());
    assert!(matches!(tab.saving(), SavingVersion::Sending));
}

/// **Whether the page has been typed into is the host's answer**, because
/// only the host has the page. The tab uses it for one thing: whether *Save*
/// is live.
#[test]
fn the_host_says_whether_the_page_has_been_changed() {
    let mut tab = reading();
    tab.begin_editing();
    assert!(!tab.document_edited());
    tab.set_document_edited(true);
    assert!(tab.document_edited());
    // And an edit that is closed starts again from untouched.
    tab.stop_editing();
    assert!(!tab.document_edited());
}

/// **The round trip, said before the first keystroke** (ADR-378).
///
/// A result whose Markdown is not what Koru would write gets one more
/// sentence when the edit opens, because saving rewrites those lines whether
/// or not anybody touched them. A result that *is* its own round trip — the
/// ordinary case, and every shape `koru-markdown-view`'s
/// `what_survives_byte_for_byte` lists — says nothing, because there is
/// nothing to say.
#[test]
fn a_result_that_would_be_respelled_says_so_before_it_is_edited() {
    let respelled = "# Findings\n\n3. three\n4. four\n";
    let mut tab = ArtifactTab::without_reader();
    let open = opened(None);
    let version = open.version;
    tab.open(open);
    tab.take_content(version, Some(place(1)), a_document(respelled));
    // Nothing is said while it is only being read.
    assert_eq!(tab.notes().len(), 0);

    tab.begin_editing();
    let notes = tab.notes();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!(
        notes[1],
        koru_i18n::t(
            koru_i18n::Lang::default(),
            koru_i18n::Key::ArtifactsSaveRespells
        ),
        "{notes:?}"
    );

    // The fixture the rest of this file uses survives its own round trip, so
    // it gets the one sentence about versions and no second one.
    let mut faithful = reading();
    faithful.begin_editing();
    assert_eq!(faithful.notes().len(), 1, "{:?}", faithful.notes());
}

/// Every word the tab draws exists in all eight languages, and nothing is
/// left with an unfilled placeholder in it.
#[test]
fn the_tab_vocabulary_exists_in_every_language() {
    for lang in koru_i18n::Lang::ALL {
        let mut tab = reading();
        tab.set_lang(lang);
        assert!(tab.rows().contains(&TabRow::Edit), "{}", lang.tag());
        for key in [
            koru_i18n::Key::ArtifactsOpen,
            koru_i18n::Key::ArtifactsEdit,
            koru_i18n::Key::ArtifactsStopEditing,
            koru_i18n::Key::ArtifactsVersion,
        ] {
            assert!(
                !koru_i18n::t(lang, key).trim().is_empty(),
                "{key:?} in {}",
                lang.tag()
            );
        }
        assert!(!tab.meta_line().contains("{}"), "{}", lang.tag());
        tab.begin_editing();
        assert!(!tab.save_label().trim().is_empty(), "{}", lang.tag());
        assert!(!tab.save_label().contains("{}"), "{}", lang.tag());
        assert_eq!(tab.notes().len(), 1, "{}", lang.tag());
        assert!(!tab.notes()[0].contains("{}"), "{}", lang.tag());
    }
}

/// **Every control is reachable from the keyboard**, in the one ordered list
/// the pointer hits, and Escape lets go.
#[test]
fn every_control_is_reachable_from_the_keyboard() {
    use winit::keyboard::{Key, ModifiersState, NamedKey};
    let mut tab = reading();
    let tab_key = Key::Named(NamedKey::Tab);
    let enter = Key::Named(NamedKey::Enter);
    let escape = Key::Named(NamedKey::Escape);

    assert!(tab.key(&tab_key, ModifiersState::empty()));
    assert_eq!(tab.focus(), Some(TabRow::Edit));
    assert!(tab.key(&enter, ModifiersState::empty()));
    assert!(tab.is_editing(), "Enter opened the page for typing");

    // The row the ring was on is gone — *Edit* became *Save* and *Stop
    // editing* — so the ring goes with it rather than pointing at a control
    // that is not there. The keyboard belongs to the page now, which is the
    // host's.
    assert_eq!(tab.rows().iter().filter(|row| row.pressable()).count(), 2);
    assert_eq!(tab.focus(), None);

    // A host whose page has the keyboard hands the ring back with this, and
    // from there Tab walks the controls and Escape lets go again.
    assert!(tab.focus_first_control());
    assert_eq!(tab.focus(), Some(TabRow::Save));
    assert!(tab.key(&escape, ModifiersState::empty()));
    assert_eq!(tab.focus(), None);
    assert!(tab.is_editing(), "letting go of a control closed nothing");

    assert!(tab.key(&tab_key, ModifiersState::empty()));
    assert_eq!(tab.focus(), Some(TabRow::Save));
    assert!(tab.key(&tab_key, ModifiersState::empty()));
    assert_eq!(tab.focus(), Some(TabRow::StopEditing));
    assert!(tab.key(&enter, ModifiersState::empty()));
    assert!(!tab.is_editing(), "the edit closed");
}

/// A press lands where the row is drawn — the same list both walk.
#[test]
fn a_press_lands_on_the_row_it_is_drawn_on() {
    let mut tab = reading();
    let edit = tab
        .laid_rows()
        .into_iter()
        .find(|(row, _)| *row == TabRow::Edit)
        .expect("an Edit row");
    let (_, rect) = edit;
    assert!(tab.mouse_down(rect.x + 4.0, rect.y + rect.h * 0.5));
    assert!(tab.is_editing());
}

/// **An answer for a version this tab no longer holds is not this tab's.**
#[test]
fn a_late_answer_for_another_version_is_ignored() {
    let mut tab = reading();
    tab.take_content(
        ArtifactVersionId::from_uuid(v4(0x60001)),
        Some(place(9)),
        a_document("# Somebody else's\n"),
    );
    assert_eq!(tab.next_ordinal(), Some(2), "the place did not move");
    assert!(tab.lines().iter().any(|line| line == "Quarterly findings"));
}
