// SPDX-License-Identifier: MPL-2.0

//! **Painting a frame** — only what is on screen, in one order:
//!
//! 1. the backdrop — the page's own backdrop colour, no grid;
//! 2. arrows, below every object: a segment edge to edge, a head, and the
//!    label on a chip at the middle;
//! 3. objects in stacking order, the newest on top, then the arrivals column;
//! 4. the selection's outline, and the handles of the selection and of the
//!    object under the pointer;
//! 5. an arrow being drawn;
//! 6. the empty state, the connect hint and the zoom readout;
//! 7. an open menu.
//!
//! Objects are drawn in canvas units under a per-object transform, so their
//! text — laid out once — scales with the zoom without being laid out again.
//! Outlines, handles and arrows are drawn in view pixels, so they keep their
//! size at every zoom.

use std::time::Instant;

use koru_draw::geometry::Rect as ViewRect;
use koru_draw::theme::{self, Brush};
use koru_draw::{ImageCache, scene};
use koru_i18n::{Key, t};
use koru_ui::button::{ActionButton, ActionStyle};
use parley::Layout;
use vello::Scene;
use vello::kurbo::{Affine, Circle, Line, Point, Rect};
use vello::peniko::Fill;

use crate::layout::{PAD, RADIUS, Tone};
use crate::model::ObjectKind;
use crate::{CONNECT_R, CanvasSurface, Gesture, Hit, arrow, overlaps, words};

/// The air inside an arrow's label chip, in canvas pixels.
pub(crate) const CHIP_PAD_X: f64 = 6.0;
pub(crate) const CHIP_PAD_Y: f64 = 3.0;

/// The empty canvas: one sentence, and the button that does what it says.
pub(crate) struct EmptyState {
    pub hint: Layout<Brush>,
    pub hint_at: (f32, f32),
    pub label: &'static str,
    pub button: ViewRect,
}

/// How many device pixels one view pixel is: the DPI and the app's display
/// zoom the host put into `root`.
fn device_scale(root: Affine) -> f64 {
    let [a, b, ..] = root.as_coeffs();
    (a * a + b * b).sqrt().max(1e-6)
}

impl CanvasSurface {
    /// The empty state's geometry — what the hit test and the painter both
    /// read.
    pub(crate) fn empty_state(&mut self) -> EmptyState {
        let (vw, vh) = self.viewport;
        let hint = self.engine.simple_layout_wrapped(
            t(self.lang, Key::CanvasEmptyHint),
            theme::role::ui(),
            (vw - 48.0).max(40.0),
        );
        let label = t(self.lang, Key::CanvasAddCard);
        let bw = ActionButton::new(label, ActionStyle::Secondary, None).width(&mut self.engine);
        let gap = 12.0;
        let top = (vh - (hint.height() + gap + ActionButton::H)) * 0.5;
        EmptyState {
            hint_at: ((vw - hint.width()) * 0.5, top),
            button: ViewRect::new(
                (vw - bw) * 0.5,
                top + hint.height() + gap,
                bw,
                ActionButton::H,
            ),
            hint,
            label,
        }
    }

    /// Paint the canvas into `scene`. `root` carries the DPI, the app's
    /// display zoom and the pane's origin; `images` is where an image
    /// object's pixels are, when they are here.
    pub fn draw(&mut self, scene: &mut Scene, root: Affine, images: &ImageCache) {
        let p = theme::p();
        let (vw, vh) = self.viewport;
        let pane = scene::rect(0.0, 0.0, vw, vh);
        // A view does not start on an empty scene: paint our own ground.
        scene::fill(scene, root, p.canvas, &pane);
        scene.push_clip_layer(Fill::NonZero, root, &pane);
        self.counters.objects_drawn = 0;
        self.spinning = false;
        let visible = self.visible();
        let device = device_scale(root);

        self.draw_arrows(scene, root, visible);
        for i in 0..self.items.len() {
            self.draw_item(scene, root, images, i, visible, device);
        }
        self.draw_decorations(scene, root);
        self.draw_connecting(scene, root);
        if self.items.is_empty() {
            self.draw_empty(scene, root);
        }
        if self.connect_pick.is_some() {
            let hint = t(self.lang, Key::CanvasConnectHint);
            self.draw_chip(scene, root, hint, 12.0, false);
        }
        match self.readout_until {
            Some(until) if Instant::now() < until => {
                let text = words::zoom_percent(self.lang, self.camera.zoom);
                self.draw_chip(scene, root, &text, vh - 12.0, true);
            }
            // The frame after the deadline draws without it, and says the
            // animation is over.
            Some(_) => self.readout_until = None,
            None => {}
        }
        scene.pop_layer();

        if let Some(open) = &self.menu {
            let enabled = vec![true; open.menu.items().len()];
            open.menu
                .draw(scene, root, &mut self.engine, self.viewport, &enabled);
        }
    }

    fn draw_arrows(&mut self, scene: &mut Scene, root: Affine, visible: Rect) {
        let p = theme::p();
        for a in 0..self.arrows.len() {
            if !self.arrow_maybe_visible(a, visible) {
                continue;
            }
            let Some((p0, p1)) = self.arrow_segment(a) else {
                continue;
            };
            let v0 = self.camera.to_view(self.viewport, (p0.x, p0.y));
            let v1 = self.camera.to_view(self.viewport, (p1.x, p1.y));
            let (v0, v1) = (Point::new(v0.0, v0.1), Point::new(v1.0, v1.1));
            draw_arrow_line(scene, root, v0, v1, p.text_dim);
            let Some(chip) = self.label_chip(a, p0.midpoint(p1)) else {
                continue;
            };
            let key = self.arrows[a].key;
            let t = root
                * self
                    .camera
                    .object_transform(self.viewport, chip.x0, chip.y0);
            let shape = scene::rrect(0.0, 0.0, chip.width() as f32, chip.height() as f32, RADIUS);
            scene::fill(scene, t, p.panel_bg, &shape);
            scene::stroke(
                scene,
                t,
                p.panel_border,
                theme::BORDER_W / self.camera.zoom,
                &shape,
            );
            if let Some((_, laid)) = self.labels.get(&key) {
                scene::draw_layout(
                    scene,
                    t,
                    laid,
                    CHIP_PAD_X as f32,
                    CHIP_PAD_Y as f32,
                    Some(p.text_dim),
                );
            }
        }
    }

    fn draw_item(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        images: &ImageCache,
        i: usize,
        visible: Rect,
        device: f64,
    ) {
        // Culled from arithmetic first: an object that cannot be on screen is
        // never laid out.
        if !overlaps(self.bound(i), visible) {
            return;
        }
        let rect = self.rect(i);
        if !overlaps(rect, visible) {
            return;
        }
        self.counters.objects_drawn += 1;
        let p = theme::p();
        let zoom = self.camera.zoom;
        let t = root
            * self
                .camera
                .object_transform(self.viewport, rect.x0, rect.y0);
        let hairline = theme::BORDER_W / zoom;
        let key = self.items[i].object.key;
        let (w, h) = (rect.width() as f32, rect.height() as f32);
        let body = scene::rrect(0.0, 0.0, w, h, RADIUS);

        if self.items[i].object.kind == ObjectKind::Image {
            let image = self.items[i].object.image.as_ref();
            let pixels = image.and_then(|image| {
                let full = images.get(&image.hash)?.clone();
                // vello samples with one tap and has no mipmaps: a picture
                // drawn small is drawn from a smaller copy (ADR-43).
                let wanted = (rect.width().max(rect.height()) * zoom * device).ceil() as u32;
                Some(images.scaled(&image.hash, wanted).unwrap_or(full))
            });
            match pixels {
                Some(brush) => {
                    let (bw, bh) = (
                        f64::from(brush.image.width.max(1)),
                        f64::from(brush.image.height.max(1)),
                    );
                    let fit = t * Affine::scale_non_uniform(rect.width() / bw, rect.height() / bh);
                    scene::draw_image(scene, fit, &brush);
                }
                None => {
                    scene::fill(scene, t, p.panel_bg, &body);
                    scene::stroke(scene, t, p.panel_border, hairline, &body);
                    if let Some(mark) = &self.layouts[&key].placeholder {
                        scene::draw_layout(
                            scene,
                            t,
                            &mark.layout,
                            mark.x,
                            mark.y,
                            Some(p.text_dim),
                        );
                    }
                }
            }
            return;
        }

        // The raised card: the sheet colour, outlined, the hover chip's
        // corners.
        scene::fill(scene, t, p.panel_bg, &body);
        scene::stroke(scene, t, p.panel_border, hairline, &body);
        let hovered = match self.hover {
            Hit::Button(k, part) if k == key => Some(part),
            _ => None,
        };
        let laid = &self.layouts[&key];
        if let Some(mark) = &laid.header_glyph {
            scene::draw_layout(scene, t, &mark.layout, mark.x, mark.y, Some(p.text_dim));
        }
        if let Some(mark) = &laid.header_label {
            scene::draw_layout(scene, t, &mark.layout, mark.x, mark.y, Some(p.text));
        }
        if let Some(block) = &laid.body {
            scene::draw_layout(scene, t, &block.layout, block.x, block.y, None);
        }
        let failed = matches!(laid.status, Some((_, Tone::Failed)));
        let ink = if failed { p.close_hover } else { p.text_dim };
        if let Some((mark, spin)) = &laid.status_glyph {
            let at = if *spin {
                // The turn is read from the clock, never counted per frame:
                // a slow frame shows the mark where it should be by now.
                self.spinning = true;
                let turn =
                    koru_draw::anim::loop_phase(self.epoch, Instant::now(), theme::PULSE_LOOP_MS);
                let centre = Point::new(
                    f64::from(mark.x + mark.layout.width() * 0.5),
                    f64::from(mark.y + mark.layout.height() * 0.5),
                );
                t * Affine::rotate_about(f64::from(turn) * std::f64::consts::TAU, centre)
            } else {
                t
            };
            scene::draw_layout(scene, at, &mark.layout, mark.x, mark.y, Some(ink));
        }
        if let Some((mark, _)) = &laid.status {
            scene::draw_layout(scene, t, &mark.layout, mark.x, mark.y, Some(ink));
        }
        if let Some(y) = laid.divider_y {
            scene::fill(
                scene,
                t,
                p.panel_border,
                &scene::rect(PAD, y, (w - PAD * 2.0).max(0.0), hairline as f32),
            );
        }
        if let Some(block) = &laid.result {
            scene::draw_layout(scene, t, &block.layout, block.x, block.y, Some(p.text_dim));
        }
        for button in &laid.buttons {
            ActionButton::new(&button.label, button.style, button.icon).draw(
                scene,
                t,
                &mut self.engine,
                button.rect,
                hovered == Some(button.part),
            );
        }
    }

    /// The selection's outline and every decorated object's handles, over
    /// everything, in view pixels.
    fn draw_decorations(&mut self, scene: &mut Scene, root: Affine) {
        let p = theme::p();
        let target = self.connect_target();
        for key in self.decorated() {
            let Some(i) = self.key_index(key) else {
                continue;
            };
            let view = {
                let r = self.rect(i);
                self.to_view_rect(r)
            };
            let kind = self.items[i].object.kind;
            let selected = self.selection.contains(&key);
            let radius = (f64::from(RADIUS) * self.camera.zoom) as f32;
            if selected {
                let ring = view.inflate(3.0, 3.0);
                scene::stroke(
                    scene,
                    root,
                    p.accent,
                    2.0,
                    &scene::rrect(
                        ring.x0 as f32,
                        ring.y0 as f32,
                        ring.width() as f32,
                        ring.height() as f32,
                        radius + 3.0,
                    ),
                );
            }
            // The width handle: an image's corner square, drawn while it is
            // selected; a card's right edge, lit while the pointer is on it.
            let on_handle = self.hover == Hit::Resize(key)
                || matches!(self.gesture, Gesture::Resize { key: k, .. } if k == key);
            if kind == ObjectKind::Image && (selected || on_handle) {
                let c = 4.0;
                let square = scene::rect(
                    (view.x1 - c) as f32,
                    (view.y1 - c) as f32,
                    (c * 2.0) as f32,
                    (c * 2.0) as f32,
                );
                scene::fill(scene, root, p.panel_bg, &square);
                scene::stroke(scene, root, p.accent, 1.5, &square);
            } else if kind != ObjectKind::Image && on_handle {
                scene::stroke(
                    scene,
                    root,
                    p.accent,
                    2.0,
                    &Line::new((view.x1, view.y0 + 2.0), (view.x1, view.y1 - 2.0)),
                );
            }
            if self.connect_pick.is_none() && !matches!(self.gesture, Gesture::Connect { .. }) {
                let at = Self::connect_point(view);
                let lit = self.hover == Hit::Connect(key);
                let dot = Circle::new(at, CONNECT_R);
                scene::fill(scene, root, if lit { p.accent } else { p.panel_bg }, &dot);
                scene::stroke(scene, root, p.accent, 1.5, &dot);
            }
        }
        if let Some(key) = target
            && let Some(i) = self.key_index(key)
        {
            let view = {
                let r = self.rect(i);
                self.to_view_rect(r)
            };
            let ring = view.inflate(2.0, 2.0);
            scene::stroke(scene, root, p.accent, 1.5, &ring);
        }
    }

    /// The object an arrow being drawn would go to, if it were let go now.
    fn connect_target(&self) -> Option<crate::ObjectKey> {
        match (&self.gesture, self.connect_pick) {
            (Gesture::Connect { from }, _) => self.hover.object().filter(|key| key != from),
            (_, Some(pick)) => pick
                .candidate
                .or_else(|| self.hover.object())
                .filter(|key| *key != pick.from),
            _ => None,
        }
    }

    /// An arrow being drawn: from the source's handle to the pointer, or to
    /// the candidate the keyboard is on.
    fn draw_connecting(&mut self, scene: &mut Scene, root: Affine) {
        let from = match (&self.gesture, self.connect_pick) {
            (Gesture::Connect { from }, _) => *from,
            (_, Some(pick)) => pick.from,
            _ => return,
        };
        let Some(i) = self.key_index(from) else {
            return;
        };
        let source = {
            let r = self.rect(i);
            self.to_view_rect(r)
        };
        let start = Self::connect_point(source);
        let end = match (
            self.connect_pick.and_then(|pick| pick.candidate),
            self.pointer,
        ) {
            (Some(candidate), _) => match self.key_index(candidate) {
                Some(j) => {
                    let r = self.rect(j);
                    self.to_view_rect(r).center()
                }
                None => return,
            },
            (None, Some((x, y))) => Point::new(f64::from(x), f64::from(y)),
            (None, None) => return,
        };
        let p = theme::p();
        if self.connect_pick.is_some() {
            scene::stroke(scene, root, p.accent, 2.0, &source.inflate(3.0, 3.0));
        }
        if (end - start).hypot() > 1.0 {
            draw_arrow_line(scene, root, start, end, p.accent);
        }
    }

    fn draw_empty(&mut self, scene: &mut Scene, root: Affine) {
        let p = theme::p();
        let empty = self.empty_state();
        scene::draw_layout(
            scene,
            root,
            &empty.hint,
            empty.hint_at.0,
            empty.hint_at.1,
            Some(p.text_dim),
        );
        let hovered = matches!(self.hover, Hit::EmptyAdd);
        ActionButton::new(empty.label, ActionStyle::Secondary, None).draw(
            scene,
            root,
            &mut self.engine,
            empty.button,
            hovered,
        );
    }

    /// A tooltip-style chip centred across the view: at the top, or with its
    /// bottom at `y` when `from_bottom`.
    fn draw_chip(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        text: &str,
        y: f32,
        from_bottom: bool,
    ) {
        let p = theme::p();
        let (vw, _) = self.viewport;
        let laid =
            self.engine
                .simple_layout_wrapped(text, theme::role::meta(), (vw - 48.0).max(40.0));
        let (pad_x, pad_y) = (10.0, 6.0);
        let (w, h) = (laid.width() + pad_x * 2.0, laid.height() + pad_y * 2.0);
        let x = (vw - w) * 0.5;
        let top = if from_bottom { y - h } else { y };
        scene::floating_panel(scene, root, x, top, w, h, &scene::Panel::tooltip(6.0));
        scene::draw_layout(
            scene,
            root,
            &laid,
            x + pad_x,
            top + pad_y,
            Some(p.bubble_fg),
        );
    }
}

/// A line with a head at `tip`, in view pixels.
fn draw_arrow_line(
    scene: &mut Scene,
    root: Affine,
    from: Point,
    tip: Point,
    color: vello::peniko::Color,
) {
    const HEAD: f64 = 10.0;
    let d = tip - from;
    let len = d.hypot();
    // Stop the line short of the tip so it does not poke through the head.
    let end = if len > HEAD {
        tip - d * (HEAD * 0.7 / len)
    } else {
        tip
    };
    scene::stroke(scene, root, color, 1.5, &Line::new(from, end));
    scene::fill(scene, root, color, &arrow::head(from, tip, HEAD, 4.5));
}
