// SPDX-License-Identifier: MPL-2.0

//! The **Move to…** picker: choose a new parent for a document.
//!
//! Browse mode lists every workspace in the profile (collapsed), with the
//! current workspace first and pre-selected. Expand in place to walk that
//! workspace's document tree. Filter mode (typing) flattens matching titles
//! and shows the workspace name on the right.
//!
//! Placement:
//! - **Enter** on a workspace → top of that workspace's library
//! - **Enter** on a document → last sibling under that document's parent
//! - **Shift+Enter** on a document → last child of that document
//! - **Alt+Enter** (with or without Shift) → same placement and **follow**
//!
//! Cross-workspace confirms become a shell transfer; same-workspace uses
//! [`PaneRequest::MoveDocument`]. The surface itself holds no store client.

use std::collections::HashSet;

use koru_core::explore::ExploreEntry;
use koru_core::{DocumentId, WorkspaceEntry, WorkspaceId};
use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_ui::selectable_list::SelectableList;
use koru_ui::text_input::{InputStyle, TextInput};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key, ModifiersState, NamedKey};

pub const SURFACE_RADIUS: f32 = 8.0;

const HEADER_H: f32 = 48.0;
const FOOTER_H: f32 = 34.0;
const ROW_H: f32 = 32.0;
const PAD: f32 = 14.0;
const INDENT: f32 = 16.0;
/// Phosphor caret size — half a caption icon, same as explore.
const CARET_SIZE: f32 = theme::CAPTION_ICON * 0.5;

/// Where the document should land, and whether the window should follow it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    pub workspace: WorkspaceId,
    /// `None` = library root of that workspace.
    pub parent: Option<DocumentId>,
    /// Entry index among siblings under `parent`. `usize::MAX` means append.
    pub index: usize,
    pub follow: bool,
    /// Human path for the toast, e.g. `Product › Roadmap`.
    pub path_label: String,
}

/// A hit from the store search index (filter mode).
#[derive(Clone, Debug)]
pub struct SearchHit {
    pub workspace: WorkspaceId,
    pub workspace_name: String,
    pub document: DocumentId,
    pub title: String,
    /// Parent for sibling placement when known from a loaded listing.
    pub parent: Option<DocumentId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    Workspace,
    Document,
}

#[derive(Clone, Debug)]
struct FlatRow {
    kind: RowKind,
    workspace: WorkspaceId,
    workspace_name: String,
    /// Workspace tile (workspace rows only).
    icon: Option<String>,
    color: Option<String>,
    /// Set for document rows.
    document: Option<DocumentId>,
    /// Parent of this document in its workspace listing (document rows only).
    parent: Option<DocumentId>,
    title: String,
    depth: u32,
    expandable: bool,
    expanded: bool,
    /// Current workspace badge.
    current: bool,
    /// Selecting this would nest the subject under itself / a descendant.
    blocked: bool,
}

/// One workspace's listing the picker can walk.
#[derive(Clone, Debug)]
pub struct WorkspaceTree {
    pub id: WorkspaceId,
    pub name: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub current: bool,
    pub entries: Vec<ExploreEntry>,
    /// Listing has been fetched (or is the open workspace's).
    pub loaded: bool,
}

pub enum Action {
    Keep,
    Cancel,
    Confirm(Destination),
    /// Expand asked for a workspace listing that is not loaded yet.
    NeedListing(WorkspaceId),
    /// Filter query changed — host should hit the search index.
    QueryChanged(String),
}

/// What the list selection is anchored to after expand/collapse shuffles indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelId {
    Workspace(WorkspaceId),
    Document(DocumentId),
}

pub struct MoveToSurface {
    /// Explore roots being moved (children ride with each root).
    subjects: Vec<DocumentId>,
    subject_title: String,
    /// Document ids in every subject's explore subtree — blocked nest targets.
    blocked: HashSet<DocumentId>,
    trees: Vec<WorkspaceTree>,
    expanded: HashSet<u64>,
    query: TextInput,
    list: SelectableList,
    /// Stable selection identity (survives expand inserting children).
    sel: SelId,
    /// Store search hits for filter mode; `None` until the first query answer.
    search_hits: Option<Vec<SearchHit>>,
    viewport: (f32, f32),
}

impl MoveToSurface {
    /// The filter box, for a dictation the container is routing.
    pub(super) fn dictation_field(&mut self) -> &mut TextInput {
        &mut self.query
    }

    pub fn new(
        engine: &mut TextEngine,
        subjects: Vec<DocumentId>,
        subject_title: String,
        workspaces: &[WorkspaceEntry],
        current: WorkspaceId,
        current_entries: &[ExploreEntry],
    ) -> Self {
        let mut blocked = HashSet::new();
        for id in &subjects {
            blocked.extend(subtree_ids(*id, current_entries));
        }
        let mut trees: Vec<WorkspaceTree> = Vec::new();
        // Current workspace first.
        if let Some(ws) = workspaces.iter().find(|w| w.id == current && !w.archived) {
            trees.push(WorkspaceTree {
                id: ws.id,
                name: ws.name.clone(),
                icon: ws.icon.clone(),
                color: ws.color.clone(),
                current: true,
                entries: current_entries
                    .iter()
                    .filter(|e| e.trashed_at.is_none())
                    .cloned()
                    .collect(),
                loaded: true,
            });
        }
        for ws in workspaces {
            if ws.id == current || ws.archived || ws.deleted_at.is_some() {
                continue;
            }
            trees.push(WorkspaceTree {
                id: ws.id,
                name: ws.name.clone(),
                icon: ws.icon.clone(),
                color: ws.color.clone(),
                current: false,
                // Filled when the host loads them via ListWorkspaceDocuments.
                entries: Vec::new(),
                loaded: false,
            });
        }
        let sel = trees
            .first()
            .map(|t| SelId::Workspace(t.id))
            .unwrap_or(SelId::Workspace(current));
        let mut surface = Self {
            subjects,
            subject_title,
            blocked,
            trees,
            expanded: HashSet::new(),
            query: TextInput::new(engine, theme::role::row()),
            list: SelectableList::new(ROW_H),
            sel,
            search_hits: None,
            viewport: (1.0, 1.0),
        };
        surface.sync_selection_index();
        surface
    }

    pub fn subjects(&self) -> &[DocumentId] {
        &self.subjects
    }

    #[allow(dead_code)]
    pub fn subject(&self) -> DocumentId {
        self.subjects[0]
    }

    pub fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport = (w.max(1.0), h.max(1.0));
        let rows = self.visible_rows();
        self.list.ensure_visible(rows.len(), self.list_height());
    }

    /// Replace the document listing for a workspace (prefetch / refresh).
    pub fn set_workspace_entries(&mut self, id: WorkspaceId, entries: Vec<ExploreEntry>) {
        if let Some(tree) = self.trees.iter_mut().find(|t| t.id == id) {
            tree.entries = entries
                .into_iter()
                .filter(|e| e.trashed_at.is_none())
                .collect();
            tree.loaded = true;
        }
        if id
            == self
                .trees
                .iter()
                .find(|t| t.current)
                .map(|t| t.id)
                .unwrap_or(id)
        {
            let entries = self
                .trees
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.entries.as_slice())
                .unwrap_or(&[]);
            self.blocked.clear();
            for sid in &self.subjects {
                self.blocked.extend(subtree_ids(*sid, entries));
            }
        }
        self.sync_selection_index();
    }

    /// Replace filter-mode rows from the store search index.
    pub fn set_search_hits(&mut self, hits: Vec<SearchHit>) {
        self.search_hits = Some(hits);
        self.list.selected = 0;
        self.list.scroll = 0.0;
        self.sync_selection_index();
    }

    /// Map entity-picker style hits into rows, filling parent from loaded trees.
    pub fn set_entity_hits(
        &mut self,
        hits: impl IntoIterator<
            Item = (
                WorkspaceId,
                DocumentId,
                String, /* title */
                String, /* workspace fallback name */
            ),
        >,
    ) {
        let mut parent_of = std::collections::HashMap::new();
        let mut name_of = std::collections::HashMap::new();
        for tree in &self.trees {
            name_of.insert(tree.id, tree.name.clone());
            for e in &tree.entries {
                parent_of.insert(e.id, e.parent);
            }
        }
        let hits: Vec<SearchHit> = hits
            .into_iter()
            .map(|(workspace, document, title, fallback_ws)| {
                let workspace_name = name_of.get(&workspace).cloned().unwrap_or(fallback_ws);
                let parent = parent_of.get(&document).copied().flatten();
                SearchHit {
                    workspace,
                    workspace_name,
                    document,
                    title,
                    parent,
                }
            })
            .collect();
        self.set_search_hits(hits);
    }

    /// Workspace ids this picker covers (for scoping a search request).
    pub fn workspace_ids(&self) -> Vec<WorkspaceId> {
        self.trees.iter().map(|t| t.id).collect()
    }

    fn row_id(row: &FlatRow) -> SelId {
        match row.kind {
            RowKind::Workspace => SelId::Workspace(row.workspace),
            RowKind::Document => SelId::Document(row.document.expect("document row")),
        }
    }

    fn sync_selection_index(&mut self) {
        let rows = self.visible_rows();
        if let Some(i) = rows.iter().position(|r| Self::row_id(r) == self.sel) {
            self.list.selected = i;
        } else {
            self.list.selected = self.list.selected.min(rows.len().saturating_sub(1));
            if let Some(r) = rows.get(self.list.selected) {
                self.sel = Self::row_id(r);
            }
        }
        self.list.ensure_visible(rows.len(), self.list_height());
    }

    fn is_expanded_ws(&self, id: WorkspaceId) -> bool {
        self.expanded.contains(&ws_key(id))
    }

    fn is_expanded_doc(&self, id: DocumentId) -> bool {
        self.expanded.contains(&doc_key(id))
    }

    fn list_height(&self) -> f32 {
        (self.viewport.1 - HEADER_H - FOOTER_H).max(0.0)
    }

    fn query_rect(&self) -> Rect {
        Rect::new(PAD, 10.0, (self.viewport.0 - PAD * 2.0).max(1.0), 28.0)
    }

    fn list_top(&self) -> f32 {
        HEADER_H
    }

    fn visible_rows(&self) -> Vec<FlatRow> {
        let q = self.query.raw_text().trim().to_lowercase();
        if q.is_empty() {
            self.browse_rows()
        } else {
            self.filter_rows(&q)
        }
    }

    fn browse_rows(&self) -> Vec<FlatRow> {
        let mut out = Vec::new();
        for tree in &self.trees {
            let exp = self.is_expanded_ws(tree.id);
            // Always expandable: empty workspaces still show a fold so the
            // user can confirm there is nothing under them.
            out.push(FlatRow {
                kind: RowKind::Workspace,
                workspace: tree.id,
                workspace_name: tree.name.clone(),
                icon: tree.icon.clone(),
                color: tree.color.clone(),
                document: None,
                parent: None,
                title: tree.name.clone(),
                depth: 0,
                expandable: true,
                expanded: exp,
                current: tree.current,
                blocked: false,
            });
            if exp {
                self.push_doc_rows(&mut out, tree, None, 1);
            }
        }
        out
    }

    fn push_doc_rows(
        &self,
        out: &mut Vec<FlatRow>,
        tree: &WorkspaceTree,
        parent: Option<DocumentId>,
        depth: u32,
    ) {
        let mut children: Vec<&ExploreEntry> =
            tree.entries.iter().filter(|e| e.parent == parent).collect();
        children.sort_by_key(|e| e.ordinal);
        for e in children {
            // Never offer a moving root (or its subtree) as a destination.
            if tree.current && self.blocked.contains(&e.id) && self.subjects.contains(&e.id) {
                continue;
            }
            if tree.current && self.subjects.contains(&e.id) {
                continue;
            }
            let has_kids = tree.entries.iter().any(|c| c.parent == Some(e.id));
            let exp = self.is_expanded_doc(e.id);
            let blocked = tree.current && self.blocked.contains(&e.id);
            out.push(FlatRow {
                kind: RowKind::Document,
                workspace: tree.id,
                workspace_name: tree.name.clone(),
                icon: None,
                color: None,
                document: Some(e.id),
                parent: e.parent,
                title: if e.title.is_empty() {
                    "Untitled".into()
                } else {
                    e.title.clone()
                },
                depth,
                expandable: has_kids,
                expanded: exp,
                current: false,
                blocked,
            });
            if exp && has_kids {
                self.push_doc_rows(out, tree, Some(e.id), depth + 1);
            }
        }
    }

    fn filter_rows(&self, q: &str) -> Vec<FlatRow> {
        // Prefer store index hits when the host has answered; fall back to
        // local listings so typing still works offline / before the first poll.
        if let Some(hits) = &self.search_hits {
            return hits
                .iter()
                .filter(|h| !self.subjects.contains(&h.document))
                .map(|h| {
                    let blocked = self.blocked.contains(&h.document);
                    FlatRow {
                        kind: RowKind::Document,
                        workspace: h.workspace,
                        workspace_name: h.workspace_name.clone(),
                        icon: None,
                        color: None,
                        document: Some(h.document),
                        parent: h.parent,
                        title: if h.title.is_empty() {
                            "Untitled".into()
                        } else {
                            h.title.clone()
                        },
                        depth: 0,
                        expandable: false,
                        expanded: false,
                        current: false,
                        blocked,
                    }
                })
                .collect();
        }
        let mut out = Vec::new();
        for tree in &self.trees {
            for e in &tree.entries {
                if tree.current && self.subjects.contains(&e.id) {
                    continue;
                }
                let title = if e.title.is_empty() {
                    "Untitled"
                } else {
                    e.title.as_str()
                };
                if !title.to_lowercase().contains(q) {
                    continue;
                }
                let blocked = tree.current && self.blocked.contains(&e.id);
                out.push(FlatRow {
                    kind: RowKind::Document,
                    workspace: tree.id,
                    workspace_name: tree.name.clone(),
                    icon: None,
                    color: None,
                    document: Some(e.id),
                    parent: e.parent,
                    title: title.to_owned(),
                    depth: 0,
                    expandable: false,
                    expanded: false,
                    current: false,
                    blocked,
                });
            }
        }
        out
    }

    /// Toggle expand; returns a workspace that still needs its listing fetched.
    fn toggle_expand(&mut self, row: &FlatRow) -> Option<WorkspaceId> {
        self.sel = Self::row_id(row);
        let key = match row.kind {
            RowKind::Workspace => ws_key(row.workspace),
            RowKind::Document => match row.document {
                Some(id) => doc_key(id),
                None => return None,
            },
        };
        let expanding = !self.expanded.contains(&key);
        if expanding {
            self.expanded.insert(key);
        } else {
            self.expanded.remove(&key);
        }
        self.sync_selection_index();
        if expanding && row.kind == RowKind::Workspace {
            let loaded = self
                .trees
                .iter()
                .find(|t| t.id == row.workspace)
                .is_some_and(|t| t.loaded);
            if !loaded {
                return Some(row.workspace);
            }
        }
        None
    }

    fn destination_for(&self, row: &FlatRow, as_child: bool) -> Option<Destination> {
        if row.blocked {
            return None;
        }
        let follow = false; // filled by caller from mods
        match row.kind {
            RowKind::Workspace => {
                let path = row.workspace_name.clone();
                Some(Destination {
                    workspace: row.workspace,
                    parent: None,
                    index: 0,
                    follow,
                    path_label: path,
                })
            }
            RowKind::Document => {
                let doc = row.document?;
                if as_child {
                    let path = format!("{} › {}", row.workspace_name, row.title);
                    Some(Destination {
                        workspace: row.workspace,
                        parent: Some(doc),
                        index: usize::MAX,
                        follow,
                        path_label: path,
                    })
                } else {
                    // Last sibling under the same parent.
                    let parent = row.parent;
                    let path = match parent.and_then(|p| {
                        self.trees
                            .iter()
                            .find(|t| t.id == row.workspace)
                            .and_then(|t| t.entries.iter().find(|e| e.id == p))
                    }) {
                        Some(p) => format!(
                            "{} › {}",
                            row.workspace_name,
                            if p.title.is_empty() {
                                "Untitled"
                            } else {
                                &p.title
                            }
                        ),
                        None => row.workspace_name.clone(),
                    };
                    Some(Destination {
                        workspace: row.workspace,
                        parent,
                        index: usize::MAX,
                        follow,
                        path_label: path,
                    })
                }
            }
        }
    }

    pub fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Action {
        if matches!(key, Key::Named(NamedKey::Escape)) {
            return Action::Cancel;
        }

        let rows = self.visible_rows();
        let list_h = self.list_height();

        // Navigation / confirm before typing, so arrows do not insert.
        match key {
            Key::Named(NamedKey::ArrowDown) if !mods.shift_key() && !mods.alt_key() => {
                self.list.step(1, rows.len(), list_h);
                if let Some(r) = self.visible_rows().get(self.list.selected) {
                    self.sel = Self::row_id(r);
                }
                return Action::Keep;
            }
            Key::Named(NamedKey::ArrowUp) if !mods.shift_key() && !mods.alt_key() => {
                self.list.step(-1, rows.len(), list_h);
                if let Some(r) = self.visible_rows().get(self.list.selected) {
                    self.sel = Self::row_id(r);
                }
                return Action::Keep;
            }
            Key::Named(NamedKey::ArrowRight) if !mods.shift_key() && !mods.alt_key() => {
                if let Some(row) = rows.get(self.list.selected).cloned()
                    && row.expandable
                    && !row.expanded
                {
                    if let Some(ws) = self.toggle_expand(&row) {
                        return Action::NeedListing(ws);
                    }
                }
                return Action::Keep;
            }
            Key::Named(NamedKey::ArrowLeft) if !mods.shift_key() && !mods.alt_key() => {
                if let Some(row) = rows.get(self.list.selected).cloned()
                    && row.expandable
                    && row.expanded
                {
                    let _ = self.toggle_expand(&row);
                }
                return Action::Keep;
            }
            Key::Named(NamedKey::Enter) => {
                let Some(row) = rows.get(self.list.selected) else {
                    return Action::Keep;
                };
                let as_child = mods.shift_key() && row.kind == RowKind::Document;
                let follow = mods.alt_key();
                if let Some(mut dest) = self.destination_for(row, as_child) {
                    dest.follow = follow;
                    return Action::Confirm(dest);
                }
                return Action::Keep;
            }
            _ => {}
        }

        let before = self.query.raw_text().to_owned();
        self.query.key(engine, key, text, mods);
        if self.query.raw_text() != before {
            self.list.selected = 0;
            self.list.scroll = 0.0;
            self.search_hits = None;
            if let Some(r) = self.visible_rows().first() {
                self.sel = Self::row_id(r);
            }
            let q = self.query.raw_text().trim().to_owned();
            return Action::QueryChanged(q);
        }
        Action::Keep
    }

    pub fn mouse_down(&mut self, engine: &mut TextEngine, x: f32, y: f32) -> Action {
        let q = self.query_rect();
        if self.query.click(engine, q, x, y) {
            return Action::Keep;
        }
        let rows = self.visible_rows();
        let top = self.list_top();
        if let Some(i) = self.list.row_at(top, y, rows.len()) {
            self.list.selected = i;
            if let Some(row) = rows.get(i).cloned() {
                self.sel = Self::row_id(&row);
                self.list.ensure_visible(rows.len(), self.list_height());
                // Caret zone, or click on a collapsed workspace row body.
                let depth_x = PAD + row.depth as f32 * INDENT;
                let on_caret = row.expandable && x >= depth_x && x < depth_x + 18.0;
                let on_ws_toggle = row.kind == RowKind::Workspace && row.expandable;
                if on_caret || (on_ws_toggle && x < depth_x + 40.0) {
                    if let Some(ws) = self.toggle_expand(&row) {
                        return Action::NeedListing(ws);
                    }
                    return Action::Keep;
                }
            }
        }
        Action::Keep
    }

    pub fn mouse_double_click(&mut self, _engine: &mut TextEngine, x: f32, y: f32) -> Action {
        let rows = self.visible_rows();
        let top = self.list_top();
        if let Some(i) = self.list.row_at(top, y, rows.len())
            && let Some(row) = rows.get(i)
            && let Some(dest) = self.destination_for(row, false)
        {
            let _ = x;
            return Action::Confirm(dest);
        }
        Action::Keep
    }

    pub fn scroll_by(&mut self, delta: f32) {
        let rows = self.visible_rows();
        self.list.scroll_by(delta, rows.len(), self.list_height());
    }

    pub fn draw(&mut self, scene: &mut Scene, engine: &mut TextEngine, root: Affine) {
        let p = theme::p();
        let w = self.viewport.0;
        let h = self.viewport.1;

        let heading = if self.subjects.len() > 1 {
            format!("Move {} nodes to\u{2026}", self.subjects.len())
        } else {
            format!("Move \u{201C}{}\u{201D} to\u{2026}", self.subject_title)
        };
        let head = engine.simple_layout(&heading, theme::role::ui());
        scene::draw_layout_first_line(scene, root, &head, PAD, 8.0, Some(p.text_dim));

        let q = self.query_rect();
        // Empty browse matches the wireframe (no search strip). Typing reveals
        // the filter field.
        if !self.query.is_empty() {
            self.query.draw(
                scene,
                root,
                engine,
                q,
                InputStyle::search(),
                Some("Filter documents\u{2026}"),
                true,
            );
        }

        let rows = self.visible_rows();
        let top = self.list_top();
        let list_h = self.list_height();
        let first = (self.list.scroll / ROW_H).floor().max(0.0) as usize;
        let visible = (list_h / ROW_H).ceil() as usize + 2;
        let last = (first + visible).min(rows.len());

        for i in first..last {
            let row = &rows[i];
            let y = self.list.row_y(top, i);
            if y + ROW_H < top || y > top + list_h {
                continue;
            }
            let selected = i == self.list.selected;
            if selected {
                scene::fill(
                    scene,
                    root,
                    p.row_selected,
                    &scene::rrect(PAD * 0.5, y, (w - PAD).max(1.0), ROW_H, 4.0),
                );
            }
            let mut x = PAD + row.depth as f32 * INDENT;
            // Phosphor carets via icon_layout — simple_layout uses the text
            // face and draws garbage for PUA codepoints.
            if row.expandable {
                let glyph = if row.expanded {
                    koru_draw::icons::CARET_DOWN
                } else {
                    koru_draw::icons::CARET_RIGHT
                };
                let layout = engine.icon_layout(glyph, CARET_SIZE);
                let (cw, ch) = (layout.width(), layout.height());
                scene::draw_layout(
                    scene,
                    root,
                    &layout,
                    x + (16.0 - cw) * 0.5,
                    y + (ROW_H - ch) * 0.5,
                    Some(p.text_dim),
                );
                x += 16.0;
            } else {
                x += 16.0;
            }
            // Workspace identity: same Tile the switcher uses.
            if row.kind == RowKind::Workspace {
                const TILE: f32 = 18.0;
                let ty = y + (ROW_H - TILE) * 0.5;
                koru_draw::tile::Tile {
                    icon: row.icon.as_deref(),
                    color: row.color.as_deref(),
                    name: &row.title,
                }
                .draw(scene, root, engine, Rect::new(x, ty, TILE, TILE));
                x += TILE + 8.0;
            }
            let color = if row.blocked { p.text_dim } else { p.text };
            let title = engine.simple_layout(&row.title, theme::role::row());
            scene::draw_layout_first_line(scene, root, &title, x, y + 7.0, Some(color));
            let badge = if !self.query.is_empty() {
                Some(row.workspace_name.as_str())
            } else if row.current {
                Some("current")
            } else {
                None
            };
            if let Some(badge) = badge {
                let bl = engine.simple_layout(badge, theme::role::ui());
                scene::draw_layout_first_line(
                    scene,
                    root,
                    &bl,
                    w - PAD - bl.width(),
                    y + 8.0,
                    Some(p.text_dim),
                );
            }
        }

        // Footer hints — same vertical rule as search: every icon and label
        // shares one `draw_layout_first_line` y so Phosphor and the UI face
        // sit on one baseline. LTR segments, group right-aligned.
        paint_footer_hints(scene, root, engine, w, h, self.query.is_empty(), p.text_dim);
    }
}

/// One footer atom: optional Phosphor glyph, then text (may be empty).
enum FooterBit {
    Icon(char),
    Text(&'static str),
}

fn paint_footer_hints(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    w: f32,
    h: f32,
    browse: bool,
    colour: vello::peniko::Color,
) {
    // Pairs glued with ICON_GAP; groups separated by GROUP_GAP.
    const ICON_GAP: f32 = 3.0;
    const GROUP_GAP: f32 = 12.0;
    const ICON_PX: f32 = 13.0;
    let y = h - FOOTER_H + 7.0;
    let style = theme::role::hint();

    // Each inner vec is one visual group (e.g. Enter + "move").
    let groups: &[&[FooterBit]] = if browse {
        &[
            &[FooterBit::Text("\u{2191}\u{2193} walk")],
            &[FooterBit::Text("\u{2192} expand")],
            &[
                FooterBit::Icon(koru_draw::icons::KEY_RETURN),
                FooterBit::Text("move"),
            ],
        ]
    } else {
        &[
            &[
                FooterBit::Icon(koru_draw::icons::KEY_RETURN),
                FooterBit::Text("sibling"),
            ],
            &[
                FooterBit::Text("shift+"),
                FooterBit::Icon(koru_draw::icons::KEY_RETURN),
                FooterBit::Text("child"),
            ],
            &[
                FooterBit::Text("alt+"),
                FooterBit::Icon(koru_draw::icons::KEY_RETURN),
                FooterBit::Text("follow"),
            ],
        ]
    };

    // Measure total width.
    let mut total = 0.0;
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            total += GROUP_GAP;
        }
        for (bi, bit) in group.iter().enumerate() {
            if bi > 0 {
                total += ICON_GAP;
            }
            total += match bit {
                FooterBit::Icon(c) => engine.icon_layout(*c, ICON_PX).width(),
                FooterBit::Text(t) => engine.simple_layout(t, style).width(),
            };
        }
    }

    let mut x = (w - PAD - total).max(PAD);
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            x += GROUP_GAP;
        }
        for (bi, bit) in group.iter().enumerate() {
            if bi > 0 {
                x += ICON_GAP;
            }
            match bit {
                FooterBit::Icon(c) => {
                    let glyph = engine.icon_layout(*c, ICON_PX);
                    scene::draw_layout_first_line(scene, root, &glyph, x, y, Some(colour));
                    x += glyph.width();
                }
                FooterBit::Text(t) => {
                    let text = engine.simple_layout(t, style);
                    scene::draw_layout_first_line(scene, root, &text, x, y, Some(colour));
                    x += text.width();
                }
            }
        }
    }
}

fn ws_key(id: WorkspaceId) -> u64 {
    id.as_uuid().as_u64_pair().0 ^ id.as_uuid().as_u64_pair().1
}

fn doc_key(id: DocumentId) -> u64 {
    id.as_uuid().as_u64_pair().0 ^ id.as_uuid().as_u64_pair().1.wrapping_add(1)
}

/// Subject plus every explore descendant in `entries`.
fn subtree_ids(root: DocumentId, entries: &[ExploreEntry]) -> HashSet<DocumentId> {
    let mut set = HashSet::new();
    set.insert(root);
    let mut changed = true;
    while changed {
        changed = false;
        for e in entries {
            if let Some(p) = e.parent
                && set.contains(&p)
                && set.insert(e.id)
            {
                changed = true;
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_core::explore::ExploreEntry;

    fn entry(
        id: DocumentId,
        parent: Option<DocumentId>,
        ordinal: u32,
        title: &str,
    ) -> ExploreEntry {
        ExploreEntry {
            id,
            parent,
            ordinal,
            title: title.into(),
            kind: "page".into(),
            mime: None,
            note: None,
            trashed_at: None,
            trashed_by: None,
        }
    }

    #[test]
    fn subtree_includes_nested_descendants() {
        let a = DocumentId::new();
        let b = DocumentId::new();
        let c = DocumentId::new();
        let entries = vec![
            entry(a, None, 0, "A"),
            entry(b, Some(a), 0, "B"),
            entry(c, Some(b), 0, "C"),
        ];
        let set = subtree_ids(a, &entries);
        assert!(set.contains(&a) && set.contains(&b) && set.contains(&c));
        assert_eq!(subtree_ids(b, &entries).len(), 2);
    }

    fn ws_entry(id: WorkspaceId, name: &str) -> WorkspaceEntry {
        WorkspaceEntry {
            id,
            name: name.into(),
            icon: None,
            color: None,
            pinned: false,
            archived: false,
            deleted_at: None,
            order: 0,
            created: 0,
        }
    }

    #[test]
    fn enter_on_workspace_goes_to_library_root() {
        let mut engine = TextEngine::new();
        let subject = DocumentId::new();
        let ws = WorkspaceId::new();
        let surface = MoveToSurface::new(
            &mut engine,
            vec![subject],
            "Page".into(),
            &[ws_entry(ws, "Specs")],
            ws,
            &[],
        );
        let rows = surface.browse_rows();
        assert_eq!(rows.len(), 1);
        let dest = surface.destination_for(&rows[0], false).unwrap();
        assert_eq!(dest.parent, None);
        assert_eq!(dest.index, 0);
        assert_eq!(dest.workspace, ws);
    }

    #[test]
    fn enter_on_document_is_sibling_shift_is_child() {
        let mut engine = TextEngine::new();
        let subject = DocumentId::new();
        let parent = DocumentId::new();
        let target = DocumentId::new();
        let ws = WorkspaceId::new();
        let entries = vec![
            entry(parent, None, 0, "Roadmap"),
            entry(target, Some(parent), 0, "Launch"),
            entry(subject, None, 1, "Move me"),
        ];
        let mut surface = MoveToSurface::new(
            &mut engine,
            vec![subject],
            "Move me".into(),
            &[ws_entry(ws, "Product")],
            ws,
            &entries,
        );
        surface.expanded.insert(ws_key(ws));
        surface.expanded.insert(doc_key(parent));
        let rows = surface.browse_rows();
        let launch = rows
            .iter()
            .find(|r| r.document == Some(target))
            .expect("launch row");
        let sib = surface.destination_for(launch, false).unwrap();
        assert_eq!(sib.parent, Some(parent));
        assert_eq!(sib.index, usize::MAX);
        let child = surface.destination_for(launch, true).unwrap();
        assert_eq!(child.parent, Some(target));
    }

    #[test]
    fn subject_row_is_hidden_from_browse() {
        let mut engine = TextEngine::new();
        let subject = DocumentId::new();
        let other = DocumentId::new();
        let ws = WorkspaceId::new();
        let entries = vec![
            entry(subject, None, 0, "Me"),
            entry(other, None, 1, "Other"),
        ];
        let mut surface = MoveToSurface::new(
            &mut engine,
            vec![subject],
            "Me".into(),
            &[ws_entry(ws, "W")],
            ws,
            &entries,
        );
        surface.expanded.insert(ws_key(ws));
        let rows = surface.browse_rows();
        assert!(rows.iter().all(|r| r.document != Some(subject)));
        assert!(rows.iter().any(|r| r.document == Some(other)));
    }

    #[test]
    fn expand_keeps_selection_on_the_workspace() {
        let mut engine = TextEngine::new();
        let subject = DocumentId::new();
        let child = DocumentId::new();
        let ws = WorkspaceId::new();
        let entries = vec![
            entry(subject, None, 0, "Moving"),
            entry(child, None, 1, "Stay"),
        ];
        let mut surface = MoveToSurface::new(
            &mut engine,
            vec![subject],
            "Moving".into(),
            &[ws_entry(ws, "Here")],
            ws,
            &entries,
        );
        assert_eq!(surface.list.selected, 0);
        let row = surface.visible_rows()[0].clone();
        let _ = surface.toggle_expand(&row);
        assert_eq!(surface.sel, SelId::Workspace(ws));
        assert_eq!(surface.list.selected, 0, "still on the workspace row");
        assert!(surface.is_expanded_ws(ws));
        assert!(
            surface
                .visible_rows()
                .iter()
                .any(|r| r.document == Some(child)),
            "children appear under the expanded workspace"
        );
    }
}
