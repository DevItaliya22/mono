// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::model::{NodeStatus, ObjectKey, SceneImage};

fn key(n: u128) -> ObjectKey {
    ObjectKey(uuid::Uuid::from_u128(n))
}

const LONG: &str = "The quick brown fox jumps over the lazy dog, and then it keeps \
    running across the whole field until the text has to wrap many times over.";

#[test]
fn a_cards_text_wraps_at_its_width() {
    let mut engine = TextEngine::new();
    let mut counters = Counters::default();
    let narrow = SceneObject::new(key(1), ObjectKind::Card, 0, 0, 240, LONG);
    let laid = lay_out(
        &mut engine,
        Lang::En,
        &narrow,
        240,
        None,
        &mut counters,
        true,
    );
    let body = laid.body.as_ref().expect("a card has text");
    let inner = 240.0 - PAD * 2.0;
    assert!(
        body.layout.lines().count() > 1,
        "a long line in a 240 px card wraps"
    );
    for line in body.layout.lines() {
        // The space a line broke at hangs past the column, as it does in
        // every text engine; the ink stops inside it.
        let ink = line.metrics().advance - line.metrics().trailing_whitespace;
        assert!(ink <= inner + 0.5, "a line ran {ink} past a {inner} column");
    }
    let wide = lay_out(
        &mut engine,
        Lang::En,
        &narrow,
        960,
        None,
        &mut counters,
        true,
    );
    let wide_lines = wide.body.as_ref().expect("text").layout.lines().count();
    assert!(wide_lines < body.layout.lines().count());
    assert!(wide.h < laid.h, "height follows the text");
}

#[test]
fn further_lines_start_new_lines() {
    let mut engine = TextEngine::new();
    let mut counters = Counters::default();
    let mut card = SceneObject::new(key(1), ObjectKind::Card, 0, 0, 600, "first");
    let one = lay_out(&mut engine, Lang::En, &card, 600, None, &mut counters, true);
    card.lines = vec!["second".into(), "third".into()];
    let three = lay_out(&mut engine, Lang::En, &card, 600, None, &mut counters, true);
    assert_eq!(one.body.as_ref().unwrap().layout.lines().count(), 1);
    assert_eq!(three.body.as_ref().unwrap().layout.lines().count(), 3);
}

/// A status that ticks re-lays the node's geometry but not its text.
#[test]
fn a_changed_status_keeps_the_text_it_already_laid_out() {
    let mut engine = TextEngine::new();
    let mut counters = Counters::default();
    let mut agent = SceneObject::new(key(2), ObjectKind::Agent, 0, 0, 280, LONG);
    agent.status = Some(NodeStatus::Running {
        activity: "Reading".into(),
        elapsed: "0:01".into(),
    });
    let first = lay_out(
        &mut engine,
        Lang::En,
        &agent,
        280,
        None,
        &mut counters,
        true,
    );
    assert_eq!(counters.texts_laid, 1);
    agent.status = Some(NodeStatus::Running {
        activity: "Reading".into(),
        elapsed: "0:02".into(),
    });
    assert!(!first.content.matches(&agent, 280, Lang::En));
    let _second = lay_out(
        &mut engine,
        Lang::En,
        &agent,
        280,
        Some(first),
        &mut counters,
        true,
    );
    assert_eq!(
        counters.texts_laid, 1,
        "the instruction was not shaped again"
    );
    assert_eq!(counters.objects_laid, 2);
}

#[test]
fn a_node_has_its_header_status_and_buttons() {
    let mut engine = TextEngine::new();
    let mut counters = Counters::default();
    let mut agent = SceneObject::new(key(3), ObjectKind::Agent, 0, 0, 280, "Summarise");
    let idle = lay_out(
        &mut engine,
        Lang::En,
        &agent,
        280,
        None,
        &mut counters,
        true,
    );
    assert!(idle.button(Part::ChooseAgent).is_some());
    assert!(idle.button(Part::Run).is_some());
    assert!(idle.button(Part::Stop).is_none());
    assert!(idle.status.is_none());

    agent.status = Some(NodeStatus::Running {
        activity: "Writing".into(),
        elapsed: "0:12".into(),
    });
    agent.result = (1..=8).map(|n| format!("line {n}")).collect();
    let running = lay_out(
        &mut engine,
        Lang::En,
        &agent,
        280,
        None,
        &mut counters,
        true,
    );
    assert!(running.button(Part::Stop).is_some());
    assert!(running.button(Part::Run).is_none());
    assert!(running.status_glyph.as_ref().is_some_and(|(_, spin)| *spin));
    assert!(running.button(Part::PlaceResult).is_some());
    assert!(running.divider_y.is_some());
    let result = running.result.as_ref().expect("a result");
    assert_eq!(
        result.layout.lines().count(),
        RESULT_LINES,
        "five lines at most"
    );
    // Everything sits inside the node, and no two buttons overlap.
    for button in &running.buttons {
        assert!(button.rect.y + button.rect.h <= running.h);
        assert!(button.rect.x + button.rect.w <= running.w);
    }
    for (a, b) in running
        .buttons
        .iter()
        .flat_map(|a| running.buttons.iter().map(move |b| (a, b)))
    {
        if a.part != b.part {
            assert!(
                !a.rect.intersects(b.rect),
                "{:?} overlaps {:?}",
                a.part,
                b.part
            );
        }
    }

    let mut output = SceneObject::new(key(4), ObjectKind::Output, 0, 0, 240, "Weekly notes");
    output.link = Some("Weekly notes — 26 Sep".into());
    let laid = lay_out(
        &mut engine,
        Lang::En,
        &output,
        240,
        None,
        &mut counters,
        true,
    );
    assert!(laid.button(Part::OpenLink).is_some());
    assert!(
        laid.button(Part::Run).is_none(),
        "an output node is not run"
    );
}

#[test]
fn an_image_keeps_its_aspect_ratio() {
    let mut image = SceneObject::new(key(5), ObjectKind::Image, 0, 0, 300, "");
    image.image = Some(SceneImage {
        hash: "b3:x".into(),
        width: 1200,
        height: 800,
    });
    assert_eq!(image_height(&image, 300), 200.0);
    assert_eq!(image_height(&image, 600), 400.0);
}

/// Culling trusts this bound: it must never be below what the layout makes.
#[test]
fn the_height_bound_is_never_below_the_laid_out_height() {
    let mut engine = TextEngine::new();
    let mut counters = Counters::default();
    let texts = [
        "",
        "short",
        LONG,
        "漢字かな交じり文がここで何度も折り返されるほど長く続いていく",
    ];
    for (n, kind) in [
        ObjectKind::Card,
        ObjectKind::Start,
        ObjectKind::Agent,
        ObjectKind::Output,
    ]
    .into_iter()
    .enumerate()
    {
        for text in texts {
            for w in [80, 160, 240, 500] {
                let w = clamp_width(kind, w);
                let mut object = SceneObject::new(key(n as u128), kind, 0, 0, w, text);
                object.lines = vec![LONG.into(), String::new()];
                object.status = Some(NodeStatus::Failed(LONG.into()));
                object.result = vec![LONG.into(); 7];
                object.link = Some(LONG.into());
                let laid = lay_out(&mut engine, Lang::En, &object, w, None, &mut counters, true);
                assert!(
                    height_bound(&object, w) >= laid.h,
                    "{kind:?} at {w}: bound {} under {}",
                    height_bound(&object, w),
                    laid.h
                );
            }
        }
    }
}

#[test]
fn widths_and_coordinates_are_clamped() {
    assert_eq!(clamp_width(ObjectKind::Card, 0), DEFAULT_WIDTH);
    assert_eq!(clamp_width(ObjectKind::Card, 5), MIN_CARD_WIDTH);
    assert_eq!(clamp_width(ObjectKind::Agent, 5), MIN_NODE_WIDTH);
    assert_eq!(clamp_width(ObjectKind::Card, 1_000_000), MAX_WIDTH);
    assert_eq!(clamp_coord(3e9), COORD_LIMIT);
    assert_eq!(clamp_coord(-3e9), -COORD_LIMIT);
    assert_eq!(clamp_coord(f64::NAN), 0);
    assert_eq!(clamp_coord(12.6), 13);
}
