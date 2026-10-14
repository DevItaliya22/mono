// SPDX-License-Identifier: MPL-2.0

//! Freshness and deletion: what the surface does with a refresh, a refused
//! delete, and a reveal that arrives before the list does.

use koru_ai_wire::Failure;

use super::*;
use crate::{Deleting, Focus, Listing, Load};

fn failure(code: &str) -> Failure {
    Failure {
        code: code.into(),
        message: "developer detail".into(),
        retryable: true,
        details: Default::default(),
    }
}

/// Nothing on the open path builds a reader, so nothing in a test build can
/// dial a gateway — and an idle surface asks for no wake.
#[test]
fn a_surface_built_under_test_has_no_reader_and_never_awaits() {
    let mut surface = ChatsSurface::new(None);
    assert!(!surface.poll());
    surface.refresh();
    assert_eq!(surface.load(), Load::Cold);
    assert!(!surface.awaiting());
}

/// **UC13.** A delete Koru could not finish — the agent would not stop —
/// leaves the row where it was, saying so.
#[test]
fn a_refused_delete_leaves_the_row_and_says_why() {
    let mut surface = three();
    surface.delete(id(2));
    assert_eq!(surface.deleting(id(2)), Some(&Deleting::Sending));
    assert!(surface.awaiting(), "an answer is owed");
    assert!(rows_of(&mut surface).contains(&Row::Note(1)));

    surface.take_deleted(id(2), Err(failure("agent_still_running")));
    assert_eq!(surface.chats().len(), 3, "the chat is kept");
    assert_eq!(
        surface.deleting(id(2)),
        Some(&Deleting::Refused("agent_still_running".into()))
    );
    assert!(!surface.awaiting());
    assert_eq!(
        crate::text::refusal(koru_i18n::Lang::En, "agent_still_running"),
        "Couldn't stop the agent — try deleting again."
    );
    // Asking again is allowed, and replaces the refusal.
    surface.delete(id(2));
    assert_eq!(surface.deleting(id(2)), Some(&Deleting::Sending));
}

/// A chat the service no longer has is as deleted as one it just removed.
#[test]
fn a_chat_already_gone_is_deleted() {
    let mut surface = three();
    surface.delete(id(1));
    surface.take_deleted(id(1), Err(failure("not_found")));
    assert_eq!(titles(&surface), ["Middle", "Oldest"]);
}

/// A second press while a delete is being sent sends nothing more.
#[test]
fn a_delete_in_flight_is_not_sent_twice() {
    let mut surface = three();
    surface.delete(id(1));
    surface.delete(id(1));
    assert_eq!(surface.deleting(id(1)), Some(&Deleting::Sending));
}

/// A listing that no longer holds a chat clears what was said about it.
#[test]
fn a_listing_forgets_deletes_about_chats_that_are_gone() {
    let mut surface = three();
    surface.take_deleted(id(3), Err(failure("history_clearing")));
    surface.take_listing(Ok(Listing {
        chats: vec![chat(1, "Newest", 1_000), chat(2, "Middle", 60_000)],
        agents: vec![],
    }));
    assert_eq!(surface.deleting(id(3)), None);
}

/// **`Ctrl+Shift+E` on a chat made a moment ago.** The list does not hold it
/// yet; the reveal is remembered and lands when the list that holds it does.
#[test]
fn a_reveal_before_the_list_lands_when_the_list_does() {
    let mut surface = three();
    assert!(!surface.reveal(id(9), area(), TOP));
    assert_eq!(surface.focus(), None);
    let mut chats = vec![chat(9, "Just made", 0)];
    chats.extend(surface.chats().iter().cloned());
    surface.take_listing(Ok(Listing {
        chats,
        agents: vec![],
    }));
    assert_eq!(surface.focus(), Some(Focus::Chat(id(9))));

    // A reveal of a chat the list holds lands at once.
    assert!(surface.reveal(id(3), area(), TOP));
    assert_eq!(surface.focused_chat(), Some(id(3)));
}

#[test]
fn the_open_chat_is_the_hosts_to_say() {
    let mut surface = three();
    assert!(surface.set_current(Some(id(2))));
    assert!(!surface.set_current(Some(id(2))));
    assert_eq!(surface.current(), Some(id(2)));
}

#[test]
fn a_row_is_named_by_title_who_answered_and_time() {
    let mut chats = vec![chat(1, "Release notes", 120_000)];
    chats[0].private = true;
    chats[0].current_model = model("agent-opencode", "default");
    let surface = showing(chats);
    let name = surface.accessible_name(id(1)).unwrap();
    assert!(name.starts_with("Release notes, OpenCode, "), "{name}");
    assert!(name.ends_with(", private"), "{name}");
    assert_eq!(surface.accessible_name(id(7)), None);
}
