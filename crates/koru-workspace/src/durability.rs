// SPDX-License-Identifier: MPL-2.0

//! **What the Store has confirmed, per document, and where the rest of it is.**
//!
//! One sentence: *this reads the bookkeeping a [`StoreSession`] already keeps
//! and answers "is **this** document's work safe" for each open document
//! separately.* It computes nothing, asks the Store nothing, and touches no
//! disk — every value here is already in this process's memory because the
//! session that owns the socket put it there.
//!
//! Why this shape rather than one number is **ADR-340-savestatus** in
//! [docs/decisions/](../../../docs/decisions/README.md), cited by slug because
//! a parallel branch may claim the number first.
//!
//! ## Why per document, and not one number
//!
//! [`Workspace::owed`] and [`Workspace::store_trouble`] answer for the *window*
//! — how many batches are outstanding anywhere, and whether any link is
//! unhappy. That is the right answer for a caption chip, which speaks for the
//! whole workspace, and it is the wrong answer for the question a person
//! actually asks: *has what I just typed, into the document in front of me,
//! been saved.*
//!
//! The two come apart in a way that is easy to miss and impossible to recover
//! from. Store acknowledgements are per version vector, per room. A window with
//! four documents open can have three of them fully acknowledged and the
//! fourth's session stopped, and every workspace-wide reading — the link is
//! live, nothing is queued, the last acknowledgement landed — says *safe*.
//! [`StoreSession::owes`] asked of that fourth document says otherwise, and it
//! is the only reading that does.
//!
//! So there is one [`DocumentDurability`] per open document, each built from
//! *that document's own* session and *that document's own* version, and
//! nothing here aggregates them.
//!
//! ## Three places work can be, and they are not two
//!
//! - **Durable** — the Store answered `Ack { durable: true }`, which
//!   [ADR-79](../../../docs/decisions/ADR-076-100.md#adr-79-durable--an-acknowledgement-means-the-bytes-are-on-the-disk)
//!   defines as *the bytes are on the disk*. Nothing else in this system means
//!   that.
//! - **Buffered on this machine** — owed, but
//!   `<store root>/outbox/<workspace>/<document>/<replica>.loro` covers it, so
//!   closing the window costs nothing.
//! - **Neither** — owed, and in this process only. The one state where closing
//!   loses something, and the only one worth interrupting anybody about.
//!
//! [`Unconfirmed`](crate::Unconfirmed) draws the same three lines for the
//! window as a whole and is what the closing dialog uses. This draws them per
//! document.
//!
//! ## What this deliberately does not answer
//!
//! **Peer synchronization.** A durable acknowledgement is one machine's disk.
//! Whether another device has seen the edit is `koru-replication`'s question,
//! it has its own failure modes, and reporting the two as one fact would make
//! "saved" mean whichever of them happened to be true. Nothing here mentions a
//! peer, and a caller that wants to must say so in its own words.
//!
//! **The legacy folder.** `Session::write_failed` and the folder transport
//! belong to the pre-Store workspace and are not the shipping save-status
//! source; the roadmap says so by name. The one folder-era fact carried here is
//! [`Workspace::read_only`], because the schema gate it reports —
//! [`Compatibility`](koru_core::schema::Compatibility) — applies to a
//! Store-backed document too and is the difference between *not saved yet* and
//! *will never be saved*.

use koru_core::DocumentId;
use koru_core::schema::Compatibility;

use crate::Workspace;
use crate::store::{StoreSession, Synced, link::LinkState};

/// Whether a document's link to the Store is getting through.
///
/// The three states [`LinkState`] has, with the transport's own diagnostic
/// dropped: a presentation that shows a raw endpoint error shows it inside a
/// localized frame, and it gets that string from
/// [`StoreTrouble::message`](crate::StoreTrouble::message), which already
/// carries it for the window. Repeating it per document would be four copies
/// of one sentence.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinkHealth {
    Live,
    /// The socket died and a new one is being sought. **Ordinary** — a Store
    /// restarting looks exactly like this — so `since` is carried and the
    /// *duration* is what decides whether it is worth telling anybody about.
    Retrying {
        since: std::time::Instant,
    },
    /// It will not come back on its own: the socket gave up, or the session
    /// was refused a join, or the Store refused several updates in a row.
    Stopped,
}

impl LinkHealth {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }
}

/// What the Store has confirmed about **one** document, and where the rest is.
///
/// A plain value with no borrow of the session it came from, so it can be
/// handed to a view that holds no workspace — the same reason
/// [`StoreTrouble`](crate::StoreTrouble) is pushed rather than asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentDurability {
    /// **The Store has confirmed everything this replica holds.** The inverse
    /// of [`StoreSession::owes`], and the only field that means *saved*.
    pub durable: bool,
    /// Batches handed over and not yet answered — what would be lost if the
    /// process died now *and* nothing had been buffered.
    ///
    /// **Never read as a safety signal on its own.** A lost connection clears
    /// this while rewinding, so it reads zero at the moment the most is at
    /// stake; `durable` is what answers the question.
    pub in_flight: usize,
    /// This machine's outbox holds a copy covering everything typed. Only
    /// interesting while `durable` is `false`.
    pub buffered: bool,
    /// Why the last attempt to buffer wrote nothing, if it wrote nothing.
    /// `Some` with `durable == false` is the only state that loses work.
    pub buffer_failure: Option<String>,
    /// Why this session gave up, if it has.
    pub stopped: Option<String>,
    pub link: LinkHealth,
    /// Why this document will not be written at all, if it will not.
    ///
    /// A different thing from every field above: those are about work that has
    /// not landed *yet*. This is a document an older build must not re-export,
    /// so nothing will ever be owed and *nothing is wrong*. Told apart because
    /// presenting read-only as a failed save is the misreading the roadmap
    /// names.
    pub read_only: Option<String>,
}

impl DocumentDurability {
    /// Whether closing now would lose this document's work.
    ///
    /// Not `!durable`: work the Store has not confirmed but this machine has
    /// written down is safe, which is the whole point of the outbox.
    pub fn would_lose_work(&self) -> bool {
        !self.durable && !self.buffered
    }

    /// Whether anything about this document is worth interrupting somebody
    /// about — as opposed to worth answering when they ask.
    ///
    /// A live link with a batch in flight is not: that is the ordinary
    /// half-second between typing and an acknowledgement, and a report of it
    /// is a report of activity rather than of news.
    pub fn is_news_worthy(&self) -> bool {
        self.stopped.is_some()
            || self.buffer_failure.is_some()
            || (!self.durable && !self.link.is_live())
    }
}

/// Every open document's answer, and the workspace's own list beside them.
///
/// **A list rather than a map**, because there are as many entries as there are
/// open documents — a handful — and the one caller looks up exactly one of
/// them. A `HashMap` would allocate more to answer the same question.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Durability {
    /// One entry per open document, keyed by the id the workspace uses.
    pub documents: Vec<(DocumentId, DocumentDurability)>,
    /// The workspace's own list of documents — a rename, a move, a new page.
    /// **Counted apart from the documents**, because it is not one of them:
    /// calling a rename nobody typed into "a document" sends a person looking
    /// for a change they did not make. `None` for a workspace with no manifest
    /// session, which is every folder-backed one.
    pub document_list: Option<DocumentDurability>,
}

impl Durability {
    /// One document's answer, if it is open.
    ///
    /// **The lookup that keeps another document's acknowledgement out of this
    /// one's answer.** A caller that wants to know about the document in front
    /// of somebody names it; there is deliberately no "is everything fine"
    /// accessor to reach for by mistake.
    pub fn document(&self, id: DocumentId) -> Option<&DocumentDurability> {
        self.documents
            .iter()
            .find(|(open, _)| *open == id)
            .map(|(_, durability)| durability)
    }

    /// Whether anything at all — a document or the list — is worth
    /// interrupting somebody about. See
    /// [`DocumentDurability::is_news_worthy`].
    pub fn is_news_worthy(&self) -> bool {
        self.documents
            .iter()
            .any(|(_, d)| d.is_news_worthy())
            .then_some(true)
            .or_else(|| {
                self.document_list
                    .as_ref()
                    .map(DocumentDurability::is_news_worthy)
            })
            .unwrap_or(false)
    }

    /// Whether closing now would lose anything — a document or the list.
    pub fn would_lose_work(&self) -> bool {
        self.documents.iter().any(|(_, d)| d.would_lose_work())
            || self
                .document_list
                .as_ref()
                .is_some_and(DocumentDurability::would_lose_work)
    }
}

/// Read the bookkeeping every open session already keeps.
///
/// **Only ever called while somebody is reading it.** Every entry costs one
/// version-vector comparison against the document's current version, and
/// [`Workspace::unconfirmed`] carries the note that says why that matters: it
/// runs on the tick of every window forever, and a healthy one must pay
/// nothing for a state it is not in. The caller — `service_accessibility` —
/// gates this on a screen reader actually being attached, which is the same
/// gate `View::semantics_revision` is behind.
pub(crate) fn of(workspace: &Workspace) -> Durability {
    // The one workspace-wide read-only fact: a folder-backed session that
    // opened a document written by a newer build. It applies to every document
    // in the workspace, so it is resolved once.
    let session_read_only = workspace.read_only().map(str::to_owned);

    let mut documents = Vec::with_capacity(workspace.doc_sessions.len());
    if let Some(active) = workspace.active
        && let Some(session) = workspace.doc_sessions.get(&active)
    {
        documents.push((
            active,
            of_document(session, &workspace.ws, session_read_only.clone()),
        ));
    }
    for (id, doc) in workspace.others.iter() {
        if let Some(session) = workspace.doc_sessions.get(id) {
            documents.push((*id, of_document(session, doc, session_read_only.clone())));
        }
    }

    let document_list = match (&workspace.manifest_session, &workspace.manifest) {
        // The manifest is a `LoroDoc` behind the same façade and syncs through
        // the same session type, so the same three states apply to it. It has
        // no schema gate of its own that this build reads, hence no read-only.
        (Some(session), Some(manifest)) => Some(of_session(session, manifest, None)),
        _ => None,
    };

    Durability {
        documents,
        document_list,
    }
}

/// One document's answer, with its own schema gate folded in.
fn of_document(
    session: &StoreSession,
    doc: &koru_core::schema::WorkspaceDoc,
    session_read_only: Option<String>,
) -> DocumentDurability {
    // **Per document, not per workspace.** `Compatibility` gates on the
    // document's own `schema_version`, and a workspace can hold one document
    // written by a newer build beside a dozen this build wrote itself.
    let read_only = session_read_only.or(match doc.compatibility() {
        Compatibility::Writable => None,
        too_new => Some(too_new.describe("document")),
    });
    of_session(session, doc, read_only)
}

fn of_session(
    session: &StoreSession,
    doc: &dyn Synced,
    read_only: Option<String>,
) -> DocumentDurability {
    let stopped = session.stopped().map(str::to_owned);
    let link = match session.link_state() {
        // **A session that has given up outranks its socket.** The socket may
        // still be perfectly live while this room has been refused a join;
        // reporting `Live` for it would be reporting the transport instead of
        // the document.
        _ if stopped.is_some() => LinkHealth::Stopped,
        LinkState::Live => LinkHealth::Live,
        LinkState::Reconnecting { since, .. } => LinkHealth::Retrying { since },
        LinkState::Stopped { .. } => LinkHealth::Stopped,
    };
    DocumentDurability {
        durable: !session.owes(doc),
        in_flight: session.unacknowledged(),
        buffered: session.buffered_covers(doc),
        buffer_failure: session.buffer_failure().map(str::to_owned),
        stopped,
        link,
        read_only,
    }
}

#[cfg(test)]
mod tests;
