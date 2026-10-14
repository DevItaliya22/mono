// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn anchored_placement_preserves_alignment_flip_and_edge_constraints() {
    let viewport = Rect::new(0.0, 0.0, 300.0, 200.0);
    for (anchor, align, expected) in [
        (
            Rect::new(100.0, 20.0, 20.0, 20.0),
            PopoverAlign::Start,
            (100.0, 46.0),
        ),
        (
            Rect::new(100.0, 20.0, 20.0, 20.0),
            PopoverAlign::End,
            (20.0, 46.0),
        ),
        (
            Rect::new(280.0, 180.0, 20.0, 20.0),
            PopoverAlign::End,
            (192.0, 94.0),
        ),
        (
            Rect::new(0.0, 0.0, 20.0, 20.0),
            PopoverAlign::End,
            (8.0, 26.0),
        ),
    ] {
        let shell = PopoverShell::anchored(anchor, viewport, 100.0, 80.0, align);
        assert_eq!((shell.rect.x, shell.rect.y), expected);
        assert_eq!(
            (shell.rect.w, shell.rect.h, shell.radius),
            (100.0, 80.0, 10.0)
        );
        assert!(shell.contains(shell.rect.x + 50.0, shell.rect.y + 40.0));
        assert!(!shell.contains(shell.rect.x - 1.0, shell.rect.y));
    }
}

#[test]
fn inspection_keeps_custom_radius_distinct_from_recipe_defaults() {
    use koru_draw::inspect;
    let frame = inspect::Frame::begin(
        1,
        Affine::IDENTITY,
        vello::kurbo::Rect::new(0.0, 0.0, 300.0, 200.0),
    );
    let mut canvas = Scene::new();
    PopoverShell {
        rect: Rect::new(20.0, 20.0, 100.0, 80.0),
        radius: 7.0,
    }
    .draw(&mut canvas, Affine::IDENTITY);
    let snapshot = frame.finish();
    let surface = snapshot
        .records
        .iter()
        .find(|r| r.name == "Popover surface")
        .unwrap();
    assert_eq!(surface.kind, "part");
    assert_eq!(surface.bounds, [20.0, 20.0, 120.0, 100.0]);
    let value = |key| {
        &surface
            .properties
            .iter()
            .find(|p| p.name == key)
            .unwrap()
            .value
    };
    assert_eq!(value("radius"), "7");
    assert_eq!(value("recipe radius"), "10");
    assert!(value("coverage").contains("placement origin and content ownership not exposed"));
    assert_eq!(snapshot.owner(0), None);
}

fn property<'a>(record: &'a koru_draw::inspect::Record, key: &str) -> &'a str {
    &record
        .properties
        .iter()
        .find(|p| p.name == key)
        .unwrap()
        .value
}

fn shell() -> PopoverShell {
    PopoverShell {
        rect: Rect::new(20.0, 20.0, 100.0, 80.0),
        radius: 7.0,
    }
}

#[test]
fn callback_always_runs_and_returns_its_value_without_retaining_capture_borrows() {
    use koru_draw::inspect;
    let mut canvas = Scene::new();
    let mut calls = 0;
    let result = shell().draw_with_content(
        &mut canvas,
        Affine::IDENTITY,
        Default::default(),
        |canvas, root| {
            calls += 1;
            scene::fill(
                canvas,
                root,
                theme::p().text,
                &scene::rect(30.0, 30.0, 10.0, 10.0),
            );
            42
        },
    );
    assert_eq!((calls, result), (1, 42));
    assert!(!inspect::enabled());
    let frame = inspect::Frame::begin(1, Affine::IDENTITY, scene::rect(0.0, 0.0, 300.0, 200.0));
    shell().draw_with_content(&mut canvas, Affine::IDENTITY, Default::default(), |_, _| {
        calls += 1;
        inspect::current(|r| r.property("callback evidence", true, "fixture"));
    });
    let snapshot = frame.finish();
    assert_eq!(calls, 2);
    assert_eq!(property(&snapshot.records[0], "callback evidence"), "true");
}

#[test]
fn nested_content_keeps_real_ownership_and_restores_parent_after_callback_unwinds() {
    use koru_draw::inspect;
    let mut canvas = Scene::new();
    let frame = inspect::Frame::begin(1, Affine::IDENTITY, scene::rect(0.0, 0.0, 300.0, 200.0));
    shell().draw_with_content(
        &mut canvas,
        Affine::IDENTITY,
        KoruPopoverInspection {
            occurrence: Some(12),
            ..Default::default()
        },
        |canvas, root| {
            let _local = inspect::scope(
                "Local content",
                "unregistered",
                root,
                scene::rect(20.0, 20.0, 100.0, 80.0),
                |_| {},
            );
            scene::fill(
                canvas,
                root,
                theme::p().text,
                &scene::rect(30.0, 30.0, 10.0, 10.0),
            );
            shell().draw_with_content(
                canvas,
                root,
                KoruPopoverInspection {
                    occurrence: Some(13),
                    owner: Some(12),
                    ..Default::default()
                },
                |_, _| {},
            );
        },
    );
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        shell().draw_with_content(
            &mut canvas,
            Affine::IDENTITY,
            Default::default(),
            |_, root| {
                let _clip = inspect::clip(root, scene::rect(30.0, 30.0, 10.0, 10.0));
                panic!("synthetic callback failure")
            },
        );
    }));
    assert!(failed.is_err());
    inspect::leaf(
        "after callback",
        Affine::IDENTITY,
        scene::rect(0.0, 0.0, 1.0, 1.0),
        |_| {},
    );
    let snapshot = frame.finish();
    let local = snapshot
        .records
        .iter()
        .position(|r| r.name == "Local content")
        .unwrap();
    assert_eq!(snapshot.owner(local + 1), None);
    let nested = snapshot
        .records
        .iter()
        .position(|r| r.name == "KoruPopover" && property(r, "occurrence") == "13")
        .unwrap();
    assert_eq!(snapshot.records[nested].parent, Some(local));
    assert_eq!(snapshot.owner(nested), Some(nested));
    assert_eq!(snapshot.records.last().unwrap().parent, None);
    assert_eq!(
        snapshot.records.last().unwrap().clip,
        [0.0, 0.0, 300.0, 200.0]
    );
}

#[test]
fn placement_lifecycle_and_clip_are_explicit_and_root_zoom_is_removed_once() {
    use koru_draw::inspect;
    let root = Affine::scale(2.0);
    let moving = root * Affine::translate((5.0, -3.0));
    let frame = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, 300.0, 200.0));
    {
        let _clip = inspect::clip(moving, scene::rect(25.0, 25.0, 20.0, 20.0));
        shell().draw_with_content(
            &mut Scene::new(),
            moving,
            KoruPopoverInspection {
                state: KoruPopoverState::Closing,
                placement: KoruPopoverPlacement::Anchored {
                    anchor: Rect::new(80.0, 10.0, 20.0, 20.0),
                    align: PopoverAlign::End,
                },
                ..Default::default()
            },
            |_, _| {},
        );
    }
    let snapshot = frame.finish();
    let panel = &snapshot.records[0];
    assert_eq!(panel.bounds, [25.0, 17.0, 125.0, 97.0]);
    assert_eq!(panel.visible(), scene::rect(30.0, 22.0, 20.0, 20.0));
    assert_eq!(property(panel, "radius"), "7");
    assert_eq!(property(panel, "recipe radius"), "10");
    assert_eq!(property(panel, "state"), "Closing");
    assert_eq!(property(panel, "placement"), "anchored End");
    assert_eq!(property(panel, "trigger occurrence"), "unavailable");
}

#[test]
fn content_callback_preserves_legacy_paint_encoding_and_adds_no_implicit_clip() {
    use koru_draw::inspect;
    let paint_content = |canvas: &mut Scene, root| {
        scene::fill(
            canvas,
            root,
            theme::p().text,
            &scene::rect(10.0, 10.0, 140.0, 100.0),
        )
    };
    let mut legacy = Scene::new();
    shell().draw(&mut legacy, Affine::IDENTITY);
    paint_content(&mut legacy, Affine::IDENTITY);
    for enabled in [false, true] {
        let mut current = Scene::new();
        let frame = enabled.then(|| {
            inspect::Frame::begin(1, Affine::IDENTITY, scene::rect(0.0, 0.0, 300.0, 200.0))
        });
        shell().draw_with_content(
            &mut current,
            Affine::IDENTITY,
            Default::default(),
            paint_content,
        );
        if let Some(frame) = frame {
            frame.finish();
        }
        let (a, b) = (legacy.encoding(), current.encoding());
        assert!(a.path_tags == b.path_tags);
        assert_eq!(a.path_data, b.path_data);
        assert!(a.draw_tags == b.draw_tags);
        assert_eq!(a.draw_data, b.draw_data);
        assert_eq!(a.transforms, b.transforms);
        assert_eq!(a.styles, b.styles);
    }
}
