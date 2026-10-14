// SPDX-License-Identifier: MPL-2.0

//! **Pointer and keyboard**, turned into previews while a gesture is in
//! flight and into one request when it ends.
//!
//! Every gesture answers to three rules (CANVAS §4):
//!
//! - **one gesture, one request** — a drag is one `Move` per object on
//!   release, a resize one `SetWidth`, a held arrow key one `Move` per object
//!   when the last arrow comes up;
//! - **Esc or a lost focus cancels and writes nothing** — the preview goes
//!   back and no request is sent;
//! - **what the pointer and the keyboard do reach the same function** — a
//!   menu row, a button and a key that mean the same thing emit the same
//!   request from the same place.

use std::time::{Duration, Instant};

use winit::event::MouseScrollDelta;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

use crate::layout::{self, clamp_coord};
use crate::menu::{self, Action};
use crate::model::{CanvasRequest, ObjectKey, ObjectKind};
use crate::{
    Camera, CanvasSurface, ConnectPick, DRAG_THRESHOLD, Gesture, Hit, MenuTarget, Nudge, OpenMenu,
    PAN_STEP, READOUT_MS,
};

impl CanvasSurface {
    // ------------------------------------------------------------ pointer

    pub fn mouse_down(&mut self, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        self.commit_nudge();
        if self.menu.is_some() {
            let chosen = self
                .menu
                .as_ref()
                .and_then(|open| open.menu.item_at(self.viewport, x, y));
            match chosen {
                Some(row) => self.choose(row),
                // A press outside an open menu closes it and does nothing
                // else: it was aimed at the menu, not at what is under it.
                None => self.menu = None,
            }
            self.refresh_hover();
            return true;
        }
        if let Some(pick) = self.connect_pick.take() {
            if let Some(to) = self.hit(x, y).object()
                && to != pick.from
            {
                self.emit(CanvasRequest::Connect {
                    from: pick.from,
                    to,
                });
            }
            self.refresh_hover();
            return true;
        }
        let hit = self.hit(x, y);
        self.hover = hit;
        let canvas = self
            .camera
            .to_canvas(self.viewport, (f64::from(x), f64::from(y)));
        match hit {
            Hit::EmptyAdd | Hit::Button(..) => self.gesture = Gesture::Button { hit },
            Hit::Connect(from) => self.gesture = Gesture::Connect { from },
            Hit::Resize(key) => {
                if let Some(i) = self.key_index(key) {
                    let start = self.item_width(i);
                    self.gesture = Gesture::Resize {
                        key,
                        grab: canvas,
                        start,
                        w: start,
                    };
                }
            }
            Hit::Object(key) => {
                let shift = self.mods.shift_key();
                let selected = self.selection.contains(&key);
                if shift {
                    let mut next = self.selection.clone();
                    if selected {
                        next.retain(|k| *k != key);
                    } else {
                        next.push(key);
                    }
                    self.set_selection(next);
                } else if !selected {
                    self.set_selection(vec![key]);
                }
                let narrow = !shift && selected && self.selection.len() > 1;
                self.gesture = Gesture::Press {
                    key,
                    at: (x, y),
                    narrow,
                };
            }
            Hit::Arrow(_) | Hit::Empty => {
                if !self.mods.shift_key() {
                    self.set_selection(Vec::new());
                }
                self.gesture = Gesture::Pan {
                    from: self.camera,
                    at: (x, y),
                };
            }
        }
        true
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        if let Some(open) = &mut self.menu {
            let next = open.menu.item_at(self.viewport, x, y);
            let changed = open.menu.hover != next;
            open.menu.hover = next;
            return changed;
        }
        match &self.gesture {
            Gesture::Idle | Gesture::Button { .. } => {}
            Gesture::Press { key, at, .. } => {
                let (key, at) = (*key, *at);
                let moved = ((x - at.0).powi(2) + (y - at.1).powi(2)).sqrt();
                if moved > DRAG_THRESHOLD && self.selection.contains(&key) {
                    let grab = self
                        .camera
                        .to_canvas(self.viewport, (f64::from(at.0), f64::from(at.1)));
                    self.gesture = Gesture::Move {
                        keys: self.selection_in_stacking_order(),
                        grab,
                        delta: (0, 0),
                    };
                    self.drag_update();
                    return true;
                }
                return false;
            }
            Gesture::Pan { from, at } => {
                let mut camera = *from;
                camera.pan_view(f64::from(x - at.0), f64::from(y - at.1));
                self.camera = camera;
                return true;
            }
            Gesture::Move { .. } | Gesture::Resize { .. } => {
                self.drag_update();
                return true;
            }
            Gesture::Connect { .. } => {
                self.hover = self.hit(x, y);
                return true;
            }
        }
        let next = self.hit(x, y);
        let changed = next != self.hover || self.connect_pick.is_some();
        self.hover = next;
        changed
    }

    pub fn mouse_up(&mut self) -> bool {
        let gesture = std::mem::replace(&mut self.gesture, Gesture::Idle);
        let changed = match gesture {
            Gesture::Idle => false,
            Gesture::Press { key, narrow, .. } => {
                if narrow {
                    self.set_selection(vec![key]);
                }
                true
            }
            Gesture::Pan { .. } => true,
            Gesture::Move { keys, delta, .. } => {
                if delta != (0, 0) {
                    self.send_moves(&keys, delta);
                }
                true
            }
            Gesture::Resize { key, start, w, .. } => {
                if w != start {
                    self.emit(CanvasRequest::SetWidth { key, w });
                    self.pending.entry(key).or_default().w = Some(w);
                }
                true
            }
            Gesture::Connect { from } => {
                if let Some((x, y)) = self.pointer
                    && let Some(to) = self.hit(x, y).object()
                    && to != from
                {
                    self.emit(CanvasRequest::Connect { from, to });
                }
                true
            }
            Gesture::Button { hit } => {
                // A button acts on release, and only if the release is still
                // over it: a press can be taken back by sliding off.
                if let Some((x, y)) = self.pointer
                    && self.hit(x, y) == hit
                {
                    self.press(hit);
                }
                true
            }
        };
        self.refresh_hover();
        changed
    }

    /// The second press of a double-click has already arrived as a
    /// [`mouse_down`](Self::mouse_down); whatever it started is dropped here
    /// in favour of what a double-click means.
    pub fn double_click(&mut self, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        if self.menu.is_some() || self.connect_pick.is_some() {
            return false;
        }
        self.cancel_gesture();
        match self.hit(x, y) {
            Hit::Object(key) => {
                self.emit(CanvasRequest::EditText { key });
                true
            }
            Hit::Empty => {
                let (cx, cy) = self
                    .camera
                    .to_canvas(self.viewport, (f64::from(x), f64::from(y)));
                self.emit(CanvasRequest::AddObject {
                    kind: ObjectKind::Card,
                    x: clamp_coord(cx),
                    y: clamp_coord(cy),
                });
                true
            }
            Hit::EmptyAdd | Hit::Button(..) | Hit::Resize(_) | Hit::Connect(_) | Hit::Arrow(_) => {
                false
            }
        }
    }

    /// A right-click: the menu for what is under the pointer.
    pub fn context_menu(&mut self, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        self.cancel_gesture();
        self.commit_nudge();
        self.connect_pick = None;
        let hit = self.hit(x, y);
        let target = match hit {
            Hit::Arrow(key) => MenuTarget::Arrow(key),
            _ => match hit.object() {
                Some(key) => {
                    if !self.selection.contains(&key) {
                        self.set_selection(vec![key]);
                    }
                    MenuTarget::Object(key)
                }
                None => {
                    let (cx, cy) = self
                        .camera
                        .to_canvas(self.viewport, (f64::from(x), f64::from(y)));
                    MenuTarget::Canvas {
                        x: clamp_coord(cx),
                        y: clamp_coord(cy),
                    }
                }
            },
        };
        self.open_menu(target, (x, y), false);
        true
    }

    /// The wheel pans; Shift turns a vertical wheel into a horizontal pan.
    pub fn wheel(&mut self, delta: MouseScrollDelta, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        let (mut dx, mut dy) = match delta {
            MouseScrollDelta::LineDelta(a, b) => (f64::from(a) * PAN_STEP, f64::from(b) * PAN_STEP),
            MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
        };
        if self.mods.shift_key() && dx == 0.0 {
            (dx, dy) = (dy, 0.0);
        }
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        self.menu = None;
        self.camera.pan_view(dx, dy);
        self.after_camera_moved();
        true
    }

    /// Ctrl+wheel or a pinch: zoom around the pointer **by how far it
    /// moved**, not by a fixed step per event — a touchpad sends many small
    /// events and a wheel a few large ones.
    pub fn zoom_wheel(&mut self, delta: MouseScrollDelta, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        let amount = match delta {
            MouseScrollDelta::LineDelta(a, b) => f64::from(if b != 0.0 { b } else { a }) * 40.0,
            MouseScrollDelta::PixelDelta(p) => {
                if p.y != 0.0 {
                    p.y
                } else {
                    p.x
                }
            }
        };
        if amount == 0.0 || !amount.is_finite() {
            return false;
        }
        let point = self
            .camera
            .to_canvas(self.viewport, (f64::from(x), f64::from(y)));
        self.zoom_by(point, (amount * 0.0035).exp())
    }

    /// Ctrl+= and Ctrl+- step the zoom around the middle of the view;
    /// Ctrl+0 goes back to 100 %, and a second Ctrl+0 at 100 % fits
    /// everything in view.
    pub fn zoom_key(&mut self, step: i32) -> bool {
        let center = (
            f64::from(self.viewport.0) * 0.5,
            f64::from(self.viewport.1) * 0.5,
        );
        let point = self.camera.to_canvas(self.viewport, center);
        match step.signum() {
            1 => self.zoom_by(point, 1.25),
            -1 => self.zoom_by(point, 0.8),
            _ if (self.camera.zoom - 1.0).abs() > 1e-9 => {
                self.zoom_by(point, 1.0 / self.camera.zoom)
            }
            _ => {
                let fitted = match self.content_bounds() {
                    Some(bounds) => Camera::fit(bounds, self.viewport),
                    None => Camera::default(),
                };
                let changed = fitted != self.camera;
                self.camera = fitted;
                self.show_readout();
                self.after_camera_moved();
                changed
            }
        }
    }

    fn zoom_by(&mut self, point: (f64, f64), factor: f64) -> bool {
        let before = self.camera;
        self.camera.zoom_at(point, factor);
        self.show_readout();
        self.after_camera_moved();
        self.camera != before
    }

    fn show_readout(&mut self) {
        self.readout_until = Some(Instant::now() + Duration::from_millis(READOUT_MS));
    }

    fn after_camera_moved(&mut self) {
        match &mut self.gesture {
            Gesture::Move { .. } | Gesture::Resize { .. } => self.drag_update(),
            Gesture::Idle => self.refresh_hover(),
            // A wheel or a zoom in the middle of a pan: the pan carries on
            // from here, rather than the next move undoing what the wheel
            // just did.
            Gesture::Pan { from, at } => {
                *from = self.camera;
                if let Some(pointer) = self.pointer {
                    *at = pointer;
                }
            }
            _ => {}
        }
    }

    /// Everything on the canvas, from bounds alone.
    fn content_bounds(&self) -> Option<vello::kurbo::Rect> {
        (0..self.items.len())
            .map(|i| self.bound(i))
            .reduce(|a, b| a.union(b))
    }

    // ----------------------------------------------------------- keyboard

    pub fn set_mods(&mut self, mods: ModifiersState) {
        self.mods = mods;
    }

    pub fn key(&mut self, key: &Key, mods: ModifiersState) -> bool {
        self.mods = mods;
        if self.menu.is_some() {
            return self.menu_key(key);
        }
        let Key::Named(named) = key else {
            return false;
        };
        let plain = !mods.control_key() && !mods.alt_key() && !mods.super_key();
        match named {
            NamedKey::Escape => self.escape(),
            NamedKey::Delete | NamedKey::Backspace if plain => {
                if self.selection.is_empty() {
                    return false;
                }
                self.delete_selection();
                true
            }
            NamedKey::Enter if plain => {
                if let Some(pick) = self.connect_pick {
                    if let Some(to) = pick.candidate {
                        self.connect_pick = None;
                        self.emit(CanvasRequest::Connect {
                            from: pick.from,
                            to,
                        });
                    }
                    return true;
                }
                match self.selection.last() {
                    Some(&key) => self.emit(CanvasRequest::EditText { key }),
                    None => {
                        let (x, y) = self.view_centre();
                        self.emit(CanvasRequest::AddObject {
                            kind: ObjectKind::Card,
                            x,
                            y,
                        });
                    }
                }
                true
            }
            // Ctrl+I arrives as Tab on Windows; a chord is not ours.
            NamedKey::Tab if plain => {
                self.step(!mods.shift_key());
                true
            }
            NamedKey::ArrowLeft
            | NamedKey::ArrowRight
            | NamedKey::ArrowUp
            | NamedKey::ArrowDown
                if plain =>
            {
                self.arrow_key(*named, mods.shift_key())
            }
            NamedKey::ContextMenu => self.keyboard_menu(),
            NamedKey::F10 if mods.shift_key() => self.keyboard_menu(),
            _ => false,
        }
    }

    /// A key coming up. What ends a nudge: the last arrow key released is
    /// when its one `Move` per object is sent.
    pub fn key_up(&mut self, key: &Key) -> bool {
        let Key::Named(named) = key else {
            return false;
        };
        let Some(nudge) = &mut self.nudge else {
            return false;
        };
        let before = nudge.held.len();
        nudge.held.retain(|held| held != named);
        if nudge.held.len() == before {
            return false;
        }
        if nudge.held.is_empty() {
            self.commit_nudge();
        }
        true
    }

    /// The window lost the keyboard: whatever was in flight is dropped, and
    /// nothing is written for it.
    pub fn focus_lost(&mut self) {
        self.cancel_gesture();
        self.nudge = None;
        self.menu = None;
        self.mods = ModifiersState::empty();
    }

    fn escape(&mut self) -> bool {
        if !matches!(self.gesture, Gesture::Idle) {
            self.cancel_gesture();
            return true;
        }
        if self.connect_pick.take().is_some() {
            return true;
        }
        if self.nudge.take().is_some() {
            return true;
        }
        if !self.selection.is_empty() {
            self.set_selection(Vec::new());
            return true;
        }
        false
    }

    /// Drop a gesture in flight without sending anything. A pan puts the
    /// camera back where it started.
    fn cancel_gesture(&mut self) {
        if let Gesture::Pan { from, .. } = self.gesture {
            self.camera = from;
        }
        self.gesture = Gesture::Idle;
    }

    fn delete_selection(&mut self) {
        let keys = self.selection_in_stacking_order();
        self.nudge = None;
        self.set_selection(Vec::new());
        self.emit(CanvasRequest::Delete { keys });
    }

    /// The view's middle, in canvas coordinates.
    pub(crate) fn view_centre(&self) -> (i32, i32) {
        let (x, y) = self.camera.to_canvas(
            self.viewport,
            (
                f64::from(self.viewport.0) * 0.5,
                f64::from(self.viewport.1) * 0.5,
            ),
        );
        (clamp_coord(x), clamp_coord(y))
    }

    /// Tab and Shift+Tab: the next or previous object in reading order, and
    /// the view follows. While an arrow is waiting for its target, they walk
    /// the candidates instead.
    fn step(&mut self, forward: bool) {
        let keys: Vec<ObjectKey> = self
            .reading_order()
            .into_iter()
            .map(|i| self.items[i].object.key)
            .collect();
        let pick = self.connect_pick;
        let (keys, current) = match pick {
            Some(pick) => {
                let keys: Vec<ObjectKey> = keys.into_iter().filter(|k| *k != pick.from).collect();
                let current = pick
                    .candidate
                    .and_then(|c| keys.iter().position(|k| *k == c));
                (keys, current)
            }
            None => {
                let current = self
                    .selection
                    .last()
                    .and_then(|s| keys.iter().position(|k| k == s));
                (keys, current)
            }
        };
        let n = keys.len();
        if n == 0 {
            return;
        }
        let next = match current {
            Some(p) if forward => (p + 1) % n,
            Some(p) => (p + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        let key = keys[next];
        match pick {
            Some(pick) => {
                self.connect_pick = Some(ConnectPick {
                    from: pick.from,
                    candidate: Some(key),
                });
            }
            None => self.set_selection(vec![key]),
        }
        self.reveal(key);
    }

    /// An arrow key: nudge the selection a pixel (Shift: ten) as a live
    /// preview, or pan when nothing is selected.
    fn arrow_key(&mut self, named: NamedKey, shift: bool) -> bool {
        let (dx, dy) = match named {
            NamedKey::ArrowLeft => (-1, 0),
            NamedKey::ArrowRight => (1, 0),
            NamedKey::ArrowUp => (0, -1),
            _ => (0, 1),
        };
        if self.selection.is_empty() {
            // The view goes the way the arrow points, so what was there
            // comes into view: the content moves the other way.
            self.camera
                .pan_view(-f64::from(dx) * PAN_STEP, -f64::from(dy) * PAN_STEP);
            self.after_camera_moved();
            return true;
        }
        if !matches!(self.gesture, Gesture::Idle) {
            return true;
        }
        let step = if shift { 10 } else { 1 };
        let keys = self.selection_in_stacking_order();
        let nudge = self.nudge.get_or_insert_with(|| Nudge {
            keys,
            delta: (0, 0),
            held: Vec::new(),
        });
        nudge.delta = (
            nudge.delta.0.saturating_add(dx * step),
            nudge.delta.1.saturating_add(dy * step),
        );
        if !nudge.held.contains(&named) {
            nudge.held.push(named);
        }
        true
    }

    /// Send a nudge that is still a preview: one `Move` per object.
    pub(crate) fn commit_nudge(&mut self) {
        let Some(nudge) = self.nudge.take() else {
            return;
        };
        if nudge.delta != (0, 0) {
            self.send_moves(&nudge.keys, nudge.delta);
        }
    }

    /// One `Move` per object, from where it rests plus `delta`, and the new
    /// place kept on screen until the container answers.
    fn send_moves(&mut self, keys: &[ObjectKey], delta: (i32, i32)) {
        for &key in keys {
            let Some(i) = self.key_index(key) else {
                continue;
            };
            let (x, y) = self.item_rest(i);
            let at = (
                x.saturating_add(delta.0)
                    .clamp(-layout::COORD_LIMIT, layout::COORD_LIMIT),
                y.saturating_add(delta.1)
                    .clamp(-layout::COORD_LIMIT, layout::COORD_LIMIT),
            );
            self.emit(CanvasRequest::Move {
                key,
                x: at.0,
                y: at.1,
            });
            self.pending.entry(key).or_default().at = Some(at);
        }
    }

    fn selection_in_stacking_order(&self) -> Vec<ObjectKey> {
        let mut keys: Vec<(usize, ObjectKey)> = self
            .selection
            .iter()
            .filter_map(|key| self.key_index(*key).map(|i| (i, *key)))
            .collect();
        keys.sort_unstable();
        keys.into_iter().map(|(_, key)| key).collect()
    }

    /// Follow the pointer with whatever is being dragged.
    fn drag_update(&mut self) {
        let Some((x, y)) = self.pointer else {
            return;
        };
        let canvas = self
            .camera
            .to_canvas(self.viewport, (f64::from(x), f64::from(y)));
        match &mut self.gesture {
            Gesture::Move { grab, delta, .. } => {
                *delta = (
                    clamp_coord(canvas.0 - grab.0),
                    clamp_coord(canvas.1 - grab.1),
                );
            }
            Gesture::Resize {
                key, grab, start, ..
            } => {
                let (key, grab, start) = (*key, *grab, *start);
                let Some(i) = self.key_index(key) else {
                    return;
                };
                let object = &self.items[i].object;
                let dx = canvas.0 - grab.0;
                let wanted = if object.kind == ObjectKind::Image {
                    // A corner keeps the shape: follow whichever axis the
                    // pointer has gone further along.
                    let h0 = f64::from(layout::image_height(object, start));
                    let aspect = f64::from(start) / h0.max(1.0);
                    let by_x = f64::from(start) + dx;
                    let by_y = (h0 + canvas.1 - grab.1) * aspect;
                    by_x.max(by_y)
                } else {
                    f64::from(start) + dx
                };
                let w = layout::clamp_width(object.kind, clamp_coord(wanted));
                if let Gesture::Resize { w: current, .. } = &mut self.gesture {
                    *current = w;
                }
            }
            _ => {}
        }
    }

    /// What pressing a button or the empty state's **Add card** does.
    fn press(&mut self, hit: Hit) {
        match hit {
            Hit::EmptyAdd => {
                let (x, y) = self.view_centre();
                self.emit(CanvasRequest::AddObject {
                    kind: ObjectKind::Card,
                    x,
                    y,
                });
            }
            Hit::Button(key, part) => self.emit(part.request(key)),
            _ => {}
        }
    }

    // --------------------------------------------------------------- menu

    fn open_menu(&mut self, target: MenuTarget, anchor: (f32, f32), from_keyboard: bool) {
        let rows = match target {
            MenuTarget::Canvas { .. } => menu::canvas_rows(self.lang, self.workflows),
            MenuTarget::Object(key) => match self.key_index(key) {
                Some(i) => menu::object_rows(self.lang, &self.items[i].object, self.workflows),
                None => return,
            },
            MenuTarget::Arrow(_) => menu::arrow_rows(self.lang),
        };
        let enabled = vec![true; rows.len()];
        let mut menu = koru_ui::Menu::new(&mut self.engine, anchor, rows);
        if from_keyboard {
            // Opened without a pointer, the first row is where the keyboard
            // starts, so Enter has something to choose.
            menu.step(&enabled, 1);
        }
        self.menu = Some(OpenMenu { menu, target });
    }

    /// Shift+F10 or the Menu key: the selection's menu, or the canvas's with
    /// nothing selected.
    fn keyboard_menu(&mut self) -> bool {
        self.cancel_gesture();
        self.commit_nudge();
        let (vw, vh) = self.viewport;
        match self.selection.last().copied() {
            Some(key) => {
                let Some(i) = self.key_index(key) else {
                    return false;
                };
                let view = {
                    let r = self.rect(i);
                    self.to_view_rect(r)
                };
                let anchor = (
                    (view.x0 as f32 + 8.0).clamp(4.0, (vw - 4.0).max(4.0)),
                    (view.y0 as f32 + 8.0).clamp(4.0, (vh - 4.0).max(4.0)),
                );
                self.open_menu(MenuTarget::Object(key), anchor, true);
            }
            None => {
                let (x, y) = self.view_centre();
                self.open_menu(MenuTarget::Canvas { x, y }, (vw * 0.5, vh * 0.5), true);
            }
        }
        true
    }

    fn menu_key(&mut self, key: &Key) -> bool {
        let Some(open) = &mut self.menu else {
            return false;
        };
        let enabled = vec![true; open.menu.items().len()];
        match key {
            Key::Named(NamedKey::Escape) => self.menu = None,
            Key::Named(NamedKey::ArrowDown) => open.menu.step(&enabled, 1),
            Key::Named(NamedKey::ArrowUp) => open.menu.step(&enabled, -1),
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                if let Some(row) = open.menu.hover {
                    self.choose(row);
                }
            }
            // A menu holds the keyboard while it is open.
            _ => {}
        }
        true
    }

    /// Do what menu row `row` says, and close the menu.
    fn choose(&mut self, row: usize) {
        let Some(open) = self.menu.take() else {
            return;
        };
        let Some(action) = open.menu.items().get(row).map(|row| row.action) else {
            return;
        };
        match (action, open.target) {
            (Action::Add(kind), MenuTarget::Canvas { x, y }) => {
                self.emit(CanvasRequest::AddObject { kind, x, y })
            }
            (Action::InsertImage, MenuTarget::Canvas { x, y }) => {
                self.emit(CanvasRequest::InsertImage { x, y })
            }
            (Action::Edit, MenuTarget::Object(key)) => self.emit(CanvasRequest::EditText { key }),
            (Action::Run, MenuTarget::Object(key)) => self.emit(CanvasRequest::Run { key }),
            (Action::Stop, MenuTarget::Object(key)) => self.emit(CanvasRequest::Stop { key }),
            (Action::Respond, MenuTarget::Object(key)) => self.emit(CanvasRequest::Respond { key }),
            (Action::ChooseAgent, MenuTarget::Object(key)) => {
                self.emit(CanvasRequest::ChooseAgent { key })
            }
            (Action::ConnectTo, MenuTarget::Object(from)) => {
                self.connect_pick = Some(ConnectPick {
                    from,
                    candidate: None,
                });
            }
            (Action::BringToFront, MenuTarget::Object(key)) => {
                self.emit(CanvasRequest::BringToFront { key })
            }
            (Action::SendToBack, MenuTarget::Object(key)) => {
                self.emit(CanvasRequest::SendToBack { key })
            }
            (Action::Delete, MenuTarget::Object(key)) => {
                if self.selection.contains(&key) {
                    self.delete_selection();
                } else {
                    self.emit(CanvasRequest::Delete { keys: vec![key] });
                }
            }
            (Action::DeleteArrow, MenuTarget::Arrow(key)) => {
                self.emit(CanvasRequest::DeleteArrow { key })
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------- state

    /// Whether a gesture holds the pointer: a pan, a drag, a resize or an
    /// arrow being drawn.
    pub fn is_dragging(&self) -> bool {
        matches!(
            self.gesture,
            Gesture::Pan { .. }
                | Gesture::Move { .. }
                | Gesture::Resize { .. }
                | Gesture::Connect { .. }
        )
    }

    /// The pointer's shape (DESIGN, *Koru does not use the hand cursor*):
    /// the arrow on empty space and on every button; the grab hand over
    /// something that can be picked up, and the grabbing hand while it — or
    /// the canvas — is held; the resize arrows on a width handle.
    pub fn cursor_icon(&self) -> CursorIcon {
        let resize = |key: ObjectKey| match self.key_index(key).map(|i| self.items[i].object.kind) {
            Some(ObjectKind::Image) => CursorIcon::NwseResize,
            _ => CursorIcon::EwResize,
        };
        match &self.gesture {
            Gesture::Pan { .. }
            | Gesture::Move { .. }
            | Gesture::Connect { .. }
            | Gesture::Press { .. } => return CursorIcon::Grabbing,
            Gesture::Resize { key, .. } => return resize(*key),
            Gesture::Button { .. } => return koru_ui::cursor::PRESS,
            Gesture::Idle => {}
        }
        if let Some(open) = &self.menu {
            return match open.menu.hover {
                Some(_) => koru_ui::cursor::PRESS,
                None => koru_ui::cursor::IDLE,
            };
        }
        match self.hover {
            Hit::Object(_) | Hit::Connect(_) if self.connect_pick.is_none() => CursorIcon::Grab,
            Hit::Resize(key) if self.connect_pick.is_none() => resize(key),
            Hit::Button(..) | Hit::EmptyAdd => koru_ui::cursor::PRESS,
            _ => koru_ui::cursor::IDLE,
        }
    }

    /// Whether the host should keep drawing frames: a running node is on
    /// screen and its mark turns, or the zoom readout is up and has yet to
    /// be taken down. False whenever nothing on screen moves.
    pub fn animating(&self) -> bool {
        self.spinning || self.readout_until.is_some()
    }
}
