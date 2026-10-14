// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::icon_button::{State, Tone};
use koru_draw::icons;

#[test]
fn slots_right_orders_leftmost_first_with_close_in_the_corner() {
    let area = Rect::new(240.0, 0.0, 960.0, 32.0);
    let slots = slots_right(area, 46.0, 3);
    assert_eq!(slots.len(), 3);
    assert!(slots.windows(2).all(|pair| pair[0].x < pair[1].x));
    assert_eq!(slots[2].x + slots[2].w, area.x + area.w);
    assert_eq!(slots[0].w, 46.0);
}

#[test]
fn the_group_answers_the_action_of_the_enabled_button_under_the_point() {
    #[derive(Copy, Clone, Debug, PartialEq)]
    enum Act {
        Min,
        Max,
        Close,
    }
    let slots = slots_right(Rect::new(0.0, 0.0, 800.0, 32.0), 46.0, 3);
    let group = ButtonGroup::new(vec![
        IconButton::new(icons::WINDOW_MINIMISE, slots[0]).on_click(Act::Min),
        IconButton::new(icons::SQUARE, slots[1]).on_click(Act::Max),
        IconButton::new(icons::WINDOW_CLOSE, slots[2])
            .on_click(Act::Close)
            .tone(Tone::Danger),
    ]);
    let (x, y) = slots[0].centre();
    assert_eq!(group.action_at(x, y), Some(Act::Min));
    let (x, y) = slots[2].centre();
    assert_eq!(group.action_at(x, y), Some(Act::Close));
    assert_eq!(group.action_at(1.0, 1.0), None);

    let disabled = ButtonGroup::new(vec![
        IconButton::new(icons::WINDOW_CLOSE, slots[2])
            .on_click(Act::Close)
            .with_state(State {
                disabled: true,
                ..State::default()
            }),
    ]);
    let (x, y) = slots[2].centre();
    assert_eq!(disabled.action_at(x, y), None);
}

#[test]
fn bounds_cover_every_slot_and_an_empty_group_has_none() {
    let slots = slots_right(Rect::new(0.0, 0.0, 800.0, 32.0), 46.0, 2);
    let group = ButtonGroup::new(vec![
        IconButton::new(icons::WINDOW_MINIMISE, slots[0]),
        IconButton::new(icons::SQUARE, slots[1]),
    ]);
    let bounds = group.bounds().unwrap();
    assert_eq!(bounds, slots[0].union(slots[1]));
    assert!(ButtonGroup::<()>::default().bounds().is_none());
}

#[test]
fn a_drawn_group_registers_its_buttons_under_one_boundary() {
    let mut engine = TextEngine::new();
    let mut scene_out = Scene::new();
    let root = Affine::IDENTITY;
    let slots = slots_right(Rect::new(0.0, 0.0, 800.0, 32.0), 46.0, 3);
    let group = ButtonGroup::new(vec![
        IconButton::new(icons::WINDOW_MINIMISE, slots[0]),
        IconButton::new(icons::SQUARE, slots[1]),
        IconButton::new(icons::WINDOW_CLOSE, slots[2]).tone(Tone::Danger),
    ]);
    let capture = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, 900.0, 650.0));
    group.draw(&mut scene_out, root, &mut engine);
    let snapshot = capture.finish();
    assert_eq!(snapshot.records[0].name, "KoruButtonGroup");
    assert_eq!(
        snapshot
            .records
            .iter()
            .filter(|r| r.name == "KoruIconButton")
            .count(),
        3
    );
    let group_index = snapshot
        .records
        .iter()
        .position(|r| r.name == "KoruButtonGroup")
        .unwrap();
    for record in &snapshot.records {
        if record.name == "KoruIconButton" {
            assert_eq!(record.parent, Some(group_index));
        }
    }
}
