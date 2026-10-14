// SPDX-License-Identifier: MPL-2.0

//! The surface driven the way a host drives it: view-local points in,
//! requests out. No GPU — a `vello::Scene` is an encoding in memory — and no
//! disk.

use super::*;
use crate::layout::Part;
use koru_draw::ImageCache;
use vello::kurbo::Affine;
use winit::event::MouseScrollDelta;
use winit::keyboard::Key as WinitKey;
use winit::window::CursorIcon;

const VIEW: (f32, f32) = (1000.0, 800.0);

fn key(n: u128) -> ObjectKey {
    ObjectKey(uuid::Uuid::from_u128(n))
}

fn card(n: u128, x: i32, y: i32, text: &str) -> SceneObject {
    SceneObject::new(key(n), ObjectKind::Card, x, y, 240, text)
}

fn surface(objects: Vec<SceneObject>) -> CanvasSurface {
    surface_with(Scene {
        objects,
        ..Scene::default()
    })
}

fn surface_with(scene: Scene) -> CanvasSurface {
    let mut s = CanvasSurface::new(Lang::En);
    s.set_viewport(VIEW.0, VIEW.1);
    s.set_scene(scene);
    s
}

/// An object's rectangle in view pixels.
fn view_rect(s: &mut CanvasSurface, k: ObjectKey) -> Rect {
    let i = s.key_index(k).expect("on the canvas");
    let r = s.rect(i);
    s.to_view_rect(r)
}

/// A point on an object's left edge, halfway down: its body, clear of every
/// button and handle.
fn body_point(s: &mut CanvasSurface, k: ObjectKey) -> (f32, f32) {
    let v = view_rect(s, k);
    ((v.x0 + 4.0) as f32, ((v.y0 + v.y1) * 0.5) as f32)
}

fn named(k: NamedKey) -> WinitKey {
    WinitKey::Named(k)
}

fn press(s: &mut CanvasSurface, k: NamedKey) -> bool {
    s.key(&named(k), ModifiersState::empty())
}

fn shift() -> ModifiersState {
    ModifiersState::SHIFT
}

/// A drag in a few steps, the way a real pointer arrives.
fn drag(s: &mut CanvasSurface, from: (f32, f32), to: (f32, f32)) {
    s.mouse_down(from.0, from.1);
    for step in 1..=5 {
        let t = step as f32 / 5.0;
        s.mouse_move(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
    }
}

fn click(s: &mut CanvasSurface, at: (f32, f32)) {
    s.mouse_down(at.0, at.1);
    s.mouse_up();
}

/// What the host does for a double-click: two presses and releases, with
/// `double_click` after the second press.
fn double_click(s: &mut CanvasSurface, at: (f32, f32)) {
    s.mouse_down(at.0, at.1);
    s.mouse_up();
    s.mouse_down(at.0, at.1);
    s.double_click(at.0, at.1);
    s.mouse_up();
}

fn draw(s: &mut CanvasSurface) {
    let mut scene = vello::Scene::new();
    s.draw(&mut scene, Affine::IDENTITY, &ImageCache::new());
}

// ------------------------------------------------------------- gestures

#[test]
fn a_drag_sends_exactly_one_move_on_release() {
    let mut s = surface(vec![card(1, -200, -100, "Move me")]);
    let from = body_point(&mut s, key(1));
    drag(&mut s, from, (from.0 + 50.0, from.1 + 30.0));
    assert!(s.is_dragging());
    assert!(
        s.take_requests().is_empty(),
        "nothing is written while dragging"
    );
    // The preview follows the pointer.
    let i = s.key_index(key(1)).unwrap();
    assert_eq!(s.item_pos(i), (-150, -70));
    s.mouse_up();
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Move {
            key: key(1),
            x: -150,
            y: -70
        }]
    );
    // Until the container answers, the card stays where it was dropped.
    assert_eq!(s.item_pos(i), (-150, -70));
    assert!(!s.is_dragging());
}

#[test]
fn a_drag_at_another_zoom_moves_by_canvas_pixels() {
    let mut s = surface(vec![card(1, 0, 0, "Zoomed")]);
    s.set_camera(Camera::new(100.0, 50.0, 2.0));
    let from = body_point(&mut s, key(1));
    drag(&mut s, from, (from.0 + 40.0, from.1 - 20.0));
    s.mouse_up();
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Move {
            key: key(1),
            x: 20,
            y: -10
        }]
    );
}

#[test]
fn a_click_without_a_drag_sends_nothing_and_selects() {
    let mut s = surface(vec![card(1, 0, 0, "Click me")]);
    let at = body_point(&mut s, key(1));
    click(&mut s, at);
    assert!(s.take_requests().is_empty());
    assert_eq!(s.selection(), &[key(1)]);
}

#[test]
fn esc_mid_drag_puts_it_back_and_sends_nothing() {
    let mut s = surface(vec![card(1, 0, 0, "Escape")]);
    let from = body_point(&mut s, key(1));
    drag(&mut s, from, (from.0 + 80.0, from.1 + 80.0));
    assert!(press(&mut s, NamedKey::Escape));
    let i = s.key_index(key(1)).unwrap();
    assert_eq!(s.item_pos(i), (0, 0));
    s.mouse_up();
    assert!(s.take_requests().is_empty());
    // The selection survives the cancelled drag; a second Esc clears it.
    assert_eq!(s.selection(), &[key(1)]);
    assert!(press(&mut s, NamedKey::Escape));
    assert!(s.selection().is_empty());
    assert!(!press(&mut s, NamedKey::Escape), "nothing left to cancel");
}

#[test]
fn losing_focus_mid_drag_or_mid_resize_sends_nothing() {
    let mut s = surface(vec![card(1, 0, 0, "Focus")]);
    let from = body_point(&mut s, key(1));
    drag(&mut s, from, (from.0 + 30.0, from.1));
    s.focus_lost();
    s.mouse_up();
    assert!(s.take_requests().is_empty());

    let v = view_rect(&mut s, key(1));
    let edge = ((v.x1 - 1.0) as f32, (v.y0 + 4.0) as f32);
    drag(&mut s, edge, (edge.0 + 60.0, edge.1));
    s.focus_lost();
    s.mouse_up();
    assert!(s.take_requests().is_empty());
    let i = s.key_index(key(1)).unwrap();
    assert_eq!(s.item_width(i), 240);
}

#[test]
fn dragging_empty_space_pans_and_esc_puts_the_camera_back() {
    let mut s = surface(vec![card(1, 0, 0, "Pan")]);
    let before = s.camera();
    drag(&mut s, (900.0, 700.0), (800.0, 650.0));
    assert!(s.is_dragging());
    assert_eq!(s.cursor_icon(), CursorIcon::Grabbing);
    let moved = s.camera();
    assert_eq!(moved.center_x, before.center_x + 100.0);
    assert_eq!(moved.center_y, before.center_y + 50.0);
    press(&mut s, NamedKey::Escape);
    assert_eq!(s.camera(), before);
    s.mouse_up();
    assert!(s.take_requests().is_empty());
}

#[test]
fn a_zoom_in_the_middle_of_a_pan_is_kept_by_the_rest_of_the_pan() {
    let mut s = surface(vec![card(1, 0, 0, "Pan and zoom")]);
    drag(&mut s, (900.0, 700.0), (880.0, 690.0));
    s.zoom_wheel(MouseScrollDelta::LineDelta(0.0, 2.0), 880.0, 690.0);
    let zoomed = s.camera();
    assert!(zoomed.zoom > 1.0);
    s.mouse_move(870.0, 690.0);
    assert_eq!(s.camera().zoom, zoomed.zoom);
    assert!((s.camera().center_x - (zoomed.center_x + 10.0 / zoomed.zoom)).abs() < 1e-9);
    s.mouse_up();
    assert!(s.take_requests().is_empty());
}

#[test]
fn double_click_on_empty_space_adds_a_card_at_that_canvas_point() {
    let mut s = surface(vec![card(1, -400, -300, "Elsewhere")]);
    s.set_camera(Camera::new(100.0, 50.0, 2.0));
    double_click(&mut s, (600.0, 500.0));
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::AddObject {
            kind: ObjectKind::Card,
            x: 150,
            y: 100
        }]
    );
    assert!(!s.is_dragging(), "the second press's pan was dropped");
}

#[test]
fn double_click_on_an_object_edits_its_text() {
    let mut s = surface(vec![card(1, 0, 0, "Edit me")]);
    let at = body_point(&mut s, key(1));
    double_click(&mut s, at);
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::EditText { key: key(1) }]
    );
}

#[test]
fn a_connect_drag_onto_another_object_connects_them() {
    let mut s = surface(vec![card(1, -400, 0, "From"), card(2, 200, 0, "To")]);
    s.select(Some(key(1)));
    let from = view_rect(&mut s, key(1));
    let handle = CanvasSurface::connect_point(from);
    let handle = (handle.x as f32, handle.y as f32);
    assert_eq!(s.hit(handle.0, handle.1), Hit::Connect(key(1)));
    let target = body_point(&mut s, key(2));
    drag(&mut s, handle, target);
    assert!(s.is_dragging());
    s.mouse_up();
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Connect {
            from: key(1),
            to: key(2)
        }]
    );
}

#[test]
fn a_connect_drag_onto_empty_space_or_itself_does_nothing() {
    let mut s = surface(vec![card(1, -400, 0, "From"), card(2, 200, 0, "To")]);
    s.select(Some(key(1)));
    let handle = {
        let v = view_rect(&mut s, key(1));
        let p = CanvasSurface::connect_point(v);
        (p.x as f32, p.y as f32)
    };
    drag(&mut s, handle, (500.0, 750.0));
    s.mouse_up();
    assert!(s.take_requests().is_empty());
    let back = body_point(&mut s, key(1));
    drag(&mut s, handle, back);
    s.mouse_up();
    assert!(s.take_requests().is_empty());
}

#[test]
fn resizing_a_card_sends_one_set_width() {
    let mut s = surface(vec![card(1, 0, 0, "Wider")]);
    let v = view_rect(&mut s, key(1));
    let edge = ((v.x1 - 1.0) as f32, (v.y0 + 4.0) as f32);
    assert_eq!(s.hit(edge.0, edge.1), Hit::Resize(key(1)));
    drag(&mut s, edge, (edge.0 + 60.0, edge.1 + 40.0));
    assert!(s.take_requests().is_empty());
    s.mouse_up();
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::SetWidth {
            key: key(1),
            w: 300
        }]
    );
}

#[test]
fn resizing_an_image_from_its_corner_keeps_its_shape() {
    let mut image = SceneObject::new(key(1), ObjectKind::Image, 0, 0, 300, "photo.png");
    image.image = Some(SceneImage {
        hash: "b3:photo".into(),
        width: 600,
        height: 400,
    });
    let mut s = surface(vec![image]);
    let v = view_rect(&mut s, key(1));
    assert_eq!(v.height(), 200.0);
    let corner = ((v.x1 - 1.0) as f32, (v.y1 - 1.0) as f32);
    assert_eq!(s.hit(corner.0, corner.1), Hit::Resize(key(1)));
    assert_eq!(s.cursor_for(corner), CursorIcon::NwseResize);
    // Mostly downwards: the height leads, and the width follows it.
    drag(&mut s, corner, (corner.0 + 10.0, corner.1 + 100.0));
    s.mouse_up();
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::SetWidth {
            key: key(1),
            w: 450
        }]
    );
    // Nothing but the width is written: the height is the aspect ratio's.
    assert_eq!(view_rect(&mut s, key(1)).height(), 300.0);
}

#[test]
fn delete_sends_the_selection_in_one_request() {
    let mut s = surface(vec![
        card(1, -400, 0, "One"),
        card(2, 0, 0, "Two"),
        card(3, 400, 0, "Three"),
    ]);
    let (a, c) = (body_point(&mut s, key(1)), body_point(&mut s, key(3)));
    click(&mut s, c);
    s.set_mods(shift());
    click(&mut s, a);
    s.set_mods(ModifiersState::empty());
    assert_eq!(s.selection(), &[key(3), key(1)]);
    assert!(press(&mut s, NamedKey::Delete));
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Delete {
            keys: vec![key(1), key(3)]
        }],
        "one request, in stacking order"
    );
    assert!(s.selection().is_empty());
    assert!(
        !press(&mut s, NamedKey::Backspace),
        "nothing selected, nothing to delete"
    );
}

#[test]
fn shift_click_toggles_and_a_click_on_empty_space_clears() {
    let mut s = surface(vec![card(1, -400, 0, "One"), card(2, 0, 0, "Two")]);
    let (a, b) = (body_point(&mut s, key(1)), body_point(&mut s, key(2)));
    click(&mut s, a);
    s.set_mods(shift());
    click(&mut s, b);
    assert_eq!(s.selection(), &[key(1), key(2)]);
    click(&mut s, a);
    assert_eq!(s.selection(), &[key(2)]);
    s.set_mods(ModifiersState::empty());
    click(&mut s, (950.0, 750.0));
    assert!(s.selection().is_empty());
}

#[test]
fn dragging_one_of_several_selected_moves_them_all() {
    let mut s = surface(vec![card(1, -400, 0, "One"), card(2, 0, 0, "Two")]);
    let (a, b) = (body_point(&mut s, key(1)), body_point(&mut s, key(2)));
    click(&mut s, a);
    s.set_mods(shift());
    click(&mut s, b);
    s.set_mods(ModifiersState::empty());
    drag(&mut s, b, (b.0 + 10.0, b.1 + 20.0));
    s.mouse_up();
    assert_eq!(
        s.take_requests(),
        vec![
            CanvasRequest::Move {
                key: key(1),
                x: -390,
                y: 20
            },
            CanvasRequest::Move {
                key: key(2),
                x: 10,
                y: 20
            },
        ]
    );
    // A click without a drag on one of them narrows the selection to it.
    let a = body_point(&mut s, key(1));
    click(&mut s, a);
    assert_eq!(s.selection(), &[key(1)]);
}

#[test]
fn enter_edits_the_selection_or_adds_a_card_in_the_middle_of_the_view() {
    let mut s = surface(vec![card(1, 0, 0, "Enter")]);
    s.set_camera(Camera::new(-50.0, 70.0, 0.5));
    assert!(press(&mut s, NamedKey::Enter));
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::AddObject {
            kind: ObjectKind::Card,
            x: -50,
            y: 70
        }]
    );
    s.select(Some(key(1)));
    assert!(press(&mut s, NamedKey::Enter));
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::EditText { key: key(1) }]
    );
}

#[test]
fn tab_walks_the_reading_order_and_the_view_follows() {
    let mut s = surface(vec![
        card(1, 0, 500, "third"),
        card(2, 400, 0, "second"),
        card(3, 0, 10, "first"),
        card(4, 3_000, 2_600, "fourth"),
    ]);
    let mut seen = Vec::new();
    for _ in 0..4 {
        assert!(press(&mut s, NamedKey::Tab));
        seen.push(*s.selection().last().unwrap());
    }
    assert_eq!(seen, vec![key(3), key(2), key(1), key(4)]);
    // The far card was brought into view.
    let v = view_rect(&mut s, key(4));
    assert!(v.x0 >= 0.0 && v.x1 <= f64::from(VIEW.0));
    assert!(v.y0 >= 0.0 && v.y1 <= f64::from(VIEW.1));
    // Shift+Tab goes back; the walk wraps.
    s.key(&named(NamedKey::Tab), shift());
    assert_eq!(s.selection(), &[key(1)]);
    press(&mut s, NamedKey::Tab);
    press(&mut s, NamedKey::Tab);
    assert_eq!(s.selection(), &[key(3)]);
    // Ctrl+Tab is somebody else's (and Ctrl+I arrives as Tab on Windows).
    assert!(!s.key(&named(NamedKey::Tab), ModifiersState::CONTROL));
}

#[test]
fn a_held_arrow_key_nudges_live_and_sends_one_move_on_release() {
    let mut s = surface(vec![card(1, 100, 100, "Nudge")]);
    s.select(Some(key(1)));
    for _ in 0..5 {
        // The OS repeats a held key as more presses.
        assert!(press(&mut s, NamedKey::ArrowRight));
    }
    assert!(s.key(&named(NamedKey::ArrowDown), shift()));
    let i = s.key_index(key(1)).unwrap();
    assert_eq!(s.item_pos(i), (105, 110), "the preview moves");
    assert!(s.take_requests().is_empty());
    assert!(s.key_up(&named(NamedKey::ArrowRight)));
    assert!(s.take_requests().is_empty(), "Down is still held");
    assert!(s.key_up(&named(NamedKey::ArrowDown)));
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Move {
            key: key(1),
            x: 105,
            y: 110
        }]
    );
    assert!(
        !s.key_up(&named(NamedKey::ArrowDown)),
        "nothing held any more"
    );
}

#[test]
fn esc_or_a_lost_focus_drops_a_held_nudge() {
    let mut s = surface(vec![card(1, 100, 100, "Nudge")]);
    s.select(Some(key(1)));
    press(&mut s, NamedKey::ArrowLeft);
    s.focus_lost();
    s.key_up(&named(NamedKey::ArrowLeft));
    assert!(s.take_requests().is_empty());
    press(&mut s, NamedKey::ArrowLeft);
    press(&mut s, NamedKey::Escape);
    s.key_up(&named(NamedKey::ArrowLeft));
    assert!(s.take_requests().is_empty());
    let i = s.key_index(key(1)).unwrap();
    assert_eq!(s.item_pos(i), (100, 100));
}

#[test]
fn arrow_keys_pan_when_nothing_is_selected() {
    let mut s = surface(vec![card(1, 0, 0, "Pan")]);
    let before = s.camera();
    assert!(press(&mut s, NamedKey::ArrowRight));
    assert!(s.camera().center_x > before.center_x, "the view goes right");
    assert!(!s.key_up(&named(NamedKey::ArrowRight)));
    assert!(s.take_requests().is_empty());
}

#[test]
fn the_wheel_pans_and_ctrl_wheel_zooms_around_the_pointer() {
    let mut s = surface(vec![card(1, 0, 0, "Wheel")]);
    let before = s.camera();
    assert!(s.wheel(MouseScrollDelta::LineDelta(0.0, -1.0), 500.0, 400.0));
    assert_eq!(s.camera().center_y, before.center_y + PAN_STEP);
    s.set_mods(shift());
    s.wheel(MouseScrollDelta::LineDelta(0.0, -1.0), 500.0, 400.0);
    assert_eq!(
        s.camera().center_x,
        before.center_x + PAN_STEP,
        "Shift turns it sideways"
    );
    s.set_mods(ModifiersState::empty());

    let pointer = (730.0, 210.0);
    let under = s.camera().to_canvas(VIEW, (730.0, 210.0));
    assert!(s.zoom_wheel(MouseScrollDelta::LineDelta(0.0, 3.0), pointer.0, pointer.1));
    assert!(s.camera().zoom > 1.0);
    let after = s.camera().to_view(VIEW, under);
    assert!((after.0 - 730.0).abs() < 1e-6 && (after.1 - 210.0).abs() < 1e-6);
    // By magnitude: a bigger movement zooms further.
    let mut small = surface(vec![]);
    let mut large = surface(vec![]);
    small.zoom_wheel(
        MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, 10.0)),
        0.0,
        0.0,
    );
    large.zoom_wheel(
        MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, 100.0)),
        0.0,
        0.0,
    );
    assert!(large.camera().zoom > small.camera().zoom);
    // Clamped at both ends.
    for _ in 0..100 {
        large.zoom_wheel(MouseScrollDelta::LineDelta(0.0, 10.0), 0.0, 0.0);
    }
    assert_eq!(large.camera().zoom, Camera::MAX_ZOOM);
    for _ in 0..100 {
        large.zoom_wheel(MouseScrollDelta::LineDelta(0.0, -10.0), 0.0, 0.0);
    }
    assert_eq!(large.camera().zoom, Camera::MIN_ZOOM);
}

#[test]
fn zoom_keys_step_reset_and_fit() {
    let mut s = surface(vec![card(1, 0, 0, "A"), card(2, 5_000, 3_000, "B")]);
    assert!(s.zoom_key(1));
    assert_eq!(s.camera().zoom, 1.25);
    assert!(s.zoom_key(-1));
    assert!((s.camera().zoom - 1.0).abs() < 1e-9);
    s.zoom_key(-1);
    assert!(s.zoom_key(0));
    assert!((s.camera().zoom - 1.0).abs() < 1e-9, "Ctrl+0 is 100 %");
    assert!(s.zoom_key(0), "and again at 100 % fits everything");
    let seen = s.camera().visible(VIEW);
    assert!(seen.x0 <= 0.0 && seen.x1 >= 5_240.0);
    assert!(s.animating(), "the readout is up");
}

// ----------------------------------------------------------------- menus

/// Open a menu by right-click and choose row `row` by clicking it.
fn choose_row(s: &mut CanvasSurface, at: (f32, f32), row: usize) -> Vec<CanvasRequest> {
    assert!(s.context_menu(at.0, at.1));
    let r = s
        .menu
        .as_ref()
        .expect("a menu opened")
        .menu
        .item_rect(s.viewport, row);
    let (x, y) = r.centre();
    s.mouse_down(x, y);
    s.mouse_up();
    assert!(s.menu.is_none(), "choosing a row closes the menu");
    s.take_requests()
}

fn menu_labels(s: &CanvasSurface) -> Vec<String> {
    s.menu
        .as_ref()
        .expect("a menu")
        .menu
        .items()
        .iter()
        .map(|row| koru_ui::MenuItem::label(row).into_owned())
        .collect()
}

#[test]
fn the_canvas_menu_adds_what_it_says_where_it_was_opened() {
    let mut s = surface(vec![card(1, -450, -350, "Corner")]);
    s.set_camera(Camera::new(10.0, 20.0, 1.0));
    let at = (700.0, 500.0);
    let point = (210, 120);
    assert!(s.context_menu(at.0, at.1));
    assert_eq!(
        menu_labels(&s),
        vec![
            "Add card",
            "Add start node",
            "Add agent node",
            "Add output node",
            "Insert image…"
        ]
    );
    s.menu = None;
    let expected = [
        CanvasRequest::AddObject {
            kind: ObjectKind::Card,
            x: point.0,
            y: point.1,
        },
        CanvasRequest::AddObject {
            kind: ObjectKind::Start,
            x: point.0,
            y: point.1,
        },
        CanvasRequest::AddObject {
            kind: ObjectKind::Agent,
            x: point.0,
            y: point.1,
        },
        CanvasRequest::AddObject {
            kind: ObjectKind::Output,
            x: point.0,
            y: point.1,
        },
        CanvasRequest::InsertImage {
            x: point.0,
            y: point.1,
        },
    ];
    for (row, want) in expected.into_iter().enumerate() {
        assert_eq!(choose_row(&mut s, at, row), vec![want], "row {row}");
    }
}

#[test]
fn an_agent_nodes_menu_rows_send_their_requests() {
    let agent = SceneObject::new(key(7), ObjectKind::Agent, -100, -100, 260, "Summarise");
    let mut s = surface(vec![agent]);
    let at = body_point(&mut s, key(7));
    assert!(s.context_menu(at.0, at.1));
    assert_eq!(
        s.selection(),
        &[key(7)],
        "a right-click selects what it is over"
    );
    assert_eq!(
        menu_labels(&s),
        vec![
            "Edit",
            "Run",
            "Choose agent…",
            "Connect to…",
            "Bring to front",
            "Send to back",
            "Delete"
        ]
    );
    s.menu = None;
    let k = key(7);
    let expected = [
        (0, vec![CanvasRequest::EditText { key: k }]),
        (1, vec![CanvasRequest::Run { key: k }]),
        (2, vec![CanvasRequest::ChooseAgent { key: k }]),
        (4, vec![CanvasRequest::BringToFront { key: k }]),
        (5, vec![CanvasRequest::SendToBack { key: k }]),
        (6, vec![CanvasRequest::Delete { keys: vec![k] }]),
    ];
    for (row, want) in expected {
        assert_eq!(choose_row(&mut s, at, row), want, "row {row}");
    }
    // Connect to… sends nothing yet: it waits for the target.
    assert!(choose_row(&mut s, at, 3).is_empty());
    assert!(s.connect_pick.is_some());
}

#[test]
fn a_running_nodes_menu_offers_stop() {
    let mut start = SceneObject::new(key(8), ObjectKind::Start, 0, 0, 240, "Begin");
    start.status = Some(NodeStatus::Running {
        activity: "Starting".into(),
        elapsed: "0:03".into(),
    });
    let mut s = surface(vec![start]);
    let at = body_point(&mut s, key(8));
    s.context_menu(at.0, at.1);
    assert_eq!(menu_labels(&s)[1], "Stop");
    s.menu = None;
    assert_eq!(
        choose_row(&mut s, at, 1),
        vec![CanvasRequest::Stop { key: key(8) }]
    );
}

#[test]
fn an_arrows_menu_deletes_the_arrow() {
    let mut s = surface_with(Scene {
        objects: vec![card(1, -400, 0, "A"), card(2, 200, 0, "B")],
        arrows: vec![SceneArrow {
            key: key(90),
            from: key(1),
            to: key(2),
            label: String::new(),
        }],
        ..Scene::default()
    });
    let (p0, p1) = arrow_view_segment(&mut s, 0);
    let at = (((p0.x + p1.x) * 0.5) as f32, ((p0.y + p1.y) * 0.5) as f32);
    assert_eq!(s.hit(at.0, at.1), Hit::Arrow(key(90)));
    assert_eq!(
        choose_row(&mut s, at, 0),
        vec![CanvasRequest::DeleteArrow { key: key(90) }]
    );
}

#[test]
fn the_keyboard_opens_menus_and_chooses_from_them() {
    let mut s = surface(vec![card(1, 0, 0, "Keys")]);
    // Nothing selected: the canvas's menu, first row ready for Enter.
    assert!(press(&mut s, NamedKey::ContextMenu));
    assert!(press(&mut s, NamedKey::Enter));
    let (x, y) = s.view_centre();
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::AddObject {
            kind: ObjectKind::Card,
            x,
            y
        }]
    );
    // Shift+F10 with a selection: its menu; Down twice to Bring to front.
    s.select(Some(key(1)));
    assert!(s.key(&named(NamedKey::F10), shift()));
    press(&mut s, NamedKey::ArrowDown);
    press(&mut s, NamedKey::ArrowDown);
    press(&mut s, NamedKey::Enter);
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::BringToFront { key: key(1) }]
    );
    // Esc closes a menu and does nothing else.
    press(&mut s, NamedKey::ContextMenu);
    assert!(press(&mut s, NamedKey::Escape));
    assert!(s.menu.is_none());
    assert_eq!(s.selection(), &[key(1)]);
    assert!(s.take_requests().is_empty());
}

#[test]
fn connect_to_picks_its_target_by_click_or_by_tab_and_enter() {
    let mut s = surface(vec![
        card(1, -400, 0, "From"),
        card(2, 0, 0, "Middle"),
        card(3, 400, 0, "Last"),
    ]);
    // A card's rows: Edit, Connect to…, Bring to front, Send to back, Delete.
    let from = body_point(&mut s, key(1));
    assert!(choose_row(&mut s, from, 1).is_empty());
    let target = body_point(&mut s, key(3));
    click(&mut s, target);
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Connect {
            from: key(1),
            to: key(3)
        }]
    );
    // By keyboard: Tab skips the source; Enter confirms.
    choose_row(&mut s, from, 1);
    press(&mut s, NamedKey::Tab);
    press(&mut s, NamedKey::Enter);
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Connect {
            from: key(1),
            to: key(2)
        }]
    );
    // Esc leaves the mode without an arrow; a click on empty space too.
    choose_row(&mut s, from, 1);
    press(&mut s, NamedKey::Escape);
    assert!(s.connect_pick.is_none());
    choose_row(&mut s, from, 1);
    click(&mut s, (980.0, 780.0));
    assert!(s.connect_pick.is_none());
    assert!(s.take_requests().is_empty());
}

// --------------------------------------------------- buttons and the empty state

#[test]
fn a_waiting_agent_offers_response_by_pointer_and_keyboard_and_keeps_stop() {
    let mut agent = SceneObject::new(key(5), ObjectKind::Agent, 0, 0, 260, "Do it");
    agent.status = Some(NodeStatus::NeedsResponse);
    let mut s = surface(vec![agent.clone()]);
    let respond = button_point(&mut s, key(5), Part::Respond);
    click(&mut s, respond);
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Respond { key: key(5) }]
    );
    let stop = button_point(&mut s, key(5), Part::Stop);
    click(&mut s, stop);
    assert_eq!(s.take_requests(), vec![CanvasRequest::Stop { key: key(5) }]);

    s.select(Some(key(5)));
    press(&mut s, NamedKey::ContextMenu);
    press(&mut s, NamedKey::ArrowDown); // Stop
    press(&mut s, NamedKey::ArrowDown); // Respond
    press(&mut s, NamedKey::Enter);
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Respond { key: key(5) }]
    );

    let index = s.key_index(key(5)).unwrap();
    let actual = s.ensure_layout(index).h;
    assert!(layout::height_bound(&agent, agent.w) >= actual);
    agent.status = Some(NodeStatus::Running {
        activity: String::new(),
        elapsed: String::new(),
    });
    s.set_scene(Scene {
        objects: vec![agent],
        ..Default::default()
    });
    assert!(
        !s.ensure_layout(0)
            .buttons
            .iter()
            .any(|button| button.part == Part::Respond)
    );
}

#[test]
fn a_nodes_buttons_act_on_release_and_a_double_click_runs_once() {
    let mut agent = SceneObject::new(key(5), ObjectKind::Agent, -100, -100, 260, "Do it");
    agent.result = vec!["one".into(), "two".into()];
    let mut output = SceneObject::new(key(6), ObjectKind::Output, 300, -100, 240, "Page");
    output.link = Some("Page — written".into());
    let mut s = surface(vec![agent, output]);
    let run = button_point(&mut s, key(5), Part::Run);
    s.mouse_down(run.0, run.1);
    assert!(s.take_requests().is_empty(), "a press alone does nothing");
    s.mouse_up();
    assert_eq!(s.take_requests(), vec![CanvasRequest::Run { key: key(5) }]);
    // Sliding off before the release takes it back.
    s.mouse_down(run.0, run.1);
    s.mouse_move(900.0, 780.0);
    s.mouse_up();
    assert!(s.take_requests().is_empty());
    // A double-click on Run is one Run, and is not an edit.
    double_click(&mut s, run);
    assert_eq!(s.take_requests(), vec![CanvasRequest::Run { key: key(5) }]);
    for (k, part, want) in [
        (
            key(5),
            Part::ChooseAgent,
            CanvasRequest::ChooseAgent { key: key(5) },
        ),
        (
            key(5),
            Part::PlaceResult,
            CanvasRequest::PlaceResult { key: key(5) },
        ),
        (
            key(6),
            Part::OpenLink,
            CanvasRequest::OpenLink { key: key(6) },
        ),
    ] {
        let at = button_point(&mut s, k, part);
        click(&mut s, at);
        assert_eq!(s.take_requests(), vec![want]);
    }
}

/// **Without the workflow half** (the host's build hides agents): the
/// canvas menu adds cards and images only, a node's menu has no Run, Stop,
/// Respond or agent choice, and its face has no such button — while the node
/// itself still draws, named, with its result's *Place on canvas*.
#[test]
fn without_workflows_nodes_draw_but_offer_nothing_that_runs() {
    let mut agent = SceneObject::new(key(5), ObjectKind::Agent, 0, 0, 260, "Do it");
    agent.status = Some(NodeStatus::NeedsResponse);
    agent.agent = Some("Claude".into());
    agent.result = vec!["one".into()];
    let mut s = surface(vec![agent]);
    s.set_workflows(false);
    assert!(!s.workflows());
    let index = s.key_index(key(5)).unwrap();
    let parts: Vec<Part> = s
        .ensure_layout(index)
        .buttons
        .iter()
        .map(|button| button.part)
        .collect();
    assert_eq!(parts, [Part::PlaceResult]);
    let at = body_point(&mut s, key(5));
    assert!(s.context_menu(at.0, at.1));
    assert_eq!(
        menu_labels(&s),
        vec![
            "Edit",
            "Connect to…",
            "Bring to front",
            "Send to back",
            "Delete"
        ]
    );
    s.menu = None;
    assert!(s.context_menu(900.0, 700.0));
    assert_eq!(menu_labels(&s), vec!["Add card", "Insert image…"]);
    s.menu = None;
    // Back on, the buttons return without a new scene.
    s.set_workflows(true);
    assert!(
        s.ensure_layout(index)
            .buttons
            .iter()
            .any(|button| button.part == Part::Respond)
    );
}

#[test]
fn the_empty_canvas_offers_add_card_and_nothing_else() {
    let mut s = surface(vec![]);
    let empty = s.empty_state();
    let (x, y) = empty.button.centre();
    assert_eq!(s.hit(x, y), Hit::EmptyAdd);
    click(&mut s, (x, y));
    let (cx, cy) = s.view_centre();
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::AddObject {
            kind: ObjectKind::Card,
            x: cx,
            y: cy
        }]
    );
    draw(&mut s);
    // With anything on the canvas, the button is gone.
    s.set_scene(Scene {
        objects: vec![card(1, 2_000, 2_000, "Far away")],
        ..Scene::default()
    });
    assert_ne!(s.hit(x, y), Hit::EmptyAdd);
}

// --------------------------------------------------------- hit testing

fn button_point(s: &mut CanvasSurface, k: ObjectKey, part: Part) -> (f32, f32) {
    let i = s.key_index(k).unwrap();
    let rect = s.rect(i);
    let r = s.layouts[&k]
        .button(part)
        .expect("the button is laid out")
        .rect;
    let (cx, cy) = r.centre();
    let v = s.camera.to_view(
        s.viewport,
        (rect.x0 + f64::from(cx), rect.y0 + f64::from(cy)),
    );
    (v.0 as f32, v.1 as f32)
}

fn arrow_view_segment(s: &mut CanvasSurface, a: usize) -> (Point, Point) {
    let (p0, p1) = s.arrow_segment(a).expect("the arrow is drawn");
    let v0 = s.camera.to_view(s.viewport, (p0.x, p0.y));
    let v1 = s.camera.to_view(s.viewport, (p1.x, p1.y));
    (Point::new(v0.0, v0.1), Point::new(v1.0, v1.1))
}

/// **Every hit region hit-tests back to itself** (DESIGN, *Rules a view
/// follows*): each object's body, each of its buttons, its resize handle, its
/// connect handle, and every arrow and label — at three zooms.
#[test]
fn every_object_handle_and_arrow_hit_tests_back_to_itself() {
    let mut image = SceneObject::new(key(4), ObjectKind::Image, 700, -300, 200, "photo");
    image.image = Some(SceneImage {
        hash: "b3:i".into(),
        width: 400,
        height: 300,
    });
    let mut agent = SceneObject::new(key(2), ObjectKind::Agent, -100, -300, 280, "Summarise");
    agent.agent = Some("Claude".into());
    agent.status = Some(NodeStatus::Done);
    agent.result = vec!["A result".into()];
    let mut output = SceneObject::new(key(3), ObjectKind::Output, 300, -300, 240, "Weekly");
    output.link = Some("Weekly — written".into());
    output.status = Some(NodeStatus::NotStarted);
    let start = SceneObject::new(key(1), ObjectKind::Start, -500, -300, 240, "Kick off");
    let note = card(5, -500, 300, "A plain card");
    let scene = Scene {
        objects: vec![start, agent, output, image, note],
        arrivals: vec![card(6, 0, 0, "Arrived from the phone")],
        arrows: vec![
            SceneArrow {
                key: key(90),
                from: key(1),
                to: key(5),
                label: "then".into(),
            },
            SceneArrow {
                key: key(91),
                from: key(5),
                to: key(3),
                label: String::new(),
            },
        ],
    };
    for zoom in [0.5, 1.0, 2.0] {
        let mut s = surface_with(scene.clone());
        s.set_viewport(3000.0, 2400.0);
        s.set_camera(Camera::new(200.0, 0.0, zoom));
        let keys: Vec<ObjectKey> = s.items.iter().map(|item| item.object.key).collect();
        for &k in &keys {
            let at = body_point(&mut s, k);
            assert_eq!(s.hit(at.0, at.1), Hit::Object(k), "body of {k:?} at {zoom}");
            let parts: Vec<Part> = s.layouts[&k].buttons.iter().map(|b| b.part).collect();
            for part in parts {
                let at = button_point(&mut s, k, part);
                assert_eq!(s.hit(at.0, at.1), Hit::Button(k, part), "{part:?} of {k:?}");
            }
            let v = view_rect(&mut s, k);
            let i = s.key_index(k).unwrap();
            let resize = if s.items[i].object.kind == ObjectKind::Image {
                ((v.x1 - 1.0) as f32, (v.y1 - 1.0) as f32)
            } else {
                ((v.x1 - 1.0) as f32, (v.y0 + 4.0) as f32)
            };
            assert_eq!(s.hit(resize.0, resize.1), Hit::Resize(k), "resize of {k:?}");
            // The connect handle exists while the object is selected.
            s.select(Some(k));
            let c = CanvasSurface::connect_point(v);
            assert_eq!(
                s.hit(c.x as f32, c.y as f32),
                Hit::Connect(k),
                "connect of {k:?}"
            );
            s.select(None);
        }
        for a in 0..s.arrows.len() {
            let k = s.arrows[a].key;
            let (v0, v1) = arrow_view_segment(&mut s, a);
            let quarter = v0.lerp(v1, 0.25);
            assert_eq!(
                s.hit(quarter.x as f32, quarter.y as f32),
                Hit::Arrow(k),
                "arrow {k:?} at {zoom}"
            );
            let (p0, p1) = s.arrow_segment(a).unwrap();
            if let Some(chip) = s.label_chip(a, p0.midpoint(p1)) {
                let c = s
                    .camera
                    .to_view(s.viewport, (chip.center().x, chip.center().y));
                assert_eq!(
                    s.hit(c.0 as f32, c.1 as f32),
                    Hit::Arrow(k),
                    "label of {k:?}"
                );
            }
        }
        // Everything drawn is what was hit: one draw pass, same geometry.
        draw(&mut s);
        assert_eq!(s.counters().objects_drawn, keys.len() as u64);
    }
}

#[test]
fn handles_keep_their_screen_size_at_every_zoom() {
    let mut s = surface(vec![card(1, 0, 0, "Handles")]);
    for zoom in [0.1, 1.0, 4.0] {
        s.set_camera(Camera::new(120.0, 20.0, zoom));
        let v = view_rect(&mut s, key(1));
        let y = ((v.y0 + v.y1) * 0.5 - 20.0 * zoom.min(1.0)) as f32;
        // Four screen pixels outside the edge still takes it, at any zoom.
        assert_eq!(
            s.hit((v.x1 + 4.0) as f32, y),
            Hit::Resize(key(1)),
            "at {zoom}"
        );
        assert_ne!(
            s.hit((v.x1 + 7.0) as f32, y),
            Hit::Resize(key(1)),
            "at {zoom}"
        );
    }
}

#[test]
fn the_topmost_object_wins_where_two_overlap() {
    let mut s = surface(vec![card(1, 0, 0, "Under"), card(2, 20, 5, "Over")]);
    let at = body_point(&mut s, key(2));
    assert_eq!(s.hit(at.0, at.1), Hit::Object(key(2)));
}

// ------------------------------------------------------------- culling

/// **A frame costs what is on screen.** Two thousand cards, ten in view:
/// ten laid out, ten drawn — and a pan or a zoom lays no text out again.
#[test]
fn drawing_two_thousand_objects_with_ten_in_view_lays_out_and_draws_ten() {
    let mut objects = Vec::new();
    for row in 0..40 {
        for col in 0..50 {
            let n = (row * 50 + col) as u128;
            objects.push(card(n, col * 300, row * 250, &format!("Card {n}")));
        }
    }
    assert_eq!(objects.len(), 2_000);
    let mut s = CanvasSurface::new(Lang::En);
    s.set_viewport(1450.0, 450.0);
    s.set_scene(Scene {
        objects,
        ..Scene::default()
    });
    // x from -10 to 1440, y from -10 to 440: five columns, two rows.
    s.set_camera(Camera::new(715.0, 215.0, 1.0));
    draw(&mut s);
    let c = s.counters();
    assert_eq!(c.objects_drawn, 10);
    assert_eq!(c.objects_laid, 10, "nothing off screen was laid out");
    assert_eq!(c.texts_laid, 10);

    // Zooming in shows a part of what is already laid out: no text again.
    s.zoom_wheel(MouseScrollDelta::LineDelta(0.0, 2.0), 700.0, 200.0);
    draw(&mut s);
    assert_eq!(
        s.counters().texts_laid,
        10,
        "zoom only changes the transform"
    );
    assert!(s.counters().objects_drawn <= 10);
    // A small pan within the laid-out region lays out nothing either.
    s.set_camera(Camera::new(715.0, 215.0, 1.0));
    s.wheel(
        MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(5.0, 3.0)),
        0.0,
        0.0,
    );
    draw(&mut s);
    assert_eq!(
        s.counters().texts_laid,
        10,
        "pan only changes the transform"
    );
    // Hit testing asks the same bounds: a point in view touches no layout
    // off screen.
    s.hit(700.0, 200.0);
    assert_eq!(s.counters().objects_laid, 10);
}

// ------------------------------------------------ scene, arrivals, a11y

#[test]
fn a_new_scene_keeps_the_selection_and_camera_by_key() {
    let mut s = surface(vec![card(1, 0, 0, "Stay"), card(2, 300, 0, "Go")]);
    s.select(Some(key(1)));
    s.set_camera(Camera::new(40.0, 30.0, 1.5));
    s.set_scene(Scene {
        objects: vec![card(2, 300, 0, "Go"), card(1, 10, 10, "Stay, moved")],
        ..Scene::default()
    });
    assert_eq!(s.selection(), &[key(1)]);
    assert_eq!(s.camera(), Camera::new(40.0, 30.0, 1.5));
    s.set_scene(Scene {
        objects: vec![card(2, 300, 0, "Go")],
        ..Scene::default()
    });
    assert!(
        s.selection().is_empty(),
        "a deleted object is no longer selected"
    );
}

#[test]
fn a_drag_whose_object_is_deleted_underneath_it_just_ends() {
    let mut s = surface(vec![card(1, 0, 0, "Doomed")]);
    let from = body_point(&mut s, key(1));
    drag(&mut s, from, (from.0 + 40.0, from.1));
    s.set_scene(Scene::default());
    assert!(!s.is_dragging());
    s.mouse_up();
    assert!(s.take_requests().is_empty());
}

#[test]
fn arrivals_wait_in_a_column_right_of_the_content() {
    let mut s = surface_with(Scene {
        objects: vec![card(1, -100, 50, "Left"), card(2, 400, 120, "Right")],
        arrivals: vec![card(3, 0, 0, "First"), card(4, 0, 0, "Second")],
        ..Scene::default()
    });
    let (a, b) = (s.key_index(key(3)).unwrap(), s.key_index(key(4)).unwrap());
    assert_eq!(s.item_pos(a), (400 + 240 + ARRIVAL_GAP, 50));
    let first_h = s.rect(a).height() as i32;
    assert!(s.item_pos(b).1 >= 50 + first_h + ARRIVAL_GAP - 1);
    assert_eq!(s.item_pos(b).0, s.item_pos(a).0);
    // An arrival is an object like any other: dragging it gives it a place.
    let at = body_point(&mut s, key(3));
    drag(&mut s, at, (at.0 + 10.0, at.1 + 10.0));
    s.mouse_up();
    assert_eq!(
        s.take_requests(),
        vec![CanvasRequest::Move {
            key: key(3),
            x: 674,
            y: 60
        }]
    );
}

#[test]
fn semantics_list_every_object_in_reading_order_with_a_label() {
    let mut agent = SceneObject::new(key(2), ObjectKind::Agent, 400, 0, 260, "Summarise");
    agent.status = Some(NodeStatus::Failed("no network".into()));
    let mut image = SceneObject::new(key(3), ObjectKind::Image, 0, 500, 200, "diagram.png");
    image.image = None;
    let mut s = surface_with(Scene {
        objects: vec![agent, image, card(1, 0, 10, "Hello")],
        arrivals: vec![card(4, 0, 0, "")],
        ..Scene::default()
    });
    s.select(Some(key(2)));
    let semantics = s.semantics();
    let labels: Vec<&str> = semantics.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "Card: Hello",
            "Agent: Summarise (Failed: no network)",
            "Image: diagram.png",
            "Card"
        ]
    );
    assert_eq!(
        semantics.iter().map(|o| o.is_selected).collect::<Vec<_>>(),
        vec![false, true, false, false]
    );
    let first = &semantics[0];
    let v = view_rect(&mut s, key(1));
    assert_eq!(first.bounds.x, v.x0 as f32);
    assert_eq!(first.bounds.w, v.width() as f32);
    let before = s.revision();
    s.select(Some(key(1)));
    assert!(s.revision() > before, "a new selection is a new revision");
    let before = s.revision();
    s.wheel(MouseScrollDelta::LineDelta(0.0, 1.0), 0.0, 0.0);
    assert_eq!(s.revision(), before, "a pan is not");
}

#[test]
fn labels_are_localized() {
    let mut s = surface(vec![SceneObject::new(
        key(1),
        ObjectKind::Output,
        0,
        0,
        240,
        "Bericht",
    )]);
    s.set_lang(Lang::De);
    assert_eq!(s.semantics()[0].label, "Ausgabe: Bericht");
    s.set_lang(Lang::Fr);
    assert_eq!(s.semantics()[0].label, "Sortie : Bericht");
}

// ----------------------------------------------------- cursor and motion

impl CanvasSurface {
    /// Move there and ask the cursor, as the host does.
    fn cursor_for(&mut self, at: (f32, f32)) -> CursorIcon {
        self.mouse_move(at.0, at.1);
        self.cursor_icon()
    }
}

#[test]
fn cursors_say_what_the_pointer_can_do_and_never_show_the_hand() {
    let mut s = surface(vec![SceneObject::new(
        key(1),
        ObjectKind::Start,
        0,
        0,
        240,
        "Go",
    )]);
    let body = body_point(&mut s, key(1));
    assert_eq!(s.cursor_for(body), CursorIcon::Grab);
    let run = button_point(&mut s, key(1), Part::Run);
    assert_eq!(s.cursor_for(run), koru_ui::cursor::PRESS);
    let v = view_rect(&mut s, key(1));
    let edge = ((v.x1 - 1.0) as f32, (v.y0 + 4.0) as f32);
    assert_eq!(s.cursor_for(edge), CursorIcon::EwResize);
    assert_eq!(s.cursor_for((990.0, 790.0)), CursorIcon::Default);
    s.mouse_down(990.0, 790.0);
    assert_eq!(s.cursor_icon(), CursorIcon::Grabbing);
    s.mouse_up();
    for at in [body, run, edge, (990.0, 790.0)] {
        assert_ne!(s.cursor_for(at), CursorIcon::Pointer);
    }
}

#[test]
fn only_a_running_node_on_screen_animates() {
    let mut agent = SceneObject::new(key(1), ObjectKind::Agent, 0, 0, 260, "Work");
    let mut s = surface(vec![agent.clone()]);
    draw(&mut s);
    assert!(!s.animating(), "idle is still");
    agent.status = Some(NodeStatus::Running {
        activity: "Thinking".into(),
        elapsed: "0:05".into(),
    });
    s.set_scene(Scene {
        objects: vec![agent.clone()],
        ..Scene::default()
    });
    draw(&mut s);
    assert!(s.animating());
    // Off screen, it does not ask for frames.
    s.set_camera(Camera::new(50_000.0, 50_000.0, 1.0));
    draw(&mut s);
    assert!(!s.animating());
    s.set_camera(Camera::default());
    agent.status = Some(NodeStatus::Done);
    s.set_scene(Scene {
        objects: vec![agent],
        ..Scene::default()
    });
    draw(&mut s);
    assert!(!s.animating());
}

#[test]
fn the_zoom_readout_comes_down_on_the_frame_after_its_deadline() {
    let mut s = surface(vec![card(1, 0, 0, "Zoom")]);
    s.zoom_key(1);
    assert!(s.animating());
    s.readout_until = Some(Instant::now() - std::time::Duration::from_millis(1));
    draw(&mut s);
    assert!(!s.animating());
}
