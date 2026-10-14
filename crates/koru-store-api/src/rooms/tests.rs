// SPDX-License-Identifier: MPL-2.0

use super::*;

fn channel() -> (mpsc::UnboundedSender<Frame>, mpsc::UnboundedReceiver<Frame>) {
    mpsc::unbounded_channel()
}

// ------------------------------------------------------------------ names

/// **What decides whether the last window may stop the store.**
///
/// A window closing asks the store to stop *only if idle*, and the store
/// answers from this count — so it has to follow a socket's whole life, and a
/// connection that left has to stop being counted. A count that never went
/// down would keep the store alive forever; one that never went up would let a
/// closing window take a live one offline with it.
#[test]
fn live_connections_follows_sockets_in_and_out() {
    let rooms = Rooms::new();
    assert_eq!(rooms.live_connections(), 0, "a fresh store holds nobody");

    let (first, _first_rx) = mpsc::unbounded_channel();
    let (second, _second_rx) = mpsc::unbounded_channel();
    let a = rooms.next_connection();
    let b = rooms.next_connection();
    rooms.register(a, first);
    rooms.register(b, second);
    assert_eq!(rooms.live_connections(), 2);

    // One window closing leaves the other counted — this is the case that
    // would otherwise strand a live window.
    rooms.leave_all(a);
    assert_eq!(rooms.live_connections(), 1);

    rooms.leave_all(b);
    assert_eq!(rooms.live_connections(), 0, "the last one out is countable");
}

#[test]
fn a_room_id_round_trips() {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    for room in [Room::Manifest(workspace), Room::Document(document)] {
        assert_eq!(Room::parse(&room.to_id()), Some(room.clone()), "{room:?}");
    }
}

#[test]
fn a_document_room_names_only_the_document() {
    // A client should not have to know which workspace something is in to open
    // it. Resolving that is the store's job.
    let document = DocumentId::new();
    let id = Room::Document(document).to_id();
    assert_eq!(id, format!("document:{}", document.to_storage()));
    assert!(!id.contains("workspace"), "{id}");
}

#[test]
fn rubbish_room_ids_are_none_rather_than_an_error() {
    // A room id arrives from a client, so it is untrusted input and "I do not
    // know that room" is the whole of the answer.
    for id in [
        "",
        "workspace:",
        "workspace:not-a-uuid:manifest",
        "document:",
        "document:not-a-uuid",
        "workspace:00000000-0000-4000-8000-000000000000",
        "../../etc/passwd",
        "مساحة العمل",
    ] {
        assert_eq!(Room::parse(id), None, "{id:?} parsed");
    }
}

// ------------------------------------------------------------------ membership

#[test]
fn an_empty_room_reaches_nobody() {
    let rooms = Rooms::new();
    let room = Room::Document(DocumentId::new());
    assert_eq!(rooms.subscribers(&room), 0);
    assert_eq!(rooms.broadcast(&room, ConnectionId(0), vec![1]), 0);
}

#[test]
fn a_subscriber_receives_what_others_send() {
    let rooms = Rooms::new();
    let room = Room::Document(DocumentId::new());
    let (sender, mut receiver) = channel();
    let listener = rooms.next_connection();
    let author = rooms.next_connection();
    rooms.join(room.clone(), listener, sender);

    assert_eq!(rooms.broadcast(&room, author, vec![7, 8]), 1);
    assert_eq!(receiver.try_recv().unwrap(), vec![7, 8]);
}

#[test]
fn a_sender_never_receives_its_own_update() {
    // It already has it — that is where it came from — and echoing it would
    // make every edit a round trip that ends in `frame_is_known`.
    let rooms = Rooms::new();
    let room = Room::Document(DocumentId::new());
    let (sender, mut receiver) = channel();
    let author = rooms.next_connection();
    rooms.join(room.clone(), author, sender);

    assert_eq!(rooms.broadcast(&room, author, vec![1]), 0);
    assert!(receiver.try_recv().is_err(), "the author was echoed to");
}

#[test]
fn every_other_subscriber_is_reached() {
    let rooms = Rooms::new();
    let room = Room::Document(DocumentId::new());
    let author = rooms.next_connection();
    let mut receivers = Vec::new();
    for _ in 0..3 {
        let (sender, receiver) = channel();
        rooms.join(room.clone(), rooms.next_connection(), sender);
        receivers.push(receiver);
    }
    assert_eq!(rooms.broadcast(&room, author, vec![9]), 3);
    for mut receiver in receivers {
        assert_eq!(receiver.try_recv().unwrap(), vec![9]);
    }
}

#[test]
fn rooms_are_independent() {
    let rooms = Rooms::new();
    let one = Room::Document(DocumentId::new());
    let two = Room::Document(DocumentId::new());
    let (sender, mut receiver) = channel();
    rooms.join(one.clone(), rooms.next_connection(), sender);

    assert_eq!(rooms.broadcast(&two, ConnectionId(99), vec![1]), 0);
    assert!(receiver.try_recv().is_err(), "a frame crossed rooms");
}

#[test]
fn a_manifest_room_and_a_document_room_are_not_the_same_room() {
    let rooms = Rooms::new();
    let manifest = Room::Manifest(WorkspaceId::new());
    let document = Room::Document(DocumentId::new());
    let (sender, mut receiver) = channel();
    rooms.join(manifest.clone(), rooms.next_connection(), sender);
    assert_eq!(rooms.broadcast(&document, ConnectionId(99), vec![1]), 0);
    assert!(receiver.try_recv().is_err());
}

// ------------------------------------------------------------------ leaving

#[test]
fn leaving_stops_delivery() {
    let rooms = Rooms::new();
    let room = Room::Document(DocumentId::new());
    let (sender, _receiver) = channel();
    let who = rooms.next_connection();
    rooms.join(room.clone(), who, sender);
    rooms.leave(&room, who);
    assert_eq!(rooms.subscribers(&room), 0);
    assert_eq!(rooms.broadcast(&room, ConnectionId(99), vec![1]), 0);
}

#[test]
fn a_closing_connection_leaves_every_room_it_was_in() {
    // A room holding dead senders would grow forever.
    let rooms = Rooms::new();
    let who = rooms.next_connection();
    let a = Room::Document(DocumentId::new());
    let b = Room::Manifest(WorkspaceId::new());
    let (one, _r1) = channel();
    let (two, _r2) = channel();
    rooms.join(a.clone(), who, one);
    rooms.join(b.clone(), who, two);

    rooms.leave_all(who);
    assert_eq!(rooms.subscribers(&a), 0);
    assert_eq!(rooms.subscribers(&b), 0);
    assert!(rooms.occupied().is_empty(), "an empty room was kept");
}

#[test]
fn a_dead_receiver_is_dropped_rather_than_retried() {
    // A connection that cannot take a frame will not take the next one.
    let rooms = Rooms::new();
    let room = Room::Document(DocumentId::new());
    let (sender, receiver) = channel();
    rooms.join(room.clone(), rooms.next_connection(), sender);
    drop(receiver);

    assert_eq!(rooms.broadcast(&room, ConnectionId(99), vec![1]), 0);
    assert_eq!(
        rooms.subscribers(&room),
        0,
        "the dead subscription was kept"
    );
}

#[test]
fn leaving_a_room_nobody_is_in_is_harmless() {
    let rooms = Rooms::new();
    rooms.leave(&Room::Document(DocumentId::new()), ConnectionId(0));
    rooms.leave_all(ConnectionId(0));
}

// ------------------------------------------------------------------ reporting

#[test]
fn occupied_rooms_are_reported_for_diagnosis() {
    let rooms = Rooms::new();
    let workspace = WorkspaceId::new();
    let room = Room::Manifest(workspace);
    let (a, _ra) = channel();
    let (b, _rb) = channel();
    rooms.join(room.clone(), rooms.next_connection(), a);
    rooms.join(room.clone(), rooms.next_connection(), b);

    let occupied = rooms.occupied();
    assert_eq!(occupied.len(), 1);
    assert_eq!(occupied[0].0, room.to_id());
    assert_eq!(occupied[0].1, 2);
}

#[test]
fn connection_ids_are_unique() {
    let rooms = Rooms::new();
    let ids: Vec<ConnectionId> = (0..100).map(|_| rooms.next_connection()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len());
}

#[test]
fn force_disconnect_counts_each_socket_once_and_closes_it() {
    let rooms = Rooms::new();
    let who = rooms.next_connection();
    let manifest = Room::Manifest(WorkspaceId::new());
    let document = Room::Document(DocumentId::new());
    let (close, mut closed) = tokio::sync::mpsc::unbounded_channel();
    rooms.register(who, close);
    let (one, _) = channel();
    let (two, _) = channel();
    assert!(rooms.join(manifest.clone(), who, one));
    assert!(rooms.join(document.clone(), who, two));

    assert_eq!(rooms.occupants(&[manifest.clone(), document.clone()]), 1);
    assert_eq!(
        rooms.block_and_disconnect(&[manifest.clone(), document.clone()]),
        1
    );
    assert_eq!(closed.try_recv(), Ok(()));
    assert_eq!(rooms.subscribers(&manifest), 0);
    assert_eq!(rooms.subscribers(&document), 0);
}

#[test]
fn a_blocked_room_refuses_late_joins_until_restore_unblocks_it() {
    let rooms = Rooms::new();
    let room = Room::Manifest(WorkspaceId::new());
    rooms.block_and_disconnect(std::slice::from_ref(&room));
    let (sender, _) = channel();
    assert!(!rooms.join(room.clone(), rooms.next_connection(), sender));

    rooms.unblock(std::slice::from_ref(&room));
    let (sender, _) = channel();
    assert!(rooms.join(room, rooms.next_connection(), sender));
}
