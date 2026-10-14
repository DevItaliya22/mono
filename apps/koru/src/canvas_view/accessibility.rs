// SPDX-License-Identifier: MPL-2.0

//! **What a canvas offers a screen reader, and what a request to one of its
//! objects means** — the Tasks page's four hooks (`tasks_view/accessibility.rs`),
//! for a canvas (CANVAS P1.6).
//!
//! ```text
//! Group     the canvas            s:canvas#page      label = the canvas's name
//! └── ListBox  its objects        s:canvas#objects   size_of_set = how many
//!     └── ListBoxOption  one      n:<node uuid>#canvas-object   in reading order
//! Dialog    the text dialog       s:canvas#dialog    modal, while it is up
//! Menu      the agent menu        s:canvas#agents    while it is up
//! └── MenuItem  one agent         s:canvas#agent-N
//! ```
//!
//! **Everything comes from the surface.** An object's label, its bounds and the
//! order they are read in are `CanvasSurface::semantics()`'s — the same layout
//! pass drawing and hit testing use — shifted into the window's space once, in
//! [`Context::place`]. Its revision is the surface's `revision()`, which moves
//! when a label or the selection does and never on a pan or a frame.
//!
//! **An action is the press that already exists.** Focus and Click select the
//! object, which is what a click on it does; ScrollIntoView centres it, which
//! is what landing on it from search does.

use accesskit::{Action, Role};
use koru_draw::geometry::Rect;
use koru_i18n::{Key, Lang, t};
use koru_workspace_host::accessibility::{
    ControlPart, ObjectKey, Occurrence, Revision, SemanticNode, ViewSlot, WindowSlot,
};

use super::CanvasView;
use super::decor::node_of;

/// The canvas's own furniture, as one object.
const PAGE: &str = "canvas";

pub(crate) mod parts {
    use koru_workspace_host::accessibility::ControlPart;

    pub const PAGE: ControlPart = ControlPart::new("page");
    pub const OBJECTS: ControlPart = ControlPart::new("objects");
    /// One object — prefixed, because a canvas's node may also be an explore
    /// row, scoped by the same uuid.
    pub const OBJECT: ControlPart = ControlPart::new("canvas-object");
    pub const RESPOND: ControlPart = ControlPart::new("canvas-respond");
    pub const DIALOG: ControlPart = ControlPart::new("dialog");
    pub const AGENTS: ControlPart = ControlPart::new("agents");
}

/// What the canvas needs from the window it is drawn in.
#[derive(Clone, Debug)]
pub(crate) struct Context {
    pub window: WindowSlot,
    pub view: ViewSlot,
    /// The pane's view rect, in window-logical coordinates.
    pub area: Rect,
    pub lang: Lang,
    /// The canvas's name, as its tab says it.
    pub name: String,
}

impl Context {
    fn surface(&self, part: ControlPart) -> Occurrence {
        Occurrence::surface(self.window, self.view, PAGE, part)
    }

    fn object(&self, key: koru_canvas_ui::ObjectKey) -> Occurrence {
        Occurrence::model(self.window, self.view, key.0, parts::OBJECT)
    }

    /// **The one translation between the canvas's space and the window's.**
    fn place(&self, r: Rect) -> Rect {
        Rect::new(r.x + self.area.x, r.y + self.area.y, r.w, r.h)
    }
}

/// The part naming agent row `i` — a keyword, never a label.
fn agent_part(i: usize) -> ControlPart {
    const ROWS: [&str; 12] = [
        "agent-0", "agent-1", "agent-2", "agent-3", "agent-4", "agent-5", "agent-6", "agent-7",
        "agent-8", "agent-9", "agent-10", "agent-11",
    ];
    ControlPart::new(ROWS[i.min(ROWS.len() - 1)])
}

impl CanvasView {
    /// The canvas, as a screen reader sees it right now. `&self`: nothing is
    /// laid out, read or written.
    pub(crate) fn semantics(&self, cx: &Context) -> SemanticNode {
        let objects = self.surface.semantics();
        let total = objects.len();
        let children = objects
            .into_iter()
            .enumerate()
            .map(|(i, object)| {
                let response = object.response_bounds.map(|bounds| {
                    SemanticNode::new(
                        Occurrence::model(cx.window, cx.view, object.key.0, parts::RESPOND),
                        Role::Button,
                    )
                    .with_label(t(cx.lang, Key::CanvasRespond))
                    .with_bounds(cx.place(bounds))
                    .with_action(Action::Click)
                });
                SemanticNode::new(cx.object(object.key), Role::ListBoxOption)
                    .with_label(object.label)
                    .with_bounds(cx.place(object.bounds))
                    .with_position(i)
                    // No `selected`, for the Tasks page's reason: it would cost
                    // the option its Invoke. The selection is published as focus.
                    .with_actions([Action::Click, Action::Focus, Action::ScrollIntoView])
                    .with_children(response.into_iter().collect())
            })
            .collect();
        let name = if cx.name.trim().is_empty() {
            t(cx.lang, Key::DocUntitled).to_owned()
        } else {
            cx.name.clone()
        };
        let viewport = self.viewport;
        let whole = Rect::new(0.0, 0.0, viewport.0, viewport.1);
        let mut page = vec![
            SemanticNode::new(cx.surface(parts::OBJECTS), Role::ListBox)
                .with_label(name.clone())
                .with_bounds(cx.area)
                .with_size_of_set(total)
                .with_children(children),
        ];
        if let Some(dialog) = &self.dialog {
            page.push(
                SemanticNode::new(cx.surface(parts::DIALOG), Role::Dialog)
                    .with_label(dialog.title(cx.lang))
                    .with_bounds(cx.place(whole))
                    .modal(),
            );
        }
        if let Some(menu) = &self.agents {
            let rows = menu
                .rows()
                .map(|(i, label, enabled)| {
                    let row = SemanticNode::new(cx.surface(agent_part(i)), Role::MenuItem)
                        .with_label(label.to_owned())
                        .with_bounds(cx.place(menu.item_rect(viewport, i)));
                    if enabled {
                        row.with_action(Action::Click)
                    } else {
                        row
                    }
                })
                .collect();
            page.push(
                SemanticNode::new(cx.surface(parts::AGENTS), Role::Menu)
                    .with_label(t(cx.lang, Key::CanvasChooseAgent))
                    .with_bounds(cx.place(menu.panel(viewport)))
                    .with_children(rows),
            );
        }
        SemanticNode::new(cx.surface(parts::PAGE), Role::Group)
            .with_label(name)
            .with_bounds(cx.area)
            .with_children(page)
    }

    /// **Which control has the keyboard**, read from what the canvas draws
    /// from: the dialog, then the agent menu, then the last object selected.
    pub(crate) fn accessible_focus(&self, cx: &Context) -> Occurrence {
        if self.dialog.is_some() {
            return cx.surface(parts::DIALOG);
        }
        if let Some(menu) = &self.agents {
            return match menu.hover() {
                Some(i) => cx.surface(agent_part(i)),
                None => cx.surface(parts::AGENTS),
            };
        }
        match self.surface.selection().last() {
            Some(&key) => cx.object(key),
            None => cx.surface(parts::OBJECTS),
        }
    }

    /// Everything the tree above is built from, folded in. The surface's own
    /// revision already moves exactly when a label or the selection does.
    pub(crate) fn revise(&self, rev: &mut Revision) {
        rev.u64(self.surface.revision());
        rev.bool(self.dialog.is_some());
        if let Some(dialog) = &self.dialog {
            rev.str(&format!("{:?}", dialog.mode));
        }
        match &self.agents {
            Some(menu) => {
                rev.byte(1);
                rev.uuid(menu.node.as_uuid());
                rev.u64(menu.built_from().len() as u64);
                rev.u64(menu.hover().map_or(u64::MAX, |i| i as u64));
            }
            None => {
                rev.byte(0);
            }
        }
        rev.f32(self.viewport.0).f32(self.viewport.1);
    }

    /// **Do what a client asked, through the call a click makes.** `false`
    /// for anything this canvas does not hold, or while its dialog or menu
    /// owns the keyboard — the same answer the keyboard gives.
    pub(crate) fn accessibility_action(&mut self, target: Occurrence, action: Action) -> bool {
        if self.dialog.is_some() || self.agents.is_some() {
            return false;
        }
        let ObjectKey::Model(uuid) = target.object else {
            return false;
        };
        let key = koru_canvas_ui::ObjectKey(uuid);
        if self.object(node_of(key)).is_none() {
            return false;
        }
        if target.part == parts::RESPOND {
            if action == Action::Click
                && self
                    .object(node_of(key))
                    .is_some_and(|object| object.kind == koru_core::canvas::ObjectKind::Agent)
                && self.decor.statuses.get(&node_of(key))
                    == Some(&koru_canvas_ui::NodeStatus::NeedsResponse)
            {
                self.asks.push(super::CanvasAsk::Respond(node_of(key)));
                return true;
            }
            return false;
        }
        if target.part != parts::OBJECT {
            return false;
        }
        match action {
            Action::Focus | Action::Click => {
                self.surface.select(Some(key));
                true
            }
            Action::ScrollIntoView => {
                self.surface.focus_object(key);
                true
            }
            _ => false,
        }
    }
}
