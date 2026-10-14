// SPDX-License-Identifier: MPL-2.0

//! Document-only structure rules.
//!
//! A list marker lives on an invisible `ListBlock`, while a task is an ordinary
//! node. The shared editor can indent either shape in isolation; this module
//! supplies the document rule that makes a task and a bullet neighbours in the
//! same visible tree.

use koru_core::Kind;
use koru_core::schema::{Parent, WorkspaceDoc};

use crate::BuiltInDocument;

#[cfg(test)]
mod tests;

impl BuiltInDocument {
    /// Indent or outdent the selected list roots, including across task/list
    /// boundaries. The fallback is the editor's established list behaviour;
    /// the two special cases use the preceding *visible* row rather than a
    /// ListBlock, which deliberately has no row of its own.
    pub(crate) fn indent_structure(&mut self, ws: &mut WorkspaceDoc, dir: i32) {
        let mut ids = self.editor.body.selection_roots(ws);
        ids.retain(|id| {
            self.editor
                .body
                .index_of(*id)
                .is_some_and(|row| !self.editor.body.is_title_row(row))
        });
        ids.sort_by_key(|id| self.editor.body.index_of(*id));
        let Some(&first) = ids.first() else {
            return;
        };

        if dir > 0 && self.indent_list_below_task(ws, &ids, first) {
            return;
        }
        if dir > 0 && self.indent_list_below_list_item(ws, &ids, first) {
            return;
        }
        if dir > 0 && self.indent_tasks_below_bullet(ws, &ids, first) {
            return;
        }
        if dir < 0 && self.outdent_tasks_from_bullet(ws, &ids, first) {
            return;
        }
        if dir < 0 && self.row_is_list_item(first) {
            if self.outdent_list_fragment(ws, &ids, first) {
                return;
            }
            // A direct ListBlock child at the document root has no shallower
            // document level. Leave it unchanged rather than extracting it and
            // changing its marker type. Other nested list shapes use the
            // editor's established order-preserving outdent path.
            if self.direct_list_fragment(ws, &ids) {
                return;
            }
        }

        if ids.iter().any(|id| self.row_is_list_item(*id)) {
            self.editor.body.indent_list_items(ws, &ids, dir);
        } else {
            // A bare task has no ListBlock to carry, so it keeps the ordinary
            // structural move it had before mixed indentation existed.
            //
            // `reparent` rebuilds its own rows, so a reopen found afterwards has
            // to rebuild them a second time — the one place in this module where
            // the classification cannot simply precede the rebuild. It is still
            // asked here rather than inside `reparent`: this is the *document's*
            // Tab, and `reparent` is also the shared row-move helper.
            //
            // `reparent` reports nothing, so what moved is read off the tree:
            // a row whose parent is the one it already had is not a move, and
            // asking about it would reopen a task holding SPEC §5's concurrent
            // residue — work a *remote* peer added, which this Tab did not.
            let before: Vec<_> = ids.iter().map(|&id| (id, ws.parent(id).ok())).collect();
            let departing = koru_core::tasks::departing_all(ws, &ids);
            self.reparent(ws, dir);
            let landed: Vec<_> = before
                .iter()
                .filter(|(id, was)| ws.parent(*id).ok() != *was)
                .map(|&(id, _)| id)
                .collect();
            if self.reclassify_moves(ws, &landed, &departing) {
                self.restore_structure_selection(ws);
            }
        }
    }

    /// Move a leading fragment of a list beneath the preceding task. Moving
    /// the item alone would discard its marker, so the core moves a same-style
    /// ListBlock with it instead.
    fn indent_list_below_task(
        &mut self,
        ws: &mut WorkspaceDoc,
        ids: &[koru_core::NodeId],
        first: koru_core::NodeId,
    ) -> bool {
        let Some(previous) = self.previous_visible_row(first) else {
            return false;
        };
        if !self.row_is_task(previous) || !self.direct_list_fragment(ws, ids) {
            return false;
        }
        // Neighbours only: the task and the fragment's ListBlock share a
        // parent. A task deeper in some subtree (the fragment follows the end
        // of that subtree) is not a neighbour, and nesting under it would
        // jump more than one level in a single Tab.
        let Ok(Parent::Node(list)) = ws.parent(first) else {
            return false;
        };
        if ws.parent(previous).ok() != ws.parent(list).ok() {
            return false;
        }
        let at = ws
            .children(Parent::Node(previous))
            .map_or(0, |children| children.len());
        self.move_list_fragment(ws, ids, Parent::Node(previous), at, false)
    }

    /// A task following a bullet becomes the bullet's child, not a child of the
    /// invisible ListBlock that happens to own that bullet. A task retains its
    /// `Kind::Task`, so it continues to draw a checkbox at the new depth.
    fn indent_tasks_below_bullet(
        &mut self,
        ws: &mut WorkspaceDoc,
        ids: &[koru_core::NodeId],
        first: koru_core::NodeId,
    ) -> bool {
        let Some(previous) = self.previous_visible_row(first) else {
            return false;
        };
        if !self.row_is_list_item(previous) || !ids.iter().all(|id| self.row_is_task(*id)) {
            return false;
        }
        // A *bullet*, not another task — the mirror of
        // `outdent_tasks_from_bullet`'s guard. A task under a task indents the
        // ordinary way, staying a sibling-shaped checkbox.
        if self.row_is_task(previous) {
            return false;
        }
        // Neighbours only: the tasks sit beside the bullet's ListBlock, not
        // somewhere shallower or deeper in the tree.
        if !matches!(ws.parent(previous), Ok(Parent::Node(list))
            if ws.parent(list).ok() == ws.parent(first).ok())
        {
            return false;
        }
        if ids
            .iter()
            .all(|id| matches!(ws.parent(*id), Ok(Parent::Node(parent)) if parent == previous))
        {
            return false;
        }

        let departing = koru_core::tasks::departing_all(ws, ids);
        ws.checkpoint();
        ws.begin_group();
        let mut at = ws
            .children(Parent::Node(previous))
            .map_or(0, |children| children.len());
        let mut landed = Vec::new();
        for &id in ids {
            if ws.move_node(id, Parent::Node(previous), at).is_ok() {
                at += 1;
                landed.push(id);
            }
        }
        ws.checkpoint();
        self.reclassify_moves(ws, &landed, &departing);
        let moved = !landed.is_empty();
        if moved {
            self.restore_structure_selection(ws);
        }
        moved
    }

    /// Adjacent list blocks can have different styles — for example a bullet
    /// followed by a `[]` checkbox. Their containers are siblings, so the
    /// shared list-only indent cannot see the visible item above. Carry the
    /// selected fragment beneath that item and retain its own marker style.
    fn indent_list_below_list_item(
        &mut self,
        ws: &mut WorkspaceDoc,
        ids: &[koru_core::NodeId],
        first: koru_core::NodeId,
    ) -> bool {
        let Some(previous) = self.previous_visible_row(first) else {
            return false;
        };
        if !self.row_is_list_item(previous) || !self.direct_list_fragment(ws, ids) {
            return false;
        }
        let Ok(Parent::Node(source)) = ws.parent(first) else {
            return false;
        };
        // The visible item above must be a direct item of *another* ListBlock
        // — that is what "adjacent list blocks" means. A row that is merely a
        // descendant inside the same list subtree (a task under a task) takes
        // the ordinary one-level indent instead of swallowing the fragment.
        if !matches!(ws.parent(previous), Ok(Parent::Node(p))
            if p != source && ws.read(p).is_ok_and(|node| node.kind == Kind::ListBlock))
        {
            return false;
        }
        // Items that already share a ListBlock keep the editor's established
        // direct-child indentation behaviour. This path is only for adjacent
        // visible items whose marker containers are separate siblings.
        if matches!(ws.parent(previous), Ok(Parent::Node(parent)) if parent == source) {
            return false;
        }
        let at = ws
            .children(Parent::Node(previous))
            .map_or(0, |children| children.len());
        self.move_list_fragment(ws, ids, Parent::Node(previous), at, false)
    }

    /// Lift a list fragment by one visible level without extracting its item
    /// from the ListBlock. The suffix travels with the selected fragment so
    /// tree order remains the same after a nested list becomes a sibling.
    fn outdent_list_fragment(
        &mut self,
        ws: &mut WorkspaceDoc,
        ids: &[koru_core::NodeId],
        first: koru_core::NodeId,
    ) -> bool {
        if !self.direct_list_fragment(ws, ids) {
            return false;
        }
        let Ok(Parent::Node(list)) = ws.parent(first) else {
            return false;
        };
        let Ok(Parent::Node(parent)) = ws.parent(list) else {
            return false;
        };

        let (destination, at) = if ws
            .read(parent)
            .is_ok_and(|node| node.kind == Kind::ListBlock)
        {
            // The source list is a sibling of another item in an enclosing
            // list. Leave that enclosing ListBlock and become a list under its
            // visible owner instead.
            let Ok(Parent::Node(owner)) = ws.parent(parent) else {
                return false;
            };
            (
                Parent::Node(owner),
                ws.sibling_index(parent).map_or(0, |index| index + 1),
            )
        } else {
            // A list directly under an item becomes a sibling after that item.
            // At the document root there is no shallower document level, so
            // preserve the marker and leave it where it is.
            if self.editor.body.root == Some(parent) {
                return false;
            }
            let Ok(destination) = ws.parent(parent) else {
                return false;
            };
            (
                destination,
                ws.sibling_index(parent).map_or(0, |index| index + 1),
            )
        };
        self.move_list_fragment(ws, ids, destination, at, true)
    }

    /// The reverse of nesting a task under a bullet: leave the bullet's
    /// ListBlock entirely, rather than merely becoming another direct child of
    /// that invisible container.
    fn outdent_tasks_from_bullet(
        &mut self,
        ws: &mut WorkspaceDoc,
        ids: &[koru_core::NodeId],
        first: koru_core::NodeId,
    ) -> bool {
        if !ids.iter().all(|id| self.row_is_task(*id)) {
            return false;
        }
        let Ok(Parent::Node(bullet)) = ws.parent(first) else {
            return false;
        };
        // Only tasks under a *bullet* leave the list — the mirror of
        // `indent_tasks_below_bullet`. A task under another task outdents the
        // ordinary way, to a sibling checkbox, or Shift+Tab on any nested
        // subtask would silently extract it from its checklist.
        if self.row_is_task(bullet) {
            return false;
        }
        let Ok(Parent::Node(list)) = ws.parent(bullet) else {
            return false;
        };
        if !ws.read(list).is_ok_and(|node| node.kind == Kind::ListBlock) {
            return false;
        }
        let Ok(destination) = ws.parent(list) else {
            return false;
        };

        let departing = koru_core::tasks::departing_all(ws, ids);
        ws.checkpoint();
        ws.begin_group();
        let mut at = ws.sibling_index(list).map_or(0, |index| index + 1);
        let mut landed = Vec::new();
        for &id in ids {
            if ws.move_node(id, destination, at).is_ok() {
                at += 1;
                landed.push(id);
            }
        }
        ws.checkpoint();
        // Leaving a bullet is still asked, and the *arrival* half still costs
        // nothing: the rows end up shallower than they started, so their
        // completed ancestors are a prefix of the ones they already had, and
        // moving out is not adding under. The **departure** half is exactly the
        // one that can answer here — an outdent far enough to clear a task is
        // how a row most often leaves one.
        self.reclassify_moves(ws, &landed, &departing);
        let moved = !landed.is_empty();
        if moved {
            self.restore_structure_selection(ws);
        }
        moved
    }

    fn move_list_fragment(
        &mut self,
        ws: &mut WorkspaceDoc,
        ids: &[koru_core::NodeId],
        destination: Parent,
        at: usize,
        include_following: bool,
    ) -> bool {
        let carrying = Self::fragment_carrying(ws, ids, include_following);
        // The whole fragment, not just the selection: an unticked follower that
        // is dragged along leaves its task exactly as the selected items do.
        let departing = koru_core::tasks::departing_all(ws, &carrying);
        ws.checkpoint();
        ws.begin_group();
        let moved = ws
            .reparent_list_fragment(ids, destination, at, include_following)
            .unwrap_or(false);
        ws.checkpoint();
        if moved {
            self.reclassify_moves(ws, &carrying, &departing);
            self.restore_structure_selection(ws);
        }
        moved
    }

    fn previous_visible_row(&self, id: koru_core::NodeId) -> Option<koru_core::NodeId> {
        let index = self.editor.body.index_of(id)?;
        index
            .checked_sub(1)
            .map(|previous| self.editor.body.rows[previous].id)
    }

    fn row_is_task(&self, id: koru_core::NodeId) -> bool {
        self.editor.body.row(id).is_some_and(|row| row.is_task())
    }

    fn row_is_list_item(&self, id: koru_core::NodeId) -> bool {
        self.editor
            .body
            .row(id)
            .is_some_and(|row| row.list_style.is_some())
    }

    fn direct_list_fragment(&self, ws: &WorkspaceDoc, ids: &[koru_core::NodeId]) -> bool {
        let Some(&first) = ids.first() else {
            return false;
        };
        let Ok(Parent::Node(list)) = ws.parent(first) else {
            return false;
        };
        if !ws.read(list).is_ok_and(|node| node.kind == Kind::ListBlock) {
            return false;
        }
        ids.iter()
            .all(|id| matches!(ws.parent(*id), Ok(Parent::Node(parent)) if parent == list))
    }

    /// **Ask the classification whether these landings reopened anything**
    /// (SPEC §5, [ADR-285]) — the document's own structural moves going through
    /// the same seam a drop, a paste and the shared `indent_list_items` already
    /// use.
    ///
    /// Told as a *move*, so the activity reads `task.child.moved`: this module
    /// only ever relocates rows that already exist. Called **after** the move
    /// has landed and after the move's own trailing `checkpoint`, so a reopen is
    /// its own undo step; `reclassify_moved` opens no group at all when nothing
    /// above the row was completed, which is every ordinary indent.
    ///
    /// Answers whether anything reopened, because a caller that has already
    /// rebuilt its rows has to rebuild them again to draw the reopened state.
    ///
    /// **`departing` is the other half, and it is recorded by the caller before
    /// the move** (T-0c): `task.child.removed` is the one task question a move
    /// cannot answer afterwards, because the ancestry that answers it is what
    /// the move took away. Spending it here rather than at each site keeps the
    /// pair — arrival and departure — in one function, so a new structural move
    /// in this module cannot wire one without the other.
    ///
    /// [ADR-285]: ../../../docs/decisions/ADR-276-300.md
    fn reclassify_moves(
        &mut self,
        ws: &mut WorkspaceDoc,
        moved: &[koru_core::NodeId],
        departing: &[koru_core::tasks::Departing],
    ) -> bool {
        let mut reopened = false;
        for &id in moved {
            let outcome = koru_editor::tasks::run(&mut self.editor.body.task_activity, ws, |ws| {
                koru_core::tasks::reclassify_moved(ws, id)
            });
            reopened |= matches!(outcome, Ok(Some(_)));
        }
        self.editor.body.note_departures(ws, departing);
        reopened
    }

    /// The items a fragment move carries: the selected ones, and — when the
    /// move takes the rest of the list with it — every direct item that follows
    /// them. Read **before** the move, because afterwards the source ListBlock
    /// may have been deleted.
    ///
    /// The followers matter rather than being belt and braces: a fragment of
    /// ticked items can drag an *unticked* one along behind it, and classifying
    /// only what the user selected would let that one land under a completed
    /// task unnoticed.
    fn fragment_carrying(
        ws: &WorkspaceDoc,
        ids: &[koru_core::NodeId],
        include_following: bool,
    ) -> Vec<koru_core::NodeId> {
        if !include_following {
            return ids.to_vec();
        }
        let Some(&first) = ids.first() else {
            return Vec::new();
        };
        let Ok(Parent::Node(list)) = ws.parent(first) else {
            return ids.to_vec();
        };
        let Ok(children) = ws.children(Parent::Node(list)) else {
            return ids.to_vec();
        };
        match children.iter().position(|&id| id == first) {
            Some(start) => children[start..].to_vec(),
            None => ids.to_vec(),
        }
    }

    fn restore_structure_selection(&mut self, ws: &WorkspaceDoc) {
        let anchor = self.editor.body.anchor;
        let focused = self.editor.body.focused_id();
        self.editor.body.rebuild(ws);
        self.editor.body.anchor = anchor;
        if let Some(id) = focused {
            self.editor.body.focus_on(id);
        }
        self.editor.body.mark_selection();
    }
}
