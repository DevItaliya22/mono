// SPDX-License-Identifier: MPL-2.0

//! Keeping the index level with the read model.
//!
//! ## The consumer only enqueues
//!
//! [`Consumer::projected`] is called on the *projector's* thread. [`Feed`]
//! records `(workspace, document)` and returns; it does not read, tokenize or
//! write. The trait's own contract is that a slow consumer must not hold the
//! projection back, and an index rebuild after a schema bump has to be batched
//! the way a sweep is — which is a state machine of its own.
//!
//! ## It re-reads rather than keeping what it was handed
//!
//! `projected` is given the whole `DocumentProjection` and throws it away. That
//! costs a re-read of rows still warm in the page cache, and it buys one code
//! path: **the trigger and the reconcile do the same thing**. The path that
//! runs after a restart is the path that runs every day, so it cannot rot.
//!
//! ## Reconciling is the mechanism, not a safety net
//!
//! Everything the index learns has a comparison behind it — its
//! `indexed_revision` against the projection's `source_revision`, and its
//! consumed listing digest against the projection's. A missed event is a delay.
//! An event that never arrives because the index was stopped, or because its
//! own schema moved and it threw everything away, is the same delay.
//!
//! ## A document nothing lists is deferred, not retried forever
//!
//! Two rules meet badly. A document with no listing row must not be indexed —
//! the hit would open nothing — and it is given no cursor either, because
//! nothing was consumed. So the reconcile found it behind again, and again.
//! On a real installation that was 5,746 of 6,754 projected documents: the
//! indexer never idled, the queue never emptied, and every search reported
//! every workspace as *updating* forever. A miss is now remembered against the
//! revision it happened at, and retried when the document's revision moves or
//! its workspace's listing changes — the only two things that could make it
//! indexable.
//!
//! ## The tag table rides the same seam
//!
//! A tag is indexed by its **name**, which lives in the workspace's
//! `koru-system` document rather than beside the id on the node
//! ([`crate::tags`], ADR-182). That document is a document: it is projected,
//! and [`Consumer::projected`] queues it like any other. So a rename needs no
//! second channel — it arrives as *the system document changed*, and this
//! module turns that into "re-read the table, and re-index the documents
//! carrying an id whose **name** moved". A colour, a new attribute definition
//! and a merge onto the same spelling all reproject the same document and
//! enqueue nothing, because none of them changes a byte of any fragment.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use koru_projection::model::DocumentProjection;
use koru_projection::{Consumer, Projections};
use koru_store_core::ids::{DocumentId, WorkspaceId};

use crate::model::{IndexStats, SearchResult};
use crate::storage::{SEARCH_SCHEMA_VERSION, SearchIndex};
use crate::tags::{KIND_SYSTEM, TAG_ROLE, TagNames, TagTable};

/// How long the indexer waits, with nothing queued, before comparing itself
/// against the projection again.
///
/// Measurable rather than settled. Short enough that a missed event is a
/// hiccup; long enough that an idle store is not scanning two tables every
/// quarter of a second.
pub const RECONCILE_MILLIS: i64 = 2_000;

/// What one tick will index before yielding.
pub const BATCH: usize = 8;

/// Work waiting for the indexer, and what it has been doing.
///
/// **Shared, in memory, and never on disk.** The query side reads
/// [`pending_workspaces`](Self::pending_workspaces) to fill
/// `SearchPage.updating_workspaces`; writing that to SQLite would put a write
/// in the path of every keystroke to answer a question that is only about right
/// now.
#[derive(Clone, Default)]
pub struct Queue(Arc<Mutex<Inner>>);

#[derive(Default)]
struct Inner {
    documents: Vec<(WorkspaceId, DocumentId)>,
    removed: Vec<DocumentId>,
    listings: Vec<WorkspaceId>,
    stats: IndexStats,
}

impl Queue {
    pub fn stats(&self) -> IndexStats {
        let inner = self.lock();
        let mut stats = inner.stats.clone();
        stats.schema_version = SEARCH_SCHEMA_VERSION;
        stats.pending = inner.documents.len() + inner.removed.len() + inner.listings.len();
        stats
    }

    /// Workspaces with work outstanding — what a query reports as updating.
    pub fn pending_workspaces(&self) -> Vec<WorkspaceId> {
        let inner = self.lock();
        let mut all: Vec<_> = inner
            .documents
            .iter()
            .map(|(w, _)| *w)
            .chain(inner.listings.iter().copied())
            .collect();
        all.sort_by_key(|w| w.to_storage());
        all.dedup();
        all
    }

    pub fn is_empty(&self) -> bool {
        let inner = self.lock();
        inner.documents.is_empty() && inner.removed.is_empty() && inner.listings.is_empty()
    }

    fn push_document(&self, workspace: WorkspaceId, document: DocumentId) {
        let mut inner = self.lock();
        if !inner.documents.iter().any(|(_, d)| *d == document) {
            inner.documents.push((workspace, document));
        }
    }

    fn push_listing(&self, workspace: WorkspaceId) {
        let mut inner = self.lock();
        if !inner.listings.contains(&workspace) {
            inner.listings.push(workspace);
        }
    }

    fn push_removed(&self, document: DocumentId) {
        let mut inner = self.lock();
        inner.documents.retain(|(_, d)| *d != document);
        if !inner.removed.contains(&document) {
            inner.removed.push(document);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What the projector tells. Registered with `Projector::add_consumer`.
pub struct Feed {
    queue: Queue,
}

impl Feed {
    pub fn new(queue: Queue) -> Self {
        Self { queue }
    }
}

impl Consumer for Feed {
    fn name(&self) -> &str {
        "search"
    }

    fn projected(&mut self, projection: &DocumentProjection) {
        self.queue
            .push_document(projection.workspace, projection.document);
    }

    fn removed(&mut self, _workspace: WorkspaceId, document: DocumentId) {
        self.queue.push_removed(document);
    }

    fn listing_changed(&mut self, workspace: WorkspaceId) {
        self.queue.push_listing(workspace);
    }

    fn source_incompatible(&mut self, _workspace: WorkspaceId, document: DocumentId) {
        self.queue.push_removed(document);
    }
}

/// Turns the read model into an index, one document at a time.
pub struct Indexer {
    index: SearchIndex,
    projections: Projections,
    queue: Queue,
    /// When the last reconcile ran, so an idle store is not comparing
    /// constantly. `None` means one is owed — which it is at construction,
    /// because that is when this may have missed the most.
    reconciled_at: Option<i64>,
    /// Where a workspace's tag names come from. `None` in a host that wired
    /// none — tags are then indexed as the ids they were before this existed,
    /// rather than disappearing.
    tags: Option<Box<dyn TagNames>>,
    /// One table per workspace, read on first need and replaced when that
    /// workspace's system document reprojects.
    ///
    /// **In memory and never on disk.** It is a projection of a projection: the
    /// cost of losing it is one fork per workspace, and persisting it would be
    /// a third copy of a table whose whole point is that there is one.
    /// `Arc` so a document's fragments can be built without cloning it or
    /// holding a borrow of `self` across the write.
    tag_tables: HashMap<WorkspaceId, Arc<TagTable>>,
    /// How many documents the last reconcile found that no listing names, and
    /// what was last said about them.
    ///
    /// Kept so the count can be published beside `behind`, and so the warning
    /// is said when the number *changes* rather than every two seconds - the
    /// same reason `failed` dedupes by message.
    unlisted: usize,
    said_unlisted: Option<usize>,
}

impl Indexer {
    pub fn new(index: SearchIndex, projections: Projections, queue: Queue) -> Self {
        Self {
            index,
            projections,
            queue,
            reconciled_at: None,
            tags: None,
            tag_tables: HashMap::new(),
            unlisted: 0,
            said_unlisted: None,
        }
    }

    /// Install the tag-name source. The daemon does this in `search::spawn`,
    /// because reading the table needs the store's lock and `prepare` runs
    /// before there is one.
    pub fn with_tag_names(mut self, tags: Box<dyn TagNames>) -> Self {
        self.tags = Some(tags);
        self
    }

    pub fn queue(&self) -> Queue {
        self.queue.clone()
    }

    pub fn index(&self) -> &SearchIndex {
        &self.index
    }

    /// Do one unit of work. Returns whether there is more to do now.
    ///
    /// Order matters: removals first, because indexing a document the store has
    /// dropped is work whose only result is a hit nobody can open; then
    /// listings, because a title a document is about to be indexed under should
    /// be the current one; then documents.
    pub fn tick(&mut self, now_millis: i64) -> bool {
        if self.remove_due() {
            return true;
        }
        if self.listing_due(now_millis) {
            return true;
        }
        if self.index_due(now_millis) {
            return true;
        }
        self.reconcile(now_millis)
    }

    fn remove_due(&mut self) -> bool {
        let Some(document) = self.queue.lock().removed.pop() else {
            return false;
        };
        match self.index.remove_document(&document.to_storage()) {
            Ok(()) => {
                self.queue.lock().stats.removed += 1;
            }
            Err(why) => self.failed(why.0),
        }
        true
    }

    fn listing_due(&mut self, now_millis: i64) -> bool {
        let Some(workspace) = self.queue.lock().listings.pop() else {
            return false;
        };
        self.consume_listing(workspace, now_millis);
        true
    }

    fn consume_listing(&mut self, workspace: WorkspaceId, _now_millis: i64) {
        match self.projections.listing(workspace) {
            Ok(Some(listing)) => {
                if let Err(why) = self.index.set_listing(
                    &workspace.to_storage(),
                    &listing.name,
                    &listing.digest,
                    &listing.documents,
                ) {
                    return self.failed(why.0);
                }
                // **A document deferred for want of a row this listing now has
                self.queue.lock().stats.listings += 1;
            }
            // The projection has nothing for this workspace: it was dropped, or
            // it was never mirrored. Either way the index must not keep results
            // for it.
            Ok(None) => {
                if let Err(why) = self.index.remove_workspace(&workspace.to_storage()) {
                    self.failed(why.0);
                }
            }
            Err(why) => self.failed(why.0),
        }
    }

    fn index_due(&mut self, now_millis: i64) -> bool {
        let batch: Vec<_> = {
            let mut inner = self.queue.lock();
            let taken = inner.documents.len().min(BATCH);
            inner.documents.drain(..taken).collect()
        };
        if batch.is_empty() {
            return false;
        }
        for (workspace, document) in batch {
            self.index_one(workspace, document, now_millis);
        }
        true
    }

    /// Read the projection, build fragments, replace that document's rows.
    fn index_one(&mut self, workspace: WorkspaceId, document: DocumentId, now_millis: i64) {
        let started = std::time::Instant::now();
        let reading = std::time::Instant::now();
        let projection = match self.projections.read(document) {
            Ok(Some(projection)) => projection,
            // Nothing to index. Not a failure: a document dropped between the
            // event and this tick is exactly this, and the removal is already
            // queued behind it.
            Ok(None) => return,
            Err(why) => return self.failed(why.0),
        };
        let listed = match self.index.listed(&document.to_storage()) {
            Ok(Some(listed)) => listed,
            // **No listing row means the index does not know this document
            // exists**, and a fragment for it would be a result nobody can
            // open. Mirror the workspace's listing and come back to it.
            Ok(None) => {
                // The ordinary absence is a race — the document was projected
                // before its workspace's listing was mirrored — and mirroring
                // it is the repair. A document no listing will *ever* name is
                // handled a level up, where `reconcile` consults the read
                // model's own set of listed documents and never queues it.
                self.consume_listing(workspace, now_millis);
                match self.index.listed(&document.to_storage()) {
                    Ok(Some(listed)) => listed,
                    Ok(None) => return,
                    Err(why) => return self.failed(why.0),
                }
            }
            Err(why) => return self.failed(why.0),
        };

        // **The tag table is a document, and this is it arriving.** A rename
        // reaches the index as its system document reprojecting; nothing else
        // is subscribed to, and nothing else needs to be.
        if listed.kind == KIND_SYSTEM {
            self.refresh_tags(workspace);
        }
        let tags = self.tag_table(workspace);

        // Timed in three. Reading the read model, turning it into fragments and
        // writing them are three different things to be slow at, and
        // `last_millis` alone could only say that one of them was.
        let read_millis = reading.elapsed().as_millis() as u64;

        let building = std::time::Instant::now();
        let fragments = crate::fragments::fragments(&projection, &listed.title, tags.as_deref());
        // The typed half of the same walk. Counted inside `build_millis` on
        // purpose: it is one pass over the projection either way, and a second
        // number would suggest a second read that does not happen.
        let values = crate::fragments::attr_values(
            &workspace.to_storage(),
            &document.to_storage(),
            &projection,
        );
        let build_millis = building.elapsed().as_millis() as u64;

        let writing = std::time::Instant::now();
        if let Err(why) = self.index.replace_document(
            &workspace.to_storage(),
            &document.to_storage(),
            &fragments,
            &values,
            &projection.source_revision,
            now_millis,
        ) {
            return self.failed(why.0);
        }
        let write_millis = writing.elapsed().as_millis() as u64;

        let took = started.elapsed().as_millis() as u64;
        let mut inner = self.queue.lock();
        inner.stats.indexed += 1;
        inner.stats.last_millis = took;
        inner.stats.total_millis += took;
        inner.stats.max_millis = inner.stats.max_millis.max(took);
        inner.stats.read_millis = read_millis;
        inner.stats.build_millis = build_millis;
        inner.stats.write_millis = write_millis;
    }

    // ------------------------------------------------------------- tags

    /// One workspace's tag table, read on first need.
    ///
    /// `None` means *not now*: no source wired, no system document yet, a fork
    /// that failed. The fragment builder then indexes the id it was
    /// given, which is what the index held before any of this — a tag must not
    /// vanish from search while its table is being waited for. **Nothing is
    /// cached on a failure**, so the next document tries again.
    fn tag_table(&mut self, workspace: WorkspaceId) -> Option<Arc<TagTable>> {
        if !self.tag_tables.contains_key(&workspace) {
            // **Through `refresh_tags`, so the first successful read is a
            // change like any other.** It used to insert the table quietly
            // here, which left the rule that function documents — *with nothing
            // to compare against, everything the table names is due* — honoured
            // only when the system document happened to be the document that
            // triggered the read. Every other order was a silent hole: a
            // content document indexed while the table could not be read
            // carried raw ids, and nothing ever asked again unless somebody
            // edited the system document.
            self.refresh_tags(workspace);
        }
        self.tag_tables.get(&workspace).cloned()
    }

    /// The workspace's system document changed. Re-read the table, and re-index
    /// exactly the documents carrying an id whose **name** moved.
    ///
    /// **Not the whole workspace.** A system document reprojects for a new
    /// attribute definition, a tag's colour and a merge onto the same spelling
    /// as well, and none of those changes one byte of one fragment; re-indexing
    /// everything on each would make defining an attribute cost a workspace
    /// walk. What is compared is the id-to-name map, and what is enqueued is
    /// the documents the read model says carry one of the ids that moved — one
    /// query, over `attrs`, and only when something actually moved.
    ///
    /// **With nothing to compare against, everything the table names is due.**
    /// That is the first time this process reads the workspace's table, and the
    /// fragments already in the index were built by some earlier process
    /// against some earlier table. Guessing that they agree is how a rename
    /// made before a restart stays invisible forever.
    fn refresh_tags(&mut self, workspace: WorkspaceId) {
        let Some(source) = self.tags.as_ref() else {
            return;
        };
        let Some(pairs) = source.tag_names(workspace) else {
            return;
        };
        let fresh = TagTable::from_pairs(pairs);
        let previous = self.tag_tables.insert(workspace, Arc::new(fresh.clone()));
        let moved = fresh.renamed_since(previous.as_deref().unwrap_or(&TagTable::default()));
        if moved.is_empty() {
            return;
        }

        let attrs = match self.projections.attrs_in_workspace(workspace) {
            Ok(attrs) => attrs,
            Err(why) => return self.failed(why.0),
        };
        let mut due = 0;
        for (document, rows) in attrs {
            if rows
                .iter()
                .any(|(_, attr)| attr.role == TAG_ROLE && moved.contains(&attr.value))
            {
                self.queue.push_document(workspace, document);
                due += 1;
            }
        }
        // **Counts and ids only.** A tag's name is what somebody wrote
        // (LOGGING.md), and this is the one place in the indexer that has one
        // in a local variable.
        tracing::debug!(
            target: "koru_search",
            %workspace,
            tags = moved.len(),
            documents = due,
            "a tag table changed; re-indexing what carries the renamed ids"
        );
    }

    /// Compare what the index consumed against what the projection holds, and
    /// enqueue the difference.
    ///
    /// **Equal is current; everything else is work.** A recovered document's
    /// revision can go backwards, so "newer replaces older" would leave it
    /// stale for good.
    fn reconcile(&mut self, now_millis: i64) -> bool {
        if let Some(at) = self.reconciled_at
            && now_millis - at < RECONCILE_MILLIS
        {
            return false;
        }
        self.reconciled_at = Some(now_millis);
        let Ok(found) = self.compare() else {
            return false;
        };
        // **This is the number that says search is stale.** It is the same
        // comparison the reconcile was already doing, so it costs nothing to
        // publish — and nothing else reports it: `pending` says how much work is
        // queued, which is zero both when the index is current and when it has
        // not noticed it is behind.
        //
        // As of the last reconcile, which is throttled to `RECONCILE_MILLIS` —
        // so it is a recent truth rather than a live one, and a caller reading
        // it while a burst is being consumed will see it fall behind and catch
        // up rather than sit at zero.
        let mut inner = self.queue.lock();
        inner.stats.behind = found;
        inner.stats.unlisted = self.unlisted;
        if found > 0 {
            inner.stats.caught_up += 1;
        }
        drop(inner);
        // Said when it changes, not every reconcile. Zero is worth saying once
        // too: it is how somebody watching a repair knows it finished.
        if self.said_unlisted != Some(self.unlisted) {
            self.said_unlisted = Some(self.unlisted);
            if self.unlisted > 0 {
                tracing::warn!(
                    documents = self.unlisted,
                    "search index: documents exist that no workspace listing names; they cannot be indexed and will not be found by searching"
                );
            } else {
                tracing::info!("search index: every projected document is listed");
            }
        }
        found > 0
    }

    fn compare(&mut self) -> SearchResult<usize> {
        let mut found = 0;
        let mut unlisted = 0;

        let ours = self.index.digests()?;
        for (workspace, digest) in self.projections.listings()? {
            if ours.get(&workspace.to_storage()).map(Vec::as_slice) != Some(digest.as_slice()) {
                self.queue.push_listing(workspace);
                found += 1;
            }
        }

        let cursors = self.index.cursors()?;
        let theirs = self.projections.revisions()?;
        // **A document no listing names is not work.** `index_one` requires a
        // listing row - a fragment for a document the listing does not name
        // would be a result nobody can open - so asking it to index one is
        // asking for a job it must abandon. It abandoned it silently and
        // recorded nothing, so the next reconcile found the same document and
        // queued it again, forever: a loop that rewrote the workspace's whole
        // listing on every attempt and never logged, because giving up there
        // was not a failure. Worse, `reconcile` answering `found > 0` every
        // time is what kept `busy` true, and the driver only sleeps when it is
        // false - so the thread never rested either.
        //
        // The check is against the *projection's* listing, not the index's.
        // A document missing from the index's listing is the ordinary case and
        // is handled by mirroring the listing and carrying on; only one the
        // projection cannot name is impossible, and only those are skipped.
        //
        // **Not by recording a cursor for them**, which is the tempting fix and
        // the wrong one: a cursor is keyed on `source_revision`, so a document
        // marked done at revision R would stay unindexed if its listing arrived
        // later without its content changing. That trades a loud loop for
        // documents that are quietly unsearchable for good.
        let listed = self.projections.listed_documents()?;
        for (document, revision) in &theirs {
            if cursors.get(&document.to_storage()).map(Vec::as_slice) == Some(revision.as_slice()) {
                continue;
            }
            if !listed.contains(document) {
                unlisted += 1;
                continue;
            }
            // The workspace comes from the projection's own row rather than
            // being guessed, so a document is never indexed under the wrong one.
            if let Ok(Some(projected)) = self.projections.read(*document) {
                self.queue.push_document(projected.workspace, *document);
                found += 1;
            }
        }
        self.unlisted = unlisted;

        // Anything the index holds that the projection no longer does was
        // dropped while this was not listening.
        let held: HashSet<String> = theirs.keys().map(DocumentId::to_storage).collect();
        for document in cursors.keys() {
            if !held.contains(document)
                && let Some(id) = DocumentId::parse(document)
            {
                self.queue.push_removed(id);
                found += 1;
            }
        }
        Ok(found)
    }

    fn failed(&mut self, why: String) {
        let mut inner = self.queue.lock();
        let changed = inner.stats.last_error.as_deref() != Some(why.as_str());
        inner.stats.failed += 1;
        inner.stats.last_error = Some(why.clone());
        drop(inner);
        // A broken derived schema can make every queued workspace fail with
        // the same SQLite error. Count every failure for diagnostics, but say
        // an identical cause once rather than turning a tight recovery loop
        // into hundreds of megabytes of duplicate logs.
        if changed {
            tracing::warn!("search index: {why}");
        }
    }
}

#[cfg(test)]
mod tests;
