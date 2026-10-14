// SPDX-License-Identifier: MPL-2.0

//! **The container's half of a canvas tab at work** (CANVAS P1.6, M4–M5): what
//! the canvas asks of the window, and what the window tells it.
//!
//! The canvas tab (`crate::canvas_view`) performs its own requests against its
//! own document. What it cannot do is here, because only the container holds
//! it: a Run or a Stop reaches the chain runner (`mainview/canvas_chain.rs`),
//! an output node's link opens a page the way every door opens one, the agent
//! menu's *nothing is ready* asks the host for readiness, and the picker, the
//! clipboard and a drop reach the canvas through the host's image seams.
//! The other way, the container dresses the canvas's scene — each agent's
//! name, where each node stands, what each output page is called — with a
//! [`Decor`] it computes from what it already has.
//!
//! Every function is reached from a one-line arm in `mod.rs` or from
//! `drain_trash_all`, the tail every input handler already runs.

use koru_canvas_ui::NodeStatus;
use koru_core::schema::WorkspaceDoc;
use koru_core::{DocumentId, NodeId};
use koru_i18n::{Key, t};
use koru_workspace_host::{
    NavigationTarget, PaneRequest, PasteTarget, PastedImage, SearchTargetDisposition, View,
};
use winit::keyboard::Key as WinitKey;

use super::super::MainView;
use super::super::canvas_chain::{CanvasNodeState, agent_key_text, parse_agent_key};
use super::super::panes::PanePath;
use super::super::tab::TabView;
use crate::canvas_view::{CanvasAsk, Decor};

/// Where a node stands, in the surface's words. The runner's words for the
/// dynamic parts are kept; *stopping* is a run that is still running until it
/// says it has stopped, and *not ready* is a node that cannot start — drawn
/// with the failure's mark and the runner's reason.
pub(crate) fn node_status(state: CanvasNodeState) -> NodeStatus {
    match state {
        CanvasNodeState::Idle => NodeStatus::Idle,
        CanvasNodeState::NeedsResponse => NodeStatus::NeedsResponse,
        CanvasNodeState::Running { activity, elapsed } => NodeStatus::Running {
            activity: activity.unwrap_or_default(),
            elapsed: elapsed.unwrap_or_default(),
        },
        CanvasNodeState::Stopping => NodeStatus::Running {
            activity: t(crate::format::lang(), Key::CanvasStatusStopping).to_owned(),
            elapsed: String::new(),
        },
        CanvasNodeState::Done => NodeStatus::Done,
        CanvasNodeState::Failed(why) => NodeStatus::Failed(why),
        CanvasNodeState::Stopped => NodeStatus::Stopped,
        CanvasNodeState::NotStarted => NodeStatus::NotStarted,
        CanvasNodeState::NotReady(why) => NodeStatus::Failed(why),
    }
}

impl MainView {
    /// **One pass over the canvases in this window**: the chains run on, a
    /// request a double-click left for a pass that can write is performed,
    /// what the canvas asked of the container is done, and a canvas on screen
    /// is dressed again when something it shows moved. `true` when anything a
    /// person can see moved.
    ///
    /// Nothing here reads a document, and an idle pass asks the runner
    /// nothing: see [`refresh_canvas_decor`](Self::refresh_canvas_decor).
    pub(in crate::mainview) fn maintain_canvas(&mut self, ws: &mut WorkspaceDoc) -> bool {
        let moved = self.maintain_canvas_chains();
        let mut changed = moved;
        if let TabView::Canvas(canvas) = self.active_view()
            && canvas.has_deferred()
        {
            changed |= canvas.perform(ws);
        }
        changed |= self.drain_canvas_asks();
        changed | self.refresh_canvas_decor(moved)
    }

    /// **Do what the canvas in front asked of the container.** Rides
    /// `drain_trash_all`, so a Run pressed with the mouse or the keyboard is
    /// started before the frame that shows it.
    pub(in crate::mainview) fn drain_canvas_asks(&mut self) -> bool {
        let (document, root, asks) = match self.active_view() {
            TabView::Canvas(canvas) => (canvas.document(), canvas.root(), canvas.take_asks()),
            _ => return false,
        };
        if asks.is_empty() {
            return false;
        }
        for ask in asks {
            match ask {
                CanvasAsk::RunChain(start) => self.run_chain(document, root, start),
                CanvasAsk::RunAgent(node) => self.run_agent_node(document, root, node),
                CanvasAsk::StopChain(start) => self.stop_chain(document, start),
                CanvasAsk::StopNode(node) => self.stop_node(document, node),
                CanvasAsk::Respond(node) => {
                    let run = self
                        .canvas_runs
                        .current(&(document, node))
                        .filter(|update| update.needs_response)
                        .and_then(|update| update.run);
                    if let Some(run) = run {
                        self.open_run_tab(run);
                        if let TabView::Run(view) = self.active_view()
                            && view.run() == Some(run)
                        {
                            view.respond();
                        }
                    }
                }
                CanvasAsk::OpenPage(page) => {
                    self.open_canvas_page(page);
                }
                CanvasAsk::WantAgents => self.canvas_runs.want_agents(),
            }
        }
        // A run just asked for says so at once, not on the next pass.
        self.refresh_canvas_decor(true);
        true
    }

    /// An output node's page, opened the way Explore opens a document.
    fn open_canvas_page(&mut self, page: DocumentId) -> bool {
        if self.request_store_document(NodeId::from_uuid(page.as_uuid()), true) {
            return true;
        }
        // Not in this window's listing yet — a page written a moment ago on
        // another device: ask for it by id all the same.
        self.ask_pane(PaneRequest::Navigate {
            target: NavigationTarget::Document {
                document: page,
                node: None,
            },
            match_text: None,
            disposition: SearchTargetDisposition::CurrentWindow,
        })
    }

    /// **Dress every canvas on screen** — the one in front of each pane — with
    /// what the container knows and its document does not: each agent's name,
    /// where each node stands, what each output page is called.
    ///
    /// **Only when something it shows may have moved**: `force` (the runner
    /// said a state moved, or a Run or Stop was just asked), the canvas asking
    /// (it was just built or read again, or the listing moved), or the ready
    /// agents changing. `canvas_node_states` is asked only then — the runner
    /// registers a canvas when it is asked, which is not a thing to do on
    /// every wake — and a canvas rebuilds its scene only when the decor it is
    /// handed differs from the last.
    fn refresh_canvas_decor(&mut self, force: bool) -> bool {
        struct Shown {
            path: PanePath,
            document: DocumentId,
            wants: bool,
            agents: Vec<(String, String)>,
            pages: Vec<DocumentId>,
            texts: Vec<String>,
        }
        let shown: Vec<Shown> = self
            .panes
            .iter_groups()
            .filter_map(|(path, group)| match &group.active_tab().view {
                TabView::Canvas(canvas) => Some(Shown {
                    path,
                    document: canvas.document(),
                    wants: canvas.wants_decor(),
                    agents: canvas.decor().agents.clone(),
                    pages: canvas.output_pages(),
                    texts: canvas.agent_texts(),
                }),
                _ => None,
            })
            .collect();
        if shown.is_empty() {
            return false;
        }
        let ready = self.canvas_runs.ready_agents();
        let agents: Vec<(String, String)> = ready
            .iter()
            .map(|(key, name)| (agent_key_text(key), name.clone()))
            .collect();
        let default_agent = self
            .canvas_runs
            .default_agent()
            .map(|key| agent_key_text(&key));
        let mut changed = false;
        for canvas in shown {
            if !(force || canvas.wants || canvas.agents != agents) {
                continue;
            }
            // A node's stored text, read back as the profile it names and
            // looked up among the ready ones.
            let agent_names = canvas
                .texts
                .into_iter()
                .filter_map(|text| {
                    let key = parse_agent_key(&text)?;
                    let (_, name) = ready.iter().find(|(ready, _)| *ready == key)?;
                    Some((text, name.clone()))
                })
                .collect();
            let decor = Decor {
                agents: agents.clone(),
                agent_names,
                default_agent: default_agent.clone(),
                statuses: self
                    .canvas_node_states(canvas.document)
                    .into_iter()
                    .map(|(node, state)| (node, node_status(state)))
                    .collect(),
                pages: canvas
                    .pages
                    .into_iter()
                    .filter_map(|page| {
                        let entries = self.entries.as_ref()?;
                        let entry = entries.iter().find(|entry| entry.id == page)?;
                        Some((page, entry.title.clone()))
                    })
                    .collect(),
            };
            if let Some(TabView::Canvas(view)) = self
                .panes
                .group_mut(&canvas.path)
                .map(|group| group.active_view())
            {
                changed |= view.set_decor(decor);
            }
        }
        changed
    }

    /// **A key coming up**, for the canvas in front: an arrow-key nudge ends
    /// on it, as one move per object.
    pub(in crate::mainview) fn canvas_key_up(
        &mut self,
        ws: &mut WorkspaceDoc,
        key: &WinitKey,
    ) -> bool {
        let (view, doc) = self.tabs_mut().active_mut().doc(ws);
        let TabView::Canvas(canvas) = view else {
            return false;
        };
        let handled = View::key_up(canvas, doc, key);
        handled | self.drain_canvas_asks()
    }

    /// **The window lost the keyboard**: every canvas on screen drops the
    /// gesture it had in flight, and writes nothing for it.
    pub(in crate::mainview) fn canvas_focus_lost(&mut self) {
        for (_, group) in self.panes.iter_groups_mut() {
            if let TabView::Canvas(canvas) = group.active_view() {
                canvas.focus_lost();
            }
        }
    }

    // ------------------------------------------------------------ images

    /// **Insert image…** on the canvas in front: the host's picker is asked
    /// for, with the canvas's root as the node, and the tab remembered so the
    /// file lands where it was asked for.
    pub(in crate::mainview) fn canvas_wants_image(&mut self) -> Option<NodeId> {
        let active = self.tabs().active();
        let TabView::Canvas(canvas) = self.active_view() else {
            return None;
        };
        if !canvas.take_wants_image() {
            return None;
        }
        let root = canvas.root();
        self.image_request = Some(active);
        Some(root)
    }

    /// The picked file, for the canvas that asked. `false` — and the page
    /// editor's path takes it — when the ask was not a canvas's.
    pub(in crate::mainview) fn canvas_insert_image(
        &mut self,
        ws: &mut WorkspaceDoc,
        node: NodeId,
        (hash, mime): (&str, &str),
        width: Option<u32>,
        height: Option<u32>,
    ) -> bool {
        let i = self.image_request.unwrap_or(self.tabs().active());
        let asked = matches!(
            self.tabs().get(i).map(|tab| &tab.view),
            Some(TabView::Canvas(canvas)) if canvas.root() == node
        );
        if !asked {
            return false;
        }
        self.image_request = None;
        let Some(TabView::Canvas(canvas)) = self.tabs_mut().get_mut(i).map(|tab| &mut tab.view)
        else {
            return false;
        };
        canvas.insert_picked_image(ws, hash, mime, width, height)
    }

    /// Images dropped on the window or pasted, for the canvas in front.
    pub(in crate::mainview) fn canvas_insert_images(
        &mut self,
        ws: &mut WorkspaceDoc,
        images: &[PastedImage],
    ) -> bool {
        let TabView::Canvas(canvas) = self.active_view() else {
            return false;
        };
        let images: Vec<(String, String, Option<u32>, Option<u32>)> = images
            .iter()
            .map(|image| {
                (
                    image.hash.clone(),
                    image.mime.clone(),
                    image.width,
                    image.height,
                )
            })
            .collect();
        canvas.insert_images(ws, &images)
    }

    /// One file dropped on the window, for the canvas in front.
    pub(in crate::mainview) fn canvas_append_image(
        &mut self,
        ws: &mut WorkspaceDoc,
        (hash, mime): (&str, &str),
        width: Option<u32>,
        height: Option<u32>,
    ) -> bool {
        let TabView::Canvas(canvas) = self.active_view() else {
            return false;
        };
        canvas.insert_images(ws, &[(hash.to_owned(), mime.to_owned(), width, height)])
    }

    /// Ctrl+V on the canvas in front: the host reads the clipboard's images.
    /// The target names the canvas's root; the canvas keeps the placement.
    pub(in crate::mainview) fn canvas_wants_paste(&mut self) -> Option<PasteTarget> {
        let TabView::Canvas(canvas) = self.active_view() else {
            return None;
        };
        canvas.take_wants_paste().then(|| PasteTarget {
            anchor: canvas.root(),
            replacing: Vec::new(),
        })
    }
}
