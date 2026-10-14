// SPDX-License-Identifier: MPL-2.0

//! **What the surface asked for, done to the document.**
//!
//! The surface turns a person's gesture into a [`CanvasRequest`] and holds no
//! document; this is where each one becomes an `koru_core::canvas` writer —
//! one undo step apiece, a whole multi-select drag or delete as one — or opens
//! the dialog or the agent menu, or becomes a [`CanvasAsk`] for the one thing
//! only the container can do (start or stop a run, open a page, find out which
//! agents are ready).
//!
//! **Every pass ends in `set_scene`, a refused request included.** A drag the
//! model refuses — an arrow onto the object it started from, a second arrow
//! between the same two — has already been drawn where the person let go; the
//! scene handed back is what snaps it back, so a refusal needs no word of its
//! own (the surface's contract).

use std::sync::OnceLock;

use koru_canvas_ui::{CanvasRequest, ObjectKind as SurfaceKind};
use koru_core::canvas::{self, ObjectKind};
use koru_core::node::{BlobHash, CanvasPoint};
use koru_core::schema::{Result, StoreError, WorkspaceDoc};
use koru_core::{DocumentId, NodeId};

use super::CanvasView;

/// **A request that belongs to the workflow half** — adding a start, agent
/// or output node, running, stopping or answering one, choosing its agent.
/// Refused while this build hides agents (koru-features), whatever asked:
/// the surface does not offer them then, and this is the second door.
fn runs_an_agent(request: &CanvasRequest) -> bool {
    match request {
        CanvasRequest::AddObject { kind, .. } => kind.is_workflow(),
        CanvasRequest::Run { .. }
        | CanvasRequest::Stop { .. }
        | CanvasRequest::Respond { .. }
        | CanvasRequest::ChooseAgent { .. } => true,
        _ => false,
    }
}
use super::decor::{key_of, model_kind, node_of, text_of};
use super::dialog::{DialogMode, TextDialog};

/// The widest an image is placed at, whatever its natural size.
const IMAGE_MAX_W: u32 = 480;
/// Between an object and what is placed beside it.
const BESIDE: i32 = 24;

/// **What only the container can do**, asked by the canvas and drained by it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CanvasAsk {
    /// Run the chain that starts at this start node.
    RunChain(NodeId),
    /// Run this agent node on its own.
    RunAgent(NodeId),
    /// Stop the chain that started at this start node.
    StopChain(NodeId),
    /// Stop this agent node's run.
    StopNode(NodeId),
    /// Open the pending approval or question in the run's session.
    Respond(NodeId),
    /// Open the page an output node wrote, the way any document opens.
    OpenPage(DocumentId),
    /// The agent menu opened with no agent in it: find out which are ready.
    WantAgents,
}

/// `KORU_CANVAS_DEBUG`: one line per request applied — ids, kinds and
/// refusal tokens, never a card's text. A `bool` load when it is off.
pub fn debug_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| koru_env::switch(koru_env::name::KORU_CANVAS_DEBUG))
}

pub fn debug(args: std::fmt::Arguments<'_>) {
    if debug_enabled() {
        eprintln!("[koru:canvas] {args}");
    }
}

macro_rules! canvas_debug {
    ($($t:tt)*) => {
        $crate::canvas_view::perform::debug(format_args!($($t)*))
    };
}
pub(crate) use canvas_debug;

/// What a writer's answer was, as the debug line says it.
fn outcome<T>(result: &Result<T>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(StoreError::CanvasRejected { why, .. }) => why.as_str(),
        Err(_) => "error",
    }
}

impl CanvasView {
    /// **Perform everything the surface has asked for**, against `ws`, and
    /// hand the surface the document as it now is.
    ///
    /// `false` — and nothing done — when `ws` is not this canvas's document,
    /// or there was nothing to do. A request that arrived where the document
    /// could not be written (a double-click borrows it read-only) waits in
    /// `deferred` and goes first.
    pub fn perform(&mut self, ws: &mut WorkspaceDoc) -> bool {
        if ws.read(self.root).is_err() {
            return false;
        }
        let mut requests = std::mem::take(&mut self.deferred);
        requests.extend(self.surface.take_requests());
        if requests.is_empty() {
            return false;
        }
        // Whatever was open is closed: a gesture is its own step.
        ws.checkpoint();
        let mut select = None;
        let mut i = 0;
        while i < requests.len() {
            // A drag of several objects is one `Move` each and one step in all.
            if matches!(requests[i], CanvasRequest::Move { .. }) {
                let end = requests[i..]
                    .iter()
                    .position(|r| !matches!(r, CanvasRequest::Move { .. }))
                    .map_or(requests.len(), |n| i + n);
                self.apply_moves(ws, &requests[i..end]);
                i = end;
                continue;
            }
            if let Some(made) = self.apply(ws, requests[i].clone()) {
                select = Some(made);
            }
            i += 1;
        }
        self.refresh(ws, true);
        if let Some(node) = select {
            self.surface.select(Some(key_of(node)));
        }
        true
    }

    /// The part of [`perform`](Self::perform) that needs no write — what a
    /// double-click asks for, with the document borrowed read-only. Opening
    /// the dialog for a new card or for an edit is all a double-click can
    /// mean; anything else waits for a pass that can write.
    pub fn perform_read_only(&mut self) -> bool {
        let mut acted = false;
        for request in self.surface.take_requests() {
            match request {
                CanvasRequest::AddObject {
                    kind: SurfaceKind::Card,
                    x,
                    y,
                } => {
                    self.open_new_card(CanvasPoint::new(x, y));
                    acted = true;
                }
                CanvasRequest::EditText { key } => {
                    acted |= self.open_edit(node_of(key));
                }
                other => self.deferred.push(other),
            }
        }
        acted
    }

    fn apply_moves(&mut self, ws: &mut WorkspaceDoc, moves: &[CanvasRequest]) {
        let result = ws.edit_group(|ws| {
            for request in moves {
                if let CanvasRequest::Move { key, x, y } = *request {
                    let node = node_of(key);
                    let moved = canvas::move_object(ws, node, CanvasPoint::new(x, y));
                    canvas_debug!("move node={} → {}", node.as_uuid(), outcome(&moved));
                }
            }
            Ok(())
        });
        let _ = result;
    }

    /// One request. Answers the node it made, so it can be selected.
    fn apply(&mut self, ws: &mut WorkspaceDoc, request: CanvasRequest) -> Option<NodeId> {
        if !koru_features::agents() && runs_an_agent(&request) {
            canvas_debug!("refused {request:?}: agents are hidden in this build");
            return None;
        }
        let root = self.root;
        match request {
            CanvasRequest::AddObject { kind, x, y } => {
                let at = CanvasPoint::new(x, y);
                return match model_kind(kind) {
                    // A card nobody typed into is never made: the dialog makes
                    // it, on Save, with what was typed.
                    ObjectKind::Card => {
                        self.open_new_card(at);
                        None
                    }
                    // An image arrives through `InsertImage`, never here.
                    ObjectKind::Image => None,
                    kind => self.add_node(ws, kind, at),
                };
            }
            CanvasRequest::InsertImage { x, y } => {
                // The container's `take_wants_image` hands this to the host's
                // picker, and the file it comes back with lands here.
                self.wants_image = Some(CanvasPoint::new(x, y));
                canvas_debug!("insert-image asked at=({x},{y})");
            }
            CanvasRequest::SetWidth { key, w } => {
                let node = node_of(key);
                let set = canvas::set_width(ws, node, w);
                canvas_debug!("width node={} w={w} → {}", node.as_uuid(), outcome(&set));
            }
            CanvasRequest::Connect { from, to } => {
                let (from, to) = (node_of(from), node_of(to));
                // A duplicate or a self-arrow is refused by the model; the
                // scene handed back below is the whole answer.
                let drawn = canvas::add_arrow(ws, root, from, to);
                canvas_debug!(
                    "connect from={} to={} → {}",
                    from.as_uuid(),
                    to.as_uuid(),
                    outcome(&drawn)
                );
            }
            CanvasRequest::Delete { keys } => {
                let nodes: Vec<NodeId> = keys.into_iter().map(node_of).collect();
                // One step for the whole selection: the arrows that named them
                // dangle and are hidden, and one Ctrl+Z brings all of it back.
                let deleted = ws.edit_group(|ws| {
                    for &node in &nodes {
                        let gone = canvas::delete_object(ws, node);
                        canvas_debug!("delete node={} → {}", node.as_uuid(), outcome(&gone));
                    }
                    Ok(())
                });
                let _ = deleted;
            }
            CanvasRequest::DeleteArrow { key } => {
                let arrow = node_of(key);
                let gone = canvas::delete_arrow(ws, arrow);
                canvas_debug!(
                    "delete-arrow arrow={} → {}",
                    arrow.as_uuid(),
                    outcome(&gone)
                );
            }
            CanvasRequest::EditText { key } => {
                self.open_edit(node_of(key));
            }
            CanvasRequest::Run { key } => {
                let node = node_of(key);
                match self.kind_of(node) {
                    Some(ObjectKind::Start) => self.asks.push(CanvasAsk::RunChain(node)),
                    Some(ObjectKind::Agent) => self.asks.push(CanvasAsk::RunAgent(node)),
                    _ => {}
                }
                canvas_debug!("run node={}", node.as_uuid());
            }
            CanvasRequest::Stop { key } => {
                let node = node_of(key);
                match self.kind_of(node) {
                    Some(ObjectKind::Start) => self.asks.push(CanvasAsk::StopChain(node)),
                    Some(ObjectKind::Agent) => self.asks.push(CanvasAsk::StopNode(node)),
                    _ => {}
                }
                canvas_debug!("stop node={}", node.as_uuid());
            }
            CanvasRequest::ChooseAgent { key } => {
                self.open_agent_menu(node_of(key));
            }
            CanvasRequest::Respond { key } => {
                let node = node_of(key);
                if self.kind_of(node) == Some(ObjectKind::Agent) {
                    self.asks.push(CanvasAsk::Respond(node));
                }
            }
            CanvasRequest::PlaceResult { key } => {
                return self.place_result(ws, node_of(key));
            }
            CanvasRequest::OpenLink { key } => {
                let node = node_of(key);
                if let Some(page) = self.object(node).and_then(|object| object.output_page) {
                    self.asks.push(CanvasAsk::OpenPage(page));
                }
                canvas_debug!("open-link node={}", node.as_uuid());
            }
            CanvasRequest::BringToFront { key } => {
                let node = node_of(key);
                let moved = canvas::bring_to_front(ws, node);
                canvas_debug!("front node={} → {}", node.as_uuid(), outcome(&moved));
            }
            CanvasRequest::SendToBack { key } => {
                let node = node_of(key);
                let moved = canvas::send_to_back(ws, node);
                canvas_debug!("back node={} → {}", node.as_uuid(), outcome(&moved));
            }
            // Handled above, a run of them at a time.
            CanvasRequest::Move { .. } => {}
        }
        None
    }

    /// A start, agent or output node, made at once — the header says what it
    /// is. An agent node starts with the machine's default agent (one step
    /// with the node), and an agent or output node opens the dialog for what
    /// it needs next: its instruction, or its page's title.
    fn add_node(
        &mut self,
        ws: &mut WorkspaceDoc,
        kind: ObjectKind,
        at: CanvasPoint,
    ) -> Option<NodeId> {
        let root = self.root;
        let agent = self.decor.default_agent.clone();
        let made = ws.edit_group(|ws| {
            let node = canvas::add_object(ws, root, kind, at, "")?;
            if kind == ObjectKind::Agent
                && let Some(agent) = agent.as_deref()
            {
                canvas::set_agent(ws, node, agent, None)?;
            }
            Ok(node)
        });
        canvas_debug!(
            "add kind={kind:?} at=({},{}) → {}",
            at.x,
            at.y,
            outcome(&made)
        );
        let node = made.ok()?;
        if matches!(kind, ObjectKind::Agent | ObjectKind::Output) {
            let mode = DialogMode::Edit { node, kind };
            self.dialog = Some(TextDialog::open(&mut self.engine, mode, ""));
        }
        Some(node)
    }

    /// **Place on canvas**: an agent node's result, copied out as a card of
    /// its own beside the node — its right edge, the same height — in one step.
    fn place_result(&mut self, ws: &mut WorkspaceDoc, node: NodeId) -> Option<NodeId> {
        let object = self.object(node)?;
        if object.result.is_empty() {
            return None;
        }
        let at = object.at.unwrap_or(CanvasPoint::new(0, 0));
        let w = object
            .extent
            .map_or(canvas::DEFAULT_WIDTH, |extent| extent.w);
        let beside = CanvasPoint::new(at.x.saturating_add(w).saturating_add(BESIDE), at.y);
        let text = object.result.join("\n");
        let made = canvas::add_object(ws, self.root, ObjectKind::Card, beside, &text);
        canvas_debug!("place-result node={} → {}", node.as_uuid(), outcome(&made));
        made.ok()
    }

    /// Open the dialog for a card that does not exist yet.
    pub(super) fn open_new_card(&mut self, at: CanvasPoint) {
        canvas_debug!("dialog new-card at=({},{})", at.x, at.y);
        let mode = DialogMode::NewCard { at };
        self.dialog = Some(TextDialog::open(&mut self.engine, mode, ""));
    }

    /// Open the dialog over an object's text. `false` for one that has none —
    /// an image — or is not here.
    pub(super) fn open_edit(&mut self, node: NodeId) -> bool {
        let Some(object) = self.object(node) else {
            return false;
        };
        if object.kind == ObjectKind::Image {
            return false;
        }
        let (kind, text) = (object.kind, text_of(object));
        canvas_debug!("dialog edit node={} kind={kind:?}", node.as_uuid());
        let mode = DialogMode::Edit { node, kind };
        self.dialog = Some(TextDialog::open(&mut self.engine, mode, &text));
        true
    }

    /// **Save**: one undo step — a new card with what was typed, or the
    /// node's text replaced. A new card with nothing typed is never made.
    pub(super) fn save_dialog(&mut self, ws: &mut WorkspaceDoc) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        if ws.read(self.root).is_err() {
            return;
        }
        let value = dialog.value();
        // A trailing Enter is not a line anybody meant to keep.
        let text = value.trim_end();
        ws.checkpoint();
        let mut select = None;
        match dialog.mode {
            DialogMode::NewCard { at } => {
                if !text.trim().is_empty() {
                    let made = canvas::add_object(ws, self.root, ObjectKind::Card, at, text);
                    canvas_debug!("add kind=Card at=({},{}) → {}", at.x, at.y, outcome(&made));
                    select = made.ok();
                } else {
                    canvas_debug!("dialog new-card saved empty → nothing made");
                }
            }
            DialogMode::Edit { node, .. } => {
                let set = canvas::set_text(ws, node, text);
                canvas_debug!("text node={} → {}", node.as_uuid(), outcome(&set));
                select = Some(node);
            }
        }
        ws.checkpoint();
        self.refresh(ws, true);
        if let Some(node) = select {
            self.surface.select(Some(key_of(node)));
        }
    }

    /// Put `agent` on the agent menu's node, as one step.
    pub(super) fn choose_agent(&mut self, ws: &mut WorkspaceDoc, node: NodeId, agent: &str) {
        if ws.read(self.root).is_err() {
            return;
        }
        ws.checkpoint();
        let set = canvas::set_agent(ws, node, agent, None);
        canvas_debug!("agent node={} → {}", node.as_uuid(), outcome(&set));
        self.refresh(ws, true);
    }

    /// Put images on the canvas — pasted, dropped or picked — at `at`, one
    /// beside the next, in one step. Each is at most [`IMAGE_MAX_W`] wide.
    pub(super) fn place_images(
        &mut self,
        ws: &mut WorkspaceDoc,
        at: CanvasPoint,
        images: &[(String, String, Option<u32>, Option<u32>)],
    ) -> bool {
        if ws.read(self.root).is_err() || images.is_empty() {
            return false;
        }
        ws.checkpoint();
        let root = self.root;
        let placed = ws.edit_group(|ws| {
            let mut x = at.x;
            let mut last = None;
            for (hash, mime, width, height) in images {
                let w = width.map_or(IMAGE_MAX_W, |natural| natural.clamp(1, IMAGE_MAX_W)) as i32;
                let node = canvas::add_image(
                    ws,
                    root,
                    CanvasPoint::new(x, at.y),
                    BlobHash(hash.clone()),
                    mime,
                    *width,
                    *height,
                    w,
                )?;
                x = x.saturating_add(w).saturating_add(BESIDE);
                last = Some(node);
            }
            Ok(last)
        });
        canvas_debug!(
            "images n={} at=({},{}) → {}",
            images.len(),
            at.x,
            at.y,
            outcome(&placed)
        );
        ws.checkpoint();
        self.refresh(ws, true);
        if let Ok(Some(node)) = placed {
            self.surface.select(Some(key_of(node)));
        }
        true
    }

    /// Undo or redo on the canvas's document, and show what it did.
    pub(super) fn undo(&mut self, ws: &mut WorkspaceDoc, redo: bool) -> bool {
        if ws.read(self.root).is_err() {
            return false;
        }
        ws.checkpoint();
        let stepped = if redo { ws.redo() } else { ws.undo() }.unwrap_or(false);
        canvas_debug!("{} → {stepped}", if redo { "redo" } else { "undo" });
        // **The first step of a new canvas is the canvas being made.** The
        // store opens a new document with undo already on and the seed — the
        // root, whose text is the canvas's name — is written after, so a
        // Ctrl+Z past the first card would unmake the canvas under the tab.
        // That step is not the canvas's to take back: it is put straight back.
        if stepped && !redo && ws.read(self.root).is_err() {
            let _ = ws.redo();
            canvas_debug!("undo would unmake the canvas → put back");
            self.refresh(ws, true);
            return false;
        }
        self.refresh(ws, true);
        stepped
    }

    /// The kind of an object on this canvas, as last read.
    fn kind_of(&self, node: NodeId) -> Option<ObjectKind> {
        self.object(node).map(|object| object.kind)
    }
}
