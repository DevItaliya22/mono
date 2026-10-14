// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn meter_uses_the_protocol_bucket_boundary() {
    assert_eq!(HudState::recording(200), HudState::Recording { level: 8 });
    let count = |level| (0..CELLS).filter(|index| *index < level).count();
    assert_eq!(count(0), 0);
    assert_eq!(count(8), 8);
}

#[test]
fn shared_geometry_makes_only_the_stop_control_finish() {
    let hud = VoiceHud::new(Lang::En, HudState::recording(3));
    let g = geometry(HUD_WIDTH, HUD_HEIGHT);
    assert_eq!(
        hud.action_at(g.stop.x + 2.0, g.stop.y + 2.0),
        Some(HudAction::Finish)
    );
    assert_eq!(hud.action_at(g.meter.x, g.meter.y), None);
    assert_eq!(hud.action_at(-1.0, -1.0), None);
    let refused = VoiceHud::new(Lang::En, HudState::Refused);
    assert_eq!(refused.action_at(g.stop.x + 2.0, g.stop.y + 2.0), None);
}

#[test]
fn level_only_changes_never_relayout_text() {
    let mut hud = VoiceHud::new(Lang::En, HudState::recording(1));
    let mut engine = TextEngine::new();
    hud.draw(&mut Scene::new(), &mut engine, HUD_WIDTH, HUD_HEIGHT);
    assert_eq!(hud.layout_builds(), 1);
    assert!(hud.set_state(HudState::recording(8)));
    hud.draw(&mut Scene::new(), &mut engine, HUD_WIDTH, HUD_HEIGHT);
    assert_eq!(hud.layout_builds(), 1);
    assert!(hud.set_state(HudState::Finalising));
    hud.draw(&mut Scene::new(), &mut engine, HUD_WIDTH, HUD_HEIGHT);
    assert_eq!(hud.layout_builds(), 2);
}

#[test]
fn every_state_and_language_builds_a_scene_at_each_scale() {
    let states = [
        HudState::recording(0),
        HudState::recording(8),
        HudState::Finalising,
        HudState::Refused,
        HudState::Failed,
    ];
    for lang in Lang::ALL {
        for state in states {
            for scale in [1.0_f64, 1.25, 1.5, 2.0] {
                let hud = VoiceHud::new(lang, state);
                let mut logical = Scene::new();
                hud.draw(&mut logical, &mut TextEngine::new(), HUD_WIDTH, HUD_HEIGHT);
                let mut device = Scene::new();
                device.append(&logical, Some(Affine::scale(scale)));
                assert!(!device.encoding().is_empty(), "{lang:?} {state:?} {scale}");
            }
        }
    }
}
