// SPDX-License-Identifier: MPL-2.0

//! **A canvas, as a tab** — the canvas surface (`koru-canvas-ui`) over a
//! canvas document.
//!
//! A canvas is an ordinary node-tree document under the manifest kind
//! [`KIND_CANVAS`](koru_core::explore::KIND_CANVAS), rooted at a
//! `Kind::Document` node whose text is its title (CANVAS P1.3). Everything a
//! workspace does with a document it already does with one — sync, search,
//! the bin, a move to another workspace — so what is new is only how it is
//! *looked at*, and that is this tab.
//!
//! ## What the tab holds, and what it hands on
//!
//! It owns a [`CanvasSurface`] — the camera, a gesture in flight, the drawing
//! — and the last [`CanvasSnapshot`] it read, which it hands the surface as a
//! plain `Scene` ([`decor`]). The surface answers input with requests;
//! [`perform`](CanvasView::perform) turns each into one `koru_core::canvas`
//! writer on the document it was handed, opens the text dialog (Q-4: a card's
//! text is written in a dialog tonight) or the agent menu, or leaves a
//! [`CanvasAsk`] for what only the container can do — start or stop a run,
//! open a page. What the document does not say and the container does — an
//! agent's name, a node's status, a page's title — arrives as a [`Decor`].
//!
//! **Nothing is read on a timer and nothing is laid out per frame.** The
//! document is re-read when its version has moved since the last read — asked
//! on `catch_up`, on a draw, and after this tab's own writes — and the scene is
//! rebuilt from the snapshot in hand when the decor changes. A canvas at rest
//! costs a version comparison per drawn frame and nothing between frames.
//!
//! ## The container's hooks
//!
//! | Hook | Container call site | What it does here |
//! | --- | --- | --- |
//! | [`root`](CanvasView::root) / [`document`](CanvasView::document) | `TabView::root`, `Tab::document` | the canvas's root and id |
//! | [`start_rename`](CanvasView::start_rename), [`is_renaming`](CanvasView::is_renaming) | F2, the tab's *Rename…*, Explore's *Rename…* | the shared name prompt, writing the root's text |
//! | [`zoom_wheel`](View::zoom_wheel), [`zoom_key`](View::zoom_key) | Ctrl+wheel, a pinch, Ctrl+= / Ctrl+- / Ctrl+0 | claimed: the surface's camera zooms |
//! | [`wheel`](CanvasView::wheel) | the plain wheel, with a pane-local pointer | the surface pans |
//! | [`set_mods`](View::set_mods) | every modifier change | Shift-click, Shift+wheel |
//! | [`is_dragging`](CanvasView::is_dragging) | pointer capture past the pane's edge | a pan, a drag, a resize, an arrow, a text drag |
//! | [`key_up`](View::key_up) | a key release (`koru-workspace-host`'s `key_release.rs`) | ends an arrow-key nudge: one move |
//! | [`focus_lost`](CanvasView::focus_lost) | the window losing focus | drops a gesture in flight, writes nothing |
//! | [`animating`](View::animating) | the shell's fast wake | a running node's mark turns |
//! | [`perform`](CanvasView::perform) / [`take_asks`](CanvasView::take_asks) | `drain_trash_all`, `maintain` | requests → the document; asks → the container |
//! | [`set_decor`](CanvasView::set_decor) | `maintain` | agent names, node statuses, page titles |
//! | [`reveal`](CanvasView::reveal) | a search hit, `koru://doc/{canvas}#{node}` | the surface centres on the node and selects it |
//! | `semantics`, `revise`, `accessible_focus`, `accessibility_action` | `mainview/accessibility.rs` | the Tasks page's four hooks |
//!
//! **Renaming is the view's, like a page's.** The container's F2, tab-menu and
//! Explore renames all end in `TabView::start_rename`; this view answers with
//! the same [`NamePrompt`] in the centred frame the container's *New document*
//! prompt uses, writing the root's text — so a canvas's name reaches the tab,
//! Explore, recents and search exactly as a page's does.
//!
//! **The camera lives here, per tab, in memory.** It is not written to the
//! device's settings: the maintainers chose that a canvas reopened after a restart
//! opens at 100 % over its content, and dropped P1.5's `DocViewPrefs` field.

use koru_canvas_ui::CanvasSurface;
use koru_core::canvas::{self, CanvasObject, CanvasSnapshot};
use koru_core::node::CanvasPoint;
use koru_core::schema::WorkspaceDoc;
use koru_core::{DocumentId, NodeId};
use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{ImageCache, scene, theme};
use koru_editor::components::Outcome;
use koru_editor::components::draw::draw_name_prompt_contents;
use koru_editor::components::name_prompt::{NamePrompt, field_rect};
use koru_editor::components::text_field::TextField;
use koru_editor::trigger::{Anchor, Trigger, TriggerKind};
use koru_ui::{KoruDialogFrame, KoruDialogFrameInspection, KoruDialogFrameState};
use koru_workspace_host::{Settings, View, ZoomIntent};
use vello::Scene;
use vello::kurbo::Affine;
use winit::event::MouseScrollDelta;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

pub(crate) mod accessibility;
mod agents;
pub(crate) mod decor;
mod dialog;
pub(crate) mod perform;

#[cfg(test)]
mod tests;

pub use decor::Decor;
pub(crate) use perform::CanvasAsk;

use agents::AgentMenu;
use decor::{key_of, scene_of};
use dialog::TextDialog;
use perform::canvas_debug;

/// A wheel as the view last saw it: the delta, flattened to `(x, y)` — lines
/// or pixels, whichever arrived — and the pane-local pointer it came at.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct WheelSeen {
    pub delta: (f32, f32),
    pub at: (f32, f32),
}

impl WheelSeen {
    fn of(delta: MouseScrollDelta, x: f32, y: f32) -> Self {
        let delta = match delta {
            MouseScrollDelta::LineDelta(dx, dy) => (dx, dy),
            MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
        };
        Self { delta, at: (x, y) }
    }
}

/// The last of each gesture the container reached this view with.
///
/// **Held so it can be asked.** The first question about a seam is whether it
/// is being reached at all, and where; these are published in the UI inspector
/// (F12) and read by the container's tests. The surface acts on the gesture;
/// this only remembers it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gestures {
    /// The last plain wheel, with its pane-local pointer.
    pub wheel: Option<WheelSeen>,
    /// The last primary+wheel or pinch the canvas claimed from the display zoom.
    pub zoom_wheel: Option<WheelSeen>,
    /// The last zoom chord the canvas claimed.
    pub zoom_key: Option<ZoomIntent>,
    /// How many zoom gestures, wheel and chord, it has claimed.
    pub zooms_claimed: u32,
}

pub struct CanvasView {
    /// The canvas, as the manifest names it.
    document: DocumentId,
    /// Its `Kind::Document` root — the node whose text is the canvas's name,
    /// and what `TabView::root` answers.
    root: NodeId,
    surface: CanvasSurface,
    viewport: (f32, f32),
    engine: TextEngine,
    /// The name prompt, while it is open.
    rename: Option<NamePrompt>,
    /// A press in the prompt's field that is still selecting text.
    selecting: bool,
    /// The text dialog, while it is open.
    dialog: Option<TextDialog>,
    /// The agent menu, while it is open.
    agents: Option<AgentMenu>,
    /// Where the pointer last was, in this view's space.
    pointer: (f32, f32),
    mods: ModifiersState,
    gestures: Gestures,
    /// The document as last read, and the version it was read at.
    snapshot: Option<CanvasSnapshot>,
    read_at: Option<loro::VersionVector>,
    decor: Decor,
    /// A node to land on once the scene is in (a search hit, a link).
    landing: Option<NodeId>,
    /// Whether the camera has been put over the content yet.
    placed: bool,
    /// Whether the container should dress this canvas again: it was just
    /// built, or read again, or the listing moved under it.
    wants_decor: bool,
    /// Requests that arrived where the document could only be read.
    deferred: Vec<koru_canvas_ui::CanvasRequest>,
    /// What only the container can do.
    asks: Vec<CanvasAsk>,
    /// **Insert image…** was chosen here: the host's picker is wanted.
    wants_image: Option<CanvasPoint>,
    /// …and the picker is open, for this point.
    image_at: Option<CanvasPoint>,
    /// Ctrl+V: the host's clipboard image read is wanted.
    wants_paste: bool,
}

impl CanvasView {
    /// A canvas tab whose document is read the first time it is caught up or
    /// drawn — the path for a tab converted in place, which has no document in
    /// hand.
    pub fn new(document: DocumentId, root: NodeId) -> Self {
        let mut view = Self {
            document,
            root,
            surface: CanvasSurface::new(crate::format::lang()),
            viewport: (0.0, 0.0),
            engine: TextEngine::new(),
            rename: None,
            selecting: false,
            dialog: None,
            agents: None,
            pointer: (-1.0, -1.0),
            mods: ModifiersState::empty(),
            gestures: Gestures::default(),
            snapshot: None,
            read_at: None,
            decor: Decor::default(),
            landing: None,
            placed: false,
            wants_decor: true,
            deferred: Vec::new(),
            asks: Vec::new(),
            wants_image: None,
            image_at: None,
            wants_paste: false,
        };
        // **The workflow half is the agents' surface** (ENABLE_AGENTS,
        // koru-features): with agents
        // hidden a canvas is cards and images, and its start, agent and
        // output nodes draw but offer nothing that runs. `perform` refuses
        // the same requests again, as `run_menu_action` does for menus.
        view.surface.set_workflows(koru_features::agents());
        view
    }

    /// A canvas tab over `ws`, read now — what every door that opens a canvas
    /// builds, so a landing on one of its objects has something to land on.
    pub fn over(document: DocumentId, root: NodeId, ws: &WorkspaceDoc) -> Self {
        let mut view = Self::new(document, root);
        view.refresh(ws, false);
        view
    }

    /// The canvas's manifest id.
    pub fn document(&self) -> DocumentId {
        self.document
    }

    /// The canvas's root node.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// What reached this view. See [`Gestures`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn gestures(&self) -> &Gestures {
        &self.gestures
    }

    /// The modifiers as they were last pushed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn mods(&self) -> ModifiersState {
        self.mods
    }

    /// The surface, for what the container and the tests ask of it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn surface(&self) -> &CanvasSurface {
        &self.surface
    }

    /// The canvas as last read.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn snapshot(&self) -> Option<&CanvasSnapshot> {
        self.snapshot.as_ref()
    }

    /// One object or arrival, as last read.
    pub(crate) fn object(&self, node: NodeId) -> Option<&CanvasObject> {
        self.snapshot.as_ref()?.get(node)
    }

    /// The pages this canvas's output nodes write — what the container looks
    /// up titles for.
    pub fn output_pages(&self) -> Vec<DocumentId> {
        self.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
            snapshot
                .objects
                .iter()
                .chain(&snapshot.arrivals)
                .filter_map(|object| object.output_page)
                .collect()
        })
    }

    /// Whether a drag this view owns is in flight — the container keeps
    /// forwarding pointer moves past the pane's edge while it is.
    pub fn is_dragging(&self) -> bool {
        self.selecting
            || self.dialog.as_ref().is_some_and(TextDialog::is_dragging)
            || self.surface.is_dragging()
    }

    /// Whether something over the canvas owns the keyboard: the name prompt,
    /// the text dialog or the agent menu.
    pub fn is_prompting(&self) -> bool {
        self.rename.is_some() || self.dialog.is_some() || self.agents.is_some()
    }

    /// Whether the text dialog is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_editing_text(&self) -> bool {
        self.dialog.is_some()
    }

    /// Whether the agent menu is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_choosing_agent(&self) -> bool {
        self.agents.is_some()
    }

    // --------------------------------------------------------------- reading

    /// **Read the document again if it moved**, and hand the surface what it
    /// says. `force` hands it over even when nothing moved — what a pass that
    /// performed requests does, so a refused request snaps back.
    ///
    /// `false` when `ws` does not hold this canvas.
    pub fn refresh(&mut self, ws: &WorkspaceDoc, force: bool) -> bool {
        if ws.read(self.root).is_err() {
            return false;
        }
        let version = ws.version();
        let moved = self.read_at.as_ref() != Some(&version);
        if moved {
            match canvas::read(ws, self.root) {
                Ok(snapshot) => self.snapshot = Some(snapshot),
                Err(_) => return false,
            }
            self.read_at = Some(version);
            // New nodes, a new agent on one, a new output page: dress again.
            self.wants_decor = true;
            // An open menu or dialog over something that has gone goes too.
            if let Some(menu) = &self.agents
                && self.object(menu.node).is_none()
            {
                self.agents = None;
            }
            if let Some(dialog::DialogMode::Edit { node, .. }) =
                self.dialog.as_ref().map(|dialog| dialog.mode)
                && self.object(node).is_none()
            {
                self.dialog = None;
            }
        }
        if moved || force {
            self.show();
        }
        moved
    }

    /// Hand the surface the snapshot in hand, dressed in the decor.
    fn show(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let lang = crate::format::lang();
        self.surface.set_lang(lang);
        self.surface
            .set_scene(scene_of(snapshot, &self.decor, lang));
        self.place_camera();
    }

    /// The first time there is something to look at: land on the node asked
    /// for, or look at the content at 100 %.
    fn place_camera(&mut self) {
        if let Some(node) = self.landing.take() {
            self.surface.focus_object(key_of(node));
            self.placed = true;
            return;
        }
        if self.placed {
            return;
        }
        self.placed = true;
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let mut bounds: Option<(i32, i32, i32, i32)> = None;
        for object in &snapshot.objects {
            let Some(at) = object.at else {
                continue;
            };
            let w = object
                .extent
                .map_or(canvas::DEFAULT_WIDTH, |extent| extent.w);
            let (x1, y1) = (at.x.saturating_add(w), at.y.saturating_add(80));
            bounds = Some(match bounds {
                None => (at.x, at.y, x1, y1),
                Some((a, b, c, d)) => (a.min(at.x), b.min(at.y), c.max(x1), d.max(y1)),
            });
        }
        if let Some((x0, y0, x1, y1)) = bounds {
            self.surface.set_camera(koru_canvas_ui::Camera::new(
                (f64::from(x0) + f64::from(x1)) * 0.5,
                (f64::from(y0) + f64::from(y1)) * 0.5,
                1.0,
            ));
        }
    }

    /// **What the container knows** — agent names, node statuses, page titles.
    /// Answers whether anything changed, and only then is the scene rebuilt.
    pub fn set_decor(&mut self, decor: Decor) -> bool {
        self.wants_decor = false;
        if decor == self.decor {
            return false;
        }
        let agents_moved = decor.agents != self.decor.agents;
        self.decor = decor;
        // A menu built from the old list shows the new one, in place.
        if agents_moved && let Some(menu) = self.agents.take() {
            let current = self.object(menu.node).and_then(|o| o.agent.clone());
            self.agents = Some(AgentMenu::open(
                &mut self.engine,
                crate::format::lang(),
                menu.anchor(),
                menu.node,
                &self.decor.agents,
                current.as_deref(),
            ));
        }
        self.show();
        true
    }

    /// Whether the container should dress this canvas again.
    pub fn wants_decor(&self) -> bool {
        self.wants_decor
    }

    /// Ask to be dressed again — the listing, and so a page's title, moved.
    pub fn request_decor(&mut self) {
        self.wants_decor = true;
    }

    /// The text every agent node on this canvas stores for its agent.
    pub fn agent_texts(&self) -> Vec<String> {
        self.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
            snapshot
                .objects
                .iter()
                .chain(&snapshot.arrivals)
                .filter_map(|object| object.agent.clone())
                .collect()
        })
    }

    /// The decor this view was last handed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn decor(&self) -> &Decor {
        &self.decor
    }

    // ---------------------------------------------------------------- asking

    /// Drain what only the container can do.
    pub fn take_asks(&mut self) -> Vec<CanvasAsk> {
        std::mem::take(&mut self.asks)
    }

    /// Whether a request is waiting for a pass that can write.
    pub fn has_deferred(&self) -> bool {
        !self.deferred.is_empty()
    }

    /// **Insert image…** asked for the picker: where the picked file goes.
    /// Taken once; the point is kept for the answer.
    pub fn take_wants_image(&mut self) -> bool {
        let Some(at) = self.wants_image.take() else {
            return false;
        };
        self.image_at = Some(at);
        true
    }

    /// Ctrl+V asked for the clipboard's images.
    pub fn take_wants_paste(&mut self) -> bool {
        std::mem::take(&mut self.wants_paste)
    }

    /// The picked image, where **Insert image…** was chosen.
    pub fn insert_picked_image(
        &mut self,
        ws: &mut WorkspaceDoc,
        hash: &str,
        mime: &str,
        width: Option<u32>,
        height: Option<u32>,
    ) -> bool {
        let at = self.image_at.take().unwrap_or_else(|| self.drop_point());
        self.place_images(ws, at, &[(hash.to_owned(), mime.to_owned(), width, height)])
    }

    /// Images dropped on the window or pasted: where the pointer is, when it
    /// is over the canvas, and the middle of the view otherwise.
    pub fn insert_images(
        &mut self,
        ws: &mut WorkspaceDoc,
        images: &[(String, String, Option<u32>, Option<u32>)],
    ) -> bool {
        let at = self.drop_point();
        self.place_images(ws, at, images)
    }

    fn drop_point(&self) -> CanvasPoint {
        let (vw, vh) = self.viewport;
        let (px, py) = self.pointer;
        let over = px >= 0.0 && py >= 0.0 && px < vw && py < vh;
        let view = if over {
            (f64::from(px), f64::from(py))
        } else {
            (f64::from(vw) * 0.5, f64::from(vh) * 0.5)
        };
        let (x, y) = self.surface.camera().to_canvas(self.viewport, view);
        CanvasPoint::new(x.round() as i32, y.round() as i32)
    }

    // --------------------------------------------------------------- landing

    /// **Land on a node**: centre the view on it and select it — a search hit,
    /// a Tasks "go to", `koru://doc/{canvas}#{node}`. A node not read yet is
    /// landed on when the scene comes in. `false` for a node this canvas does
    /// not hold.
    pub fn reveal(&mut self, node: NodeId) -> bool {
        if node == self.root {
            return true;
        }
        match &self.snapshot {
            Some(snapshot) if snapshot.get(node).is_some() => {
                self.surface.focus_object(key_of(node));
                self.placed = true;
                true
            }
            Some(_) => false,
            None => {
                self.landing = Some(node);
                true
            }
        }
    }

    /// **The window lost the keyboard**: a gesture in flight is dropped and
    /// nothing is written for it.
    pub fn focus_lost(&mut self) {
        self.surface.focus_lost();
        self.selecting = false;
    }

    // ----------------------------------------------------------------- wheel

    /// **The plain wheel, where the pointer is** — `(x, y)` in this view's own
    /// space, the pane's origin already subtracted. `View::scroll_by` carries no
    /// position, and a canvas pans under the pointer, so the container calls
    /// this instead.
    pub fn wheel(&mut self, delta: MouseScrollDelta, x: f32, y: f32) {
        self.gestures.wheel = Some(WheelSeen::of(delta, x, y));
        if let Some(dialog) = &mut self.dialog {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_, dy) => -dy * 24.0,
                MouseScrollDelta::PixelDelta(p) => -p.y as f32,
            };
            dialog.scroll(dy);
            return;
        }
        if self.rename.is_some() || self.agents.is_some() {
            return;
        }
        self.surface.wheel(delta, x, y);
    }

    // ---------------------------------------------------------------- rename

    /// Put **this canvas's own name** into the name prompt — F2, the tab's
    /// *Rename…* and Explore's *Rename…* all arrive here, as they arrive at a
    /// page's.
    ///
    /// `false` when `ws` does not hold the root: the canvas is not the document
    /// the container is talking to, and a prompt that wrote into another one
    /// would rename the wrong thing.
    pub fn start_rename(&mut self, ws: &WorkspaceDoc) -> bool {
        let Ok(node) = ws.read(self.root) else {
            return false;
        };
        let current = node.content.as_text().unwrap_or_default().to_owned();
        let mut input = TextField::with_text(&mut self.engine, theme::role::body(), &current);
        // Selected, not just focused: renaming usually means replacing.
        input.select_all(&mut self.engine);
        self.rename = Some(NamePrompt {
            // Anchored to the root for what it names, but as an insert rather
            // than a document rename: that anchor is what draws the page-icon
            // button, and a canvas has no icon to set from here.
            trigger: Trigger {
                kind: TriggerKind::Slash,
                start: 0,
                node: self.root,
                anchor: Anchor::Insert,
                stable: None,
            },
            input,
            page_icon: None,
        });
        self.selecting = false;
        self.dialog = None;
        self.agents = None;
        true
    }

    /// Whether the name prompt is open — which is also whether Enter would
    /// commit a name, the question the container asks before routing it.
    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    fn frame(&self) -> KoruDialogFrame {
        KoruDialogFrame::name_prompt(self.viewport)
    }

    /// The prompt's field, in this view's space.
    fn field(&self) -> Rect {
        let panel = self.frame().panel();
        let field = field_rect(panel.w);
        Rect::new(panel.x + field.x, panel.y + field.y, field.w, field.h)
    }

    /// Write the committed name into the canvas's root, the way a page's
    /// rename writes its title. The manifest follows it on the next tick.
    fn commit(&mut self, ws: &mut WorkspaceDoc, name: &str) -> bool {
        self.rename = None;
        self.selecting = false;
        ws.read(self.root).is_ok() && ws.set_text(self.root, name).is_ok()
    }

    // ------------------------------------------------------------ the menus

    /// Open the agent menu over an agent node. With no agent ready it says so
    /// in one dimmed row, and the container is asked to find out.
    fn open_agent_menu(&mut self, node: NodeId) {
        let current = self.object(node).and_then(|object| object.agent.clone());
        let anchor = self.menu_anchor(node);
        let menu = AgentMenu::open(
            &mut self.engine,
            crate::format::lang(),
            anchor,
            node,
            &self.decor.agents,
            current.as_deref(),
        );
        if menu.is_empty() {
            self.asks.push(CanvasAsk::WantAgents);
        }
        canvas_debug!(
            "agent-menu node={} agents={}",
            node.as_uuid(),
            self.decor.agents.len()
        );
        self.agents = Some(menu);
    }

    /// Where a menu over an object hangs: from the pointer when it pressed
    /// the object's header, and from under the header otherwise (Shift+F10,
    /// the menu's own *Choose agent…*).
    fn menu_anchor(&self, node: NodeId) -> (f32, f32) {
        let key = key_of(node);
        let bounds = self
            .surface
            .semantics()
            .into_iter()
            .find(|object| object.key == key)
            .map(|object| object.bounds);
        match bounds {
            Some(b) if b.contains(self.pointer.0, self.pointer.1) => self.pointer,
            Some(b) => (b.x + 8.0, b.y + 40.0 * self.surface.camera().zoom as f32),
            None => (self.viewport.0 * 0.5, self.viewport.1 * 0.5),
        }
    }

    /// What an agent menu's answer does.
    fn agent_outcome(&mut self, ws: &mut WorkspaceDoc, outcome: agents::Outcome) {
        match outcome {
            agents::Outcome::Keep => {}
            agents::Outcome::Close => self.agents = None,
            agents::Outcome::Choose(agent) => {
                if let Some(menu) = self.agents.take() {
                    self.choose_agent(ws, menu.node, &agent);
                }
            }
        }
    }

    /// What a dialog's answer does.
    fn dialog_outcome(&mut self, ws: &mut WorkspaceDoc, outcome: dialog::Outcome) {
        match outcome {
            dialog::Outcome::Keep => {}
            dialog::Outcome::Cancel => {
                canvas_debug!("dialog cancelled → nothing written");
                self.dialog = None;
            }
            dialog::Outcome::Save => self.save_dialog(ws),
        }
    }

    /// Ctrl+Z, Ctrl+Y and Ctrl+Shift+Z — the editor's own table decides, so
    /// a canvas undoes on the chords a page does.
    fn history_key(key: &Key, text: Option<&str>, mods: ModifiersState) -> Option<bool> {
        use koru_editor::input::action::Action;
        match koru_editor::input::keymap::resolve(key, text, mods)? {
            Action::Undo => Some(false),
            Action::Redo => Some(true),
            _ => None,
        }
    }
}

impl View for CanvasView {
    fn key(
        &mut self,
        ws: &mut WorkspaceDoc,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> bool {
        if let Some(prompt) = self.rename.as_mut() {
            if matches!(key, Key::Named(NamedKey::Escape)) {
                self.rename = None;
                self.selecting = false;
                return true;
            }
            if let Outcome::Commit(spec) = prompt.key(&mut self.engine, key, text, mods) {
                self.commit(ws, &spec.label);
            }
            // The prompt owns the keyboard while it stands, whatever the key was.
            return true;
        }
        if let Some(dialog) = self.dialog.as_mut() {
            let outcome = dialog.key(&mut self.engine, key, text, mods);
            self.dialog_outcome(ws, outcome);
            return true;
        }
        if let Some(menu) = self.agents.as_mut() {
            let outcome = menu.key(key);
            self.agent_outcome(ws, outcome);
            return true;
        }
        if let Some(redo) = Self::history_key(key, text, mods) {
            self.undo(ws, redo);
            return true;
        }
        if koru_keys::is_chord(mods) && koru_keys::letter(key, text) == Some('v') {
            // The host reads the clipboard and hands back what it found;
            // pictures land through `insert_images`.
            self.wants_paste = true;
            return true;
        }
        let handled = self.surface.key(key, mods);
        let performed = self.perform(ws);
        handled || performed
    }

    fn key_up(&mut self, ws: &mut WorkspaceDoc, key: &Key) -> bool {
        let handled = self.surface.key_up(key);
        let performed = self.perform(ws);
        handled || performed
    }

    fn mouse_down(&mut self, ws: &mut WorkspaceDoc, x: f32, y: f32, _ctrl: bool) -> bool {
        self.pointer = (x, y);
        if let Some(prompt) = self.rename.as_mut() {
            let panel = KoruDialogFrame::name_prompt(self.viewport).panel();
            if panel.contains(x, y) {
                self.selecting = prompt.click(&mut self.engine, x - panel.x, y - panel.y, panel.w);
            } else {
                // A press on the scrim puts the prompt away, the way Escape
                // does: the name is not changed by a click somewhere else.
                self.rename = None;
                self.selecting = false;
            }
            return true;
        }
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.mouse_down(&mut self.engine, self.viewport, x, y);
            return true;
        }
        if let Some(menu) = &self.agents {
            let outcome = menu.mouse_down(self.viewport, x, y);
            self.agent_outcome(ws, outcome);
            return true;
        }
        let handled = self.surface.mouse_down(x, y);
        let performed = self.perform(ws);
        handled || performed
    }

    fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        if self.rename.is_some() {
            if !self.selecting {
                return false;
            }
            let panel = KoruDialogFrame::name_prompt(self.viewport).panel();
            if let Some(prompt) = self.rename.as_mut() {
                prompt.drag(&mut self.engine, x - panel.x, panel.w);
            }
            return true;
        }
        if let Some(dialog) = self.dialog.as_mut() {
            return dialog.mouse_move(&mut self.engine, self.viewport, x, y);
        }
        if let Some(menu) = self.agents.as_mut() {
            return menu.mouse_move(self.viewport, x, y);
        }
        self.surface.mouse_move(x, y)
    }

    fn mouse_up(&mut self, ws: &mut WorkspaceDoc) -> bool {
        if self.rename.is_some() {
            return std::mem::take(&mut self.selecting);
        }
        if let Some(dialog) = self.dialog.as_mut() {
            let outcome = dialog.mouse_up(&mut self.engine, self.viewport, self.pointer);
            self.dialog_outcome(ws, outcome);
            return true;
        }
        if self.agents.is_some() {
            return false;
        }
        let handled = self.surface.mouse_up();
        let performed = self.perform(ws);
        handled || performed
    }

    fn mouse_double_click(&mut self, _ws: &WorkspaceDoc, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        if let Some(prompt) = self.rename.as_mut() {
            let panel = KoruDialogFrame::name_prompt(self.viewport).panel();
            prompt.double_click(&mut self.engine, x - panel.x, y - panel.y, panel.w);
            return true;
        }
        if self.dialog.is_some() || self.agents.is_some() {
            return true;
        }
        // Read-only here (the trait borrows the document so), and a
        // double-click only ever opens the dialog: for a new card on empty
        // space, for the text of what it landed on.
        let handled = self.surface.double_click(x, y);
        let acted = self.perform_read_only();
        handled || acted
    }

    fn context_menu(&mut self, _ws: &mut WorkspaceDoc, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        if self.is_prompting() {
            return true;
        }
        self.surface.context_menu(x, y)
    }

    fn set_mods(&mut self, mods: ModifiersState) {
        self.mods = mods;
        self.surface.set_mods(mods);
    }

    /// The position-less wheel. The container reaches [`CanvasView::wheel`]
    /// instead, which knows where the pointer is; this is kept for a host that
    /// has nothing better, and pans from the view's centre.
    fn scroll_by(&mut self, delta: MouseScrollDelta) {
        let (w, h) = self.viewport;
        self.wheel(delta, w * 0.5, h * 0.5);
    }

    fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport = (w, h);
        self.surface.set_viewport(w, h);
    }

    /// **Claimed**: a canvas zooms its own camera, never the app's display
    /// zoom — even when the camera is already at its limit, or a dialog stands
    /// over it, the gesture is the canvas's.
    fn zoom_wheel(&mut self, delta: MouseScrollDelta, x: f32, y: f32) -> bool {
        self.gestures.zoom_wheel = Some(WheelSeen::of(delta, x, y));
        self.gestures.zooms_claimed += 1;
        if !self.is_prompting() {
            self.surface.zoom_wheel(delta, x, y);
        }
        true
    }

    fn zoom_key(&mut self, intent: ZoomIntent) -> bool {
        self.gestures.zoom_key = Some(intent);
        self.gestures.zooms_claimed += 1;
        if !self.is_prompting() {
            self.surface.zoom_key(match intent {
                ZoomIntent::In => 1,
                ZoomIntent::Out => -1,
                ZoomIntent::ActualSize => 0,
            });
        }
        true
    }

    fn cursor_icon(&self) -> CursorIcon {
        let (x, y) = self.pointer;
        if self.rename.is_some() {
            if self.field().contains(x, y) {
                return CursorIcon::Text;
            }
            return koru_ui::cursor::IDLE;
        }
        if let Some(dialog) = &self.dialog {
            if dialog.over_field(x, y) {
                return CursorIcon::Text;
            }
            if dialog.over_button(x, y) {
                return koru_ui::cursor::PRESS;
            }
            return koru_ui::cursor::IDLE;
        }
        if let Some(menu) = &self.agents {
            return if menu.over_row(self.viewport, x, y) {
                koru_ui::cursor::PRESS
            } else {
                koru_ui::cursor::IDLE
            };
        }
        self.surface.cursor_icon()
    }

    fn animating(&self) -> bool {
        self.surface.animating()
    }

    /// Another window or a peer changed the document. The version comparison
    /// in [`refresh`](CanvasView::refresh) is what notices, so this only reads
    /// when the tab is the one in front — `catch_up` and the next draw ask.
    fn adopt(&mut self, _ws: &WorkspaceDoc) {}

    /// The document was replaced under the view: forget the last read.
    fn reseat(&mut self, ws: &WorkspaceDoc) {
        self.read_at = None;
        self.refresh(ws, true);
    }

    fn invalidate(&mut self) {
        self.engine = TextEngine::new();
        self.surface.invalidate();
    }

    fn document_root(&self) -> Option<NodeId> {
        Some(self.root)
    }

    fn draw(
        &mut self,
        scene: &mut Scene,
        ws: &koru_workspace::Workspace,
        images: &ImageCache,
        root: Affine,
        _caret_on: bool,
        _pointer: (f32, f32),
    ) {
        // The document this tab is over, wherever it is open: the active one,
        // or a background document when this tab is in another pane.
        if let Some(doc) = ws
            .document(self.document)
            .filter(|doc| doc.read(self.root).is_ok())
            .or_else(|| Some(&ws.ws).filter(|doc| doc.read(self.root).is_ok()))
        {
            self.refresh(doc, false);
        }
        let (w, h) = self.viewport;
        let camera = self.surface.camera();
        let gestures = self.gestures.clone();
        let objects = self
            .snapshot
            .as_ref()
            .map_or(0, |s| s.objects.len() + s.arrivals.len());
        let counters = self.surface.counters();
        let _inspect = koru_draw::inspect::scope(
            "Canvas",
            "container",
            root,
            scene::rect(0.0, 0.0, w, h),
            |r| {
                r.property("surface", "koru_canvas_ui::CanvasSurface", "CanvasView");
                r.property("objects", objects, "CanvasSnapshot");
                r.property(
                    "camera",
                    format!(
                        "({:.0}, {:.0}) at {:.0}%",
                        camera.center_x,
                        camera.center_y,
                        camera.zoom * 100.0
                    ),
                    "Camera",
                );
                r.property("drawn", counters.objects_drawn, "Counters");
                r.property("zooms claimed", gestures.zooms_claimed, "Gestures");
            },
        );
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.surface.set_lang(crate::format::lang());
        // The surface paints its own ground over its own extent (DESIGN.md,
        // *A view does not start on an empty scene*).
        self.surface.draw(scene, root, images);
        if let Some(menu) = &self.agents {
            menu.draw(scene, root, &mut self.engine, self.viewport);
        }
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.draw(scene, root, &mut self.engine, self.viewport);
        }
        if self.rename.is_some() {
            self.draw_rename(scene, root);
        }
    }

    fn restore(&mut self, _ws: &koru_workspace::Workspace, _settings: &Settings) {}

    fn capture(&self, _settings: &mut Settings) {}
}

impl CanvasView {
    /// Settle a document change for the tab about to be drawn. `true` when
    /// the canvas was read again.
    pub fn catch_up(&mut self, ws: &WorkspaceDoc) -> bool {
        self.refresh(ws, false)
    }

    /// The name prompt, centred over the canvas in the frame the container's
    /// *New document* prompt uses — one field, a heading and a hint.
    fn draw_rename(&mut self, scene: &mut Scene, root: Affine) {
        let frame = self.frame();
        let lang = crate::format::lang();
        let Some(prompt) = self.rename.as_mut() else {
            return;
        };
        let engine = &mut self.engine;
        frame.draw(
            scene,
            root,
            KoruDialogFrameInspection {
                state: KoruDialogFrameState::Open,
                ..Default::default()
            },
            |scene, root, panel| {
                draw_name_prompt_contents(
                    scene,
                    root,
                    prompt,
                    engine,
                    panel.x,
                    panel.y,
                    panel.w,
                    koru_i18n::t(lang, koru_i18n::Key::CanvasNameTitle),
                    koru_i18n::t(lang, koru_i18n::Key::DocUntitled),
                    false,
                );
                let hint = engine.simple_layout(
                    koru_i18n::t(lang, koru_i18n::Key::CanvasNameHint),
                    theme::role::hint(),
                );
                scene::draw_layout_first_line(
                    scene,
                    root,
                    &hint,
                    panel.x + theme::PANEL_PAD + 4.0,
                    panel.y + 62.0,
                    Some(theme::p().text_dim),
                );
            },
        );
    }
}

impl CanvasView {
    /// Queue a request as though the surface had sent it — for a test that
    /// reaches a button by what it asks rather than by where it is drawn.
    #[cfg(test)]
    pub(crate) fn push_request(&mut self, request: koru_canvas_ui::CanvasRequest) {
        self.deferred.push(request);
    }
}

/// A node's id, as the surface names it — for the container's tests.
#[cfg(test)]
pub(crate) fn object_key(node: NodeId) -> koru_canvas_ui::ObjectKey {
    key_of(node)
}
