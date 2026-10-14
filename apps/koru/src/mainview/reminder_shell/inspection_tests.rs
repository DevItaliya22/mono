// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_draw::inspect;

fn property<'a>(r: &'a inspect::Record, key: &str) -> &'a str {
    &r.properties.iter().find(|p| p.name == key).unwrap().value
}

#[test]
fn bell_capture_tracks_rise_fade_page_clip_and_restores_siblings_without_rebuilding() {
    let now = Instant::now();
    let mut shell = tests::opened_popover(now);
    let mut engine = TextEngine::new();
    shell.prepare_action_widths(&mut engine, Lang::En);
    for (page, closing) in [
        (Page::Attention, false),
        (Page::All, false),
        (Page::All, true),
    ] {
        if page == Page::All {
            shell.show_all(now);
        }
        shell.popover.as_mut().unwrap().progress = 0.5;
        shell.popover.as_mut().unwrap().closing = closing;
        shell.popover.as_mut().unwrap().fade = Some((now, 0.0));
        if let Some(motion) = &mut shell.page_motion {
            motion.progress = 0.25;
        }
        shell.maintain_geometry(tests::VIEWPORT, tests::bell());
        let panel = shell.geometry.popover.unwrap().rect;
        let old_focus = shell.geometry.focus_order.as_ptr();
        let root = Affine::scale(2.0);
        let frame = inspect::Frame::begin(
            1,
            root,
            scene::rect(0.0, 0.0, tests::VIEWPORT.0, tests::VIEWPORT.1),
        );
        shell.draw_surfaces(
            &mut Scene::new(),
            root,
            &mut engine,
            tests::VIEWPORT,
            tests::bell(),
            Lang::En,
        );
        inspect::leaf("sibling", root, scene::rect(0.0, 0.0, 5.0, 5.0), |_| {});
        let snapshot = frame.finish();
        let popover = &snapshot.records[0];
        assert_eq!(popover.name, "KoruPopover");
        assert_eq!(property(popover, "occurrence"), "unavailable");
        assert_eq!(property(popover, "recipe body padding"), "12");
        assert_eq!(
            property(popover, "state"),
            if closing { "Closing" } else { "Opening" }
        );
        let rise = -(1.0 - 0.5) * theme::POPOVER_RISE;
        assert_eq!(
            popover.rect(),
            scene::rect(panel.x, panel.y + rise, panel.w, panel.h)
        );
        let expected_clip = popover.rect();
        for content in snapshot.records.iter().filter(|r| r.kind == "unregistered") {
            assert_eq!(content.parent, Some(0));
            assert_eq!(content.clip, inspect::array(expected_clip));
            if content.name == "Reminder management content" {
                let offset = shell
                    .page_motion
                    .as_ref()
                    .map_or(0.0, |motion| panel.w * (1.0 - motion.progress));
                assert_eq!(content.rect().x0, (panel.x + offset) as f64);
            }
        }
        let sibling = snapshot.records.last().unwrap();
        assert_eq!(sibling.parent, None);
        assert_eq!(
            sibling.clip,
            [0.0, 0.0, tests::VIEWPORT.0 as f64, tests::VIEWPORT.1 as f64]
        );
        assert_eq!(shell.geometry.focus_order.as_ptr(), old_focus);
        assert!(!shell.maintain_geometry(tests::VIEWPORT, tests::bell()));
    }
}

#[test]
fn bell_content_capture_does_not_change_motion_or_paint_encoding() {
    let now = Instant::now();
    let mut shell = tests::opened_popover(now);
    let mut engine = TextEngine::new();
    shell.prepare_action_widths(&mut engine, Lang::En);
    shell.show_all(now);
    shell.maintain_geometry(tests::VIEWPORT, tests::bell());
    shell.popover.as_mut().unwrap().progress = 0.5;
    shell.page_motion.as_mut().unwrap().progress = 0.25;
    let mut off = Scene::new();
    let mut on = Scene::new();
    shell.draw_surfaces(
        &mut off,
        Affine::IDENTITY,
        &mut engine,
        tests::VIEWPORT,
        tests::bell(),
        Lang::En,
    );
    let frame = inspect::Frame::begin(
        1,
        Affine::IDENTITY,
        scene::rect(0.0, 0.0, tests::VIEWPORT.0, tests::VIEWPORT.1),
    );
    shell.draw_surfaces(
        &mut on,
        Affine::IDENTITY,
        &mut engine,
        tests::VIEWPORT,
        tests::bell(),
        Lang::En,
    );
    frame.finish();
    let (a, b) = (off.encoding(), on.encoding());
    assert!(a.path_tags == b.path_tags);
    assert_eq!(a.path_data, b.path_data);
    assert!(a.draw_tags == b.draw_tags);
    assert_eq!(a.draw_data, b.draw_data);
    assert_eq!(a.transforms, b.transforms);
    assert_eq!(a.styles, b.styles);
    assert_eq!(shell.page_motion.as_ref().unwrap().progress, 0.25);
}
