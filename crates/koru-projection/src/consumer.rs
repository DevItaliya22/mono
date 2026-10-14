// SPDX-License-Identifier: MPL-2.0

//! What reads the projection.
//!
//! **Consumers own their cursors; this layer tracks nobody.** A consumer records
//! what it consumed in its own store and reconciles by comparing against
//! [`Projections::revisions`](crate::store::Projections::revisions) — never by
//! replaying a stream it must not miss.
//!
//! That is *events are triggers, never truth* applied one layer down, and it
//! buys three things at once. A consumer that was stopped catches up. A consumer
//! that bumps its own schema rebuilds without this layer knowing there is such a
//! thing as a schema. And a consumer that lives in another process one day needs
//! no new mechanism, because reconciling is already how it works.
//!
//! It also means a slow or broken consumer cannot hold the projection back: it
//! is told, it fails, and it repairs itself the next time it compares.

use koru_store_core::ids::{DocumentId, WorkspaceId};
use koru_store_core::{DictionaryCommitted, IdentityCommitted, ReminderCommitted};

use crate::events::AttributeEvent;
use crate::model::DocumentProjection;

/// Something that reads the projection as it changes.
///
/// Called on the projector's own thread, one document at a time, **after** the
/// projection is on the disk — so a consumer that panics or errors leaves the
/// projection correct and only itself behind.
pub trait Consumer: Send {
    /// For logs and diagnostics. Stable, because a consumer's own cursor is
    /// usually filed under it.
    fn name(&self) -> &str;

    /// Retry delivery work retained by the consumer after a transient sink
    /// failure. Called once per projector tick, including otherwise idle
    /// ticks, so recovery does not depend on another document commit.
    fn flush_pending(&mut self) {}

    /// A document was projected. `projection` is what was just written.
    fn projected(&mut self, projection: &DocumentProjection);

    /// A document was **dropped** — not binned.
    ///
    /// A document in the trash still has content somebody can search and
    /// restore; only a real removal reaches this.
    fn removed(&mut self, workspace: WorkspaceId, document: DocumentId);

    /// The workspace's listing changed — a title, a parent, a trash flag.
    ///
    /// Separate from [`projected`](Self::projected) because the manifest is a
    /// document to the store and is **not** a Koru document: nothing walks it
    /// here, but a consumer that joins against titles or the trash needs to know
    /// it moved. The default does nothing, since most consumers do not care.
    fn listing_changed(&mut self, _workspace: WorkspaceId) {}

    /// The Store has this document, but this build cannot safely interpret its
    /// source schema. Consumers must withhold any previously derived current
    /// result for it until a later build projects it successfully.
    fn source_incompatible(&mut self, _workspace: WorkspaceId, _document: DocumentId) {}

    /// Semantic attribute changes the projection just made true — the D10
    /// vocabulary (`tag.added`, `due.changed`, …), derived by diffing the
    /// document's attr rows across [`projected`](Self::projected) /
    /// [`removed`](Self::removed) and delivered right after that call, same
    /// thread, one document at a time. Not called when nothing changed.
    ///
    /// **Triggers, never truth** — the slice is transient and nothing here is
    /// persisted or replayable, so a consumer acts by reading state and
    /// reconciles the way it already does for projections. A rebuilt read
    /// model re-emits `Set` for attributes it meets again; a consumer that
    /// cannot tolerate that keeps its own record, which it needed anyway.
    ///
    /// The default ignores them — most consumers do not care, and no shipped
    /// consumer subscribes yet (notifications and agents are R1's).
    fn attribute_events(&mut self, _events: &[AttributeEvent]) {}

    /// Profile reminder-root changes, after their snapshot commit.
    fn reminder_changes(&mut self, _events: &[ReminderCommitted]) {}

    /// Personal-dictionary changes, after their private Profile unit committed.
    fn dictionary_changes(&mut self, _events: &[DictionaryCommitted]) {}

    /// A Profile's name or appearance, after an identity unit that arrived from
    /// a peer or the Cloud committed. A locally made change never comes through
    /// here: the route that made it announces it itself.
    fn identity_changes(&mut self, _events: &[IdentityCommitted]) {}

    /// Stable node ids that were present in the previous projection and are
    /// absent from the new current one.
    fn entities_removed(
        &mut self,
        _workspace: WorkspaceId,
        _document: DocumentId,
        _nodes: &[String],
    ) {
    }
}

/// A consumer that records what it was told and nothing else.
///
/// **For tests, and it earns its place.** A seam with no consumer is untested,
/// and the first real one does not exist until the search index — so this is
/// what proves the cursor-and-reconcile path works before anything depends on
/// it: stop it, commit, start it again, and watch it catch up by comparison
/// rather than by having been told.
#[derive(Default)]
pub struct Counting {
    pub projected: Vec<(DocumentId, Vec<u8>)>,
    pub removed: Vec<DocumentId>,
    pub listings: Vec<WorkspaceId>,
    pub incompatible: Vec<DocumentId>,
    /// Every attribute event, in delivery order — the exact sequence, because
    /// what a test asserts about events is their order and their content.
    pub attribute_events: Vec<AttributeEvent>,
}

impl Counting {
    /// The revision this consumer last saw for a document — its cursor, kept
    /// where a real consumer would keep it: in its own store.
    pub fn cursor(&self, document: DocumentId) -> Option<&[u8]> {
        self.projected
            .iter()
            .rev()
            .find(|(d, _)| *d == document)
            .map(|(_, r)| r.as_slice())
    }

    /// Which documents this consumer is behind on, by comparing its own record
    /// against the projection's — the reconcile every consumer performs.
    pub fn behind(&self, projection: &crate::store::Projections) -> Vec<DocumentId> {
        projection
            .revisions()
            .unwrap_or_default()
            .into_iter()
            .filter(|(d, r)| self.cursor(*d) != Some(r.as_slice()))
            .map(|(d, _)| d)
            .collect()
    }
}

impl Consumer for Counting {
    fn name(&self) -> &str {
        "counting"
    }

    fn projected(&mut self, projection: &DocumentProjection) {
        self.projected
            .push((projection.document, projection.source_revision.clone()));
    }

    fn removed(&mut self, _workspace: WorkspaceId, document: DocumentId) {
        self.removed.push(document);
    }

    fn listing_changed(&mut self, workspace: WorkspaceId) {
        self.listings.push(workspace);
    }

    fn source_incompatible(&mut self, _workspace: WorkspaceId, document: DocumentId) {
        self.incompatible.push(document);
    }

    fn attribute_events(&mut self, events: &[AttributeEvent]) {
        self.attribute_events.extend_from_slice(events);
    }
}
