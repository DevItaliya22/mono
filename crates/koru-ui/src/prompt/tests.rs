// SPDX-License-Identifier: MPL-2.0

use super::*;

fn engine() -> TextEngine {
    TextEngine::new()
}

fn prompt() -> KoruPrompt<&'static str> {
    KoruPrompt::new(
        "Delegate work".to_owned(),
        vec!["Delegated. The run is quiet-fox.".to_owned()],
        vec![
            KoruPromptAction {
                action: "close",
                label: "OK".to_owned(),
                style: ActionStyle::Secondary,
            },
            KoruPromptAction {
                action: "view",
                label: "View run".to_owned(),
                style: ActionStyle::Secondary,
            },
        ],
    )
}

/// The ancestry walk text_paragraph's tests also use: does this record sit
/// inside the subtree rooted at `root`?
fn inside(snapshot: &inspect::Snapshot, record: &inspect::Record, root: usize) -> bool {
    let mut ancestor = record.parent;
    while let Some(index) = ancestor {
        if index == root {
            return true;
        }
        ancestor = snapshot.records[index].parent;
    }
    false
}

#[test]
fn a_drawn_prompt_owns_its_title_and_one_button_per_action() {
    let mut engine = engine();
    let prompt = prompt();
    let mut scene_out = Scene::new();
    let root = Affine::IDENTITY;
    let panel = prompt.panel(&mut engine, (900.0, 650.0));
    let capture = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, 900.0, 650.0));
    prompt.draw(
        &mut scene_out,
        root,
        &mut engine,
        panel,
        None,
        Some(&"close"),
    );
    let snapshot = capture.finish();
    let at = snapshot
        .records
        .iter()
        .position(|r| r.name == "KoruPrompt")
        .expect("the prompt registers");
    assert_eq!(
        snapshot.records[at].rect(),
        scene::rect(panel.x, panel.y, panel.w, panel.h)
    );
    let mut title = 0;
    let mut buttons = 0;
    for record in &snapshot.records {
        if !inside(&snapshot, record, at) {
            continue;
        }
        match record.name {
            "Prompt title" => title += 1,
            "KoruButton" => buttons += 1,
            _ => {}
        }
    }
    assert_eq!(title, 1, "one title part, not one per line of chrome");
    assert_eq!(buttons, 2, "one KoruButton per action");
}

#[test]
fn action_rects_agree_with_the_drawn_button_bounds() {
    let mut engine = engine();
    let prompt = prompt();
    let mut scene_out = Scene::new();
    let root = Affine::IDENTITY;
    let panel = prompt.panel(&mut engine, (900.0, 650.0));
    let expected = prompt.action_rects(&mut engine, panel);
    let capture = inspect::Frame::begin(2, root, scene::rect(0.0, 0.0, 900.0, 650.0));
    prompt.draw(&mut scene_out, root, &mut engine, panel, None, None);
    let snapshot = capture.finish();
    let drawn: Vec<_> = snapshot
        .records
        .iter()
        .filter(|r| r.name == "KoruButton")
        .map(|r| r.rect())
        .collect();
    assert_eq!(drawn.len(), expected.len());
    for (drawn, expected) in drawn.iter().zip(&expected) {
        assert_eq!(
            *drawn,
            scene::rect(expected.x, expected.y, expected.w, expected.h)
        );
    }
    // The first action owns the right-hand corner; the second walks left.
    let right = panel.x + panel.w - KoruPrompt::<&str>::PAD;
    assert!((expected[0].x + expected[0].w - right).abs() < 0.01);
    assert!(expected[1].x + expected[1].w < expected[0].x);
}

#[test]
fn the_panel_clamps_to_a_tiny_viewport() {
    let mut engine = engine();
    let prompt = prompt();
    let panel = prompt.panel(&mut engine, (100.0, 80.0));
    assert_eq!(panel.w, 84.0);
    assert_eq!(panel.h, 64.0);
    assert_eq!(panel.x, 8.0);
    assert_eq!(panel.y, 8.0);
}

#[test]
fn the_panel_centres_and_sizes_to_its_content() {
    let mut engine = engine();
    let prompt = prompt();
    let panel = prompt.panel(&mut engine, (900.0, 650.0));
    assert_eq!(panel.w, KoruPrompt::<&str>::WIDTH);
    assert!((panel.x - (900.0 - panel.w) * 0.5).abs() < 0.01);
    assert!((panel.y - (650.0 - panel.h) * 0.5).abs() < 0.01);
    // The flow: pad, title line, gap, one body sentence and its gap, gap,
    // the action row, pad.
    let inner = panel.w - KoruPrompt::<&str>::PAD * 2.0;
    let body = engine
        .simple_layout_wrapped(&prompt.body[0], theme::role::hint(), inner)
        .height();
    let expected = KoruPrompt::<&str>::PAD
        + theme::role::ui().metrics.line_height
        + KoruPrompt::<&str>::GAP
        + body
        + 4.0
        + KoruPrompt::<&str>::GAP
        + ActionButton::H
        + KoruPrompt::<&str>::PAD;
    assert!(
        (panel.h - expected).abs() < 0.01,
        "{} vs {expected}",
        panel.h
    );
}
