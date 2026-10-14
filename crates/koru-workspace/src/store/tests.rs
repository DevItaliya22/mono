// SPDX-License-Identifier: MPL-2.0

//! What a session does when the answer is not "fine".
//!
//! Everything here drives a [`FakeLink`], so a refused acknowledgement, a
//! rejoin request and a store that stops answering are one line each. None of
//! them could be provoked reliably against a real store, which is why none of
//! them was tested and why three of them were wrong.
//!
//! **The assertions are on bytes, imported into a fresh replica.** "The update
//! was re-sent" checked by counting calls passes while both sides are empty; the
//! same claim checked by finding the row in a replica that never saw the
//! original does not.

use std::sync::Arc;

use koru_api_client::Discovery;
use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{Align, Mark};
use loro_protocol::UpdateStatusCode;

use super::link::fake::FakeLink;
use super::*;

mod manifest_now;
mod title_reconcile;

/// A session over a fake link, and a document to drive it with.
///
/// The `Client` is real but never called: nothing in this file reaches HTTP, and
/// constructing one costs a `reqwest::blocking::Client` and no connection.
fn session() -> (StoreSession, Arc<FakeLink>, WorkspaceDoc) {
    let link = Arc::new(FakeLink::new());
    // Port 0 and no token: nothing in this file makes a request, and a
    // `Client` is a `reqwest` builder rather than a connection.
    let client = Client::new(Discovery {
        port: 0,
        token: String::new(),
        expires_at_millis: i64::MAX,
    });
    let session = StoreSession::over(
        link.clone(),
        client,
        StoreWorkspaceId::new(),
        StoreDocumentId::new(),
        "document:test".to_owned(),
    );
    let doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "T").expect("create");
    (session, link, doc)
}

/// Everything the link was handed, replayed into a replica that never saw the
/// original. What comes out is what the store would hold.
fn replica_of(link: &FakeLink) -> WorkspaceDoc {
    let mut replica = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "R").expect("create");
    for bytes in link.sent() {
        // Deliberately not `expect`: an update whose ancestors are missing
        // imports `Ok` and applies nothing, so a failure here is a *different*
        // fault from the one under test and the row assertion will catch it.
        let _ = replica.import_frames(&[bytes]);
    }
    replica
}

fn rows(doc: &WorkspaceDoc) -> Vec<String> {
    doc.walk()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, _)| doc.read(id).ok())
        .filter_map(|n| n.content.as_text().map(str::to_owned))
        .collect()
}

fn write(doc: &mut WorkspaceDoc, text: &str) {
    doc.insert(Parent::Root, Node::paragraph(text).unwrap())
        .expect("insert");
}

#[test]
fn text_blocks_and_formatting_each_publish_as_durable_updates() {
    let (mut session, link, mut doc) = session();

    let paragraph = doc
        .insert(Parent::Root, Node::paragraph("format me").unwrap())
        .expect("insert paragraph");
    assert!(session.publish(&doc).expect("publish block").is_some());

    doc.insert_text(paragraph, 6, " this").expect("insert text");
    assert!(session.publish(&doc).expect("publish text").is_some());

    doc.insert_after(paragraph, Node::paragraph("another block").unwrap())
        .expect("insert another block");
    assert!(session.publish(&doc).expect("publish block").is_some());

    for mark in [Mark::Bold, Mark::Italic, Mark::Underline] {
        doc.set_mark(paragraph, 0..6, mark, true)
            .expect("apply mark");
        assert!(
            session.publish(&doc).expect("publish mark").is_some(),
            "{mark:?} did not produce an outbound update"
        );
    }
    doc.set_align(paragraph, Some(Align::End))
        .expect("right-align paragraph");
    assert!(
        session.publish(&doc).expect("publish alignment").is_some(),
        "right alignment did not produce an outbound update"
    );

    let replica = replica_of(&link);
    let replicated = replica
        .walk()
        .expect("walk replica")
        .into_iter()
        .map(|(id, _)| id)
        .find(|id| {
            replica
                .read(*id)
                .ok()
                .and_then(|node| node.content.as_text().map(str::to_owned))
                .as_deref()
                == Some("format this me")
        })
        .expect("replicated formatted paragraph");
    let marks = replica.marks_in(replicated, 0..6).expect("read marks");
    assert!(marks.has(Mark::Bold));
    assert!(marks.has(Mark::Italic));
    assert!(marks.has(Mark::Underline));
    assert_eq!(
        replica
            .read(replicated)
            .expect("read aligned paragraph")
            .meta
            .align,
        Some(Align::End)
    );
    assert!(rows(&replica).iter().any(|row| row == "another block"));
}

// ------------------------------------------------------------- refused acks

/// **A refused acknowledgement must not swallow the edits it refused.**
///
/// This is the bug that hid best: `record` removed the batch from the pending
/// set whatever the status was, so the operations left the outbound stream and
/// nothing ever sent them again — and `unacknowledged()` then reported zero, so
/// the loss looked like success.
#[test]
fn a_refused_acknowledgement_is_re_sent_rather_than_dropped() {
    let (mut session, link, mut doc) = session();

    write(&mut doc, "before the refusal");
    let batch = session.publish(&doc).expect("publish").expect("something");

    link.acks(batch, UpdateStatusCode::Unknown);
    let _ = session.drain(&mut doc);

    // Nothing new was typed. The next publish must still have something to say.
    let again = session.publish(&doc).expect("publish");
    assert!(again.is_some(), "the refused update was never re-sent");

    assert!(
        rows(&replica_of(&link))
            .iter()
            .any(|r| r == "before the refusal"),
        "a replica built from what was sent does not hold the row"
    );
}

/// The other half. Without it, "resend everything, always" would pass the test
/// above and be indistinguishable from a fix.
#[test]
fn an_acknowledged_batch_is_not_re_sent() {
    let (mut session, link, mut doc) = session();

    write(&mut doc, "landed");
    let batch = session.publish(&doc).expect("publish").expect("something");
    link.acks(batch, UpdateStatusCode::Ok);
    let _ = session.drain(&mut doc);

    assert!(session.is_durable(batch), "the ack was not recorded");
    assert_eq!(session.unacknowledged(), 0);
    assert!(
        session.publish(&doc).expect("publish").is_none(),
        "an acknowledged document was published again"
    );
}

/// An acknowledgement confirms everything sent before it, because a room
/// answers in send order. Otherwise `acked` would stall at the first batch and
/// every rewind would resend the whole session.
#[test]
fn an_acknowledgement_confirms_the_batches_before_it() {
    let (mut session, link, mut doc) = session();

    write(&mut doc, "one");
    let _first = session.publish(&doc).expect("publish").expect("something");
    write(&mut doc, "two");
    let second = session.publish(&doc).expect("publish").expect("something");
    assert_eq!(session.unacknowledged(), 2);

    link.acks(second, UpdateStatusCode::Ok);
    let _ = session.drain(&mut doc);
    assert_eq!(
        session.unacknowledged(),
        0,
        "the earlier batch is still owed"
    );
}

/// **A refusal the store will always refuse must not become an infinite
/// resend.** The rewind that fixes the first test is exactly what makes this
/// possible, so the two arrive together.
#[test]
fn three_refusals_in_a_row_stop_the_session() {
    let (mut session, link, mut doc) = session();

    for i in 0..3 {
        write(&mut doc, &format!("attempt {i}"));
        let batch = session.publish(&doc).expect("publish").expect("something");
        link.acks(batch, UpdateStatusCode::InvalidUpdate);
        let _ = session.drain(&mut doc);
    }

    assert!(session.stopped().is_some(), "it kept trying forever");
    assert!(
        session.publish(&doc).is_err(),
        "a stopped session went on publishing"
    );
}

/// A rate limit is the store asking for patience, not refusing the content. It
/// rewinds like any other refusal and must not count toward giving up, or a busy
/// store would look like a broken one.
#[test]
fn a_rate_limit_is_retried_forever_rather_than_counted() {
    let (mut session, link, mut doc) = session();

    for i in 0..5 {
        write(&mut doc, &format!("attempt {i}"));
        let batch = session.publish(&doc).expect("publish").expect("something");
        link.acks(batch, UpdateStatusCode::RateLimited);
        let _ = session.drain(&mut doc);
    }

    assert!(
        session.stopped().is_none(),
        "a busy store stopped a session"
    );
    assert!(session.publish(&doc).expect("publish").is_some());
}

/// A late answer from a connection that is gone names a batch this session has
/// no record of. Acting on it would confirm — or rewind — something it knows
/// nothing about.
#[test]
fn an_acknowledgement_for_an_unknown_batch_is_ignored() {
    let (mut session, link, mut doc) = session();

    write(&mut doc, "mine");
    let batch = session.publish(&doc).expect("publish").expect("something");

    link.acks(
        loro_protocol::BatchId(9999u64.to_le_bytes()),
        UpdateStatusCode::Ok,
    );
    let _ = session.drain(&mut doc);

    assert_eq!(session.unacknowledged(), 1, "a stranger's ack was counted");
    assert!(!session.is_durable(batch));
}

// ------------------------------------------------------------- room problems

/// **`RejoinSuggested` is an instruction, not a failure.** It is the one room
/// error that means *recover*, and the code used to be flattened into a string
/// and then swallowed — so it was indistinguishable from the errors that are
/// fatal.
#[test]
fn a_rejoin_suggestion_rejoins_naming_what_this_replica_holds() {
    let (mut session, link, mut doc) = session();
    write(&mut doc, "already here");

    link.push(koru_api_client::Event::RoomProblem {
        room: "document:test".to_owned(),
        code: loro_protocol::RoomErrorCode::RejoinSuggested,
        message: "you have fallen behind".to_owned(),
    });
    session.drain(&mut doc).expect("a rejoin is not an error");
    assert!(
        session.stopped().is_none(),
        "a recoverable problem stopped it"
    );

    session.publish(&doc).expect("publish");
    let joins = link.joins();
    assert_eq!(joins.len(), 1, "it did not rejoin");
    assert_eq!(
        joins[0],
        doc.version(),
        "the rejoin did not name what this replica holds"
    );
}

/// Every other room error is fatal, and says so once rather than being retried
/// into silence.
#[test]
fn any_other_room_problem_stops_the_session() {
    let (mut session, link, mut doc) = session();

    link.push(koru_api_client::Event::RoomProblem {
        room: "document:test".to_owned(),
        code: loro_protocol::RoomErrorCode::Evicted,
        message: "the room is gone".to_owned(),
    });
    assert!(session.drain(&mut doc).is_err());
    assert_eq!(session.stopped(), Some("the room is gone"));
}

/// A refused join is final: `Unauthenticated` or a document that does not exist
/// will be refused identically forever.
#[test]
fn a_refused_join_stops_the_session_immediately() {
    let (mut session, link, mut doc) = session();

    link.push(koru_api_client::Event::JoinRefused {
        room: "document:test".to_owned(),
        code: loro_protocol::JoinErrorCode::AuthFailed,
        app_code: None,
        message: "not allowed".to_owned(),
    });
    assert!(session.drain(&mut doc).is_err());
    assert_eq!(session.stopped(), Some("not allowed"));
}

// ------------------------------------------------------- a store that went away

/// **An edit made while the store was down reaches it when the store comes
/// back.**
///
/// Reported from a running window: write, stop the store, write more, start the
/// store, and the second edit was gone. There was no reconnection at all — a
/// dead socket stayed dead, and everything typed after it stayed in memory until
/// the window closed.
///
/// The assertion is on what a *replica* built from the sent bytes holds, because
/// that is what the store would hold. Counting sends would pass while both were
/// empty.
#[test]
fn an_edit_made_while_the_store_was_down_is_sent_when_it_returns() {
    let (mut session, link, mut doc) = session();

    write(&mut doc, "before the fall");
    let batch = session.publish(&doc).expect("publish").expect("something");
    link.acks(batch, UpdateStatusCode::Ok);
    let _ = session.drain(&mut doc);

    // The store goes away. Typing does not stop.
    link.drops_and_returns();
    write(&mut doc, "while it was down");
    let _ = session.drain(&mut doc);

    session.publish(&doc).expect("publish after the reconnect");

    let replica = replica_of(&link);
    let rows = rows(&replica);
    assert!(
        rows.iter().any(|r| r == "while it was down"),
        "what was typed while the store was down never reached it: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r == "before the fall"),
        "the reconnect lost what had already landed: {rows:?}"
    );
}

/// A reconnect **rejoins naming what this replica holds**, not an empty vector.
///
/// The two are opposite mistakes. An empty vector says "I have nothing" and asks
/// the store for everything, which is right on a first join and wasteful here.
/// A *stale* vector asks for a delta whose causal ancestors this replica cannot
/// obtain — and Loro parks that as pending, reporting `Ok` while applying
/// nothing, which is the silent version of losing the catch-up entirely.
#[test]
fn a_reconnect_rejoins_at_the_version_this_replica_holds() {
    let (mut session, link, mut doc) = session();
    write(&mut doc, "something local");

    link.drops_and_returns();
    let _ = session.drain(&mut doc);
    session.publish(&doc).expect("publish");

    let joins = link.joins();
    assert_eq!(joins.len(), 1, "it did not rejoin");
    assert_eq!(joins[0], doc.version());
}

/// A connection that dies with batches in flight owes them again.
///
/// Whether they landed is unknowable from here, so the session rewinds to the
/// last version the store *confirmed* — which re-sends a little that may already
/// be there rather than dropping a little that is not. On a CRDT the surplus is
/// idempotent; the shortfall is content.
#[test]
fn batches_in_flight_when_the_connection_died_are_owed_again() {
    let (mut session, link, mut doc) = session();

    write(&mut doc, "in flight");
    session.publish(&doc).expect("publish").expect("something");
    assert_eq!(session.unacknowledged(), 1);

    link.drops_and_returns();
    let _ = session.drain(&mut doc);
    assert_eq!(session.unacknowledged(), 0, "still counted as in flight");

    // Nothing new typed, and yet there is something to say.
    assert!(
        session.publish(&doc).expect("publish").is_some(),
        "the in-flight batch was not owed again"
    );
}

// -------------------------------------------------------------------- waking

/// **A frame arriving wakes whoever will collect it**, without anybody ticking.
///
/// This is what lets `next_due` stop answering "now" and the window actually
/// sleep. Proved over an `AtomicBool` rather than an event loop — the waker is a
/// closure precisely so that `koru-workspace` never learns what winit is.
#[test]
fn an_arriving_frame_wakes_the_loop() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (session, link, _doc) = session();
    let woken = Arc::new(AtomicUsize::new(0));
    session.set_waker(super::link::Waker::new({
        let woken = woken.clone();
        move || {
            woken.fetch_add(1, Ordering::Release);
        }
    }));

    let mut peer = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "P").expect("create");
    write(&mut peer, "from the peer");
    link.delivers(vec![peer.export_all_updates().expect("export")]);

    assert_eq!(woken.load(Ordering::Acquire), 1, "nobody was woken");
}

/// **Edge-triggered.** A busy room posts one wake-up, not one per frame: the
/// loop is going to drain all of them anyway, and a queue of redundant wake-ups
/// is a busy loop wearing a different hat. Draining is what arms it again.
#[test]
fn a_flood_of_frames_wakes_the_loop_once() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (mut session, link, mut doc) = session();
    let woken = Arc::new(AtomicUsize::new(0));
    session.set_waker(super::link::Waker::new({
        let woken = woken.clone();
        move || {
            woken.fetch_add(1, Ordering::Release);
        }
    }));

    let mut peer = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "P").expect("create");
    for i in 0..5 {
        write(&mut peer, &format!("row {i}"));
        link.delivers(vec![peer.export_all_updates().expect("export")]);
    }
    assert_eq!(
        woken.load(Ordering::Acquire),
        1,
        "five frames posted five wake-ups"
    );

    // Drained: the next frame has to wake it again, or the window would sit on
    // somebody else's edit until something unrelated stirred it.
    let _ = session.drain(&mut doc);
    write(&mut peer, "after the drain");
    link.delivers(vec![peer.export_all_updates().expect("export")]);
    assert_eq!(
        woken.load(Ordering::Acquire),
        2,
        "the waker was never re-armed"
    );
}

/// A link is live before the shell has an event loop to hand it. If the Store
/// wins that race, installing the waker has to announce the frame already in
/// the inbox; there may never be a second frame to do it.
#[test]
fn a_frame_queued_before_the_waker_is_installed_wakes_the_loop() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (session, link, _doc) = session();
    let mut peer = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "P").expect("create");
    write(&mut peer, "already waiting");
    link.delivers(vec![peer.export_all_updates().expect("export")]);

    let woken = Arc::new(AtomicUsize::new(0));
    session.set_waker(super::link::Waker::new({
        let woken = woken.clone();
        move || {
            woken.fetch_add(1, Ordering::Release);
        }
    }));

    assert_eq!(
        woken.load(Ordering::Acquire),
        1,
        "installing the waker left existing Store work asleep"
    );
}

/// One event-loop turn is bounded to 64 inbound messages. Work beyond that
/// boundary must schedule another turn even when the socket has nothing new to
/// read, or the 65th message waits for a click.
#[test]
fn a_bounded_drain_rewakes_for_the_backlog() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (mut session, link, mut doc) = session();
    let woken = Arc::new(AtomicUsize::new(0));
    session.set_waker(super::link::Waker::new({
        let woken = woken.clone();
        move || {
            woken.fetch_add(1, Ordering::Release);
        }
    }));

    let mut peer = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "P").expect("create");
    for i in 0..65 {
        write(&mut peer, &format!("row {i}"));
        link.delivers(vec![peer.export_all_updates().expect("export")]);
    }
    assert_eq!(
        woken.load(Ordering::Acquire),
        1,
        "the burst did not coalesce"
    );

    session.drain(&mut doc).expect("bounded drain");
    assert_eq!(
        woken.load(Ordering::Acquire),
        2,
        "the queued remainder did not schedule another turn"
    );
    assert!(session.drain(&mut doc).expect("remaining frame"));
}

// ------------------------------------------------------------------- leaving

/// Closing a document leaves its room.
///
/// Politeness on the wire, and the observable half of something that is not
/// polite at all: a session owns a **thread** now, so a `doc_sessions` entry
/// that is never removed leaks one per tab for as long as the window is up. The
/// leave is what a test can see; the join happens in `Drop`.
#[test]
fn leaving_a_room_is_announced() {
    let (session, link, _doc) = session();
    session.leave();
    assert_eq!(link.left(), vec!["document:test".to_owned()]);
}

// ------------------------------------------------------------------ draining

/// **A failure must not take the frames that arrived with it.** `take` has
/// already lifted the whole batch off the link, so returning on the first
/// problem would discard whatever came after — and one of those may be content.
/// Loro parks a delta whose predecessor is missing and returns `Ok`, so losing
/// one is silent.
#[test]
fn updates_arriving_beside_a_problem_are_still_applied() {
    let (mut session, link, mut doc) = session();

    // What another replica wrote, as the bytes a room would carry.
    let mut peer = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "P").expect("create");
    write(&mut peer, "from the peer");
    let update = peer.export_all_updates().expect("export");

    link.push(koru_api_client::Event::RoomProblem {
        room: "document:test".to_owned(),
        code: loro_protocol::RoomErrorCode::Evicted,
        message: "trouble".to_owned(),
    });
    link.delivers(vec![update]);

    assert!(
        session.drain(&mut doc).is_err(),
        "the problem was not reported"
    );
    assert!(
        rows(&doc).iter().any(|r| r == "from the peer"),
        "the update that arrived with the problem was thrown away"
    );
}

// ----------------------------------------------------------------- the outbox

/// A session with somewhere to write.
///
/// **The `TempDir` stays with the caller**, because dropping it deletes the
/// outbox — a test that let it go would be asserting about a directory that no
/// longer exists, and would pass for the wrong reason.
fn buffering_session(
    root: &std::path::Path,
    workspace: StoreWorkspaceId,
    document: StoreDocumentId,
) -> (StoreSession, Arc<FakeLink>) {
    let link = Arc::new(FakeLink::new());
    let client = Client::new(Discovery {
        port: 0,
        token: String::new(),
        expires_at_millis: i64::MAX,
    });
    let session = StoreSession::over(
        link.clone(),
        client,
        workspace,
        document,
        "document:test".to_owned(),
    )
    .buffering_into(Some(root));
    (session, link)
}

fn a_document() -> WorkspaceDoc {
    WorkspaceDoc::create(koru_core::WorkspaceId::new(), "T").expect("create")
}

fn buffered_files(
    root: &std::path::Path,
    workspace: StoreWorkspaceId,
    document: StoreDocumentId,
) -> usize {
    outbox::read_all(Some(root), workspace, document).0.len()
}

/// The whole point. A connection that dies with work owed leaves that work on
/// this machine, because the process may not be here to re-send it.
#[test]
fn a_connection_that_dies_leaves_what_was_owed_on_the_disk() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = (StoreWorkspaceId::new(), StoreDocumentId::new());
    let (mut session, link) = buffering_session(root.path(), workspace, document);
    let mut doc = a_document();

    write(&mut doc, "typed while it was up");
    session.publish(&doc).expect("publish");

    // Nothing acknowledged it, and now the socket is gone.
    link.drops_and_returns();
    let _ = session.drain(&mut doc);

    assert_eq!(
        buffered_files(root.path(), workspace, document),
        1,
        "a lost connection left nothing on the disk"
    );
}

/// The other half, and without it "always write a file" would pass the test
/// above. An acknowledgement means the store has it, so the copy has done its
/// job — a buffer nothing removes is replayed on every open forever.
#[test]
fn an_acknowledgement_takes_the_buffer_away() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = (StoreWorkspaceId::new(), StoreDocumentId::new());
    let (mut session, link) = buffering_session(root.path(), workspace, document);
    let mut doc = a_document();

    write(&mut doc, "typed");
    let batch = session.publish(&doc).expect("publish").expect("something");
    session.buffer_now(&doc);
    assert_eq!(buffered_files(root.path(), workspace, document), 1);

    link.acks(batch, UpdateStatusCode::Ok);
    let _ = session.drain(&mut doc);

    assert_eq!(
        buffered_files(root.path(), workspace, document),
        0,
        "the buffer outlived the acknowledgement that made it pointless"
    );
}

/// The round trip this exists for: what one process buffered is what the next
/// one reads back, on top of the store's own copy.
#[test]
fn what_was_buffered_is_read_back_on_top_of_the_stores_copy() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = (StoreWorkspaceId::new(), StoreDocumentId::new());
    let (mut session, link) = buffering_session(root.path(), workspace, document);
    let mut doc = a_document();

    // One row the store confirms...
    write(&mut doc, "the store has this");
    let batch = session.publish(&doc).expect("publish").expect("something");
    link.acks(batch, UpdateStatusCode::Ok);
    let _ = session.drain(&mut doc);
    // ...and this is what it would hand back on the next open.
    let snapshot = doc.export_snapshot().expect("snapshot");

    // Then the store goes away and another row is typed.
    write(&mut doc, "typed after the store went");
    session.buffer_now(&doc);

    // A fresh process: the store's copy, plus whatever this machine kept.
    let mut fresh = WorkspaceDoc::from_snapshot(&snapshot).expect("from snapshot");
    assert!(
        !rows(&fresh)
            .iter()
            .any(|r| r == "typed after the store went"),
        "the store's copy already had the row, so this would prove nothing"
    );

    let (mut next, _) = buffering_session(root.path(), workspace, document);
    let complaints = next.replay_buffered(&mut fresh);

    assert!(complaints.is_empty(), "{complaints:?}");
    assert!(
        rows(&fresh)
            .iter()
            .any(|r| r == "typed after the store went"),
        "what was buffered never came back"
    );
}

/// **Replaying is not delivering.** The work is back in memory, which is where
/// it already was when it was buffered — so a process that died here would have
/// thrown the copy away and kept nothing. The file goes when the store confirms
/// it, and not before.
#[test]
fn a_replayed_buffer_stays_until_the_store_confirms_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = (StoreWorkspaceId::new(), StoreDocumentId::new());
    let (mut session, _link) = buffering_session(root.path(), workspace, document);
    let mut doc = a_document();

    write(&mut doc, "owed");
    session.buffer_now(&doc);

    let (mut next, next_link) = buffering_session(root.path(), workspace, document);
    let mut fresh = a_document();
    next.replay_buffered(&mut fresh);

    assert_eq!(
        buffered_files(root.path(), workspace, document),
        1,
        "the buffer was thrown away before the store had heard of it"
    );

    let batch = next.publish(&fresh).expect("publish").expect("something");
    next_link.acks(batch, UpdateStatusCode::Ok);
    let _ = next.drain(&mut fresh);

    assert_eq!(
        buffered_files(root.path(), workspace, document),
        0,
        "the store confirmed it and the copy is still there"
    );
}

fn system_doc(name: &str) -> koru_core::system::SystemDoc {
    let mut doc = koru_core::system::SystemDoc::create().expect("system");
    doc.create_tag(name, None, 1).expect("tag");
    doc
}

fn system_names(doc: &koru_core::system::SystemDoc) -> Vec<String> {
    doc.tags().into_iter().map(|tag| tag.name).collect()
}

/// The crash boundary for duplicate system documents. Replaying L restores the
/// tag, publishing L starts delivery, and C receives the union immediately.
/// The outbox remains until L itself is durably acknowledged.
#[test]
fn a_system_loser_reaches_canonical_before_its_replay_is_acknowledged() {
    let root = tempfile::tempdir().expect("tempdir");
    let workspace = StoreWorkspaceId::new();
    let loser = StoreDocumentId::new();
    let canonical = StoreDocumentId::new();
    let (mut old, old_link) = buffering_session(root.path(), workspace, loser);
    let mut loser_doc = system_doc("already stored");

    let stored = old.publish(&loser_doc).expect("publish").expect("batch");
    old_link.acks(stored, UpdateStatusCode::Ok);
    old.drain(&mut loser_doc).expect("ack");
    let store_snapshot = loser_doc.export_snapshot().expect("snapshot");

    loser_doc
        .create_tag("survived the crash", None, 2)
        .expect("tag");
    old.buffer_now(&loser_doc);
    assert_eq!(buffered_files(root.path(), workspace, loser), 1);
    drop(old);

    let (mut recovered, recovered_link) = buffering_session(root.path(), workspace, loser);
    let mut recovered_loser =
        koru_core::system::SystemDoc::from_snapshot(&store_snapshot).expect("open L");
    assert!(recovered.replay_buffered(&mut recovered_loser).is_empty());
    let loser_batch = recovered
        .publish(&recovered_loser)
        .expect("publish L")
        .expect("recovered update");

    let (mut canonical_session, canonical_link) =
        buffering_session(root.path(), workspace, canonical);
    let mut canonical_doc = system_doc("canonical");
    canonical_doc
        .absorb(&recovered_loser.export_snapshot().expect("recovered L"))
        .expect("absorb L into C");
    let canonical_batch = canonical_session
        .publish(&canonical_doc)
        .expect("publish C")
        .expect("union update");

    assert_eq!(
        system_names(&canonical_doc),
        vec!["already stored", "canonical", "survived the crash"]
    );
    assert_eq!(
        buffered_files(root.path(), workspace, loser),
        1,
        "absorbing into C incorrectly treated L as acknowledged"
    );

    canonical_link.acks(canonical_batch, UpdateStatusCode::Ok);
    canonical_session
        .drain(&mut canonical_doc)
        .expect("canonical ack");
    assert_eq!(
        buffered_files(root.path(), workspace, loser),
        1,
        "C's acknowledgement removed L's recovery file"
    );

    recovered_link.acks(loser_batch, UpdateStatusCode::Ok);
    recovered.drain(&mut recovered_loser).expect("loser ack");
    assert_eq!(
        buffered_files(root.path(), workspace, loser),
        0,
        "L's durable acknowledgement did not prune its replay"
    );
}

/// No acknowledgement is a valid long-lived state: C has the tag while the
/// local recovery copy remains available for the next process.
#[test]
fn a_system_loser_without_an_ack_keeps_its_outbox_after_canonical_absorb() {
    let root = tempfile::tempdir().expect("tempdir");
    let workspace = StoreWorkspaceId::new();
    let loser = StoreDocumentId::new();
    let (mut old, _) = buffering_session(root.path(), workspace, loser);
    let loser_doc = system_doc("owed tag");
    old.buffer_now(&loser_doc);
    drop(old);

    let (mut recovered, _) = buffering_session(root.path(), workspace, loser);
    let mut recovered_loser = koru_core::system::SystemDoc::create().expect("fresh L");
    assert!(recovered.replay_buffered(&mut recovered_loser).is_empty());
    recovered.publish(&recovered_loser).expect("publish L");

    let mut canonical_doc = system_doc("canonical");
    canonical_doc
        .absorb(&recovered_loser.export_snapshot().expect("L snapshot"))
        .expect("absorb");

    assert_eq!(system_names(&canonical_doc), vec!["canonical", "owed tag"]);
    assert_eq!(buffered_files(root.path(), workspace, loser), 1);
}

/// Replay is not only duplicate reconciliation. The ordinary one-row open must
/// also recover its own buffered tags before publishing.
#[test]
fn a_single_system_document_replays_its_outbox_on_open() {
    let root = tempfile::tempdir().expect("tempdir");
    let workspace = StoreWorkspaceId::new();
    let document = StoreDocumentId::new();
    let (mut old, _) = buffering_session(root.path(), workspace, document);
    let doc = system_doc("single row tag");
    old.buffer_now(&doc);
    drop(old);

    let (mut reopened, link) = buffering_session(root.path(), workspace, document);
    let mut fresh = koru_core::system::SystemDoc::create().expect("fresh");
    assert!(reopened.replay_buffered(&mut fresh).is_empty());
    let batch = reopened
        .publish(&fresh)
        .expect("publish")
        .expect("replayed update");

    assert_eq!(system_names(&fresh), vec!["single row tag"]);
    assert_eq!(buffered_files(root.path(), workspace, document), 1);
    link.acks(batch, UpdateStatusCode::Ok);
    reopened.drain(&mut fresh).expect("ack");
    assert_eq!(buffered_files(root.path(), workspace, document), 0);
}

/// A delta whose ancestors are missing imports `Ok` and applies nothing, so
/// importing one that cannot apply looks exactly like delivery. It has to be
/// refused, said out loud, and left where it is.
#[test]
fn a_buffer_the_document_cannot_reach_back_to_is_kept_and_reported() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = (StoreWorkspaceId::new(), StoreDocumentId::new());
    let (mut session, link) = buffering_session(root.path(), workspace, document);
    let mut doc = a_document();

    // Acknowledge one row, so the buffer starts from a point only this
    // replica's history contains.
    write(&mut doc, "confirmed");
    let batch = session.publish(&doc).expect("publish").expect("something");
    link.acks(batch, UpdateStatusCode::Ok);
    let _ = session.drain(&mut doc);
    write(&mut doc, "owed");
    session.buffer_now(&doc);

    // A document that has never heard of any of it — a store that lost the
    // workspace, or the wrong store entirely.
    let mut stranger = a_document();
    let (mut next, _) = buffering_session(root.path(), workspace, document);
    let complaints = next.replay_buffered(&mut stranger);

    assert_eq!(complaints.len(), 1, "the refusal was silent");
    assert!(
        !rows(&stranger).iter().any(|r| r == "owed"),
        "a delta that could not apply appears to have applied"
    );
    assert_eq!(
        buffered_files(root.path(), workspace, document),
        1,
        "the buffer was removed even though nothing took its contents"
    );
}

/// A session that was never given anywhere to write is not a failure — it is a
/// session with no disk, which is what every in-memory one is.
#[test]
fn a_session_with_no_outbox_claims_nothing() {
    let (mut session, _link, mut doc) = session();
    write(&mut doc, "typed");
    session.buffer_now(&doc);
    assert!(session.buffer_failure().is_none());
    assert!(session.owes(&doc), "unacknowledged work is still owed");
}

/// `owes` is the honest question and `unacknowledged` is not it: a lost
/// connection clears the in-flight batches while rewinding, so that count reads
/// zero at the exact moment the most is at stake.
#[test]
fn a_lost_connection_owes_everything_while_counting_nothing() {
    let root = tempfile::tempdir().expect("tempdir");
    let (workspace, document) = (StoreWorkspaceId::new(), StoreDocumentId::new());
    let (mut session, link) = buffering_session(root.path(), workspace, document);
    let mut doc = a_document();

    write(&mut doc, "typed");
    session.publish(&doc).expect("publish");
    link.drops_and_returns();
    let _ = session.drain(&mut doc);

    assert_eq!(session.unacknowledged(), 0, "the rewind clears these");
    assert!(
        session.owes(&doc),
        "the work is unconfirmed and nothing said so"
    );
}

/// An acknowledged document owes nothing, or the close warning would fire on
/// every window that was working perfectly.
#[test]
fn a_confirmed_document_owes_nothing() {
    let (mut session, link, mut doc) = session();
    write(&mut doc, "landed");
    let batch = session.publish(&doc).expect("publish").expect("something");
    link.acks(batch, UpdateStatusCode::Ok);
    let _ = session.drain(&mut doc);

    assert!(!session.owes(&doc));
}

/// **The number the outbox log prints is the number of operations recovered.**
///
/// A version vector is a counter per peer, so the difference between two of
/// them is the count — which is what lets the log say "20 op(s) applied"
/// without decoding the opaque payload the outbox carries.
#[test]
fn the_op_count_is_the_distance_between_two_versions() {
    let mut ws = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "T").expect("create");
    let before = ws.version();
    let doc = ws
        .insert(Parent::Root, Node::document("Recovered").unwrap())
        .unwrap();
    let after_one = ws.version();
    assert!(
        ops_between(&before, &after_one) > 0,
        "inserting a document is at least one operation"
    );

    ws.insert(Parent::Node(doc), Node::paragraph("and its text").unwrap())
        .unwrap();
    let after_two = ws.version();
    assert!(
        ops_between(&before, &after_two) > ops_between(&before, &after_one),
        "the count grows with the work"
    );
}

/// A replay of a file the store already had is a legitimate **zero**, not a
/// failure — and the arithmetic must not go negative when the version it is
/// compared against is ahead. A negative would be the implementation showing
/// through where an answer should be.
#[test]
fn replaying_what_is_already_here_counts_nothing() {
    let mut ws = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "T").expect("create");
    ws.insert(Parent::Root, Node::document("Already here").unwrap())
        .unwrap();
    let now = ws.version();

    assert_eq!(ops_between(&now, &now), 0);
    assert_eq!(
        ops_between(&now, &loro::VersionVector::new()),
        0,
        "an empty 'after' subtracts nothing rather than going negative"
    );
}
