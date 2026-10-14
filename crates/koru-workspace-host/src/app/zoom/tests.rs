// SPDX-License-Identifier: MPL-2.0

use super::*;
use winit::dpi::PhysicalPosition;

/// A view that may or may not claim zoom, and records what it was offered.
#[derive(Default)]
struct Probe {
    claims: bool,
    wheels: Vec<(MouseScrollDelta, f32, f32)>,
    keys: Vec<ZoomIntent>,
}

impl View for Probe {
    fn zoom_wheel(&mut self, delta: MouseScrollDelta, x: f32, y: f32) -> bool {
        self.wheels.push((delta, x, y));
        self.claims
    }
    fn zoom_key(&mut self, intent: ZoomIntent) -> bool {
        self.keys.push(intent);
        self.claims
    }
    // The rest of the trait is what every view must answer; a probe needs none
    // of it.
    fn key(
        &mut self,
        _ws: &mut koru_core::schema::WorkspaceDoc,
        _key: &winit::keyboard::Key,
        _text: Option<&str>,
        _mods: winit::keyboard::ModifiersState,
    ) -> bool {
        false
    }
    fn mouse_down(
        &mut self,
        _ws: &mut koru_core::schema::WorkspaceDoc,
        _x: f32,
        _y: f32,
        _ctrl: bool,
    ) -> bool {
        false
    }
    fn mouse_move(&mut self, _x: f32, _y: f32) -> bool {
        false
    }
    fn mouse_up(&mut self, _ws: &mut koru_core::schema::WorkspaceDoc) -> bool {
        false
    }
    fn mouse_double_click(
        &mut self,
        _ws: &koru_core::schema::WorkspaceDoc,
        _x: f32,
        _y: f32,
    ) -> bool {
        false
    }
    fn scroll_by(&mut self, _delta: MouseScrollDelta) {}
    fn set_viewport(&mut self, _w: f32, _h: f32) {}
    fn adopt(&mut self, _ws: &koru_core::schema::WorkspaceDoc) {}
    fn reseat(&mut self, _ws: &koru_core::schema::WorkspaceDoc) {}
    fn invalidate(&mut self) {}
    fn draw(
        &mut self,
        _scene: &mut vello::Scene,
        _workspace: &koru_workspace::Workspace,
        _images: &koru_draw::ImageCache,
        _root: vello::kurbo::Affine,
        _caret_on: bool,
        _pointer: (f32, f32),
    ) {
    }
}

/// A pinch on a precision touchpad: a fractional pixel delta, which the old
/// arm reduced to its sign.
fn pinch() -> MouseScrollDelta {
    MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 7.25))
}

/// **A view that claims the wheel gets the raw delta, and the app's zoom does
/// not move.** The delta arrives whole — a canvas zooms by how far the pinch
/// travelled, not by one step per event — and the pointer arrives in the
/// view's own coordinates, display zoom divided out, the space `mouse_move`
/// is handed.
#[test]
fn a_claiming_view_receives_the_raw_delta_and_the_display_zoom_stays() {
    let mut canvas = Probe {
        claims: true,
        ..Probe::default()
    };
    assert_eq!(
        primary_wheel(&mut canvas, pinch(), (300.0, 150.0), 1.5),
        None,
        "no display zoom for a claimed gesture"
    );
    assert_eq!(canvas.wheels, vec![(pinch(), 200.0, 100.0)]);

    let line = MouseScrollDelta::LineDelta(0.0, -2.5);
    assert_eq!(primary_wheel(&mut canvas, line, (10.0, 20.0), 1.0), None);
    assert_eq!(canvas.wheels.last(), Some(&(line, 10.0, 20.0)));
}

/// **A view that does not claim it leaves the display zoom working** — one
/// step per event, in the wheel's direction, exactly as before the seam.
#[test]
fn a_declining_view_leaves_the_display_zoom_working() {
    let mut page = Probe::default();
    let up = MouseScrollDelta::LineDelta(0.0, 3.0);
    assert_eq!(
        primary_wheel(&mut page, up, (0.0, 0.0), 1.0),
        Some(1.0 + theme::ZOOM_STEP)
    );
    let down = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -40.0));
    let next = primary_wheel(&mut page, down, (0.0, 0.0), 1.2).expect("one step out");
    assert!((next - (1.2 - theme::ZOOM_STEP)).abs() < 1e-6, "{next}");
    assert_eq!(page.wheels.len(), 2, "it was asked first, both times");
    // A sideways-only wheel moves nothing vertical, so nothing zooms.
    let sideways = MouseScrollDelta::LineDelta(1.0, 0.0);
    assert_eq!(primary_wheel(&mut page, sideways, (0.0, 0.0), 1.0), None);
}

/// The chords follow the same rule: offered to the view first, the display
/// zoom's only on a refusal.
#[test]
fn the_zoom_chords_are_offered_to_the_view_first() {
    let mut canvas = Probe {
        claims: true,
        ..Probe::default()
    };
    for intent in [ZoomIntent::In, ZoomIntent::Out, ZoomIntent::ActualSize] {
        assert_eq!(zoom_chord(&mut canvas, intent, 1.3), None);
    }
    assert_eq!(
        canvas.keys,
        vec![ZoomIntent::In, ZoomIntent::Out, ZoomIntent::ActualSize]
    );

    let mut page = Probe::default();
    assert_eq!(
        zoom_chord(&mut page, ZoomIntent::In, 1.3),
        Some(1.3 + theme::ZOOM_STEP)
    );
    let out = zoom_chord(&mut page, ZoomIntent::Out, 1.3).expect("one step out");
    assert!((out - (1.3 - theme::ZOOM_STEP)).abs() < 1e-6, "{out}");
    assert_eq!(
        zoom_chord(&mut page, ZoomIntent::ActualSize, 1.3),
        Some(1.0)
    );
}

/// The three keys the chords are, and nothing else — `+` is superscript.
#[test]
fn only_equals_minus_and_zero_are_zoom_chords() {
    assert_eq!(chord_intent(Some('=')), Some(ZoomIntent::In));
    assert_eq!(chord_intent(Some('-')), Some(ZoomIntent::Out));
    assert_eq!(chord_intent(Some('0')), Some(ZoomIntent::ActualSize));
    assert_eq!(chord_intent(Some('+')), None);
    assert_eq!(chord_intent(Some('a')), None);
    assert_eq!(chord_intent(None), None);
}

/// **No change for any view that says nothing.** The trait's defaults decline
/// both, which is what keeps every existing view zooming the app.
#[test]
fn the_defaults_decline_both() {
    struct Silent;
    impl View for Silent {
        fn key(
            &mut self,
            _ws: &mut koru_core::schema::WorkspaceDoc,
            _key: &winit::keyboard::Key,
            _text: Option<&str>,
            _mods: winit::keyboard::ModifiersState,
        ) -> bool {
            false
        }
        fn mouse_down(
            &mut self,
            _ws: &mut koru_core::schema::WorkspaceDoc,
            _x: f32,
            _y: f32,
            _ctrl: bool,
        ) -> bool {
            false
        }
        fn mouse_move(&mut self, _x: f32, _y: f32) -> bool {
            false
        }
        fn mouse_up(&mut self, _ws: &mut koru_core::schema::WorkspaceDoc) -> bool {
            false
        }
        fn mouse_double_click(
            &mut self,
            _ws: &koru_core::schema::WorkspaceDoc,
            _x: f32,
            _y: f32,
        ) -> bool {
            false
        }
        fn scroll_by(&mut self, _delta: MouseScrollDelta) {}
        fn set_viewport(&mut self, _w: f32, _h: f32) {}
        fn adopt(&mut self, _ws: &koru_core::schema::WorkspaceDoc) {}
        fn reseat(&mut self, _ws: &koru_core::schema::WorkspaceDoc) {}
        fn invalidate(&mut self) {}
        fn draw(
            &mut self,
            _scene: &mut vello::Scene,
            _workspace: &koru_workspace::Workspace,
            _images: &koru_draw::ImageCache,
            _root: vello::kurbo::Affine,
            _caret_on: bool,
            _pointer: (f32, f32),
        ) {
        }
    }
    let mut view = Silent;
    assert!(!view.zoom_wheel(pinch(), 0.0, 0.0));
    assert!(!view.zoom_key(ZoomIntent::In));
    // The release seam (`key_release.rs`) declines the same way, and writes
    // nothing: a view that does not end a gesture on a release is untouched.
    let mut ws =
        koru_core::schema::WorkspaceDoc::create(koru_core::WorkspaceId::new(), "t").unwrap();
    let before = ws.version();
    assert!(!view.key_up(
        &mut ws,
        &winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowLeft)
    ));
    assert_eq!(ws.version(), before);
    assert_eq!(
        primary_wheel(
            &mut view,
            MouseScrollDelta::LineDelta(0.0, 1.0),
            (0.0, 0.0),
            1.0
        ),
        Some(1.0 + theme::ZOOM_STEP)
    );
}
