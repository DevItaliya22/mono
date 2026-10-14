// SPDX-License-Identifier: MPL-2.0

//! **The document, as the surface is handed it.**
//!
//! A canvas document reads as a [`CanvasSnapshot`] (`koru_core::canvas::read`);
//! the surface draws a plain [`Scene`]. This is the one mapping between them —
//! objects, arrivals and arrows one to one, a node's id as the object's key —
//! plus the three things the document does not say and the container does:
//! which agent a stored profile key *is* by name, where each workflow node
//! stands, and what an output node's page is called. Those arrive as a
//! [`Decor`], and a scene is rebuilt from the snapshot already in hand when
//! either changes. Nothing here reads the document.

use std::collections::HashMap;

use koru_canvas_ui::{
    NodeStatus, ObjectKey, ObjectKind as SurfaceKind, Scene, SceneArrow, SceneImage, SceneObject,
};
use koru_core::canvas::{CanvasObject, CanvasSnapshot, ObjectKind};
use koru_core::{DocumentId, NodeId};
use koru_i18n::{Key, Lang, t};

/// **What the container knows about a canvas that its document does not.**
///
/// Pushed by the container with [`CanvasView::set_decor`](super::CanvasView::set_decor)
/// and compared, so a pass that changes nothing rebuilds nothing.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Decor {
    /// The agents ready on this machine, as `(stored key, display name)`, in
    /// the order the agent menu lists them. The stored key is what an agent
    /// node's `koru.agent` attribute holds (`canvas_chain::agent_key_text`).
    pub agents: Vec<(String, String)>,
    /// The name of each agent this canvas's nodes name, by the text each node
    /// stores — resolved by the container, which can read that text back as a
    /// profile (`canvas_chain::parse_agent_key`).
    pub agent_names: HashMap<String, String>,
    /// The agent a new agent node starts with, as its stored key.
    pub default_agent: Option<String>,
    /// Where each workflow node stands. A node with no entry draws no status.
    pub statuses: HashMap<NodeId, NodeStatus>,
    /// The titles of the pages the output nodes write, by document.
    pub pages: HashMap<DocumentId, String>,
}

impl Decor {
    /// An agent's name for the text a node stores: the ready agent it reads
    /// back as, or the text itself for an agent this machine does not offer —
    /// a node chosen on another computer still says *something*, and the menu
    /// offers the ones here.
    pub fn agent_name(&self, stored: &str) -> String {
        if let Some(name) = self.agent_names.get(stored) {
            return name.clone();
        }
        // A node given its agent a moment ago, before the container has read
        // its text back: the menu's own list already knows the name.
        self.agents
            .iter()
            .find(|(key, _)| key == stored.trim())
            .map_or_else(|| stored.to_owned(), |(_, name)| name.clone())
    }
}

/// A node's id as the surface's key — the same UUID, so a request names
/// exactly the node it is about.
pub fn key_of(id: NodeId) -> ObjectKey {
    ObjectKey(id.as_uuid())
}

/// …and back.
pub fn node_of(key: ObjectKey) -> NodeId {
    NodeId::from_uuid(key.0)
}

pub fn surface_kind(kind: ObjectKind) -> SurfaceKind {
    match kind {
        ObjectKind::Card => SurfaceKind::Card,
        ObjectKind::Image => SurfaceKind::Image,
        ObjectKind::Start => SurfaceKind::Start,
        ObjectKind::Agent => SurfaceKind::Agent,
        ObjectKind::Output => SurfaceKind::Output,
    }
}

pub fn model_kind(kind: SurfaceKind) -> ObjectKind {
    match kind {
        SurfaceKind::Card => ObjectKind::Card,
        SurfaceKind::Image => ObjectKind::Image,
        SurfaceKind::Start => ObjectKind::Start,
        SurfaceKind::Agent => ObjectKind::Agent,
        SurfaceKind::Output => ObjectKind::Output,
    }
}

/// The whole canvas, as a scene.
pub fn scene_of(snapshot: &CanvasSnapshot, decor: &Decor, lang: Lang) -> Scene {
    Scene {
        objects: snapshot
            .objects
            .iter()
            .map(|object| object_of(object, decor, lang))
            .collect(),
        arrivals: snapshot
            .arrivals
            .iter()
            .map(|object| object_of(object, decor, lang))
            .collect(),
        arrows: snapshot
            .arrows
            .iter()
            .map(|arrow| SceneArrow {
                key: key_of(arrow.id),
                from: key_of(arrow.from),
                to: key_of(arrow.to),
                label: arrow.label.clone(),
            })
            .collect(),
    }
}

fn object_of(object: &CanvasObject, decor: &Decor, lang: Lang) -> SceneObject {
    let kind = surface_kind(object.kind);
    let (x, y) = object.at.map_or((0, 0), |at| (at.x, at.y));
    SceneObject {
        key: key_of(object.id),
        kind,
        x,
        y,
        // Zero is "not set": the surface draws its default width.
        w: object.extent.map_or(0, |extent| extent.w),
        text: object.text.clone(),
        lines: object.lines.clone(),
        image: object.image.as_ref().map(|image| SceneImage {
            hash: image.hash.0.clone(),
            width: image.width.unwrap_or(0),
            height: image.height.unwrap_or(0),
        }),
        agent: object
            .agent
            .as_deref()
            .map(|stored| decor.agent_name(stored)),
        status: kind
            .is_node()
            .then(|| decor.statuses.get(&object.id).cloned())
            .flatten(),
        result: object.result.clone(),
        link: object.output_page.map(|page| {
            decor
                .pages
                .get(&page)
                .filter(|title| !title.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| t(lang, Key::DocUntitled).to_owned())
        }),
    }
}

/// A node's whole text as the dialog edits it: its own line, then its
/// further lines.
pub fn text_of(object: &CanvasObject) -> String {
    std::iter::once(object.text.as_str())
        .chain(object.lines.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}
