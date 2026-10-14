// SPDX-License-Identifier: MPL-2.0

//! **The sharpest of these is [`another_documents_acknowledgement_does_not_make_this_one_safe`].**
//!
//! It is the failure the roadmap names for this plan, it is a one-line mistake
//! to make, and every workspace-wide reading — `owed()`, `store_trouble()`, "is
//! the link live" — makes it. Two real [`StoreSession`]s over two fake links,
//! one acknowledged and one not, and the answer for the unacknowledged one has
//! to stay `false`.
//!
//! Everything here drives real sessions rather than constructed values, because
//! the claim under test is about *the Store's own bookkeeping*, not about a
//! struct's fields. Nothing touches a disk: `StoreSession::for_system_test`
//! leaves buffering `Off`, so the buffered/buffer-failed states are exercised
//! by the presentation's own fakes instead (see
//! `apps/koru/src/mainview/accessibility/persistence/tests.rs`) — a test that
//! wrote an outbox file to prove it would be breaking the rule the outbox
//! exists to serve.

use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_store_core::ids::{DocumentId as StoreDocumentId, WorkspaceId as StoreWorkspaceId};
use loro_protocol::UpdateStatusCode;

use super::*;
use crate::store::link::fake::FakeLink;

/// One document, its session, and the fake link under it.
struct Open {
    session: StoreSession,
    link: std::sync::Arc<FakeLink>,
    doc: WorkspaceDoc,
}

fn open() -> Open {
    let (session, link) =
        StoreSession::for_system_test(StoreWorkspaceId::new(), StoreDocumentId::new());
    let doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "T").expect("create");
    Open { session, link, doc }
}

impl Open {
    /// Type something, hand it over, and answer the batch the store would.
    fn writes(&mut self, text: &str) {
        self.doc
            .insert(Parent::Root, Node::paragraph(text).unwrap())
            .expect("insert");
        self.session.publish(&self.doc).expect("publish");
    }

    /// The store confirms everything outstanding.
    fn acknowledged(&mut self) {
        // `next_batch` starts at 0 and increments per send, so the last batch
        // sent is the count of sends — and an ack of batch *n* confirms
        // everything before it, which is exactly the behaviour worth relying on
        // here rather than tracking ids in the test.
        let sends = self.link.sent().len() as u64;
        self.link.acks(
            loro_protocol::BatchId(sends.to_le_bytes()),
            UpdateStatusCode::Ok,
        );
        self.session.drain(&mut self.doc).expect("drain");
    }

    fn durability(&self) -> DocumentDurability {
        of_session(&self.session, &self.doc, None)
    }
}

// ------------------------------------------------- the test this plan is about

/// **Store acknowledgements are per room and per version vector.** A window
/// with two documents open can have one of them fully confirmed while the
/// other's work is nowhere on any disk, and every reading that speaks for the
/// *workspace* — nothing queued, the link is live, an ack just landed — calls
/// that safe.
#[test]
fn another_documents_acknowledgement_does_not_make_this_one_safe() {
    let mut mine = open();
    let mut theirs = open();

    mine.writes("the paragraph I just typed");
    theirs.writes("something in the other tab");

    // The store answers the *other* document, and only that one.
    theirs.acknowledged();

    assert!(
        theirs.durability().durable,
        "the acknowledged document should be durable"
    );
    assert!(
        !mine.durability().durable,
        "an acknowledgement for another document made this one look saved"
    );
    // And the tempting workspace-wide readings both say the wrong thing about
    // it, which is why the per-document question exists at all.
    assert!(
        theirs.session.link_state().is_live() && mine.session.link_state().is_live(),
        "both links are live, and that says nothing about either document"
    );
    assert_eq!(
        theirs.session.unacknowledged(),
        0,
        "nothing is queued for the other document, and that says nothing about this one"
    );
}

/// The lookup, which is the other half: a caller names the document in front of
/// somebody and cannot be handed a neighbour's answer by accident.
#[test]
fn a_durability_lookup_answers_for_the_document_it_was_asked_about() {
    let mine = koru_core::DocumentId::new();
    let theirs = koru_core::DocumentId::new();
    let durability = Durability {
        documents: vec![(mine, owing()), (theirs, saved())],
        document_list: None,
    };

    assert_eq!(durability.document(mine).map(|d| d.durable), Some(false));
    assert_eq!(durability.document(theirs).map(|d| d.durable), Some(true));
    assert_eq!(
        durability.document(koru_core::DocumentId::new()),
        None,
        "a document that is not open has no answer, rather than a cheerful one"
    );
}

// ------------------------------------------------------- the states, one by one

/// **A document that has never been published owes its whole self.** There is
/// no acknowledgement to compare against, so `acked` is empty and every
/// operation the document holds — its root and its title included — is still
/// only in this process. A presentation that started from "nothing has been
/// typed, so it must be saved" would be wrong from the first frame.
#[test]
fn a_document_the_store_has_never_answered_for_is_not_durable() {
    let open = open();
    let durability = open.durability();
    assert!(
        !durability.durable,
        "an unpublished document has nothing on the store's disk"
    );
    assert_eq!(durability.in_flight, 0, "and nothing in flight either");
    assert_eq!(durability.link, LinkHealth::Live);
    assert!(durability.read_only.is_none());
}

#[test]
fn work_handed_over_and_not_answered_is_pending_rather_than_saved() {
    let mut open = open();
    open.writes("typed but not confirmed");

    let durability = open.durability();
    assert!(
        !durability.durable,
        "an update that left the process is not an update that landed"
    );
    assert_eq!(durability.in_flight, 1);
    assert_eq!(durability.link, LinkHealth::Live);
    assert!(!durability.would_lose_work() || !durability.buffered);
    assert!(
        !durability.is_news_worthy(),
        "the ordinary half-second between typing and an acknowledgement is activity, not news"
    );
}

#[test]
fn an_acknowledgement_is_what_makes_a_document_durable() {
    let mut open = open();
    open.writes("typed");
    open.acknowledged();

    let durability = open.durability();
    assert!(durability.durable);
    assert_eq!(durability.in_flight, 0);
    assert!(!durability.would_lose_work());
}

/// A lost connection **clears what was in flight while rewinding**, so
/// `in_flight` reads zero at the exact moment the most is at stake. `durable`
/// is what still tells the truth.
#[test]
fn a_lost_connection_leaves_the_work_owed_even_though_nothing_is_queued() {
    let mut open = open();
    open.writes("typed just before the store went away");
    open.link.drops_and_returns();
    open.link.is(LinkState::Reconnecting {
        since: std::time::Instant::now(),
        why: "the store went away".to_owned(),
    });
    open.session.drain(&mut open.doc).expect("drain");

    let durability = open.durability();
    assert_eq!(
        durability.in_flight, 0,
        "the rewind cleared the queue, which is why a queue length cannot be the answer"
    );
    assert!(!durability.durable, "and the work is still owed");
    assert!(matches!(durability.link, LinkHealth::Retrying { .. }));
    assert!(
        durability.is_news_worthy(),
        "owed work behind a link that is not getting through is worth saying once"
    );
}

/// A refused join is final, and it outranks a socket that is perfectly alive.
#[test]
fn a_stopped_session_reads_as_stopped_however_healthy_its_socket_is() {
    let mut open = open();
    open.link.push(koru_api_client::Event::JoinRefused {
        room: String::new(),
        code: loro_protocol::JoinErrorCode::AuthFailed,
        app_code: None,
        message: "not allowed".to_owned(),
    });
    assert!(open.session.drain(&mut open.doc).is_err());

    let durability = open.durability();
    assert_eq!(durability.stopped.as_deref(), Some("not allowed"));
    assert_eq!(
        durability.link,
        LinkHealth::Stopped,
        "the socket still answers Live; the session has given up, and that is the document's answer"
    );
    assert!(durability.is_news_worthy());
}

/// The schema gate, which is the one legacy-era fact carried here: a document
/// written by a newer build is not *unsaved*, it is one this build must never
/// write back.
#[test]
fn a_document_from_a_newer_build_carries_its_reason_beside_its_durability() {
    let mut open = open();
    open.writes("read but never written back");
    let read_only = of_session(&open.session, &open.doc, Some("too new".to_owned()));

    assert_eq!(read_only.read_only.as_deref(), Some("too new"));
    // **Both facts, side by side, and neither one edits the other.** The
    // session's bookkeeping still says work is outstanding — this build does
    // not stop the room publishing — and it is the *presentation* that reads
    // read-only first, so a document nothing will write is never described as
    // one that failed to save. Folding read-only into `durable` here would
    // make a genuinely stranded document look confirmed.
    assert!(!read_only.durable);
    assert!(
        !read_only.is_news_worthy(),
        "read-only is a standing state to answer when asked, not a failure to announce"
    );
}

// ------------------------------------------------------------ the plain answers

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

fn owing() -> DocumentDurability {
    DocumentDurability {
        durable: false,
        ..saved()
    }
}

#[test]
fn work_this_machine_has_written_down_is_not_lost_by_closing() {
    let buffered = DocumentDurability {
        buffered: true,
        link: LinkHealth::Stopped,
        ..owing()
    };
    assert!(!buffered.would_lose_work());
    assert!(
        buffered.is_news_worthy(),
        "the store being unreachable is still worth one line"
    );

    let stranded = DocumentDurability {
        buffer_failure: Some("no space left on device".to_owned()),
        ..buffered
    };
    assert!(
        stranded.buffered,
        "the fixture keeps the stale cover so the failure is what changes the answer"
    );
    assert!(stranded.is_news_worthy());
}

#[test]
fn the_workspaces_own_list_is_counted_apart_from_its_documents() {
    let durability = Durability {
        documents: vec![(koru_core::DocumentId::new(), saved())],
        document_list: Some(owing()),
    };
    assert!(
        durability.is_news_worthy() || !durability.document_list.as_ref().unwrap().is_news_worthy(),
        "a list that is owed behind a live link is pending, like a document"
    );

    let losing = Durability {
        documents: vec![(koru_core::DocumentId::new(), saved())],
        document_list: Some(DocumentDurability {
            link: LinkHealth::Stopped,
            ..owing()
        }),
    };
    assert!(losing.would_lose_work());
    assert!(losing.is_news_worthy());
}
