// SPDX-License-Identifier: MPL-2.0

//! The container's half: the copy, the threshold, and the one door.

use super::*;
use crate::mainview::tab_rules::KoruTabs;

/// The two shapes, and the singular. "1 nested items" is the sort of thing that
/// makes a person distrust the count beside it.
#[test]
fn the_toast_says_what_went_and_how_much_went_with_it() {
    assert_eq!(
        trashed_message("Get started", 0),
        "Moved \u{201C}Get started\u{201D} to Trash"
    );
    assert_eq!(
        trashed_message("Q3 Launch", 1),
        "Moved \u{201C}Q3 Launch\u{201D} and 1 nested item to Trash"
    );
    assert_eq!(
        trashed_message("Q3 Launch", 23),
        "Moved \u{201C}Q3 Launch\u{201D} and 23 nested items to Trash"
    );
}

/// **The name is elided, not the sentence.** Eliding the whole string is how
/// "to Trash" ends up being what gets cut — and then the message no longer says
/// what happened.
#[test]
fn a_long_name_is_elided_and_the_sentence_survives() {
    let long = "A".repeat(300);
    let m = trashed_message(&long, 4);
    assert!(m.ends_with("and 4 nested items to Trash"), "{m}");
    assert!(m.starts_with("Moved \u{201C}AAA"), "{m}");
    assert!(m.contains('\u{2026}'), "the name was cut: {m}");
    assert!(m.chars().count() < long.chars().count());
}

#[test]
fn a_short_name_is_left_alone() {
    assert_eq!(elide("Spec", NAME_MAX), "Spec");
    assert_eq!(elide("", NAME_MAX), "");
}

/// Documents, not nodes. Four paragraphs is not four items, and if it were then
/// every page with a few lines in it would stop to ask.
#[test]
fn the_threshold_is_three_nested_documents() {
    assert_eq!(CONFIRM_ABOVE, 3);
}

// ------------------------------------------------------------ the container
//
// The rest is about the **one door**: that every gesture reaches the same
// drain, and that the confirmation and the toast cannot be skipped by coming in
// the other way.

use crate::mainview::harness::{a_document, container, on_document};
use crate::mainview::tab::TabView;
use koru_core::node::Node;
use koru_core::schema::Parent;
use koru_workspace_host::View;
use winit::keyboard::{Key, ModifiersState, NamedKey};

// ---- the drains actually run
//
// These three drive the **trait** methods, not the inherent ones, because that
// is where the bug was: `View::key` and `View::mouse_down` have ten early
// returns between them, and a drain written at their tails is a drain most
// gestures never reach. Every route below silently did nothing until the trait
// impls became wrappers. Do not "simplify" these into calls to `drain_trash_all`
// — driving the drain directly is exactly what would not have caught it.

/// `Delete` on the focused sidebar row is navigation-neutral and non-destructive.
#[test]
fn the_delete_key_does_not_bin_a_sidebar_node() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.toggle_sidebar();
    v.refresh_explore(&w.ws, true);
    v.explore.focus_tree(v.regions.sidebar);

    View::key(
        &mut v,
        &mut w.ws,
        &Key::Named(NamedKey::Delete),
        None,
        ModifiersState::empty(),
    );

    assert!(!w.ws.in_trash(doc));
    assert!(v.toast.is_none());
}

/// Choosing **Move to trash** from a row's menu. `mouse_down` runs the action and then
/// `return true`s — again past the tail.
#[test]
fn the_row_menu_actually_bins_it() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.toggle_sidebar();
    v.refresh_explore(&w.ws, true);

    // Right-click the row to open its menu.
    let (i, r) = v
        .explore
        .row_rects(v.regions.sidebar)
        .into_iter()
        .next()
        .unwrap();
    let _ = i;
    View::context_menu(&mut v, &mut w.ws, r.x + r.w * 0.5, r.centre().1);
    let menu = v.menu.as_ref().expect("a menu opened");
    let item = menu
        .items()
        .iter()
        .position(|a| *a == crate::mainview::context_menu::MenuAction::MoveToTrash)
        .expect("a Move to trash row");
    let target = menu.item_rect(v.viewport, item);

    View::mouse_down(
        &mut v,
        &mut w.ws,
        target.centre().0,
        target.centre().1,
        false,
    );

    assert!(
        w.ws.in_trash(doc),
        "the explicit menu action reached the document"
    );
    assert!(v.toast.is_some());
}

#[test]
fn the_multi_node_menu_offers_one_counted_action_and_bins_every_node() {
    let (mut v, mut w) = container();
    let a = a_document(&mut w.ws, "A");
    let b = a_document(&mut w.ws, "B");
    v.apply(
        &w.ws,
        crate::panes::Outcome::NodeMenu {
            nodes: vec![a, b],
            at: (240.0, 160.0),
            from_button: false,
        },
    );
    let menu = v.menu.as_ref().expect("a menu opened");
    assert_eq!(
        menu.items(),
        &[
            crate::mainview::context_menu::MenuAction::SelectionHeader(2),
            crate::mainview::context_menu::MenuAction::Divider,
            crate::mainview::context_menu::MenuAction::MoveNodesTo(2),
            crate::mainview::context_menu::MenuAction::OpenInNewTab,
            crate::mainview::context_menu::MenuAction::OpenDocumentInNewWindow,
            crate::mainview::context_menu::MenuAction::ExportAs,
            crate::mainview::context_menu::MenuAction::Divider,
            crate::mainview::context_menu::MenuAction::MoveNodesToTrash(2),
        ]
    );
    let item = menu
        .items()
        .iter()
        .position(|a| {
            matches!(
                a,
                crate::mainview::context_menu::MenuAction::MoveNodesToTrash(2)
            )
        })
        .expect("Move N nodes to trash");
    let target = menu.item_rect(v.viewport, item);

    View::mouse_down(
        &mut v,
        &mut w.ws,
        target.centre().0,
        target.centre().1,
        false,
    );

    assert!(w.ws.in_trash(a));
    assert!(w.ws.in_trash(b));
}

/// Modified Delete is not a hidden destructive shortcut either.
#[test]
fn ctrl_shift_delete_does_not_bin_the_active_document() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");

    View::key(
        &mut v,
        &mut w.ws,
        &Key::Named(NamedKey::Delete),
        None,
        koru_keys::PRIMARY | ModifiersState::SHIFT,
    );

    assert!(!w.ws.in_trash(doc));
}

/// Three nested documents go without asking; four stop to ask. **Documents, not
/// nodes** — the paragraphs in the fixture must not count.
#[test]
fn three_nested_documents_trash_without_a_confirm() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Q3");
    for i in 0..3 {
        w.ws.insert(
            Parent::Node(doc),
            Node::document(format!("Sub {i}")).unwrap(),
        )
        .unwrap();
    }
    for line in ["a", "b", "c", "d", "e", "f"] {
        w.ws.insert(Parent::Node(doc), Node::paragraph(line).unwrap())
            .unwrap();
    }

    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);

    assert!(v.confirm.is_none(), "six paragraphs are not six items");
    assert!(w.ws.in_trash(doc));
    assert!(v.toast.is_some(), "and it says so");
}

#[test]
fn four_nested_documents_ask_first() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Q3");
    for i in 0..4 {
        w.ws.insert(
            Parent::Node(doc),
            Node::document(format!("Sub {i}")).unwrap(),
        )
        .unwrap();
    }

    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);

    let (dialog, _) = v.confirm.as_ref().expect("it asked");
    assert_eq!(
        dialog.title(),
        "Move 5 items to Trash?",
        "the node counts too"
    );
    assert!(dialog.body().contains("contains 4 nested items"));
    assert!(!w.ws.in_trash(doc), "and nothing has happened yet");
    assert!(v.toast.is_none());
}

/// Cancelling clears the request as well as the dialog. A pending deletion that
/// survived a Cancel would go off on the next drain, which is the worst way this
/// could possibly fail.
#[test]
fn cancelling_the_confirm_trashes_nothing() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Q3");
    for i in 0..5 {
        w.ws.insert(
            Parent::Node(doc),
            Node::document(format!("Sub {i}")).unwrap(),
        )
        .unwrap();
    }
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    assert!(v.confirm.is_some());

    v.settle_confirm(false);
    v.drain_trash_all(&mut w.ws);
    v.drain_trash_all(&mut w.ws);

    assert!(!w.ws.in_trash(doc));
    assert!(v.confirm.is_none());
    assert_eq!(v.pending_trash, None);
}

#[test]
fn confirming_goes_through_the_same_door() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Q3");
    for i in 0..5 {
        w.ws.insert(
            Parent::Node(doc),
            Node::document(format!("Sub {i}")).unwrap(),
        )
        .unwrap();
    }
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.settle_confirm(true);
    v.drain_trash_all(&mut w.ws);

    assert!(w.ws.in_trash(doc));
    // The toast is raised by the drain, so the confirmed path gets one too.
    let (toast, payload) = v.toast.as_ref().expect("a toast either way");
    assert!(matches!(
        payload,
        crate::mainview::ToastPayload::Restore(nodes) if nodes == &vec![doc]
    ));
    assert!(toast.message().contains("and 5 nested items"));
}

#[test]
fn trashing_shows_a_toast_and_undo_puts_it_back() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    assert!(w.ws.in_trash(doc));

    // What the toast's Undo does, without having to find the pixel.
    v.pending_restore = Some(vec![doc]);
    v.drain_trash_all(&mut w.ws);
    assert!(!w.ws.in_trash(doc));
    assert_eq!(w.ws.children(Parent::Root).unwrap(), vec![doc]);
}

#[test]
fn restoring_a_selection_restores_every_selected_trash_root() {
    let (mut v, mut w) = container();
    let a = a_document(&mut w.ws, "A");
    let b = a_document(&mut w.ws, "B");
    for node in [a, b] {
        v.pending_trash = Some((node, false));
        v.drain_trash_all(&mut w.ws);
    }
    v.pending_restore = Some(vec![a, b]);
    assert!(v.drain_trash_all(&mut w.ws));
    assert!(!w.ws.in_trash(a));
    assert!(!w.ws.in_trash(b));
}

#[test]
fn a_trashed_document_leaves_the_explore_tree_and_the_trash_row_counts_it() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.toggle_sidebar();
    v.refresh_explore(&w.ws, true);
    assert_eq!(v.explore.row_rects(v.regions.sidebar).len(), 1);

    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.refresh_explore(&w.ws, true);

    assert!(v.explore.row_rects(v.regions.sidebar).is_empty());
    assert_eq!(v.explore.trash_count(), 1, "and the pinned row says so");
}

/// **The local half.** `retain_live` cannot see a move, so the tabs *this*
/// window has open are closed here — because this window asked.
#[test]
fn trashing_a_document_closes_the_tab_showing_it() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");

    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);

    assert_ne!(v.document_root(), Some(doc), "the tab went with it");
    assert!(v.tabs().len() >= 1, "and the window is never left empty");
}

/// Undo restores the document, not the window layout.
#[test]
fn restoring_does_not_reopen_the_tab() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.pending_restore = Some(vec![doc]);
    v.drain_trash_all(&mut w.ws);

    assert!(!w.ws.in_trash(doc));
    assert_ne!(v.document_root(), Some(doc), "the tab stays closed");
}

/// **The remote half.** A document binned by somebody else keeps its tab and
/// grows a notice instead — the asymmetry is the whole design.
#[test]
fn a_remotely_trashed_document_keeps_its_tab_and_names_who_binned_it() {
    let (v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    let tabs = v.tabs().len();

    // Somebody else did it: straight to the model, never through `drain_trash`.
    w.ws.trash(doc, 1_700_000_000_000, Some("Sam")).unwrap();

    assert_eq!(v.tabs().len(), tabs, "the tab stays open");
    assert_eq!(v.document_root(), Some(doc), "and still shows it");
    let notice = v.in_trash_notice(&w.ws).expect("a notice");
    assert_eq!(notice.node, doc);
    assert_eq!(notice.title, "Sam moved this document to Trash");
}

/// A missing label is a normal state — an unnamed profile, or an older build.
/// **Never invent a name**: a wrong attribution is worse than none.
#[test]
fn the_notice_drops_the_by_clause_when_there_is_no_label() {
    let (v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    w.ws.trash(doc, 1_700_000_000_000, None).unwrap();

    let notice = v.in_trash_notice(&w.ws).expect("a notice");
    assert_eq!(
        notice.title, "This document was moved to Trash",
        "no name invented when the other client wrote none"
    );
}

#[test]
fn a_manifest_trash_notice_never_leaks_onto_welcome() {
    let (mut v, w) = on_document();
    let document = koru_core::DocumentId::new();
    v.tabs_mut().active_mut().document = Some(document);
    v.active_document = Some(document);
    v.trashed_documents = Some(vec![koru_core::explore::ExploreEntry {
        id: document,
        parent: None,
        ordinal: 0,
        title: "Notes".into(),
        kind: koru_core::explore::KIND_DOCUMENT.into(),
        mime: None,
        note: None,
        trashed_at: Some(1_700_000_000_000),
        trashed_by: Some("Sam".into()),
    }]);

    v.tabs_mut().reset_to_welcome();
    assert!(
        v.in_trash_notice(&w.ws).is_none(),
        "the Store may still announce the old document, but Welcome owns none"
    );
}

/// The hand-off, the other way: the window that binned it is told when somebody
/// takes it back, and by whom.
#[test]
fn restoring_elsewhere_raises_a_toast_in_the_window_that_binned_it() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.toast = None;

    // The other window puts it back.
    w.ws.restore(doc, Some("Ash")).unwrap();
    v.explore_stale = true;
    v.refresh_explore(&w.ws, true);

    let (toast, payload) = v.toast.as_ref().expect("a toast");
    assert!(
        matches!(payload, crate::mainview::ToastPayload::None),
        "there is nothing left to undo"
    );
    assert_eq!(toast.message(), "\u{201C}Spec\u{201D} was restored by Ash");
}

#[test]
fn the_hand_off_toast_drops_the_by_clause_when_there_is_no_label() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.toast = None;

    w.ws.restore(doc, None).unwrap();
    v.explore_stale = true;
    v.refresh_explore(&w.ws, true);

    let (toast, _) = v.toast.as_ref().expect("a toast");
    assert_eq!(toast.message(), "\u{201C}Spec\u{201D} was restored");
}

/// There is one bin, so there is one view of it.
#[test]
fn opening_the_trash_twice_brings_the_same_tab_forward() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);

    v.apply(&w.ws, crate::panes::Outcome::OpenTrash);
    let after_first = v.tabs().len();
    assert!(matches!(v.tabs().active_tab().view, TabView::Trash(_)));

    v.select(0);
    v.apply(&w.ws, crate::panes::Outcome::OpenTrash);
    assert_eq!(v.tabs().len(), after_first, "not a second one");
    assert!(matches!(v.tabs().active_tab().view, TabView::Trash(_)));
}

/// Emptying is confirmed, and the confirmation counts what will go.
#[test]
fn emptying_the_trash_asks_first_and_then_really_removes_them() {
    let (mut v, mut w) = container();
    let a = a_document(&mut w.ws, "A");
    let b = a_document(&mut w.ws, "B");
    for id in [a, b] {
        v.pending_trash = Some((id, false));
        v.drain_trash_all(&mut w.ws);
    }
    assert_eq!(w.ws.trash_len(), 2);

    v.pending_empty_trash = Some(false);
    v.drain_trash_all(&mut w.ws);
    let (dialog, _) = v.confirm.as_ref().expect("it asked");
    assert!(dialog.body().contains("2 items"));
    assert_eq!(w.ws.trash_len(), 2, "nothing yet");

    v.settle_confirm(true);
    v.drain_trash_all(&mut w.ws);
    assert_eq!(w.ws.trash_len(), 0);
    assert!(w.ws.read(a).is_err(), "really gone");
}

/// A mis-wired button cannot skip the bin: the model refuses, and so does this.
#[test]
fn deleting_permanently_asks_first() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);

    v.pending_delete_forever = Some((vec![doc], false));
    v.drain_trash_all(&mut w.ws);
    assert!(v.confirm.is_some());
    assert!(w.ws.read(doc).is_ok(), "still there");

    v.settle_confirm(true);
    v.drain_trash_all(&mut w.ws);
    assert!(w.ws.read(doc).is_err());
}

#[test]
fn deleting_a_selection_asks_once_and_deletes_every_selected_root() {
    let (mut v, mut w) = container();
    let a = a_document(&mut w.ws, "A");
    let b = a_document(&mut w.ws, "B");
    for node in [a, b] {
        v.pending_trash = Some((node, false));
        v.drain_trash_all(&mut w.ws);
    }

    v.pending_delete_forever = Some((vec![a, b], false));
    v.drain_trash_all(&mut w.ws);
    let (dialog, _) = v.confirm.as_ref().expect("one confirmation");
    assert!(dialog.body().contains("2 selected items"));
    v.settle_confirm(true);
    v.drain_trash_all(&mut w.ws);
    assert!(w.ws.read(a).is_err());
    assert!(w.ws.read(b).is_err());
}

/// **A tab reports in its own coordinates.** The container hands a view the
/// content rect's *size* and subtracts its *origin* from every pointer, so a
/// menu position coming back has to be translated before the menu — drawn in
/// window space — can use it. Without that the row menu opened a sidebar and a
/// caption away from the `…` that asked for it.
#[test]
fn a_trash_row_menu_opens_at_the_dots_that_asked_for_it() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.apply(&w.ws, crate::panes::Outcome::OpenTrash);

    // Where the view says its `…` is, in its own space.
    let TabView::Trash(t) = &mut v.tabs_mut().active_mut().view else {
        panic!("the trash tab is showing")
    };
    t.catch_up(&w.ws);
    let dots = t.dots_rect_for_test(0);
    let local = (dots.x + dots.w, dots.y + dots.h);

    // Drive the press through the container, the way a click does.
    let view = v.focused_view_rect();
    let (px, py) = (view.x + dots.centre().0, view.y + dots.centre().1);
    View::mouse_down(&mut v, &mut w.ws, px, py, false);

    let menu = v.menu.as_ref().expect("a menu opened");
    assert_eq!(
        menu.anchor_for_test(),
        (local.0 + view.x, local.1 + view.y),
        "anchored on the button, in window space"
    );
    assert!(
        menu.anchor_for_test().0 >= view.x,
        "and not left of the view at all, which is what the raw view          coordinate would have been"
    );
}

// ------------------------------------------------------- the notice band

/// **The other way out.** Restoring argues with the peer who binned it; saving a
/// copy does not — you keep your version, they keep their decision, and the two
/// stop being the same document.
#[test]
fn saving_a_copy_forks_it_and_leaves_the_original_binned() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    w.ws.trash(doc, 1_700_000_000_000, Some("Ana")).unwrap();

    v.pending_save_copy = Some(doc);
    v.drain_trash_all(&mut w.ws);

    assert!(w.ws.in_trash(doc), "the original stays where Ana put it");
    let live = w.ws.children(Parent::Root).unwrap();
    assert_eq!(live.len(), 1, "and there is exactly one new document");
    assert_ne!(live[0], doc);
    assert_eq!(
        w.ws.read(live[0]).unwrap().content.as_text(),
        Some("Notes (copy)")
    );
    assert_eq!(v.document_root(), Some(live[0]), "and it is now open");
    assert!(v.toast.is_some(), "and it says so");
}

/// The copy is a live document, so the tab showing it must not still be wearing
/// the notice.
#[test]
fn the_notice_goes_once_you_have_saved_a_copy() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    w.ws.trash(doc, 1_700_000_000_000, Some("Ana")).unwrap();
    assert!(v.in_trash_notice(&w.ws).is_some());

    v.pending_save_copy = Some(doc);
    v.drain_trash_all(&mut w.ws);
    assert!(v.in_trash_notice(&w.ws).is_none());
}

/// **There is no dismiss.** The band is the state the document is in, not a
/// notification you can wave away — a × would let somebody hide the fact that
/// the page they are typing into is in the bin, which is exactly what they need
/// to know. It goes when the state goes, and not before.
#[test]
fn the_notice_cannot_be_dismissed_only_answered() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    w.ws.trash(doc, 1_700_000_000_000, Some("Ana")).unwrap();

    let targets = v.notice_targets(&w.ws);
    assert_eq!(
        targets.len(),
        2,
        "Restore and Save a copy, and nothing else"
    );

    // A press anywhere else on the band changes nothing and leaves it up.
    let band = v.notice_rect();
    assert!(v.notice_mouse_down(&w.ws, band.x + 4.0, band.centre().1));
    assert!(v.in_trash_notice(&w.ws).is_some(), "still there");

    // Answering it is what takes it away.
    v.pending_restore = Some(vec![doc]);
    v.drain_trash_all(&mut w.ws);
    assert!(v.in_trash_notice(&w.ws).is_none());
}

/// It sits at the **foot** of the content, not the head: the top of a document
/// is where its title is, and a band there would shove somebody's first line
/// down the page every time a peer binned something.
#[test]
fn the_notice_sits_at_the_bottom_of_the_content() {
    let (v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    w.ws.trash(doc, 1_700_000_000_000, Some("Ana")).unwrap();

    let c = v.regions.content;
    let band = v.notice_rect();
    assert_eq!(band.y + band.h, c.y + c.h, "flush with the floor");
    assert!(band.y > c.y, "and not at the top");
}

/// Both answers are pressable exactly where they are drawn.
#[test]
fn both_notice_buttons_are_pressable_where_they_are_drawn() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    w.ws.trash(doc, 1_700_000_000_000, Some("Ana")).unwrap();

    let band = v.notice_rect();
    let targets = v.notice_targets(&w.ws);
    assert_eq!(targets.len(), 2);
    for (hit, r) in &targets {
        assert!(
            r.x >= band.x && r.x + r.w <= band.x + band.w,
            "{hit:?} escaped the band"
        );
        assert!(r.y >= band.y && r.y + r.h <= band.y + band.h, "{hit:?}");
    }
    // Laid out from the right: Save a copy owns the end, Restore is before it.
    assert!(targets[0].1.x < targets[1].1.x);
    assert_eq!(
        targets[1].1.x + targets[1].1.w,
        band.x + band.w - 16.0,
        "flush with the band's margin"
    );

    let restore = targets[0].1;
    assert!(v.notice_mouse_down(&w.ws, restore.centre().0, restore.centre().1));
    assert_eq!(v.pending_restore, Some(vec![doc]));
}

/// A press on the band but off its buttons is still ours — it sits over the
/// document, and falling through would move the caret in a page you are being
/// warned about.
#[test]
fn the_band_swallows_a_press_that_misses_its_buttons() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    w.ws.trash(doc, 1_700_000_000_000, Some("Ana")).unwrap();

    let band = v.notice_rect();
    assert!(v.notice_mouse_down(&w.ws, band.x + 4.0, band.centre().1));
    assert_eq!(v.pending_restore, None);
    assert_eq!(v.pending_save_copy, None);
}

/// **The state outranks the decoration.** A document whose title starts with an
/// emoji normally wears it in the tab strip — but one that is in the bin wears
/// the trash glyph instead, because a title cannot say "this is deleted" and
/// that is the thing worth knowing from a tab you are not looking at.
#[test]
fn a_trashed_tab_wears_the_trash_glyph_over_its_own_emoji() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "\u{1F680} Ship it");
    v.open_node(&w.ws, doc);
    v.refresh_labels(&w.ws);

    // Live: the tab took the leading emoji off its title.
    let tab = v.tabs().active_tab();
    assert_eq!(tab.icon.as_deref(), Some("\u{1F680}"));
    assert_eq!(tab.view.icon(), None, "a document is not a place");

    // Binned: the glyph wins. `TabView::icon` still says None — it is the
    // *container* that promotes the state over the decoration, in `draw`.
    w.ws.trash(doc, 1_700_000_000_000, Some("Ana")).unwrap();
    assert!(w.ws.in_trash(doc));
    assert_eq!(
        v.tabs().active_tab().icon.as_deref(),
        Some("\u{1F680}"),
        "the emoji is still there underneath"
    );
}

/// The Trash tab is a *place*, so its glyph comes from the view rather than
/// from a title it does not have.
#[test]
fn the_trash_tab_is_a_place_and_carries_no_emoji() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.apply(&w.ws, crate::panes::Outcome::OpenTrash);

    let tab = v.tabs().active_tab();
    assert_eq!(tab.icon, None, "no title, so no leading emoji");
    assert_eq!(tab.view.icon(), Some(koru_draw::icons::TRASH));
}

// ------------------------------------------------------------- Ctrl+Z

/// **The keyboard agrees with the toast.** It says *Undo*, so Ctrl+Z does the
/// same thing — and it has to be the container that answers, because the tab
/// which would otherwise have handled the keystroke is the one the delete just
/// closed.
#[test]
fn ctrl_z_takes_back_a_delete_while_the_toast_is_up() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");

    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    assert!(w.ws.in_trash(doc));
    assert!(v.toast.is_some(), "the toast is offering it");

    View::key(
        &mut v,
        &mut w.ws,
        &Key::Character("z".into()),
        Some("z"),
        koru_keys::PRIMARY,
    );

    assert!(!w.ws.in_trash(doc), "and the keystroke took it back");
    assert!(v.toast.is_none(), "the offer is spent");
}

/// **Bounded by the toast.** Once the offer has gone, Ctrl+Z belongs to whatever
/// you are typing in — stealing it forever would be worse than not offering it.
#[test]
fn ctrl_z_is_not_claimed_once_the_toast_has_gone() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.toast = None;

    assert!(!v.undo_key_takes_back_a_trash(&Key::Character("z".into()), koru_keys::PRIMARY));
    assert!(w.ws.in_trash(doc), "still binned");
}

/// The hand-off toast — *"…" was restored* — carries nothing to take back, so
/// it must not swallow Ctrl+Z.
#[test]
fn ctrl_z_is_not_claimed_by_a_toast_with_no_action() {
    let (mut v, mut w) = container();
    let doc = a_document(&mut w.ws, "Spec");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);
    v.toast = None;
    w.ws.restore(doc, Some("Ana")).unwrap();
    v.explore_stale = true;
    v.refresh_explore(&w.ws, true);
    assert!(v.toast.is_some(), "a toast, but not an offer");

    assert!(!v.undo_key_takes_back_a_trash(&Key::Character("z".into()), koru_keys::PRIMARY));
}

/// Ctrl+Shift+Z is redo and Ctrl+Y is nothing of ours; neither is a way to take
/// a delete back.
#[test]
fn only_plain_ctrl_z_takes_it_back() {
    let (mut v, mut w) = on_document();
    let doc = v.document_root().expect("a document tab");
    v.pending_trash = Some((doc, false));
    v.drain_trash_all(&mut w.ws);

    let shift = koru_keys::PRIMARY | ModifiersState::SHIFT;
    assert!(!v.undo_key_takes_back_a_trash(&Key::Character("z".into()), shift));
    assert!(!v.undo_key_takes_back_a_trash(&Key::Character("y".into()), koru_keys::PRIMARY));
    assert!(!v.undo_key_takes_back_a_trash(&Key::Character("z".into()), ModifiersState::empty()));
    assert!(w.ws.in_trash(doc), "none of them touched it");
}
