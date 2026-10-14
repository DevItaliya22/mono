// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_draw::inspect;

#[test]
fn save_and_rename_keep_empty_names_and_preserve_distinct_nonempty_outcomes() {
    let mut engine = TextEngine::new();
    let mut dialog = SnapshotPrompt::new(&mut engine);
    for rename in [false, true] {
        if rename {
            dialog.become_rename(&mut engine, "entry".into(), None);
        }
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
        dialog.prompt.input.set_text(&mut engine, "  Snapshot  ");
        match dialog.key(
            &mut engine,
            &Key::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        ) {
            Action::Save(label) => {
                assert!(!rename);
                assert_eq!(label, "Snapshot");
            }
            Action::Rename { entry, label } => {
                assert!(rename);
                assert_eq!(entry, "entry");
                assert_eq!(label, "Snapshot");
            }
            _ => panic!("nonempty snapshot name must reach existing mode outcome"),
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

#[test]
fn rename_probe_preserves_typed_name_and_drawing_keeps_composition_authorship() {
    let mut engine = TextEngine::new();
    let mut untouched = SnapshotPrompt::new(&mut engine);
    untouched.become_rename(&mut engine, "entry".into(), Some("prefill".into()));
    assert_eq!(untouched.prompt.input.value(), "prefill");
    let mut typed = SnapshotPrompt::new(&mut engine);
    typed.key(
        &mut engine,
        &Key::Character("x".into()),
        Some("x"),
        ModifiersState::empty(),
    );
    typed.become_rename(&mut engine, "entry".into(), Some("replacement".into()));
    assert_eq!(typed.prompt.input.value(), "x");
    assert!(typed.dictation_field().begin_composition(&mut engine));
    typed
        .dictation_field()
        .update_composition(&mut engine, " provisional");
    typed.draw(
        &mut Scene::new(),
        Affine::IDENTITY,
        &mut engine,
        (900.0, 650.0),
    );
    assert_eq!(typed.prompt.input.value(), "x");
    assert!(typed.dictation_field().is_composing());
    typed.dictation_field().cancel_composition(&mut engine);
    assert_eq!(typed.prompt.input.value(), "x");
}

#[test]
fn snapshot_modes_capture_shared_frame_without_changing_complete_paint_or_field_bounds() {
    let mut engine = TextEngine::new();
    let mut dialog = SnapshotPrompt::new(&mut engine);
    let viewport = (350.0, 300.0);
    let root = Affine::scale(2.0);
    for rename in [false, true] {
        if rename {
            dialog.become_rename(&mut engine, "private entry id".into(), None);
        }
        dialog
            .prompt
            .input
            .set_text(&mut engine, "private snapshot اسم");
        let panel = Rect::new(15.0, 108.0, 320.0, 84.0);
        assert_eq!(SnapshotPrompt::panel(viewport), panel);
        let field = field_rect(panel.w);
        let target = Rect::new(panel.x + field.x, panel.y + field.y, field.w, field.h);
        assert_eq!(dialog.field(viewport), target);
        assert_eq!(
            dialog.cursor(viewport, target.centre().0, target.centre().1),
            CursorIcon::Text
        );
        let mut before = Scene::new();
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
        let lang = crate::format::lang();
        let (title, hint) = if rename {
            (I18nKey::SnapshotRenameTitle, I18nKey::SnapshotRenameHint)
        } else {
            (I18nKey::SnapshotSaveTitle, I18nKey::SnapshotSaveHint)
        };
        draw_name_prompt_contents(
            &mut before,
            root,
            &mut dialog.prompt,
            &mut engine,
            panel.x,
            panel.y,
            panel.w,
            koru_i18n::t(lang, title),
            koru_i18n::t(lang, I18nKey::SnapshotNamePlaceholder),
            false,
        );
        let hint = engine.simple_layout(koru_i18n::t(lang, hint), theme::role::hint());
        scene::draw_layout_first_line(
            &mut before,
            root,
            &hint,
            panel.x + theme::PANEL_PAD + 4.0,
            panel.y + 62.0,
            Some(theme::p().text_dim),
        );
        let mut after = Scene::new();
        let frame = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, viewport.0, viewport.1));
        dialog.draw(&mut after, root, &mut engine, viewport);
        let snapshot = frame.finish();
        assert_eq!(snapshot.records[0].name, "KoruDialogFrame");
        let exported = snapshot.export(None).unwrap();
        assert!(!exported.contains("private snapshot"));
        assert!(!exported.contains("private entry id"));
        let (a, b) = (before.encoding(), after.encoding());
        assert!(a.path_tags == b.path_tags);
        assert_eq!(a.path_data, b.path_data);
        assert!(a.draw_tags == b.draw_tags);
        assert_eq!(a.draw_data, b.draw_data);
        assert_eq!(a.transforms, b.transforms);
        assert_eq!(a.styles, b.styles);
    }
}
