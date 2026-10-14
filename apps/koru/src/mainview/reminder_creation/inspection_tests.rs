// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_draw::inspect;

const VIEWPORT: (f32, f32) = (900.0, 600.0);

fn property<'a>(r: &'a inspect::Record, key: &str) -> &'a str {
    &r.properties.iter().find(|p| p.name == key).unwrap().value
}

#[test]
fn editor_capture_reuses_opening_identity_and_cached_geometry_for_both_modes() {
    let mut engine = TextEngine::new();
    let mut surface = Surface::default();
    let anchor = Rect::new(300.0, 80.0, 10.0, 20.0);
    let request = Request {
        identity: RequestIdentity {
            token: RequestToken::new(),
            node: NodeId::new(),
        },
        phrase: "private synthetic phrase".into(),
        due_anchor: None,
    };
    let creation = Creation::loading(request);
    surface.sync_creation(&mut engine, Some(&creation), Some(anchor));
    let first_instance = surface.instance();
    let management = Management::loading("private target".into());
    for mode in [Mode::Creation, Mode::Management] {
        if mode == Mode::Management {
            surface.open_management(NodeId::new(), anchor);
            assert_ne!(surface.instance(), first_instance);
        }
        surface.maintain_projection(
            &mut engine,
            VIEWPORT,
            Lang::En,
            (mode == Mode::Creation).then_some(&creation),
            (mode == Mode::Management).then_some(&management),
            &[],
        );
        let old_geometry = surface.geometry() as *const Geometry;
        let old_targets = surface.geometry().targets.as_ptr();
        let old_key = surface.geometry_key.clone();
        let body_clip = surface.geometry().body_clip;
        let mut off = Scene::new();
        let mut on = Scene::new();
        surface.draw(&mut off, Affine::IDENTITY, &mut engine, Lang::En);
        let frame = inspect::Frame::begin(
            1,
            Affine::IDENTITY,
            scene::rect(0.0, 0.0, VIEWPORT.0, VIEWPORT.1),
        );
        surface.draw(&mut on, Affine::IDENTITY, &mut engine, Lang::En);
        inspect::leaf(
            "sibling",
            Affine::IDENTITY,
            scene::rect(0.0, 0.0, 5.0, 5.0),
            |_| {},
        );
        let snapshot = frame.finish();
        let (a, b) = (off.encoding(), on.encoding());
        assert!(a.path_tags == b.path_tags);
        assert_eq!(a.path_data, b.path_data);
        assert!(a.draw_tags == b.draw_tags);
        assert_eq!(a.draw_data, b.draw_data);
        assert_eq!(a.transforms, b.transforms);
        assert_eq!(a.styles, b.styles);
        let popover = &snapshot.records[0];
        assert_eq!(popover.name, "KoruPopover");
        assert_eq!(
            property(popover, "occurrence"),
            surface.instance().to_string()
        );
        assert_eq!(property(popover, "placement"), "anchored Start");
        assert_eq!(property(popover, "trigger occurrence"), "unavailable");
        assert_eq!(property(popover, "state"), "Open");
        assert!(
            snapshot
                .records
                .iter()
                .any(|r| r.name == "Reminder editor content"
                    && r.kind == "unregistered"
                    && r.parent == Some(0))
        );
        if let Some(clip) = body_clip {
            let expected = inspect::array(scene::rect(clip.x, clip.y, clip.w, clip.h));
            assert!(
                snapshot
                    .records
                    .iter()
                    .any(|r| r.kind == "primitive" && r.clip == expected)
            );
        }
        assert_eq!(snapshot.records.last().unwrap().parent, None);
        assert_eq!(
            snapshot.records.last().unwrap().clip,
            [0.0, 0.0, 900.0, 600.0]
        );
        assert_eq!(surface.geometry() as *const Geometry, old_geometry);
        assert_eq!(surface.geometry().targets.as_ptr(), old_targets);
        assert_eq!(surface.geometry_key, old_key);
        assert!(!snapshot.export(None).unwrap().contains("private"));
    }
}
