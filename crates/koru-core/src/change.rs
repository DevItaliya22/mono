// SPDX-License-Identifier: MPL-2.0

//! Per-view change tracking: which nodes a Loro event subscription says changed,
//! resolved to node ids so a view can patch only those rows.
//!
//! A [`crate::schema::WorkspaceDoc`] is shared by every window on it, so when one
//! window types the others must catch up. The catch-up used to be a
//! whole-document `Body::rebuild` on every keystroke — O(document), a dropped
//! frame at two thousand rows. A view now registers one [`ChangeTracker`] and
//! drains it in `adopt`, refreshing only the rows that actually changed.
//!
//! Each view owns its own tracker. The subscription fires into that tracker
//! alone, so one view's [`ChangeTracker::drain`] cannot steal another's changes
//! the way a single shared set would: `App::adopt_others` calls `adopt` on every
//! other pane in turn, and each has to see every change.
//!
//! The subscription records [`loro::TreeID`]s — a `Node`'s identity — plus a
//! flag for structural changes. It cannot resolve tree ids to [`crate::NodeId`]s
//! itself, because the callback cannot borrow the document; [`ChangeTracker`]
//! does that at drain time.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use loro::TreeID;

use crate::NodeId;
use crate::schema::WorkspaceDoc;

/// The raw, unresolved record a subscription callback writes to.
///
/// Internal: the callback runs without access to the document, so it records
/// tree ids and a structural flag and leaves resolution to
/// [`ChangeTracker::drain`].
#[derive(Default)]
pub(crate) struct ChangeSet {
    pub(crate) touched: HashSet<TreeID>,
    /// A node was created, moved or deleted — the row *sequence* may have
    /// changed, which a per-row refresh cannot represent.
    pub(crate) structure_dirty: bool,
    /// **Where** that happened: the nodes a tree event named, and their parents.
    ///
    /// `structure_dirty` alone says a tree changed *somewhere in the document*,
    /// and a document holds every open tab's content — so on that flag alone a
    /// paragraph created in one document rebuilt the rows of every other. This
    /// is what lets a view ask whether the change was anywhere near it.
    ///
    /// **Parents are recorded, not just targets**, and that is what makes it
    /// work for a delete: the deleted node's own id stops resolving once it is
    /// gone, but its old parent is still there and still names the place.
    pub(crate) structural: HashSet<TreeID>,
}

/// What changed since a view last looked, resolved to node ids.
///
/// [`ChangeTracker::drain`] returns this. A view refreshes [`touched`] in place
/// and falls back to a full rebuild when [`structure_dirty`] is set.
///
/// [`touched`]: ChangeBatch::touched
/// [`structure_dirty`]: ChangeBatch::structure_dirty
pub struct ChangeBatch {
    /// Nodes whose text or meta changed. Refresh these where they sit.
    pub touched: Vec<NodeId>,
    /// A node was created, moved or deleted. The set of rows — not just their
    /// content — may have changed, so the caller rebuilds.
    pub structure_dirty: bool,
    /// The nodes that structural change happened at or under, where they could
    /// still be resolved — see [`ChangeSet::structural`].
    ///
    /// **Empty while `structure_dirty` is set means "somewhere, but nothing
    /// nameable survived"**, and a caller must treat that as concerning it. Use
    /// [`structural_touches`] rather than reading this directly.
    ///
    /// [`structural_touches`]: ChangeBatch::structural_touches
    pub structural: Vec<NodeId>,
    /// A structural target or node parent could not be mapped to its stable
    /// `NodeId`. Search uses this as an explicit integrity-repair signal rather
    /// than quietly guessing at an incremental partition.
    pub unresolved_structure: bool,
}

impl ChangeBatch {
    /// Whether this batch's structural change could affect a view showing
    /// `root` and the rows in `has_row`.
    ///
    /// `root` is `None` for a view over a whole realm, which cannot rule
    /// anything out. Otherwise a change concerns the view when it names the
    /// root itself — a child created directly under it — or any row it is
    /// currently showing, which covers a delete or a move by way of the old
    /// parent.
    ///
    /// **Unnameable means yes.** If nothing in the batch resolved, the honest
    /// answer is that we cannot tell, and a needless rebuild is a great deal
    /// better than rows that quietly stop matching the document.
    pub fn structural_touches(
        &self,
        root: Option<NodeId>,
        has_row: impl Fn(NodeId) -> bool,
    ) -> bool {
        if !self.structure_dirty {
            return false;
        }
        let Some(root) = root else { return true };
        if self.structural.is_empty() {
            return true;
        }
        self.structural.iter().any(|id| *id == root || has_row(*id))
    }
}

/// A handle on the changes a Loro subscription has seen, owned by one view.
///
/// Build one with [`WorkspaceDoc::track_changes`]. Dropping it unsubscribes, so
/// it belongs on the view (a `Body`) and dies with it.
pub struct ChangeTracker {
    changes: Arc<Mutex<ChangeSet>>,
    /// Tree nodes remain in Loro's diff after deletion but no longer resolve
    /// through the live tree. Retaining the last stable identity is what lets
    /// incremental consumers remove precisely the deleted partition.
    known: Mutex<std::collections::HashMap<TreeID, NodeId>>,
    _sub: loro::Subscription,
}

impl ChangeTracker {
    /// Register a subscription on `doc` that records node-level diffs into this
    /// tracker. Package-private: views build one through
    /// [`WorkspaceDoc::track_changes`].
    ///
    /// Takes the `LoroDoc` rather than the `WorkspaceDoc` because the profile
    /// document has a tree too and wants exactly this. Nothing below reads
    /// anything but the subscription.
    pub(crate) fn new(
        doc: &loro::LoroDoc,
        known: std::collections::HashMap<TreeID, NodeId>,
    ) -> Self {
        use loro::Index;
        let changes: Arc<Mutex<ChangeSet>> = Arc::new(Mutex::new(ChangeSet::default()));
        let cb_changes = changes.clone();
        // The callback records tree ids (a `Diff::Text` or `Diff::Map` event
        // names its node in the path) and a structural flag (a `Diff::Tree`
        // event). It never touches the document, so it cannot deadlock against
        // the edit path — only against another lock of `changes`, which `drain`
        // avoids by committing before it locks.
        let _sub = doc.subscribe_root(Arc::new(move |e| {
            use loro::event::Diff;
            use loro::{TreeExternalDiff, TreeParentId};
            let mut cs = cb_changes.lock().unwrap();
            for c in &e.events {
                let Diff::Tree(tree) = &c.diff else {
                    for (_, idx) in c.path {
                        if let Index::Node(tid) = idx {
                            cs.touched.insert(*tid);
                        }
                    }
                    continue;
                };
                cs.structure_dirty = true;
                // Record the target *and* every parent the item names. The
                // parent is the load-bearing half: a deleted node's own id will
                // not resolve by the time anyone asks, and `old_parent` is what
                // still points at the place it was deleted from.
                for item in &tree.diff {
                    let (a, b) = match item.action {
                        TreeExternalDiff::Create { parent, .. } => (Some(parent), None),
                        TreeExternalDiff::Move {
                            parent, old_parent, ..
                        } => (Some(parent), Some(old_parent)),
                        TreeExternalDiff::Delete { old_parent, .. } => (Some(old_parent), None),
                    };
                    for p in [a, b].into_iter().flatten() {
                        if let TreeParentId::Node(tid) = p {
                            cs.structural.insert(tid);
                        }
                    }
                    cs.structural.insert(item.target);
                }
            }
        }));
        ChangeTracker {
            changes,
            known: Mutex::new(known),
            _sub,
        }
    }

    /// Commit any uncommitted write, then hand back what changed for this view,
    /// resolved to node ids, and clear the tracker.
    ///
    /// The commit runs **before** the lock is taken. The subscription callback
    /// fires *during* the commit and locks `changes` itself, so taking the lock
    /// across the commit would deadlock. The commit is usually a no-op anyway —
    /// Loro auto-commits on read, and the edit path reads right after it writes.
    pub fn drain(&self, ws: &WorkspaceDoc) -> ChangeBatch {
        ws.commit();
        let taken = {
            let mut cs = self.changes.lock().unwrap();
            std::mem::take(&mut *cs)
        };
        let mut known = self.known.lock().unwrap();
        let resolve = |tid, known: &mut std::collections::HashMap<TreeID, NodeId>| {
            if let Some(id) = ws.node_id_of(tid) {
                known.insert(tid, id);
                Some(id)
            } else {
                known.get(&tid).copied()
            }
        };
        let touched = taken
            .touched
            .into_iter()
            .filter_map(|tid| resolve(tid, &mut known))
            .collect();
        // Some of these will not resolve — a deleted node is gone from the
        // index by now, which is exactly why its parent was recorded too.
        let structural_count = taken.structural.len();
        let structural = taken
            .structural
            .into_iter()
            .filter_map(|tid| resolve(tid, &mut known))
            .collect::<Vec<_>>();
        ChangeBatch {
            touched,
            structure_dirty: taken.structure_dirty,
            unresolved_structure: taken.structure_dirty
                && structural.len() < structural_count
                && structural.is_empty(),
            structural,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspaceId;
    use crate::node::{Content, Kind, Node};
    use crate::schema::Parent;

    /// The path a Loro text event carries names the tree node, so a tracker
    /// resolves an edit to exactly the node that was edited.
    #[test]
    fn a_text_edit_resolves_to_its_node() {
        let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").unwrap();
        let a = ws
            .insert(
                Parent::Root,
                Node::new(Kind::Task, Content::text("aaaa")).unwrap(),
            )
            .unwrap();
        let b = ws
            .insert(
                Parent::Root,
                Node::new(Kind::Task, Content::text("bbbb")).unwrap(),
            )
            .unwrap();
        let tracker = ws.track_changes();

        ws.insert_text(b, 1, "X").unwrap();
        let batch = tracker.drain(&ws);
        assert!(!batch.structure_dirty);
        assert!(!batch.unresolved_structure);
        assert_eq!(batch.touched, vec![b], "only the edited node");
        // `a` is untouched and absent.
        let _ = a;
    }

    /// A structural change — here, creating a node — sets `structure_dirty`,
    /// because the row sequence changes rather than one row's content.
    #[test]
    fn a_structural_change_marks_structure_dirty() {
        let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").unwrap();
        let _root = ws
            .insert(Parent::Root, Node::task("root").unwrap())
            .unwrap();
        let tracker = ws.track_changes();

        ws.insert(Parent::Root, Node::task("sibling").unwrap())
            .unwrap();
        let batch = tracker.drain(&ws);
        assert!(batch.structure_dirty, "a create is structural");
    }

    /// `drain` clears the tracker, so a second drain with no intervening edit
    /// reports nothing.
    #[test]
    fn drain_clears_the_tracker() {
        let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").unwrap();
        let a = ws
            .insert(
                Parent::Root,
                Node::new(Kind::Task, Content::text("a")).unwrap(),
            )
            .unwrap();
        let tracker = ws.track_changes();

        ws.insert_text(a, 0, "x").unwrap();
        let _ = tracker.drain(&ws);

        let again = tracker.drain(&ws);
        assert!(again.touched.is_empty(), "nothing new since the last drain");
        assert!(!again.structure_dirty);
    }

    /// A meta change (ticking a task) resolves to its node, just as a text edit.
    #[test]
    fn a_meta_change_resolves_to_its_node() {
        let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").unwrap();
        let a = ws
            .insert(
                Parent::Root,
                Node::new(Kind::Task, Content::text("a")).unwrap(),
            )
            .unwrap();
        let tracker = ws.track_changes();

        ws.set_done(a, true).unwrap();
        let batch = tracker.drain(&ws);
        assert_eq!(batch.touched, vec![a]);
        assert!(!batch.structure_dirty);
    }
}
