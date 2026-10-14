// SPDX-License-Identifier: MPL-2.0

//! **The states the roadmap names, and the two mistakes it names with them.**
//!
//! *Another document's acknowledgement cannot establish success*, and *routine
//! success stays quiet while the question stays answerable*. Everything else
//! here is one of the seven states — pending, acknowledged, locally buffered,
//! buffering failed, stopped link, recovery, read-only — asserted against the
//! sentence a screen reader would actually be given.
//!
//! Nothing here touches disk or reaches the Store. The facts arrive as
//! [`Durability`] values through the real `View::set_durability` seam, which is
//! what the shell pushes; the Store's own half — that those values are per
//! document and per acknowledgement — is proved against real sessions over a
//! fake link in `crates/koru-workspace/src/durability/tests.rs`.

use koru_i18n::Lang;
use koru_workspace_host::View;
use koru_workspace_host::accessibility::{Scope, SemanticNode, ViewSlot, WindowSlot};

use super::*;
use crate::mainview::harness::on_document;

const WINDOW: WindowSlot = WindowSlot::from_raw(1);
const VIEW: ViewSlot = ViewSlot::from_raw(1);

/// A container showing one document, with a real `DocumentId` stamped on the
/// tab — which is what the strip resolves against and what this module asks
/// for. An in-memory fixture has none until something gives it one.
fn seated() -> (MainView, DocumentId) {
    let (view, _workspace, id) = seated_over_a_workspace();
    (view, id)
}

/// The same, keeping the workspace, for the one test that drives a real
/// pointer press — `mouse_down` routes into the document it is handed.
fn seated_over_a_workspace() -> (MainView, koru_workspace::Workspace, DocumentId) {
    let (mut view, workspace) = on_document();
    view.seat_accessibility(WINDOW);
    let id = DocumentId::new();
    let active = view.tabs().active();
    view.tabs_mut()
        .get_mut(active)
        .expect("the active tab")
        .document = Some(id);
    (view, workspace, id)
}

fn nodes(view: &MainView) -> Vec<SemanticNode> {
    let mut out = Vec::new();
    for node in view.semantics(VIEW, Scope::Whole) {
        flatten(&node, &mut out);
    }
    out
}

fn flatten(node: &SemanticNode, out: &mut Vec<SemanticNode>) {
    out.push(node.clone());
    for child in &node.children {
        flatten(child, out);
    }
}

fn find(view: &MainView, part: &str) -> Option<SemanticNode> {
    let wanted = format!("koru:w1/v1/s:save#{part}");
    nodes(view)
        .into_iter()
        .find(|node| node.occurrence.locator() == wanted)
}

/// The sentence a screen reader would read off the status node.
fn status_label(view: &MainView) -> Option<String> {
    find(view, "document").and_then(|node| node.label)
}

/// The sentence the live region would speak, if it is standing.
fn news_label(view: &MainView) -> Option<String> {
    find(view, "news").and_then(|node| node.label)
}

fn says(key: Key) -> String {
    t(koru_workspace_host::accessibility::ui_lang(), key).to_owned()
}

// ------------------------------------------------------------- the seven states

fn saved() -> DocumentDurability {
    DocumentDurability {
        durable: true,
        in_flight: 0,
        buffered: false,
        buffer_failure: None,
        stopped: None,
        link: LinkHealth::Live,
        read_only: None,
    }
}

fn pending() -> DocumentDurability {
    DocumentDurability {
        durable: false,
        in_flight: 1,
        ..saved()
    }
}

fn retrying(seconds: u64) -> LinkHealth {
    LinkHealth::Retrying {
        since: std::time::Instant::now() - std::time::Duration::from_secs(seconds),
    }
}

fn buffered_here() -> DocumentDurability {
    DocumentDurability {
        durable: false,
        in_flight: 0,
        buffered: true,
        link: retrying(30),
        ..saved()
    }
}

fn buffering_failed() -> DocumentDurability {
    DocumentDurability {
        buffered: false,
        buffer_failure: Some("no space left on device".to_owned()),
        ..buffered_here()
    }
}

fn stopped() -> DocumentDurability {
    DocumentDurability {
        durable: false,
        stopped: Some("the store refused 3 updates in a row".to_owned()),
        link: LinkHealth::Stopped,
        ..saved()
    }
}

fn read_only() -> DocumentDurability {
    DocumentDurability {
        durable: false,
        read_only: Some("this document was written by a newer version".to_owned()),
        ..saved()
    }
}

fn one(id: DocumentId, durability: DocumentDurability) -> Durability {
    Durability {
        documents: vec![(id, durability)],
        document_list: None,
    }
}

#[test]
fn every_state_the_plan_names_has_its_own_sentence() {
    for (durability, expected) in [
        (saved(), Key::A11ySaveSaved),
        (pending(), Key::A11ySaveSaving),
        (buffered_here(), Key::A11ySaveBuffered),
        (buffering_failed(), Key::A11ySaveAtRisk),
        (stopped(), Key::A11ySaveStopped),
        (read_only(), Key::A11ySaveReadOnly),
    ] {
        let (mut view, id) = seated();
        view.set_durability(&one(id, durability.clone()));
        assert_eq!(
            status_label(&view),
            Some(says(expected)),
            "{durability:?} was not described as {expected:?}"
        );
    }
}

/// **Read-only is not a failed save**, and the two produce the same Store
/// bookkeeping: work outstanding that will never be sent. Asked in the wrong
/// order, a document an older build must not write reads as one that could not
/// be written, and somebody goes looking for a disk problem.
#[test]
fn read_only_is_not_reported_as_a_failed_save() {
    let (mut view, id) = seated();
    // Every failure signal at once, and read-only on top of them.
    view.set_durability(&one(
        id,
        DocumentDurability {
            read_only: Some("newer build".to_owned()),
            ..buffering_failed()
        },
    ));
    assert_eq!(status_label(&view), Some(says(Key::A11ySaveReadOnly)));
}

// ------------------------------------- another document's acknowledgement

/// **The sharpest test in this plan's list.** Two documents open, one of them
/// acknowledged, and the acknowledged one must not answer for the other. Every
/// workspace-wide reading gets this wrong; the lookup is by document id and the
/// facts are per session, so this one cannot.
#[test]
fn another_documents_acknowledgement_does_not_make_this_one_look_saved() {
    let (mut view, mine) = seated();
    let theirs = DocumentId::new();

    view.set_durability(&Durability {
        // The other tab's document is fully confirmed. Mine is not.
        documents: vec![(theirs, saved()), (mine, pending())],
        document_list: None,
    });

    assert_eq!(
        status_label(&view),
        Some(says(Key::A11ySaveSaving)),
        "the focused document was described using its neighbour's acknowledgement"
    );
    // And the reverse, so the fixture is not simply always answering "saving".
    let (mut view, mine) = seated();
    view.set_durability(&Durability {
        documents: vec![(DocumentId::new(), pending()), (mine, saved())],
        document_list: None,
    });
    assert_eq!(status_label(&view), Some(says(Key::A11ySaveSaved)));
}

/// A document that is not open at all has no answer, rather than a cheerful
/// one inherited from whatever else the workspace holds.
#[test]
fn a_tab_over_no_document_publishes_no_document_status() {
    let (mut view, _id) = seated();
    let active = view.tabs().active();
    view.tabs_mut().get_mut(active).expect("tab").document = None;
    view.set_durability(&one(DocumentId::new(), saved()));

    assert_eq!(status_label(&view), None);
    assert!(
        find(&view, "root").is_none(),
        "a named group with nothing in it is a dead end for somebody asking"
    );
}

// -------------------------------------------------------- quiet, and then news

/// **Routine success stays quiet.** The whole type-publish-acknowledge cycle,
/// several times over, and the live region is never published at all — so
/// `accesskit_windows` has no name change to raise an event from and a screen
/// reader says nothing.
#[test]
fn the_ordinary_saving_cycle_is_never_announced() {
    let (mut view, id) = seated();
    for _ in 0..20 {
        view.set_durability(&one(id, pending()));
        assert_eq!(news_label(&view), None);
        view.set_durability(&one(id, saved()));
        assert_eq!(news_label(&view), None);
    }
    // …and the question is still answerable throughout.
    assert_eq!(status_label(&view), Some(says(Key::A11ySaveSaved)));
}

/// **A failure is said once, and recovery is said once.** The label is what an
/// adapter raises an event from, so "once" is asserted as *the label changed
/// exactly on the transitions and not in between* — a node republished with an
/// identical name raises nothing (`guide/contributing/deps/ACCESSKIT.md`).
#[test]
fn a_failure_announces_once_and_so_does_the_recovery() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    assert_eq!(news_label(&view), None, "nothing has happened yet");

    view.set_durability(&one(id, buffering_failed()));
    let announced = news_label(&view);
    assert_eq!(announced, Some(says(Key::A11ySaveNewsAtRisk)));

    // Three more passes in the same state — a watchdog tick, a keystroke, a
    // redraw — and the announcement does not move, so nothing is said again.
    for _ in 0..3 {
        view.set_durability(&one(id, buffering_failed()));
        assert_eq!(news_label(&view), announced, "the failure spoke twice");
    }

    view.set_durability(&one(id, saved()));
    assert_eq!(news_label(&view), Some(says(Key::A11ySaveNewsRecovered)));
    for _ in 0..3 {
        view.set_durability(&one(id, saved()));
        assert_eq!(
            news_label(&view),
            Some(says(Key::A11ySaveNewsRecovered)),
            "the recovery spoke twice"
        );
    }
}

/// **A Store restarting is not news.** It looks exactly like a link that is
/// retrying, which is why the duration is what decides.
#[test]
fn a_brief_reconnect_says_nothing_and_a_long_one_says_where_the_work_is() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));

    let blip = DocumentDurability {
        link: retrying(1),
        ..pending()
    };
    view.set_durability(&one(id, blip));
    assert_eq!(
        news_label(&view),
        None,
        "a one-second reconnect is a store restarting"
    );

    view.set_durability(&one(id, buffered_here()));
    assert_eq!(news_label(&view), Some(says(Key::A11ySaveNewsBuffered)));
}

/// A session that has given up will not come back, so it is said whether or not
/// anything is currently owed: nothing typed from here on can be saved.
#[test]
fn a_stopped_session_is_announced() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    view.set_durability(&one(id, stopped()));
    assert_eq!(news_label(&view), Some(says(Key::A11ySaveNewsStopped)));
}

/// **The state a client attaches into is status, not news.** A person starting
/// a screen reader while the Store is away is told when they ask; they are not
/// told, as though it had just happened, about something that was already true.
#[test]
fn the_first_observation_seeds_rather_than_announces() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, buffering_failed()));

    assert_eq!(news_label(&view), None, "an arrival is not a transition");
    assert_eq!(
        status_label(&view),
        Some(says(Key::A11ySaveAtRisk)),
        "and the current state is answerable straight away"
    );
}

/// Read-only is a standing state to answer when asked, not a failure to
/// interrupt somebody with.
#[test]
fn read_only_is_not_announced() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    view.set_durability(&one(id, read_only()));
    assert_eq!(news_label(&view), None);
    assert_eq!(status_label(&view), Some(says(Key::A11ySaveReadOnly)));
}

// ------------------------------------------------------------ what the tree is

/// Exactly one node speaks, and it is not the one that moves with every
/// keystroke. `live` is **inherited by descendants**, so a group carrying it
/// would make the status announce itself — the collapse §5 forbids.
#[test]
fn the_announcement_is_live_and_nothing_else_is() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    view.set_durability(&one(id, stopped()));

    let live: Vec<String> = nodes(&view)
        .into_iter()
        .filter(|node| node.live.is_some())
        .map(|node| node.occurrence.locator())
        .collect();
    assert_eq!(live, vec!["koru:w1/v1/s:save#news".to_owned()]);
    assert_eq!(
        find(&view, "news").and_then(|node| node.live),
        Some(Live::Polite)
    );
    assert_eq!(find(&view, "document").and_then(|node| node.live), None);
    assert_eq!(find(&view, "root").and_then(|node| node.live), None);
}

/// A status change must not steal the caret (§2 step 6). It cannot: focus is a
/// function over the states the focus ring is drawn from, and none of them is
/// here.
#[test]
fn a_status_change_does_not_move_accessible_focus() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    let before = view.accessibility_focus(VIEW);

    for state in [pending(), buffering_failed(), stopped(), saved()] {
        view.set_durability(&one(id, state));
        assert_eq!(
            view.accessibility_focus(VIEW),
            before,
            "publishing a save status moved the keyboard"
        );
    }
    // And none of this surface's nodes is ever the focused one.
    assert!(
        !before
            .expect("a focused control")
            .locator()
            .contains("s:save"),
        "the save status is read, not focused"
    );
}

/// **No Store read and no work from a semantic callback**, asserted through the
/// seam rather than by timing: the projection is `&self` over a value the shell
/// pushed, so asking for the tree cannot take another observation.
#[test]
fn a_semantic_query_takes_no_durability_reading_of_its_own() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, pending()));
    let after_push = view.save_status.observations();
    assert_eq!(after_push, 1);

    for _ in 0..50 {
        let _ = view.semantics(VIEW, Scope::Whole);
        let _ = view.semantics_revision();
        let _ = view.accessibility_focus(VIEW);
    }
    assert_eq!(
        view.save_status.observations(),
        after_push,
        "a semantic query read the Store's bookkeeping"
    );
}

/// A view nobody is listening to is never told anything, so it never reads
/// anything. The host's `listening` gate is what enforces it; this pins the
/// view's half — an untouched view has taken no reading at all.
#[test]
fn a_view_that_was_never_told_has_taken_no_reading() {
    let (view, _id) = seated();
    assert_eq!(view.save_status.observations(), 0);
    assert!(find(&view, "root").is_none());
}

// ------------------------------------------------------------- the one control

/// The chip is the existing control, and what an action on it does is what a
/// press on it does — not a second opinion about what pressing it means.
#[test]
fn an_action_on_the_chip_matches_the_press() {
    let (mut view, mut workspace, id) = seated_over_a_workspace();
    view.set_durability(&one(id, buffered_here()));
    view.set_store_trouble(Some(trouble()));

    let chip = find(&view, "chip").expect("a chip while the store is away");
    assert_eq!(chip.role, Role::Button);
    assert_eq!(chip.bounds, view.offline_chip());
    assert!(
        chip.selected.is_none(),
        "a button carrying `selected` is a button that cannot be pressed"
    );

    let target = chip.occurrence;
    assert!(!view.offline_open);
    assert!(super::action(&mut view, target, Action::Click));
    assert!(view.offline_open, "the request did not open the detail");
    assert!(super::action(&mut view, target, Action::Click));
    assert!(!view.offline_open, "a second request did not shut it");

    // The same two presses through the pointer, for comparison: the request
    // and the press are the same expression, so they must agree.
    let (cx, cy) = view.offline_chip().expect("a chip").centre();
    assert!(view.mouse_down(&mut workspace.ws, cx, cy, false));
    assert!(view.offline_open);
    assert!(view.mouse_down(&mut workspace.ws, cx, cy, false));
    assert!(!view.offline_open);
}

/// **Only the control has pixels.** The status sentences are text nothing
/// draws, so they publish no rectangle rather than borrowing the tab's or the
/// chip's — a control published somewhere other than where it is is worse than
/// one with no bounds, which AccessKit serves as an empty rectangle.
#[test]
fn the_chip_has_a_rectangle_and_the_status_text_says_it_has_none() {
    let (mut view, id) = seated();
    // Twice, so there is a transition and the live region is standing too.
    view.set_durability(&one(id, saved()));
    view.set_durability(&one(id, buffering_failed()));
    view.set_store_trouble(Some(trouble()));

    for part in ["root", "document", "storage", "news"] {
        let node = find(&view, part).unwrap_or_else(|| panic!("no {part} node"));
        assert_eq!(node.bounds, None, "{part} borrowed somebody's rectangle");
        assert!(
            node.actions.is_empty(),
            "{part} advertises an action it cannot perform"
        );
    }
    let chip = find(&view, "chip").expect("a chip");
    assert_eq!(chip.bounds, view.offline_chip());
    assert_eq!(chip.actions, vec![Action::Click]);
}

/// The chip's absence is the Store being reachable, and a request naming a node
/// that is no longer published is refused rather than performed.
#[test]
fn there_is_no_chip_while_the_store_is_reachable() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    assert!(find(&view, "chip").is_none());
    assert!(find(&view, "storage").is_none());
    assert_eq!(
        status_label(&view),
        Some(says(Key::A11ySaveSaved)),
        "and the question is still answerable"
    );
}

/// The workspace-wide sentence is the chip's own, assembled by `koru_i18n`.
/// Repeating the assembly here is how two surfaces start describing one Store
/// two ways.
#[test]
fn the_storage_sentence_is_the_chips_own() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, buffered_here()));
    let trouble = trouble();
    view.set_store_trouble(Some(trouble.clone()));

    assert_eq!(
        find(&view, "storage").and_then(|node| node.label),
        Some(crate::mainview::draw::store_detail(
            koru_workspace_host::accessibility::ui_lang(),
            &trouble
        ))
    );
}

fn trouble() -> koru_workspace::StoreTrouble {
    koru_workspace::StoreTrouble {
        message: "connection refused".to_owned(),
        owed: 2,
        since: Some(std::time::Instant::now()),
        fatal: false,
        unconfirmed: koru_workspace::Unconfirmed {
            documents: 1,
            document_list: false,
            not_buffered: None,
        },
    }
}

// ---------------------------------------------------------------- the revision

/// The fingerprint moves when what is published moves, and **not** when the
/// clock does: `StoreTrouble::since` never stops advancing, and folding it in
/// would rebuild the tree on every watchdog tick forever.
#[test]
fn the_revision_follows_the_status_and_not_the_clock() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    let saved_revision = view.semantics_revision();

    view.set_durability(&one(id, pending()));
    assert_ne!(
        view.semantics_revision(),
        saved_revision,
        "the status changed and the tree was not rebuilt"
    );

    view.set_durability(&one(id, saved()));
    assert_eq!(view.semantics_revision(), saved_revision);

    // A second `StoreTrouble` differing only in its instant is the same tree.
    view.set_store_trouble(Some(trouble()));
    let with_trouble = view.semantics_revision();
    view.set_store_trouble(Some(koru_workspace::StoreTrouble {
        since: Some(std::time::Instant::now()),
        ..trouble()
    }));
    assert_eq!(
        view.semantics_revision(),
        with_trouble,
        "a moving clock rebuilt the tree"
    );
}

// ------------------------------------------------------------------- language

/// Every locator is byte-identical in all eight languages and every published
/// label is the translated one — §7.2, held the same way Plan 1a.2 holds it.
#[test]
fn a_locator_is_the_same_in_every_language() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    view.set_durability(&one(id, buffering_failed()));
    view.set_store_trouble(Some(trouble()));

    let locators: Vec<String> = nodes(&view)
        .into_iter()
        .filter(|node| node.occurrence.locator().contains("s:save"))
        .map(|node| node.occurrence.locator())
        .collect();
    assert_eq!(
        locators,
        vec![
            "koru:w1/v1/s:save#root",
            "koru:w1/v1/s:save#document",
            "koru:w1/v1/s:save#storage",
            "koru:w1/v1/s:save#chip",
            "koru:w1/v1/s:save#news",
        ]
    );

    // The labels are rows, and every row answers in every language.
    for lang in Lang::ALL {
        for key in [
            Key::A11ySaveStatus,
            Key::A11ySaveSaved,
            Key::A11ySaveSaving,
            Key::A11ySaveBuffered,
            Key::A11ySaveAtRisk,
            Key::A11ySaveStopped,
            Key::A11ySaveReadOnly,
            Key::A11ySaveNewsBuffered,
            Key::A11ySaveNewsAtRisk,
            Key::A11ySaveNewsStopped,
            Key::A11ySaveNewsRecovered,
        ] {
            assert!(
                !t(lang, key).is_empty(),
                "{key:?} has nothing to say in {lang:?}"
            );
        }
    }
}

// ------------------------------------------------------ the workspace's own list

/// **A workspace with nothing open is not "everything is saved".** Switching
/// away from a workspace whose work was at risk empties the facts, and calling
/// that a recovery would be the most confidently wrong thing this module could
/// say. The last situation is left standing instead.
#[test]
fn an_empty_workspace_is_not_a_recovery() {
    let (mut view, id) = seated();
    view.set_durability(&one(id, saved()));
    view.set_durability(&one(id, buffering_failed()));
    let announced = news_label(&view);
    assert_eq!(announced, Some(says(Key::A11ySaveNewsAtRisk)));

    // No documents and no list: a folder-backed workspace, or the moment
    // between leaving one Store workspace and arriving in the next.
    view.set_durability(&koru_workspace::durability::Durability::default());
    assert_eq!(
        news_label(&view),
        announced,
        "an empty workspace announced a recovery that did not happen"
    );

    // …and coming back to the same state announces nothing either.
    view.set_durability(&one(id, buffering_failed()));
    assert_eq!(news_label(&view), announced);
}

/// A rename nobody typed into is not a document, and it is still work that can
/// be lost. It reaches the announcement without ever being counted as one of
/// the documents.
#[test]
fn the_document_list_can_be_the_news_on_its_own() {
    let (mut view, id) = seated();
    view.set_durability(&Durability {
        documents: vec![(id, saved())],
        document_list: Some(saved()),
    });
    assert_eq!(news_label(&view), None);

    view.set_durability(&Durability {
        documents: vec![(id, saved())],
        document_list: Some(buffering_failed()),
    });
    assert_eq!(news_label(&view), Some(says(Key::A11ySaveNewsAtRisk)));
    assert_eq!(
        status_label(&view),
        Some(says(Key::A11ySaveSaved)),
        "and the document itself is still saved, because it is"
    );
}
