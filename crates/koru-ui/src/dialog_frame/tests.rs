// SPDX-License-Identifier: MPL-2.0

use super::*;

// Pre-extraction host recipe, deliberately literal: a before-adoption fixture
// that can detect a future change to the shared resolver or painter.
fn legacy(viewport: (f32, f32), canvas: &mut Scene, root: Affine) -> Rect {
    let panel = Rect::new(
        ((viewport.0 - 320.0) * 0.5).max(8.0),
        ((viewport.1 - 84.0) * 0.5).max(8.0),
        320.0_f32.min((viewport.0 - 16.0).max(1.0)),
        84.0,
    );
    scene::fill(
        canvas,
        root,
        theme::p().scrim,
        &scene::rect(0.0, 0.0, viewport.0, viewport.1),
    );
    scene::floating_panel(
        canvas,
        root,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        &scene::Panel::popover(8.0),
    );
    panel
}

#[test]
fn legacy_baseline_and_shared_frame_have_identical_geometry_and_paint_at_edges() {
    for viewport in [
        (900.0, 650.0),
        (350.0, 300.0),
        (200.0, 120.0),
        (24.0, 20.0),
        (0.0, 0.0),
    ] {
        for scale in [1.0, 2.0] {
            let root = Affine::scale(scale);
            let mut before = Scene::new();
            let baseline_capture =
                inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, viewport.0, viewport.1));
            let old_panel = legacy(viewport, &mut before, root);
            let baseline = baseline_capture.finish();
            assert!(baseline.records.iter().all(|r| r.kind != "component"));
            let frame = KoruDialogFrame::name_prompt(viewport);
            assert_eq!(frame.panel(), old_panel);
            for capturing in [false, true] {
                let mut after = Scene::new();
                let capture = capturing.then(|| {
                    inspect::Frame::begin(2, root, scene::rect(0.0, 0.0, viewport.0, viewport.1))
                });
                let returned =
                    frame.draw(&mut after, root, Default::default(), |_, _, panel| panel);
                assert_eq!(returned, old_panel);
                if let Some(capture) = capture {
                    let snapshot = capture.finish();
                    assert_eq!(snapshot.records[0].name, "KoruDialogFrame");
                    let props = &snapshot.records[0].properties;
                    assert_eq!(
                        props.iter().find(|p| p.name == "state").unwrap().value,
                        "Unknown"
                    );
                    assert_eq!(
                        props.iter().find(|p| p.name == "occurrence").unwrap().value,
                        "unavailable"
                    );
                    assert_eq!(
                        snapshot.records[0].rect(),
                        scene::rect(old_panel.x, old_panel.y, old_panel.w, old_panel.h)
                    );
                }
                let (a, b) = (before.encoding(), after.encoding());
                assert!(a.path_tags == b.path_tags);
                assert_eq!(a.path_data, b.path_data);
                assert!(a.draw_tags == b.draw_tags);
                assert_eq!(a.draw_data, b.draw_data);
                assert_eq!(a.transforms, b.transforms);
                assert_eq!(a.styles, b.styles);
            }
        }
    }
}

#[test]
fn local_content_stays_unregistered_and_unwind_restores_parent() {
    let capture = inspect::Frame::begin(1, Affine::IDENTITY, scene::rect(0.0, 0.0, 500.0, 300.0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        KoruDialogFrame::name_prompt((500.0, 300.0)).draw(
            &mut Scene::new(),
            Affine::IDENTITY,
            Default::default(),
            |_, root, panel| {
                inspect::current(|r| r.property("callback", true, "fixture"));
                let _local = inspect::scope(
                    "Local content",
                    "unregistered",
                    root,
                    scene::rect(panel.x, panel.y, panel.w, panel.h),
                    |_| {},
                );
                inspect::leaf(
                    "local fill",
                    root,
                    scene::rect(panel.x, panel.y, 10.0, 10.0),
                    |_| {},
                );
                panic!("synthetic callback failure");
            },
        );
    }));
    assert!(result.is_err());
    inspect::leaf(
        "sibling",
        Affine::IDENTITY,
        scene::rect(0.0, 0.0, 1.0, 1.0),
        |_| {},
    );
    let snapshot = capture.finish();
    let local = snapshot
        .records
        .iter()
        .position(|r| r.name == "local fill")
        .unwrap();
    assert_eq!(snapshot.owner(local), None);
    assert_eq!(snapshot.records.last().unwrap().parent, None);
}

#[test]
fn scrim_is_identifiable_outside_the_panel_and_explicit_host_state_is_preserved() {
    let capture = inspect::Frame::begin(1, Affine::IDENTITY, scene::rect(0.0, 0.0, 900.0, 650.0));
    KoruDialogFrame::name_prompt((900.0, 650.0)).draw(
        &mut Scene::new(),
        Affine::IDENTITY,
        KoruDialogFrameInspection {
            occurrence: Some(19),
            state: KoruDialogFrameState::Closing,
        },
        |_, _, _| {},
    );
    let snapshot = capture.finish();
    let picked = snapshot.pick(2.0, 2.0).unwrap();
    assert_eq!(
        snapshot.records[snapshot.records[picked].parent.unwrap()].name,
        "Dialog scrim"
    );
    assert!(!snapshot.records[0].rect().contains((2.0, 2.0)));
    assert_eq!(
        snapshot.records[0]
            .properties
            .iter()
            .find(|p| p.name == "state")
            .unwrap()
            .value,
        "Closing"
    );
    assert_eq!(
        snapshot.records[0]
            .properties
            .iter()
            .find(|p| p.name == "occurrence")
            .unwrap()
            .value,
        "19"
    );
}
