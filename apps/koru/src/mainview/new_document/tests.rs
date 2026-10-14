// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_draw::inspect;

fn legacy_panel(viewport: (f32, f32)) -> Rect {
    Rect::new(
        ((viewport.0 - 320.0) * 0.5).max(8.0),
        ((viewport.1 - 84.0) * 0.5).max(8.0),
        320.0_f32.min((viewport.0 - 16.0).max(1.0)),
        84.0,
    )
}

#[test]
fn frame_and_field_hit_bounds_preserve_the_legacy_recipe_including_tiny_limits() {
    let mut engine = TextEngine::new();
    let mut dialog = NewDocument::new(&mut engine);
    for viewport in [(900.0, 650.0), (350.0, 300.0), (200.0, 120.0), (24.0, 20.0)] {
        let panel = legacy_panel(viewport);
        assert_eq!(NewDocument::panel(viewport), panel);
        let field = field_rect(panel.w);
        assert_eq!(
            dialog.field(viewport),
            Rect::new(panel.x + field.x, panel.y + field.y, field.w, field.h)
        );
        if field.w > 0.0 {
            let (x, y) = dialog.field(viewport).centre();
            assert_eq!(dialog.cursor(viewport, x, y), CursorIcon::Text);
            dialog.mouse_down(&mut engine, viewport, x, y);
            assert!(dialog.mouse_up());
        } else {
            assert!(panel.y + panel.h > viewport.1, "retained GP-L01 overflow");
            assert!(
                field.w < 0.0,
                "retained child width limit, not repaired by frame"
            );
        }
    }
}

#[test]
fn new_document_capture_matches_the_legacy_complete_painter_and_excludes_private_names() {
    let viewport = (900.0, 650.0);
    let root = Affine::scale(2.0);
    let mut engine = TextEngine::new();
    let mut dialog = NewDocument::new(&mut engine);
    dialog
        .prompt
        .input
        .set_text(&mut engine, "private synthetic اسم طويل");
    let panel = legacy_panel(viewport);
    let mut before = Scene::new();
    let baseline = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, viewport.0, viewport.1));
    scene::fill(
        &mut before,
        root,
        theme::p().scrim,
        &scene::rect(0.0, 0.0, viewport.0, viewport.1),
    );
    scene::floating_panel(
        &mut before,
        root,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        &scene::Panel::popover(8.0),
    );
    draw_name_prompt_contents(
        &mut before,
        root,
        &mut dialog.prompt,
        &mut engine,
        panel.x,
        panel.y,
        panel.w,
        "New document",
        "Document name",
        false,
    );
    let hint = engine.simple_layout("Enter to create · Esc to cancel", theme::role::hint());
    scene::draw_layout_first_line(
        &mut before,
        root,
        &hint,
        panel.x + theme::PANEL_PAD + 4.0,
        panel.y + 62.0,
        Some(theme::p().text_dim),
    );
    assert!(
        !baseline
            .finish()
            .records
            .iter()
            .any(|r| r.name == "KoruDialogFrame")
    );
    let mut after = Scene::new();
    let frame = inspect::Frame::begin(2, root, scene::rect(0.0, 0.0, viewport.0, viewport.1));
    dialog.draw(&mut after, root, &mut engine, viewport);
    let snapshot = frame.finish();
    assert_eq!(snapshot.records[0].name, "KoruDialogFrame");
    assert_eq!(
        snapshot.records[0].rect(),
        scene::rect(panel.x, panel.y, panel.w, panel.h)
    );
    assert!(
        snapshot
            .records
            .iter()
            .any(|r| r.name == "Name prompt content" && r.kind == "unregistered")
    );
    assert!(!snapshot.export(None).unwrap().contains("private"));
    let (a, b) = (before.encoding(), after.encoding());
    assert!(a.path_tags == b.path_tags);
    assert_eq!(a.path_data, b.path_data);
    assert!(a.draw_tags == b.draw_tags);
    assert_eq!(a.draw_data, b.draw_data);
    assert_eq!(a.transforms, b.transforms);
    assert_eq!(a.styles, b.styles);
}

#[test]
fn name_prompt_keeps_empty_input_and_preserves_creation_and_composition_routing() {
    let mut engine = TextEngine::new();
    for purpose in [
        Purpose::Document,
        Purpose::Workspace,
        Purpose::NewAgentProject,
    ] {
        let mut dialog = NewDocument::with_purpose(&mut engine, purpose, "");
        for value in ["", " \t "] {
            dialog.prompt.input.set_text(&mut engine, value);
            assert!(matches!(
                dialog.key(
                    &mut engine,
                    &Key::Named(NamedKey::Enter),
                    None,
                    ModifiersState::empty()
                ),
                Action::Keep
            ));
        }
        dialog.prompt.input.set_text(&mut engine, "");
        assert!(dialog.dictation_field().begin_composition(&mut engine));
        dialog
            .dictation_field()
            .update_composition(&mut engine, "provisional");
        dialog.draw(
            &mut Scene::new(),
            Affine::IDENTITY,
            &mut engine,
            (900.0, 650.0),
        );
        assert!(dialog.dictation_field().is_composing());
        assert!(matches!(
            dialog.key(
                &mut engine,
                &Key::Named(NamedKey::Enter),
                None,
                ModifiersState::empty()
            ),
            Action::Keep
        ));
        dialog
            .dictation_field()
            .commit_composition(&mut engine, "名前");
        match dialog.key(
            &mut engine,
            &Key::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        ) {
            Action::CreateWorkspace(name) => {
                assert_eq!(purpose, Purpose::Workspace);
                assert_eq!(name, "名前");
            }
            Action::CreateDocument(name) => {
                assert_ne!(purpose, Purpose::Workspace);
                assert_eq!(name, "名前");
            }
            _ => panic!("nonempty name must reach the existing outcome"),
        }
        assert!(matches!(
            dialog.key(
                &mut engine,
                &Key::Named(NamedKey::Escape),
                None,
                ModifiersState::empty()
            ),
            Action::Cancel
        ));
    }
}
