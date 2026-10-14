// SPDX-License-Identifier: MPL-2.0

//! A canvas: an ordinary document whose root's positioned children are
//! **objects** on an infinite surface.
//!
//! There is no second Loro shape here. A canvas is a [`WorkspaceDoc`] whose root
//! is a [`Kind::Document`] node (its text is the title), listed in the store's
//! manifest under the kind token `canvas`. Search, sync, history, MCP, copy and
//! undo already speak that tree, so every one of them works on a canvas without
//! learning a new container. What makes a node an object is one value in its
//! meta map, [`NodeMeta::at`](crate::node::NodeMeta::at).
//!
//! ## The object rule
//!
//! Among the canvas root's children:
//!
//! - **An arrow** is a node carrying an arrow-end attribute
//!   ([`ROLE_ARROW_FROM`], [`ROLE_ARROW_TO`]). It is never an object, a line or
//!   an arrival. Its own text is its optional label. It is listed only while
//!   both of its ends are objects or arrivals of this canvas — an arrow whose
//!   end was deleted *dangles* and is hidden, so an undo of the delete brings
//!   both back without anybody having touched the arrow.
//! - **An object** is a child with a position. An `image` node is an image;
//!   every other kind is a *card*, whose text is its first line and whose
//!   children without a position are its further lines — a card is a small
//!   page, and every other writer (the phone, MCP, an older build's page
//!   editor) already makes that shape.
//! - **An arrival** is a child with no position that is not an arrow and not an
//!   empty paragraph: something a writer with no notion of place put there (a
//!   line typed on the phone, a malformed position). A surface places it.
//! - Documents, files and the trash are never objects.
//!
//! Workflow nodes are marked cards: a **start** node carries [`ROLE_START`], an
//! **agent** node [`ROLE_AGENT`] (its lines are its instruction, its latest run
//! is a whole-node [`ROLE_RUN`] attribute, its latest result lives inside it
//! under one child marked [`ROLE_RESULT`]), and an **output** node
//! [`ROLE_OUTPUT`] (it links the page it writes with [`ROLE_OUTPUT_PAGE`]).
//! A chain's state lives on its nodes too: a start node names the run of its
//! chain it last began and the device that began it
//! ([`ROLE_START_EXECUTION`], [`ROLE_START_DEVICE`]), and each agent and
//! output node the run its latest run or page belongs to
//! ([`ROLE_STEP_EXECUTION`]).
//! Every role is an open attribute string, so none of this moves the document
//! `schema_version`: an older build reads a canvas as a page whose paragraphs
//! carry attributes it preserves and does not draw.
//!
//! ## Writes
//!
//! Every writer here is **one undo step**: it runs inside
//! [`WorkspaceDoc::edit_group`], which joins a group the caller already holds
//! open rather than closing it. A caller that has been letting a person type
//! should [`checkpoint`](WorkspaceDoc::checkpoint) first, exactly as for any
//! other `edit_group`. A write that changes nothing (a move to where the object
//! already is, a bring-to-front of the topmost card) writes no op and leaves no
//! step behind. Refusals are [`StoreError::CanvasRejected`] tokens and are
//! checked before anything is written.
//!
//! ## What two devices do at once
//!
//! Pinned in `tests/convergence.rs`: two moves of one card resolve to one whole
//! position; a move and a width change both survive; a delete beats a
//! concurrent move or text edit. A delete against a concurrent reorder (bring
//! to front, send to back) is decided by Loro's tree order — **the later of the
//! two by Lamport order wins**, so a reorder Loro orders after the delete brings
//! the card back on both devices. Accepted: nothing is lost, and the person who
//! deleted it can delete it again.
//!
//! Undo, which is local-only, adds three more. A's Ctrl+Z of a move puts the
//! card back where A found it even over B's later move. A's Ctrl+Z of a delete
//! brings the card back **as A deleted it**: Loro undoes a tree delete by
//! creating a new tree node with the old meta, so text B typed into the card
//! meanwhile is not in it (the `NodeId` is the same, so arrows find it). And
//! a Ctrl+Z whose step has nothing left to act on — a move of a card B deleted
//! — is never silently used up: Loro skips it and undoes the step before, in
//! the same press.
//!
//! ## What a copy does not do yet
//!
//! `cutting`/`plant` carry `at` and `extent` (they are ordinary meta), but
//! `plant` mints fresh ids without remapping references inside the copy, so a
//! copied arrow still names the originals and is hidden in the copy. Remapping
//! arrow ends is the arrows plan's (canvas roadmap P3.2).

use std::collections::HashSet;

use crate::attributes::ROLE_RUN;
use crate::ids::{DocRef, DocumentId, DocumentReference, NodeId, Ref};
use crate::node::{Attr, AttrValue, BlobHash, CanvasExtent, CanvasPoint, Content, Kind, Node};
use crate::schema::{Parent, Result, StoreError, WorkspaceDoc};
use crate::text::ANCHOR;

// ------------------------------------------------------------------ roles
//
// **Stored names.** Each is the literal `role` text of an attribute record in
// every canvas that carries one. Cheap to change until the canvas ships, and
// permanent after it.

/// The node an arrow starts at: `AttrValue::Ref(Ref::new(node))`.
pub const ROLE_ARROW_FROM: &str = "koru.arrow.from";
/// The node an arrow ends at: `AttrValue::Ref(Ref::new(node))`.
pub const ROLE_ARROW_TO: &str = "koru.arrow.to";
/// Marks a **start node**: `AttrValue::Text(`[`START_MANUAL`]`)`.
pub const ROLE_START: &str = "koru.start";
/// The one start kind there is: a person presses it.
pub const START_MANUAL: &str = "manual";
/// Marks an **agent node**, and names the agent profile key it runs:
/// `AttrValue::Text(key)`. The empty string means no agent is chosen yet.
pub const ROLE_AGENT: &str = "koru.agent";
/// The model an agent node asks for, when it asks for one: `AttrValue::Text`.
pub const ROLE_AGENT_MODEL: &str = "koru.agent.model";
/// Marks an **output node**: `AttrValue::Text(`[`OUTPUT_PAGE`]`)`.
pub const ROLE_OUTPUT: &str = "koru.output";
/// The one output kind there is: it writes a page.
pub const OUTPUT_PAGE: &str = "page";
/// The page an output node writes:
/// `AttrValue::Document(DocumentReference::new(page))`.
///
/// Deliberately **not** added to `attributes::DOCUMENT_REFERENCE_ROLES`, which
/// is pinned short. The record is a document reference's either way; this
/// module reads it back from both the typed form and the `Ref` homed at the
/// document that every other reader sees.
pub const ROLE_OUTPUT_PAGE: &str = "koru.output.page";
/// Marks the one child paragraph that holds an agent node's latest result:
/// `AttrValue::Bool(true)`. Its text is the result's first line and its
/// children the rest; it is never one of the agent node's own lines.
pub const ROLE_RESULT: &str = "koru.agent.result";
/// The run of its chain a start node is running: `AttrValue::Text(id)`, a
/// fresh id per press of Run, and empty once that run has ended. What a
/// chain has done is read from its nodes against this id, so a window that
/// closed mid-chain can tell, on opening again, which steps belong to this
/// run and which to an earlier one.
pub const ROLE_START_EXECUTION: &str = "koru.start.execution";
/// The device that began [`ROLE_START_EXECUTION`], and so the only one that
/// advances it: `AttrValue::Text(device id)`.
pub const ROLE_START_DEVICE: &str = "koru.start.device";
/// On an agent or output node: the run of a chain its latest run or page
/// write belongs to — a start node's [`ROLE_START_EXECUTION`] at the time —
/// `AttrValue::Text(id)`.
pub const ROLE_STEP_EXECUTION: &str = "koru.step.execution";

/// The width a new card or workflow node is given.
pub const DEFAULT_WIDTH: i32 = 240;

/// How far below a row's first object another object may sit and still read
/// as part of the same row, in canvas pixels. See [`reading_order`].
pub const ROW_BAND: i32 = 40;

// ------------------------------------------------------------------ types

/// What an object on a canvas is.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum ObjectKind {
    /// A block with a position: its text is its first line.
    Card,
    /// An `image` node with a position.
    Image,
    /// A card marked [`ROLE_START`].
    Start,
    /// A card marked [`ROLE_AGENT`].
    Agent,
    /// A card marked [`ROLE_OUTPUT`].
    Output,
}

/// Why a canvas operation was refused — a token a host maps and a test
/// asserts on, never prose. Carried by [`StoreError::CanvasRejected`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum CanvasRefusal {
    /// [`add_object`] was handed [`ObjectKind::Image`]; an image has a blob
    /// and goes through [`add_image`].
    ImageNeedsAddImage,
    /// An arrow from an object to itself.
    SelfArrow,
    /// An arrow with the same two ends, in the same direction, already exists.
    DuplicateArrow,
    /// An arrow end that is not an object or an arrival of this canvas.
    NotOnCanvas,
    /// An object operation pointed at an arrow, or at a node that is never an
    /// object (a document, a file, the trash).
    NotAnObject,
    /// An arrow operation pointed at something that is not an arrow.
    NotAnArrow,
    /// An agent-node operation pointed at something that is not one.
    NotAnAgent,
    /// An output-node operation pointed at something that is not one.
    NotAnOutput,
    /// Text given to an object that has none — an image.
    NoText,
    /// A start-node operation pointed at something that is not one.
    NotAStart,
    /// A step operation pointed at something that is neither an agent nor an
    /// output node.
    NotAStep,
}

impl CanvasRefusal {
    /// The stable token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImageNeedsAddImage => "image_needs_add_image",
            Self::SelfArrow => "self_arrow",
            Self::DuplicateArrow => "duplicate_arrow",
            Self::NotOnCanvas => "not_on_canvas",
            Self::NotAnObject => "not_an_object",
            Self::NotAnArrow => "not_an_arrow",
            Self::NotAnAgent => "not_an_agent",
            Self::NotAnOutput => "not_an_output",
            Self::NoText => "no_text",
            Self::NotAStart => "not_a_start",
            Self::NotAStep => "not_a_step",
        }
    }
}

/// An image object's blob, as the node stores it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CanvasImage {
    pub hash: BlobHash,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// One object — or one arrival, which is the same shape with no `at`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CanvasObject {
    pub id: NodeId,
    pub kind: ObjectKind,
    pub at: Option<CanvasPoint>,
    pub extent: Option<CanvasExtent>,
    /// Its own line, as plain text (pill anchors removed). Empty for an image.
    pub text: String,
    /// Its further lines, as plain text: every descendant without a position,
    /// depth first, excluding the marked result child. A list container adds
    /// no line of its own (its items do); a code block adds one per line.
    pub lines: Vec<String>,
    /// The blob, for an image.
    pub image: Option<CanvasImage>,
    /// The agent profile key an agent node runs; `None` until one is chosen.
    pub agent: Option<String>,
    /// The model an agent node asks for, if it asks for one.
    pub agent_model: Option<String>,
    /// The Run document of an agent node's latest run.
    pub run: Option<DocumentId>,
    /// An agent node's latest result, as plain lines; empty when there is none.
    pub result: Vec<String>,
    /// The page an output node writes.
    pub output_page: Option<DocumentId>,
}

/// The run of its chain a start node last began, and where.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CanvasExecution {
    /// [`ROLE_START_EXECUTION`].
    pub id: String,
    /// [`ROLE_START_DEVICE`]; empty when the device that began it did not
    /// know its own id.
    pub device: String,
}

/// An arrow whose two ends are both on the canvas.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CanvasArrow {
    pub id: NodeId,
    pub from: NodeId,
    pub to: NodeId,
    /// Its label, as plain text; empty when it has none.
    pub label: String,
}

/// What a canvas holds, read once.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CanvasSnapshot {
    pub root: NodeId,
    /// Positioned root children, in sibling order — which is stacking order:
    /// the last is drawn on top.
    pub objects: Vec<CanvasObject>,
    /// Root children with no position that are not arrows and not empty
    /// paragraphs, in the order they arrived.
    pub arrivals: Vec<CanvasObject>,
    /// Only arrows whose two ends are both objects or arrivals here.
    pub arrows: Vec<CanvasArrow>,
}

impl CanvasSnapshot {
    /// The object or arrival with this id.
    pub fn get(&self, id: NodeId) -> Option<&CanvasObject> {
        self.objects
            .iter()
            .chain(&self.arrivals)
            .find(|object| object.id == id)
    }
}

// ------------------------------------------------------------------ read

/// Read the canvas rooted at `root`.
///
/// Lenient, as every reader of a synced document has to be: a child this build
/// cannot read (an unknown kind from a newer peer) is skipped, a malformed
/// position makes an arrival, and an arrow with a missing end is hidden. Only a
/// `root` that is not in the document is an error.
pub fn read(ws: &WorkspaceDoc, root: NodeId) -> Result<CanvasSnapshot> {
    ws.read(root)?;
    let mut objects = Vec::new();
    let mut arrivals = Vec::new();
    let mut arrow_nodes = Vec::new();
    for child in ws.children(Parent::Node(root))? {
        let Ok(node) = ws.read(child) else {
            continue;
        };
        if never_on_canvas(node.kind) {
            continue;
        }
        if is_arrow(&node) {
            arrow_nodes.push(node);
            continue;
        }
        if node.meta.at.is_none() && is_empty_paragraph(ws, &node)? {
            continue;
        }
        let object = object_of(ws, node)?;
        if object.at.is_some() {
            objects.push(object);
        } else {
            arrivals.push(object);
        }
    }

    let present: HashSet<NodeId> = objects.iter().chain(&arrivals).map(|o| o.id).collect();
    let arrows = arrow_nodes
        .into_iter()
        .filter_map(|node| {
            let (from, to) = (
                arrow_end(&node, ROLE_ARROW_FROM)?,
                arrow_end(&node, ROLE_ARROW_TO)?,
            );
            (from != to && present.contains(&from) && present.contains(&to)).then(|| CanvasArrow {
                id: node.id,
                from,
                to,
                label: plain(node.content.as_text().unwrap_or_default()),
            })
        })
        .collect();

    Ok(CanvasSnapshot {
        root,
        objects,
        arrivals,
        arrows,
    })
}

// ------------------------------------------------------------------ writers

/// Put a new card or workflow node on the canvas at `at`, on top of everything
/// else (appended last among the root's children), [`DEFAULT_WIDTH`] wide.
/// `text` is split on `'\n'`: the first line is the node's own text and each
/// further line becomes a line of it. A workflow kind gets its marker role — an
/// agent node with no agent chosen yet.
///
/// Refuses [`ObjectKind::Image`]; an image is made by [`add_image`].
pub fn add_object(
    ws: &mut WorkspaceDoc,
    root: NodeId,
    kind: ObjectKind,
    at: CanvasPoint,
    text: &str,
) -> Result<NodeId> {
    let marker = match kind {
        ObjectKind::Image => return Err(refused(root, CanvasRefusal::ImageNeedsAddImage)),
        ObjectKind::Card => None,
        ObjectKind::Start => Some(Attr::new(ROLE_START, AttrValue::Text(START_MANUAL.into()))),
        ObjectKind::Agent => Some(Attr::new(ROLE_AGENT, AttrValue::Text(String::new()))),
        ObjectKind::Output => Some(Attr::new(ROLE_OUTPUT, AttrValue::Text(OUTPUT_PAGE.into()))),
    };
    let (first, rest) = split_lines(text);
    let mut node = Node::new(
        Kind::Paragraph,
        Content::Text {
            text: first.to_owned(),
            attrs: marker.into_iter().collect(),
        },
    )?;
    node.meta.at = Some(at.clamped());
    node.meta.extent = Some(CanvasExtent::new(DEFAULT_WIDTH, None).clamped());
    ws.edit_group(|ws| {
        let id = ws.insert(Parent::Node(root), node)?;
        write_lines(ws, id, &rest)?;
        Ok(id)
    })
}

/// Put an image on the canvas at `at`, `w` pixels wide (its height follows
/// its aspect ratio), on top of everything else. The bytes are already in the
/// blob store; the node carries only the hash, type and natural size.
#[allow(clippy::too_many_arguments)]
pub fn add_image(
    ws: &mut WorkspaceDoc,
    root: NodeId,
    at: CanvasPoint,
    hash: BlobHash,
    mime: &str,
    width: Option<u32>,
    height: Option<u32>,
    w: i32,
) -> Result<NodeId> {
    let mut node = Node::image(hash, mime, width, height)?;
    node.meta.at = Some(at.clamped());
    node.meta.extent = Some(CanvasExtent::new(w, None).clamped());
    ws.edit_group(|ws| ws.insert(Parent::Node(root), node))
}

/// Move an object to `at` — one whole position, so two devices moving it at
/// once end with one of the two, never a mix.
pub fn move_object(ws: &mut WorkspaceDoc, id: NodeId, at: CanvasPoint) -> Result<()> {
    object_node(ws, id)?;
    ws.edit_group(|ws| ws.set_at(id, Some(at.clamped())))
}

/// Change an object's width. A height it carries is kept; a card's and an
/// image's is derived and stays absent.
pub fn set_width(ws: &mut WorkspaceDoc, id: NodeId, w: i32) -> Result<()> {
    let node = object_node(ws, id)?;
    let h = node.meta.extent.and_then(|extent| extent.h);
    ws.edit_group(|ws| ws.set_extent(id, Some(CanvasExtent::new(w, h).clamped())))
}

/// Replace a card's text. Split on `'\n'`: the first line becomes the node's
/// own text and the rest replace its further lines — its direct children with
/// no position, other than an agent node's result. A line whose plain text is
/// unchanged is not rewritten (its pills and formatting survive), an existing
/// plain line is edited in place rather than recreated, and anything that is
/// not a plain line (a nested list, an image) is replaced by a paragraph.
///
/// On an arrow this sets its label, which is one line. A code block card keeps
/// its whole text as its own. Refuses an image ([`CanvasRefusal::NoText`]).
pub fn set_text(ws: &mut WorkspaceDoc, id: NodeId, text: &str) -> Result<()> {
    let node = ws.read(id)?;
    if is_arrow(&node) {
        return ws.edit_group(|ws| set_own_line(ws, &node, text));
    }
    if never_on_canvas(node.kind) {
        return Err(refused(id, CanvasRefusal::NotAnObject));
    }
    if !matches!(node.content, Content::Text { .. }) {
        return Err(refused(id, CanvasRefusal::NoText));
    }
    if node.kind.allows_newlines() {
        return ws.edit_group(|ws| set_own_line(ws, &node, text));
    }
    if node.kind == Kind::ListBlock {
        // A list container has no line of its own: its items are the card's
        // lines, and the empty first line `read` reports for it is dropped.
        let (first, rest) = split_lines(text);
        let reported_empty_first = first.is_empty() && !rest.is_empty();
        let all: Vec<&str> = (!reported_empty_first)
            .then_some(first)
            .into_iter()
            .chain(rest)
            .collect();
        return ws.edit_group(|ws| write_lines(ws, id, &all));
    }
    let (first, rest) = split_lines(text);
    ws.edit_group(|ws| {
        set_own_line(ws, &node, first)?;
        write_lines(ws, id, &rest)
    })
}

/// Draw an arrow from `from` to `to`, both objects or arrivals of the canvas
/// at `root`. Refuses an arrow from a thing to itself, and a second arrow with
/// the same two ends in the same direction.
pub fn add_arrow(ws: &mut WorkspaceDoc, root: NodeId, from: NodeId, to: NodeId) -> Result<NodeId> {
    if from == to {
        return Err(refused(from, CanvasRefusal::SelfArrow));
    }
    let canvas = read(ws, root)?;
    for end in [from, to] {
        if canvas.get(end).is_none() {
            return Err(refused(end, CanvasRefusal::NotOnCanvas));
        }
    }
    if canvas.arrows.iter().any(|a| a.from == from && a.to == to) {
        return Err(refused(from, CanvasRefusal::DuplicateArrow));
    }
    let node = Node::new(
        Kind::Paragraph,
        Content::Text {
            text: String::new(),
            attrs: vec![
                Attr::new(ROLE_ARROW_FROM, AttrValue::Ref(Ref::new(from))),
                Attr::new(ROLE_ARROW_TO, AttrValue::Ref(Ref::new(to))),
            ],
        },
    )?;
    ws.edit_group(|ws| ws.insert(Parent::Node(root), node))
}

/// Delete an object and its lines. **Only the object**: an arrow that names it
/// is left dangling, and [`read`] hides it, so one undo brings both back.
pub fn delete_object(ws: &mut WorkspaceDoc, id: NodeId) -> Result<()> {
    object_node(ws, id)?;
    ws.edit_group(|ws| ws.delete(id))
}

/// Delete an arrow.
pub fn delete_arrow(ws: &mut WorkspaceDoc, id: NodeId) -> Result<()> {
    if !is_arrow(&ws.read(id)?) {
        return Err(refused(id, CanvasRefusal::NotAnArrow));
    }
    ws.edit_group(|ws| ws.delete(id))
}

/// Choose the agent an agent node runs, and the model it asks for (`None`, or
/// an empty string, asks for none).
pub fn set_agent(
    ws: &mut WorkspaceDoc,
    id: NodeId,
    agent: &str,
    model: Option<&str>,
) -> Result<()> {
    let node = agent_node(ws, id)?;
    ws.edit_group(|ws| {
        upsert(ws, &node, ROLE_AGENT, AttrValue::Text(agent.to_owned()))?;
        match model.filter(|model| !model.is_empty()) {
            Some(model) => upsert(
                ws,
                &node,
                ROLE_AGENT_MODEL,
                AttrValue::Text(model.to_owned()),
            ),
            None => remove_role(ws, &node, ROLE_AGENT_MODEL),
        }
    })
}

/// Link an agent node to the Run document of its latest run, replacing the
/// link to the one before.
pub fn set_run(ws: &mut WorkspaceDoc, id: NodeId, run: DocumentId) -> Result<()> {
    let node = agent_node(ws, id)?;
    ws.edit_group(|ws| {
        upsert(
            ws,
            &node,
            ROLE_RUN,
            AttrValue::Document(DocumentReference::new(run)),
        )
    })
}

/// Keep an agent node's latest result inside it: creates the marked result
/// child, or rewrites the one there (the same rules as [`set_text`], so an
/// unchanged line is not rewritten). An element holding `'\n'` is several
/// lines. No lines removes the result.
pub fn set_result(ws: &mut WorkspaceDoc, id: NodeId, lines: &[String]) -> Result<()> {
    agent_node(ws, id)?;
    let lines: Vec<&str> = lines
        .iter()
        .flat_map(|line| line.split('\n'))
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    ws.edit_group(|ws| {
        let holders = result_children(ws, id)?;
        let Some((first, rest)) = lines.split_first() else {
            for holder in holders {
                ws.delete(holder)?;
            }
            return Ok(());
        };
        let holder = match holders.split_first() {
            // Two peers that each wrote a first result made two holders; the
            // one `read` shows is the first, and the other goes now.
            Some((&holder, extra)) => {
                for &other in extra {
                    ws.delete(other)?;
                }
                let current = ws.read(holder)?;
                set_own_line(ws, &current, first)?;
                holder
            }
            None => ws.insert(
                Parent::Node(id),
                Node::new(
                    Kind::Paragraph,
                    Content::Text {
                        text: (*first).to_owned(),
                        attrs: vec![Attr::new(ROLE_RESULT, AttrValue::Bool(true))],
                    },
                )?,
            )?,
        };
        write_lines(ws, holder, rest)
    })
}

/// Link an output node to the page it writes.
pub fn set_output_page(ws: &mut WorkspaceDoc, id: NodeId, page: DocumentId) -> Result<()> {
    let node = ws.read(id)?;
    if is_arrow(&node) || node.attr(ROLE_OUTPUT).is_none() {
        return Err(refused(id, CanvasRefusal::NotAnOutput));
    }
    ws.edit_group(|ws| {
        upsert(
            ws,
            &node,
            ROLE_OUTPUT_PAGE,
            AttrValue::Document(DocumentReference::new(page)),
        )
    })
}

/// Record that a start node's chain is running as `execution`, begun on
/// `device` — both open text, rewritten in place, so a second press replaces
/// the first rather than adding to it. An empty `execution` says the run has
/// ended.
pub fn set_execution(
    ws: &mut WorkspaceDoc,
    start: NodeId,
    execution: &str,
    device: &str,
) -> Result<()> {
    let node = ws.read(start)?;
    if is_arrow(&node) || object_kind(&node) != ObjectKind::Start {
        return Err(refused(start, CanvasRefusal::NotAStart));
    }
    ws.edit_group(|ws| {
        upsert(
            ws,
            &node,
            ROLE_START_EXECUTION,
            AttrValue::Text(execution.to_owned()),
        )?;
        upsert(
            ws,
            &node,
            ROLE_START_DEVICE,
            AttrValue::Text(device.to_owned()),
        )
    })
}

/// The run of its chain a start node is running, if one is — begun and not
/// yet ended.
pub fn execution(ws: &WorkspaceDoc, start: NodeId) -> Result<Option<CanvasExecution>> {
    let node = ws.read(start)?;
    let text = |role: &str| node.attr(role).and_then(Attr::as_text).map(str::to_owned);
    Ok(text(ROLE_START_EXECUTION)
        .filter(|id| !id.is_empty())
        .map(|id| CanvasExecution {
            id,
            device: text(ROLE_START_DEVICE).unwrap_or_default(),
        }))
}

/// Record which run of a chain an agent or output node's latest run or page
/// write belongs to.
pub fn set_step_execution(ws: &mut WorkspaceDoc, node: NodeId, execution: &str) -> Result<()> {
    let read = step_node(ws, node)?;
    ws.edit_group(|ws| {
        upsert(
            ws,
            &read,
            ROLE_STEP_EXECUTION,
            AttrValue::Text(execution.to_owned()),
        )
    })
}

/// Which run of a chain an agent or output node last ran or wrote in.
pub fn step_execution(ws: &WorkspaceDoc, node: NodeId) -> Result<Option<String>> {
    Ok(ws
        .read(node)?
        .attr(ROLE_STEP_EXECUTION)
        .and_then(Attr::as_text)
        .filter(|id| !id.is_empty())
        .map(str::to_owned))
}

/// Draw an object above every other: the last of its siblings. Nothing is
/// written when it is already there.
pub fn bring_to_front(ws: &mut WorkspaceDoc, id: NodeId) -> Result<()> {
    object_node(ws, id)?;
    let parent = ws.parent(id)?;
    let siblings = ws.children(parent)?;
    if siblings.last() == Some(&id) {
        return Ok(());
    }
    // Loro reads the index after lifting the node out, so the last slot is
    // `len - 1`; `len` itself is out of bounds (`explore.rs`,
    // `raw_index_for_entry`).
    let last = siblings.len().saturating_sub(1);
    ws.edit_group(|ws| ws.move_node(id, parent, last))
}

/// Draw an object below every other: the first of its siblings. Nothing is
/// written when it is already there.
pub fn send_to_back(ws: &mut WorkspaceDoc, id: NodeId) -> Result<()> {
    object_node(ws, id)?;
    let parent = ws.parent(id)?;
    if ws.children(parent)?.first() == Some(&id) {
        return Ok(());
    }
    ws.edit_group(|ws| ws.move_node(id, parent, 0))
}

// ------------------------------------------------------------------ order

/// The order a reader with no layout — Markdown export, AI context, MCP, the
/// phone, a screen reader walking the canvas — meets its objects in.
///
/// Formed from **stored values only**, because those readers have no text
/// layout and so no card heights: objects are sorted by `at.y`, and an object
/// joins the current row when its `y` is within [`ROW_BAND`] pixels of the
/// row's first object. Rows read top to bottom; within a row objects read by
/// `at.x`, right to left when `rtl`. Two columns therefore read row by row,
/// interleaved, as a page's table would. Objects without `at` are left out —
/// arrivals come after every object, in their own order, which the caller
/// already has. Ties fall back to stacking order, so the answer is total.
pub fn reading_order(objects: &[CanvasObject], rtl: bool) -> Vec<NodeId> {
    let mut placed: Vec<(usize, CanvasPoint, NodeId)> = objects
        .iter()
        .enumerate()
        .filter_map(|(stacking, object)| Some((stacking, object.at?, object.id)))
        .collect();
    placed.sort_by_key(|&(stacking, at, _)| (at.y, stacking));

    let mut rows: Vec<Vec<(usize, CanvasPoint, NodeId)>> = Vec::new();
    for item in placed {
        match rows.last_mut() {
            // `i64`, because a caller can build a `CanvasObject` with any
            // `i32` and the difference of two of them need not fit one.
            Some(row) if i64::from(item.1.y) - i64::from(row[0].1.y) <= i64::from(ROW_BAND) => {
                row.push(item)
            }
            _ => rows.push(vec![item]),
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| {
            let across = if rtl {
                b.1.x.cmp(&a.1.x)
            } else {
                a.1.x.cmp(&b.1.x)
            };
            across.then(a.1.y.cmp(&b.1.y)).then(a.0.cmp(&b.0))
        });
    }
    rows.into_iter().flatten().map(|(_, _, id)| id).collect()
}

/// Whether the canvas is written right to left: more strong right-to-left
/// characters (Hebrew, Arabic, and the other `R`/`AL` scripts) than strong
/// left-to-right ones across the text and lines of its objects and arrivals.
/// A tie, and a canvas with no text, reads left to right.
///
/// For the readers that have no app language of their own (export, AI
/// context, MCP, the phone), so one canvas reads the same on every device.
///
/// **Counted here rather than through `unicode-bidi`**, which no crate in the
/// workspace depends on directly: a majority count needs only whether a
/// letter belongs to a right-to-left block, not the bidi algorithm, and the
/// domain model's leaf crate should not grow a dependency for eight ranges.
pub fn dominant_direction(snapshot: &CanvasSnapshot) -> bool {
    let (mut rtl, mut ltr) = (0usize, 0usize);
    let texts = snapshot
        .objects
        .iter()
        .chain(&snapshot.arrivals)
        .flat_map(|object| std::iter::once(&object.text).chain(&object.lines));
    for c in texts.flat_map(|text| text.chars()) {
        if !c.is_alphabetic() {
            continue;
        }
        if is_right_to_left(c) {
            rtl += 1;
        } else {
            ltr += 1;
        }
    }
    rtl > ltr
}

/// Whether a letter belongs to a block whose strong characters are
/// right-to-left (bidi class `R` or `AL`). Only asked of alphabetic
/// characters, so the blocks' digits and marks never reach it as letters.
fn is_right_to_left(c: char) -> bool {
    matches!(
        c as u32,
        // Hebrew, Arabic, Syriac, Arabic Supplement, Thaana, NKo, Samaritan,
        // Mandaic, Syriac Supplement, Arabic Extended-B and -A.
        0x0590..=0x08FF
        // Hebrew and Arabic presentation forms.
        | 0xFB1D..=0xFDFF
        | 0xFE70..=0xFEFF
        // The historic right-to-left scripts of the SMP (Phoenician,
        // Kharoshthi, Old Turkic, Hanifi Rohingya, …), Adlam and Mende Kikakui,
        // and the Arabic mathematical letters.
        | 0x10800..=0x10FFF
        | 0x1E800..=0x1EFFF
    )
}

// ------------------------------------------------------------------ private

fn refused(node: NodeId, why: CanvasRefusal) -> StoreError {
    StoreError::CanvasRejected { node, why }
}

/// Kinds that are never an object, a line or an arrival: a nested page, a file
/// record and the bin.
fn never_on_canvas(kind: Kind) -> bool {
    matches!(kind, Kind::Document | Kind::File | Kind::Trash)
}

/// A node carrying either arrow-end role is an arrow, even with one end
/// missing — so it is never mistaken for an arrival.
fn is_arrow(node: &Node) -> bool {
    node.attr(ROLE_ARROW_FROM).is_some() || node.attr(ROLE_ARROW_TO).is_some()
}

fn arrow_end(node: &Node, role: &str) -> Option<NodeId> {
    node.attr(role)?.as_ref_target().map(|target| target.node)
}

/// A node that is not an object on any canvas — an arrow or a document — is
/// refused by every object writer.
fn object_node(ws: &WorkspaceDoc, id: NodeId) -> Result<Node> {
    let node = ws.read(id)?;
    if is_arrow(&node) || never_on_canvas(node.kind) {
        return Err(refused(id, CanvasRefusal::NotAnObject));
    }
    Ok(node)
}

fn agent_node(ws: &WorkspaceDoc, id: NodeId) -> Result<Node> {
    let node = ws.read(id)?;
    if is_arrow(&node) || object_kind(&node) != ObjectKind::Agent {
        return Err(refused(id, CanvasRefusal::NotAnAgent));
    }
    Ok(node)
}

/// An agent or an output node: what a chain runs.
fn step_node(ws: &WorkspaceDoc, id: NodeId) -> Result<Node> {
    let node = ws.read(id)?;
    if is_arrow(&node) || !matches!(object_kind(&node), ObjectKind::Agent | ObjectKind::Output) {
        return Err(refused(id, CanvasRefusal::NotAStep));
    }
    Ok(node)
}

/// What the empty trailing line a page editor leaves behind looks like: a
/// childless paragraph with no text and no attributes. Not an arrival.
fn is_empty_paragraph(ws: &WorkspaceDoc, node: &Node) -> Result<bool> {
    Ok(node.kind == Kind::Paragraph
        && matches!(&node.content, Content::Text { text, attrs } if text.is_empty() && attrs.is_empty())
        && ws.children(Parent::Node(node.id))?.is_empty())
}

fn is_result_holder(node: &Node) -> bool {
    node.attr(ROLE_RESULT).is_some()
}

fn object_kind(node: &Node) -> ObjectKind {
    if matches!(node.content, Content::Blob { .. }) {
        ObjectKind::Image
    } else if node.attr(ROLE_START).is_some() {
        ObjectKind::Start
    } else if node.attr(ROLE_AGENT).is_some() {
        ObjectKind::Agent
    } else if node.attr(ROLE_OUTPUT).is_some() {
        ObjectKind::Output
    } else {
        ObjectKind::Card
    }
}

fn object_of(ws: &WorkspaceDoc, node: Node) -> Result<CanvasObject> {
    let kind = object_kind(&node);
    let text_attr = |role: &str| {
        node.attr(role)
            .and_then(Attr::as_text)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let (agent, agent_model) = if kind == ObjectKind::Agent {
        (text_attr(ROLE_AGENT), text_attr(ROLE_AGENT_MODEL))
    } else {
        (None, None)
    };
    let image = match &node.content {
        Content::Blob {
            hash,
            mime,
            width,
            height,
            ..
        } => Some(CanvasImage {
            hash: hash.clone(),
            mime: mime.clone(),
            width: *width,
            height: *height,
        }),
        _ => None,
    };
    let mut lines = Vec::new();
    lines_under(ws, node.id, &mut lines)?;
    let mut result = Vec::new();
    if let Some(holder) = result_children(ws, node.id)?.first() {
        let holder_node = ws.read(*holder)?;
        result.extend(own_lines(&holder_node));
        lines_under(ws, *holder, &mut result)?;
    }
    Ok(CanvasObject {
        id: node.id,
        kind,
        at: node.meta.at,
        extent: node.meta.extent,
        // A list container's text is always empty, and a code block's is its
        // whole source, newlines and all.
        text: plain(node.content.as_text().unwrap_or_default()),
        lines,
        image,
        agent,
        agent_model,
        run: document_attr(&node, ROLE_RUN),
        result,
        output_page: document_attr(&node, ROLE_OUTPUT_PAGE),
    })
}

/// The document an attribute names, from either form a reader hands back: the
/// typed reference, or a `Ref` homed at the document (a role the reader does
/// not normalize — the record is byte-identical).
fn document_attr(node: &Node, role: &str) -> Option<DocumentId> {
    match node.attr(role)?.value {
        AttrValue::Document(reference) => Some(reference.document),
        AttrValue::Ref(Ref {
            home: Some(DocRef::Document(document)),
            ..
        })
        | AttrValue::Ref(Ref {
            home: Some(DocRef::DocumentAt { document, .. }),
            ..
        }) => Some(document),
        _ => None,
    }
}

/// A node's text with pill anchors removed.
fn plain(text: &str) -> String {
    text.chars().filter(|&c| c != ANCHOR).collect()
}

/// The lines one node contributes of its own: one per line of its text, none
/// for a list container, a divider or a blob.
fn own_lines(node: &Node) -> Vec<String> {
    let Content::Text { text, .. } = &node.content else {
        return Vec::new();
    };
    match node.kind {
        Kind::ListBlock | Kind::Divider => Vec::new(),
        kind if kind.allows_newlines() => text.split('\n').map(plain).collect(),
        _ => vec![plain(text)],
    }
}

/// Every descendant of `id` without a position, depth first, as lines. It
/// does not enter a positioned descendant (an object inside a frame, later), a
/// nested page, or a result holder.
fn lines_under(ws: &WorkspaceDoc, id: NodeId, out: &mut Vec<String>) -> Result<()> {
    for child in ws.children(Parent::Node(id))? {
        let Ok(node) = ws.read(child) else {
            continue;
        };
        if node.meta.at.is_some() || never_on_canvas(node.kind) || is_result_holder(&node) {
            continue;
        }
        out.extend(own_lines(&node));
        lines_under(ws, child, out)?;
    }
    Ok(())
}

fn result_children(ws: &WorkspaceDoc, id: NodeId) -> Result<Vec<NodeId>> {
    Ok(ws
        .children(Parent::Node(id))?
        .into_iter()
        .filter(|&child| ws.read(child).is_ok_and(|node| is_result_holder(&node)))
        .collect())
}

/// `text` split into its first line and the rest, a trailing `'\r'` dropped
/// from each so a Windows clipboard does not leave one on every line.
fn split_lines(text: &str) -> (&str, Vec<&str>) {
    let mut lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    let first = lines.next().unwrap_or_default();
    (first, lines.collect())
}

/// Set a node's own text, unless its plain text already says that — so an
/// unchanged line keeps its pills and nothing is written.
fn set_own_line(ws: &mut WorkspaceDoc, node: &Node, text: &str) -> Result<()> {
    if plain(node.content.as_text().unwrap_or_default()) == text {
        return Ok(());
    }
    ws.set_text(node.id, text)
}

/// A line that can be edited in place: a one-line text block with nothing
/// under it.
fn is_plain_line(ws: &WorkspaceDoc, node: &Node) -> Result<bool> {
    Ok(
        matches!(node.kind, Kind::Paragraph | Kind::Task | Kind::Quote)
            && ws.children(Parent::Node(node.id))?.is_empty(),
    )
}

/// Make `owner`'s further lines exactly `lines`: its direct children with no
/// position that are not a result holder or a nested page, in order.
///
/// Diffed rather than rebuilt, so a long card with one line changed is one
/// small edit that merges with a concurrent edit to another line. A line is
/// kept when its plain text is unchanged, edited in place when it is a plain
/// line, and replaced by a paragraph otherwise; surplus lines are deleted.
/// New lines go directly after the previous line, so they stay above the
/// result holder and any object inside the card.
fn write_lines(ws: &mut WorkspaceDoc, owner: NodeId, lines: &[&str]) -> Result<()> {
    let mut existing = Vec::new();
    for child in ws.children(Parent::Node(owner))? {
        let node = ws.read(child)?;
        if node.meta.at.is_none() && !never_on_canvas(node.kind) && !is_result_holder(&node) {
            existing.push(node);
        }
    }

    let mut previous: Option<NodeId> = None;
    for (index, &line) in lines.iter().enumerate() {
        let kept = match existing.get(index) {
            Some(node) if is_plain_line(ws, node)? => {
                set_own_line(ws, node, line)?;
                Some(node.id)
            }
            Some(node) => {
                ws.delete(node.id)?;
                None
            }
            None => None,
        };
        let id = match kept {
            Some(id) => id,
            None => {
                let paragraph = Node::paragraph(line)?;
                match previous {
                    Some(previous) => ws.insert_after(previous, paragraph)?,
                    None => {
                        let id = ws.insert(Parent::Node(owner), paragraph)?;
                        ws.move_node(id, Parent::Node(owner), 0)?;
                        id
                    }
                }
            }
        };
        previous = Some(id);
    }
    for node in existing.iter().skip(lines.len()) {
        ws.delete(node.id)?;
    }
    Ok(())
}

/// Write `role` as `value`, rewriting the first attribute already under that
/// role (the one [`read`] shows) rather than adding a second.
fn upsert(ws: &mut WorkspaceDoc, node: &Node, role: &str, value: AttrValue) -> Result<()> {
    let attr = match node.attr(role) {
        Some(existing) if existing.value == value => return Ok(()),
        Some(existing) => Attr {
            value,
            ..existing.clone()
        },
        None => Attr::new(role, value),
    };
    ws.set_attr(node.id, &attr)?;
    Ok(())
}

fn remove_role(ws: &mut WorkspaceDoc, node: &Node, role: &str) -> Result<()> {
    for attr in node.attrs_with_role(role) {
        ws.remove_attr(node.id, attr.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
