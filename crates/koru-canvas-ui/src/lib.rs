// SPDX-License-Identifier: MPL-2.0

//! **The canvas surface** — an open plane a person spreads cards, images and
//! workflow nodes out on, links with arrows, and runs a chain of agents from.
//!
//! This crate draws a canvas and turns input into requests. It knows nothing
//! of Koru's document model: the container reads its document into a plain
//! [`Scene`], hands it over with [`CanvasSurface::set_scene`], and drains
//! [`CanvasRequest`]s with [`CanvasSurface::take_requests`] to perform against
//! the document — the `koru-runs-ui` shape, with one layout pass shared by
//! drawing and hit testing and nothing done on the surface's behalf that the
//! surface could get wrong.
//!
//! ## What it holds, and what it does not
//!
//! It holds **where the person is looking** — the [`Camera`] — and the state
//! of a gesture in flight: a drag, a resize, a pan, an arrow being drawn. It
//! holds no document. A drag moves a *preview*; the one `Move` is sent when
//! the button comes up, and the scene the container hands back is what is on
//! screen from then on. Esc during a gesture, or the window losing focus,
//! puts everything back and sends nothing (CANVAS §4).
//!
//! ## What a frame costs
//!
//! **What is on screen, and nothing else.** An object is laid out once, in
//! canvas units, at its width; pan and zoom change the transform it is drawn
//! under and never lay text out again. Culling decides from arithmetic —
//! `layout::height_bound` — whether an object could be in view before its
//! text is shaped, so a canvas of two thousand cards with ten in view lays out
//! and draws ten. Nothing here touches the disk or the network, and
//! [`CanvasSurface::animating`] is false unless something on screen moves.
//!
//! ## Coordinates
//!
//! Positions are absolute canvas coordinates in whole logical pixels at
//! 100 % zoom. Input arrives in **view-local logical pixels** — the host has
//! divided by DPI and display zoom and subtracted the pane's origin — and the
//! `root` transform [`CanvasSurface::draw`] is given multiplies them back.
//! Handles, hit slop and the selection outline are sized in screen pixels, so
//! they keep their size at every zoom. The canvas is never mirrored for a
//! right-to-left language.

mod arrow;
mod camera;
mod draw;
mod input;
mod layout;
mod menu;
mod model;
mod order;
mod words;

#[cfg(test)]
mod dependency;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::time::Instant;

use koru_draw::geometry::Rect as ViewRect;
use koru_draw::text::TextEngine;
use koru_draw::theme::Brush;
use koru_i18n::Lang;
use koru_ui::Menu;
use parley::Layout;
use vello::kurbo::{Point, Rect};
use winit::keyboard::{ModifiersState, NamedKey};

pub use camera::Camera;
pub use layout::{
    COORD_LIMIT, Counters, DEFAULT_WIDTH, MAX_WIDTH, MIN_CARD_WIDTH, MIN_IMAGE_WIDTH,
    MIN_NODE_WIDTH,
};
pub use model::{
    CanvasRequest, NodeStatus, ObjectKey, ObjectKind, Scene, SceneArrow, SceneImage, SceneObject,
};

use layout::{ObjectLayout, Part};

/// Between the content's right edge and the arrivals column, and between two
/// arrivals.
const ARRIVAL_GAP: i32 = 24;

// Sizes in **screen** pixels: these keep their size at every zoom.

/// Half the width of the band along a card's right edge that resizes it.
const EDGE_GRAB: f64 = 5.0;
/// Half the side of the square on an image's bottom-right corner that
/// resizes it.
const CORNER_GRAB: f64 = 8.0;
/// The connect handle's drawn radius, and how far from its centre a press
/// still takes it.
const CONNECT_R: f64 = 5.0;
const CONNECT_HIT: f64 = 9.0;
/// How far from an arrow a press still hits it.
const ARROW_SLOP: f64 = 6.0;
/// How far the pointer moves before a press on an object becomes a drag.
const DRAG_THRESHOLD: f32 = 3.0;
/// How far one step of the wheel or an arrow key pans, in view pixels.
const PAN_STEP: f64 = 48.0;
/// How long the zoom readout stays after the last zoom.
const READOUT_MS: u64 = 900;

/// Whether two rectangles share any area.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 < b.x1 && a.x1 > b.x0 && a.y0 < b.y1 && a.y1 > b.y0
}

/// One object, as the surface keeps it: what the scene said, and where it is
/// drawn. An arrival's place is the surface's own, in the column.
struct Item {
    object: SceneObject,
    arrival: bool,
    x: i32,
    y: i32,
}

/// What this surface sent and the container has not answered yet. Drawn in
/// place of the scene's value until the next [`CanvasSurface::set_scene`], so
/// a released drag does not flick back for the frame before the container
/// catches up.
#[derive(Copy, Clone, Default, Debug)]
struct Pending {
    at: Option<(i32, i32)>,
    w: Option<i32>,
}

/// What is under a point.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Hit {
    Empty,
    /// The empty canvas's **Add card** button.
    EmptyAdd,
    Object(ObjectKey),
    Button(ObjectKey, Part),
    /// A card's right edge or an image's bottom-right corner.
    Resize(ObjectKey),
    /// The small circle an arrow is drawn out of.
    Connect(ObjectKey),
    Arrow(ObjectKey),
}

impl Hit {
    /// The object this hit is about, if it is about one.
    pub(crate) fn object(self) -> Option<ObjectKey> {
        match self {
            Self::Object(key) | Self::Button(key, _) | Self::Resize(key) | Self::Connect(key) => {
                Some(key)
            }
            Self::Empty | Self::EmptyAdd | Self::Arrow(_) => None,
        }
    }
}

/// A gesture in flight.
#[derive(Clone, Debug)]
pub(crate) enum Gesture {
    Idle,
    /// A press on an object that has not moved far enough to be a drag yet.
    Press {
        key: ObjectKey,
        at: (f32, f32),
        /// It was already selected, with others: a click without a drag
        /// narrows the selection to it.
        narrow: bool,
    },
    /// Dragging empty space. Esc puts the camera back where it was.
    Pan {
        from: Camera,
        at: (f32, f32),
    },
    Move {
        keys: Vec<ObjectKey>,
        /// Where the press landed, in canvas coordinates.
        grab: (f64, f64),
        delta: (i32, i32),
    },
    Resize {
        key: ObjectKey,
        grab: (f64, f64),
        start: i32,
        w: i32,
    },
    /// An arrow being drawn out of an object's connect handle.
    Connect {
        from: ObjectKey,
    },
    /// A button held down; it acts if the release is still over it.
    Button {
        hit: Hit,
    },
}

/// Arrow keys held with a selection: a live nudge, sent as one `Move` per
/// object when the last of them comes up.
#[derive(Clone, Debug)]
struct Nudge {
    keys: Vec<ObjectKey>,
    delta: (i32, i32),
    held: Vec<NamedKey>,
}

/// "Connect to…" was chosen: the next click, or Tab then Enter, picks the
/// object the arrow goes to.
#[derive(Copy, Clone, Debug)]
struct ConnectPick {
    from: ObjectKey,
    candidate: Option<ObjectKey>,
}

/// What a menu was opened over.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum MenuTarget {
    /// Empty canvas, at this canvas point — where an added object goes.
    Canvas {
        x: i32,
        y: i32,
    },
    Object(ObjectKey),
    Arrow(ObjectKey),
}

struct OpenMenu {
    menu: Menu<menu::Row>,
    target: MenuTarget,
}

/// One object as a screen reader is told about it.
#[derive(Clone, PartialEq, Debug)]
pub struct SemanticObject {
    /// Bounds of the visible response action, from the same cached layout
    /// used to paint and hit-test it. Absent when no question is pending.
    pub response_bounds: Option<ViewRect>,
    pub key: ObjectKey,
    /// Localized: the kind, the first line, and the status when there is one.
    pub label: String,
    /// In view pixels, like the input this surface takes.
    pub bounds: ViewRect,
    pub is_selected: bool,
}

/// The canvas surface. See the crate documentation.
pub struct CanvasSurface {
    lang: Lang,
    engine: TextEngine,
    /// Positioned objects in stacking order, then the arrivals.
    items: Vec<Item>,
    arrows: Vec<SceneArrow>,
    index: HashMap<ObjectKey, usize>,
    layouts: HashMap<ObjectKey, ObjectLayout>,
    /// An arrow's label, laid out once per label.
    labels: HashMap<ObjectKey, (String, Layout<Brush>)>,
    camera: Camera,
    viewport: (f32, f32),
    mods: ModifiersState,
    /// In the order they were selected; the last is the one Enter edits.
    selection: Vec<ObjectKey>,
    hover: Hit,
    pointer: Option<(f32, f32)>,
    gesture: Gesture,
    nudge: Option<Nudge>,
    pending: HashMap<ObjectKey, Pending>,
    menu: Option<OpenMenu>,
    connect_pick: Option<ConnectPick>,
    requests: Vec<CanvasRequest>,
    counters: Counters,
    revision: u64,
    /// Where the running mark's turn is measured from.
    epoch: Instant,
    /// A running node was on screen in the last frame.
    spinning: bool,
    readout_until: Option<Instant>,
    /// Whether the workflow half is offered: adding start, agent and output
    /// nodes, and a node's Run, Stop, Respond and agent choice. The host
    /// turns it off when its build hides agents ([`Self::set_workflows`]).
    workflows: bool,
}

impl CanvasSurface {
    pub fn new(lang: Lang) -> Self {
        Self {
            lang,
            engine: TextEngine::new(),
            items: Vec::new(),
            arrows: Vec::new(),
            index: HashMap::new(),
            layouts: HashMap::new(),
            labels: HashMap::new(),
            camera: Camera::default(),
            viewport: (800.0, 600.0),
            mods: ModifiersState::empty(),
            selection: Vec::new(),
            hover: Hit::Empty,
            pointer: None,
            gesture: Gesture::Idle,
            nudge: None,
            pending: HashMap::new(),
            menu: None,
            connect_pick: None,
            requests: Vec::new(),
            counters: Counters::default(),
            revision: 1,
            epoch: Instant::now(),
            spinning: false,
            readout_until: None,
            workflows: true,
        }
    }

    // ------------------------------------------------------------- setters

    /// **Offer the workflow half, or not.** Off, the menus have no *Add start
    /// / agent / output node*, and a node has no Run, Stop, Respond or agent
    /// choice — in its menu, on its face, or in the accessibility tree. A
    /// workflow node already on the canvas still draws: it is document
    /// content, and its words and its last result stay readable. The host
    /// decides why (Koru hides it with its agents); this crate only obeys.
    pub fn set_workflows(&mut self, on: bool) {
        if self.workflows != on {
            self.workflows = on;
            // The buttons are part of a node's layout.
            self.layouts.clear();
            self.menu = None;
            self.revision += 1;
        }
    }

    pub fn workflows(&self) -> bool {
        self.workflows
    }

    pub fn set_lang(&mut self, lang: Lang) {
        if self.lang != lang {
            self.lang = lang;
            // A node's words are laid out with it; the cache compares the
            // language and relays what it has to.
            self.labels.clear();
            self.menu = None;
            self.place_arrivals();
            self.revision += 1;
        }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Show a new scene. **Selection, the camera and a gesture in flight are
    /// kept by key**: a selected object that is still there stays selected,
    /// and a drag whose object was deleted underneath it simply ends.
    pub fn set_scene(&mut self, scene: Scene) {
        let Scene {
            objects,
            arrivals,
            arrows,
        } = scene;
        self.items = objects
            .into_iter()
            .map(|object| Item {
                x: object.x,
                y: object.y,
                object,
                arrival: false,
            })
            .chain(arrivals.into_iter().map(|object| Item {
                x: 0,
                y: 0,
                object,
                arrival: true,
            }))
            .collect();
        self.index = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.object.key, i))
            .collect();
        self.arrows = arrows;
        // The container has answered whatever was sent: its scene is what is
        // on screen now.
        self.pending.clear();
        let index = &self.index;
        self.layouts.retain(|key, _| index.contains_key(key));
        let arrows: std::collections::HashSet<ObjectKey> =
            self.arrows.iter().map(|arrow| arrow.key).collect();
        self.labels.retain(|key, _| arrows.contains(key));
        self.selection.retain(|key| index.contains_key(key));
        if let Some(nudge) = &mut self.nudge {
            nudge.keys.retain(|key| index.contains_key(key));
            if nudge.keys.is_empty() {
                self.nudge = None;
            }
        }
        if let Some(pick) = self.connect_pick
            && !self.index.contains_key(&pick.from)
        {
            self.connect_pick = None;
        }
        if let Some(pick) = &mut self.connect_pick
            && pick
                .candidate
                .is_some_and(|key| !self.index.contains_key(&key))
        {
            pick.candidate = None;
        }
        if self.gesture_lost_its_object() {
            self.gesture = Gesture::Idle;
        }
        if let Some(open) = &self.menu {
            let gone = match open.target {
                MenuTarget::Object(key) => !self.index.contains_key(&key),
                MenuTarget::Arrow(key) => !self.arrows.iter().any(|a| a.key == key),
                MenuTarget::Canvas { .. } => false,
            };
            if gone {
                self.menu = None;
            }
        }
        self.place_arrivals();
        self.refresh_hover();
        self.revision += 1;
    }

    fn gesture_lost_its_object(&self) -> bool {
        let gone = |key: &ObjectKey| !self.index.contains_key(key);
        match &self.gesture {
            Gesture::Press { key, .. } | Gesture::Resize { key, .. } => gone(key),
            Gesture::Connect { from } => gone(from),
            Gesture::Move { keys, .. } => keys.iter().any(gone),
            Gesture::Button { hit } => hit.object().is_some_and(|key| gone(&key)),
            Gesture::Idle | Gesture::Pan { .. } => false,
        }
    }

    pub fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport = (w.max(1.0), h.max(1.0));
    }

    pub fn viewport(&self) -> (f32, f32) {
        self.viewport
    }

    pub fn camera(&self) -> Camera {
        self.camera
    }

    pub fn set_camera(&mut self, camera: Camera) {
        self.camera = camera.clamped();
    }

    /// Lay everything out again: the palette, the fonts or the density moved
    /// under this surface. Brushes are baked into a layout, so a theme flip
    /// is this and not a redraw.
    pub fn invalidate(&mut self) {
        self.engine = TextEngine::new();
        self.layouts.clear();
        self.labels.clear();
        self.place_arrivals();
    }

    /// Drain what only the container can do.
    pub fn take_requests(&mut self) -> Vec<CanvasRequest> {
        std::mem::take(&mut self.requests)
    }

    /// The objects selected, in the order they were selected.
    pub fn selection(&self) -> &[ObjectKey] {
        &self.selection
    }

    /// A number that changes exactly when [`semantics`](Self::semantics)'
    /// labels or selection change — a new scene, a new selection, a new
    /// language — and not on a pan, a zoom or a frame.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// What the layout pass has done so far.
    pub fn counters(&self) -> Counters {
        self.counters
    }

    // ------------------------------------------------------------ geometry

    fn emit(&mut self, request: CanvasRequest) {
        self.requests.push(request);
    }

    fn key_index(&self, key: ObjectKey) -> Option<usize> {
        self.index.get(&key).copied()
    }

    /// The width an object is drawn at right now: a resize in flight, then
    /// what was sent and not yet answered, then the scene's.
    fn item_width(&self, i: usize) -> i32 {
        let item = &self.items[i];
        let key = item.object.key;
        if let Gesture::Resize { key: k, w, .. } = &self.gesture
            && *k == key
        {
            return *w;
        }
        let w = self
            .pending
            .get(&key)
            .and_then(|p| p.w)
            .unwrap_or(item.object.w);
        layout::clamp_width(item.object.kind, w)
    }

    /// Where an object's top-left corner is, before any gesture in flight.
    fn item_rest(&self, i: usize) -> (i32, i32) {
        let item = &self.items[i];
        self.pending
            .get(&item.object.key)
            .and_then(|p| p.at)
            .unwrap_or((item.x, item.y))
    }

    /// Where an object's top-left corner is drawn right now: at rest, plus a
    /// drag or a nudge in flight.
    fn item_pos(&self, i: usize) -> (i32, i32) {
        let key = self.items[i].object.key;
        let (mut x, mut y) = self.item_rest(i);
        if let Gesture::Move { keys, delta, .. } = &self.gesture
            && keys.contains(&key)
        {
            x = x.saturating_add(delta.0);
            y = y.saturating_add(delta.1);
        }
        if let Some(nudge) = &self.nudge
            && nudge.keys.contains(&key)
        {
            x = x.saturating_add(nudge.delta.0);
            y = y.saturating_add(nudge.delta.1);
        }
        (
            x.clamp(-COORD_LIMIT, COORD_LIMIT),
            y.clamp(-COORD_LIMIT, COORD_LIMIT),
        )
    }

    /// Lay one object out if what it says or how wide it is has changed.
    fn ensure_layout(&mut self, i: usize) -> &ObjectLayout {
        let w = self.item_width(i);
        let key = self.items[i].object.key;
        let fresh = self
            .layouts
            .get(&key)
            .is_some_and(|l| l.content.matches(&self.items[i].object, w, self.lang));
        if !fresh {
            let old = self.layouts.remove(&key);
            let laid = layout::lay_out(
                &mut self.engine,
                self.lang,
                &self.items[i].object,
                w,
                old,
                &mut self.counters,
                self.workflows,
            );
            self.layouts.insert(key, laid);
        }
        &self.layouts[&key]
    }

    /// An object's rectangle in canvas coordinates, laid out if it has to be.
    fn rect(&mut self, i: usize) -> Rect {
        let (x, y) = self.item_pos(i);
        let laid = self.ensure_layout(i);
        let (w, h) = (f64::from(laid.w), f64::from(laid.h));
        Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(x) + w,
            f64::from(y) + h,
        )
    }

    /// An object's rectangle, or a rectangle certain to contain it, **without
    /// laying anything out**. What culling decides from.
    fn bound(&self, i: usize) -> Rect {
        let (x, y) = self.item_pos(i);
        let w = self.item_width(i);
        let object = &self.items[i].object;
        let h = match self.layouts.get(&object.key) {
            Some(laid) if laid.content.matches(object, w, self.lang) => laid.h,
            _ => layout::height_bound(object, w),
        };
        Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(x) + f64::from(w),
            f64::from(y) + f64::from(h),
        )
    }

    fn to_view_rect(&self, rect: Rect) -> Rect {
        self.camera.rect_to_view(self.viewport, rect)
    }

    fn visible(&self) -> Rect {
        self.camera.visible(self.viewport)
    }

    /// Put the arrivals in their column: right of the content, top-aligned
    /// with it, one under another. The content is measured from the scene's
    /// own positions, so the column does not slide while something is being
    /// dragged.
    fn place_arrivals(&mut self) {
        let mut right: Option<i32> = None;
        let mut top: Option<i32> = None;
        for item in self.items.iter().filter(|item| !item.arrival) {
            let w = layout::clamp_width(item.object.kind, item.object.w);
            let edge = item.x.saturating_add(w);
            right = Some(right.map_or(edge, |r| r.max(edge)));
            top = Some(top.map_or(item.y, |t| t.min(item.y)));
        }
        let x = right.map_or(0, |r| r.saturating_add(ARRIVAL_GAP));
        let mut y = top.unwrap_or(0);
        for i in 0..self.items.len() {
            if !self.items[i].arrival {
                continue;
            }
            self.items[i].x = x.clamp(-COORD_LIMIT, COORD_LIMIT);
            self.items[i].y = y.clamp(-COORD_LIMIT, COORD_LIMIT);
            let h = self.ensure_layout(i).h.ceil() as i32;
            y = y.saturating_add(h).saturating_add(ARRIVAL_GAP);
        }
    }

    /// Positioned objects in reading order, then the arrivals in the order
    /// they arrived.
    fn reading_order(&self) -> Vec<usize> {
        let positioned: Vec<usize> = (0..self.items.len())
            .filter(|&i| !self.items[i].arrival)
            .collect();
        let positions: Vec<(i32, i32)> = positioned.iter().map(|&i| self.item_rest(i)).collect();
        let mut out: Vec<usize> = order::reading_order(&positions)
            .into_iter()
            .map(|j| positioned[j])
            .collect();
        out.extend((0..self.items.len()).filter(|&i| self.items[i].arrival));
        out
    }

    /// The objects whose handles are drawn: the selection and the one under
    /// the pointer.
    fn decorated(&self) -> Vec<ObjectKey> {
        let mut keys = self.selection.clone();
        if let Some(key) = self.hover.object()
            && !keys.contains(&key)
        {
            keys.push(key);
        }
        keys
    }

    /// The zone that resizes an object, in view pixels: a band along a
    /// card's right edge, a square on an image's bottom-right corner.
    fn resize_zone(kind: ObjectKind, view: Rect) -> Rect {
        match kind {
            ObjectKind::Image => Rect::new(
                view.x1 - CORNER_GRAB,
                view.y1 - CORNER_GRAB,
                view.x1 + CORNER_GRAB,
                view.y1 + CORNER_GRAB,
            ),
            _ => Rect::new(view.x1 - EDGE_GRAB, view.y0, view.x1 + EDGE_GRAB, view.y1),
        }
    }

    /// Where an object's connect handle sits, in view pixels: the middle of
    /// its right edge.
    fn connect_point(view: Rect) -> Point {
        Point::new(view.x1, (view.y0 + view.y1) * 0.5)
    }

    /// The button of object `i` under a canvas point inside its `rect`, if
    /// the point is on one rather than on the object's body.
    fn part_at(&mut self, i: usize, rect: Rect, canvas: (f64, f64)) -> Option<Part> {
        let (lx, ly) = ((canvas.0 - rect.x0) as f32, (canvas.1 - rect.y0) as f32);
        self.ensure_layout(i).button_at(lx, ly)
    }

    /// An arrow's segment, in canvas coordinates, when both of its objects
    /// are there and apart.
    fn arrow_segment(&mut self, a: usize) -> Option<(Point, Point)> {
        let arrow = &self.arrows[a];
        let (from, to) = (self.key_index(arrow.from)?, self.key_index(arrow.to)?);
        let (from, to) = (self.rect(from), self.rect(to));
        arrow::segment(from, to)
    }

    /// Whether an arrow could be in view, from its objects' bounds alone.
    fn arrow_maybe_visible(&self, a: usize, visible: Rect) -> bool {
        let arrow = &self.arrows[a];
        match (self.key_index(arrow.from), self.key_index(arrow.to)) {
            (Some(from), Some(to)) => overlaps(self.bound(from).union(self.bound(to)), visible),
            _ => false,
        }
    }

    /// An arrow's label chip, in canvas coordinates, laid out once per label.
    fn label_chip(&mut self, a: usize, mid: Point) -> Option<Rect> {
        let arrow = &self.arrows[a];
        if arrow.label.trim().is_empty() {
            return None;
        }
        let key = arrow.key;
        let stale = self
            .labels
            .get(&key)
            .is_none_or(|(text, _)| *text != arrow.label);
        if stale {
            let text = arrow.label.clone();
            let laid = self
                .engine
                .simple_layout(&text, koru_draw::theme::role::meta());
            self.labels.insert(key, (text, laid));
        }
        let (_, laid) = &self.labels[&key];
        let (w, h) = (
            f64::from(laid.width()) + draw::CHIP_PAD_X * 2.0,
            f64::from(laid.height()) + draw::CHIP_PAD_Y * 2.0,
        );
        Some(Rect::new(
            mid.x - w * 0.5,
            mid.y - h * 0.5,
            mid.x + w * 0.5,
            mid.y + h * 0.5,
        ))
    }

    // ------------------------------------------------------------ hit test

    /// **What is under a view point** — the one hit test pointer input and
    /// the cursor both ask, in the order things are drawn from the top down.
    pub(crate) fn hit(&mut self, x: f32, y: f32) -> Hit {
        let view = Point::new(f64::from(x), f64::from(y));
        let canvas = self.camera.to_canvas(self.viewport, (view.x, view.y));
        if self.items.is_empty() {
            let empty = self.empty_state();
            return if empty.button.contains(x, y) {
                Hit::EmptyAdd
            } else {
                Hit::Empty
            };
        }
        // The selection's and the hovered object's handles are drawn over
        // every object, so they are hit before any of them.
        for key in self.decorated().into_iter().rev() {
            let Some(i) = self.key_index(key) else {
                continue;
            };
            let view_rect = {
                let r = self.rect(i);
                self.to_view_rect(r)
            };
            if (Self::connect_point(view_rect) - view).hypot() <= CONNECT_HIT {
                return Hit::Connect(key);
            }
            if Self::resize_zone(self.items[i].object.kind, view_rect).contains(view) {
                return Hit::Resize(key);
            }
        }
        let zone = EDGE_GRAB.max(CORNER_GRAB) / self.camera.zoom;
        let point = Point::new(canvas.0, canvas.1);
        for i in (0..self.items.len()).rev() {
            if !self.bound(i).inflate(zone, zone).contains(point) {
                continue;
            }
            let rect = self.rect(i);
            let key = self.items[i].object.key;
            let view_rect = self.to_view_rect(rect);
            if Self::resize_zone(self.items[i].object.kind, view_rect).contains(view) {
                return Hit::Resize(key);
            }
            if !rect.contains(point) {
                continue;
            }
            return match self.part_at(i, rect, canvas) {
                Some(part) => Hit::Button(key, part),
                None => Hit::Object(key),
            };
        }
        let visible = self.visible();
        for a in (0..self.arrows.len()).rev() {
            if !self.arrow_maybe_visible(a, visible) {
                continue;
            }
            let Some((p0, p1)) = self.arrow_segment(a) else {
                continue;
            };
            let key = self.arrows[a].key;
            let mid = p0.midpoint(p1);
            if self
                .label_chip(a, mid)
                .is_some_and(|chip| chip.contains(point))
            {
                return Hit::Arrow(key);
            }
            let (v0, v1) = (
                self.camera.to_view(self.viewport, (p0.x, p0.y)),
                self.camera.to_view(self.viewport, (p1.x, p1.y)),
            );
            if arrow::distance(view, Point::new(v0.0, v0.1), Point::new(v1.0, v1.1)) <= ARROW_SLOP {
                return Hit::Arrow(key);
            }
        }
        Hit::Empty
    }

    /// Ask the hit test again where the pointer already is — the scene or
    /// the camera moved under it.
    fn refresh_hover(&mut self) {
        self.hover = match self.pointer {
            Some((x, y)) => self.hit(x, y),
            None => Hit::Empty,
        };
    }

    // ---------------------------------------------------------- selection

    /// Select one object, or nothing.
    pub fn select(&mut self, key: Option<ObjectKey>) {
        let next: Vec<ObjectKey> = key
            .filter(|key| self.index.contains_key(key))
            .into_iter()
            .collect();
        self.set_selection(next);
    }

    fn set_selection(&mut self, next: Vec<ObjectKey>) {
        if self.selection != next {
            self.commit_nudge();
            self.selection = next;
            self.revision += 1;
        }
    }

    /// Centre the view on an object and select it — where a search hit, a
    /// Tasks "go to" or a `koru://` link lands.
    pub fn focus_object(&mut self, key: ObjectKey) {
        let Some(i) = self.key_index(key) else {
            return;
        };
        let center = self.rect(i).center();
        self.camera = Camera::new(center.x, center.y, self.camera.zoom);
        self.select(Some(key));
        self.refresh_hover();
    }

    /// Pan just far enough that an object is in view, with a margin.
    fn reveal(&mut self, key: ObjectKey) {
        const MARGIN: f64 = 24.0;
        let Some(i) = self.key_index(key) else {
            return;
        };
        let r = {
            let rect = self.rect(i);
            self.to_view_rect(rect)
        };
        let (vw, vh) = (f64::from(self.viewport.0), f64::from(self.viewport.1));
        let shift = |lo: f64, hi: f64, room: f64| -> f64 {
            if hi - lo > room - MARGIN * 2.0 || lo < MARGIN {
                MARGIN - lo
            } else if hi > room - MARGIN {
                room - MARGIN - hi
            } else {
                0.0
            }
        };
        let (dx, dy) = (shift(r.x0, r.x1, vw), shift(r.y0, r.y1, vh));
        if dx != 0.0 || dy != 0.0 {
            self.camera.pan_view(dx, dy);
        }
    }

    // ------------------------------------------------------- accessibility

    /// **Every object as a screen reader is told about it**, in reading
    /// order. Reads what is already laid out and lays nothing out: an object
    /// never drawn is published with the bound culling uses.
    pub fn semantics(&self) -> Vec<SemanticObject> {
        self.reading_order()
            .into_iter()
            .map(|i| {
                let object = &self.items[i].object;
                let view = self.to_view_rect(self.bound(i));
                SemanticObject {
                    response_bounds: self
                        .layouts
                        .get(&object.key)
                        .filter(|layout| {
                            layout
                                .content
                                .matches(object, self.item_width(i), self.lang)
                        })
                        .and_then(|layout| {
                            layout
                                .buttons
                                .iter()
                                .find(|button| button.part == layout::Part::Respond)
                        })
                        .map(|button| {
                            let (x, y) = self.item_pos(i);
                            let r = button.rect;
                            let view = self.to_view_rect(Rect::new(
                                f64::from(x) + f64::from(r.x),
                                f64::from(y) + f64::from(r.y),
                                f64::from(x) + f64::from(r.x + r.w),
                                f64::from(y) + f64::from(r.y + r.h),
                            ));
                            ViewRect::new(
                                view.x0 as f32,
                                view.y0 as f32,
                                view.width() as f32,
                                view.height() as f32,
                            )
                        }),
                    key: object.key,
                    label: words::object_label(self.lang, object),
                    bounds: ViewRect::new(
                        view.x0 as f32,
                        view.y0 as f32,
                        view.width() as f32,
                        view.height() as f32,
                    ),
                    is_selected: self.selection.contains(&object.key),
                }
            })
            .collect()
    }
}
