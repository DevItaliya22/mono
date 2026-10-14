// SPDX-License-Identifier: MPL-2.0

//! What [`WindowApp`]'s own defaults promise.
//!
//! The loop itself needs a window, a surface and a device, so the discipline is
//! asserted in [`crate::dispatch`] against a recording window instead. What is
//! here is the trait: the adapters that let an application implement the old,
//! smaller hooks and still receive everything, and the two hooks added for the
//! gaps SP-0 found, whose defaults must leave existing hosts alone.

use super::*;

#[derive(Debug, PartialEq, Eq)]
struct CapturedKey {
    logical: Key,
    physical: PhysicalKey,
    location: winit::keyboard::KeyLocation,
    text: Option<String>,
    state: crate::KeyState,
    repeat: bool,
    modifiers: ModifiersState,
}

#[derive(Default)]
struct RichInputRecorder {
    keys: Vec<CapturedKey>,
    ime: Vec<String>,
    right_clicks: Vec<(u32, u32)>,
    drops: Vec<std::path::PathBuf>,
}

impl WindowApp for RichInputRecorder {
    type Event = ();

    fn should_exit(&self) -> bool {
        false
    }

    fn close_requested(&mut self) {}

    fn pointer_moved(&mut self, _x: f32, _y: f32) -> bool {
        false
    }

    fn click(&mut self, _engine: &mut TextEngine, _x: f32, _y: f32, _ctrl: bool) -> bool {
        false
    }

    fn key(
        &mut self,
        _engine: &mut TextEngine,
        _key: &Key,
        _text: Option<&str>,
        _mods: ModifiersState,
    ) -> bool {
        panic!("rich recorder must not use the legacy adapter")
    }

    fn rich_key(&mut self, _engine: &mut TextEngine, event: RichKeyEvent<'_>) -> bool {
        self.keys.push(CapturedKey {
            logical: event.logical_key.clone(),
            physical: event.physical_key,
            location: event.location,
            text: event.text.map(str::to_owned),
            state: event.state,
            repeat: event.repeat,
            modifiers: event.modifiers,
        });
        true
    }

    fn ime(&mut self, _engine: &mut TextEngine, event: ImeEvent<'_>) -> bool {
        self.ime.push(match event {
            ImeEvent::Enabled => "enabled".into(),
            ImeEvent::Preedit { text, cursor } => format!("preedit:{text}:{cursor:?}"),
            ImeEvent::Commit { text } => format!("commit:{text}"),
            ImeEvent::Disabled => "disabled".into(),
        });
        true
    }

    fn right_click(&mut self, _engine: &mut TextEngine, x: f32, y: f32) -> bool {
        self.right_clicks.push((x.to_bits(), y.to_bits()));
        true
    }

    fn dropped_file(&mut self, path: &Path) -> bool {
        self.drops.push(path.to_path_buf());
        true
    }

    fn draw(&mut self, _scene: &mut Scene, _engine: &mut TextEngine, _w: f32, _h: f32) {}
}

#[derive(Default)]
struct LegacyKeyRecorder {
    calls: Vec<(Key, Option<String>, ModifiersState)>,
}

impl WindowApp for LegacyKeyRecorder {
    type Event = ();

    fn should_exit(&self) -> bool {
        false
    }

    fn close_requested(&mut self) {}

    fn pointer_moved(&mut self, _x: f32, _y: f32) -> bool {
        false
    }

    fn click(&mut self, _engine: &mut TextEngine, _x: f32, _y: f32, _ctrl: bool) -> bool {
        false
    }

    fn key(
        &mut self,
        _engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> bool {
        self.calls
            .push((key.clone(), text.map(str::to_owned), mods));
        true
    }

    fn draw(&mut self, _scene: &mut Scene, _engine: &mut TextEngine, _w: f32, _h: f32) {}
}

fn press(logical: &Key) -> RichKeyEvent<'_> {
    RichKeyEvent {
        logical_key: logical,
        physical_key: PhysicalKey::Code(winit::keyboard::KeyCode::KeyK),
        location: winit::keyboard::KeyLocation::Standard,
        text: Some("K"),
        state: crate::KeyState::Pressed,
        repeat: false,
        modifiers: ModifiersState::SUPER | ModifiersState::SHIFT,
    }
}

#[test]
fn the_default_rich_hook_preserves_every_legacy_key_press() {
    let mut app = LegacyKeyRecorder::default();
    let mut engine = TextEngine::new();
    let logical = Key::Character("k".into());
    let event = press(&logical);
    assert!(app.rich_key(&mut engine, event));
    assert_eq!(
        app.calls,
        vec![(logical.clone(), Some("K".into()), event.modifiers)],
        "the macOS primary modifier and text reach the old callback unchanged"
    );

    assert!(!app.rich_key(
        &mut engine,
        RichKeyEvent {
            state: crate::KeyState::Released,
            ..event
        }
    ));
    assert_eq!(app.calls.len(), 1, "legacy applications never saw releases");
}

#[test]
fn a_rich_app_receives_physical_tab_repeat_release_ime_and_files() {
    let mut app = RichInputRecorder::default();
    let mut engine = TextEngine::new();
    let logical_tab = Key::Named(winit::keyboard::NamedKey::Tab);
    let modifiers = ModifiersState::CONTROL;
    let event = RichKeyEvent {
        logical_key: &logical_tab,
        // Windows can report Ctrl+I with this logical/physical split. Both are
        // load-bearing: shell tab routing must not erase the physical I.
        physical_key: PhysicalKey::Code(winit::keyboard::KeyCode::KeyI),
        location: winit::keyboard::KeyLocation::Standard,
        text: Some("\t"),
        state: crate::KeyState::Pressed,
        repeat: true,
        modifiers,
    };
    assert!(app.rich_key(&mut engine, event));
    assert!(app.rich_key(
        &mut engine,
        RichKeyEvent {
            state: crate::KeyState::Released,
            repeat: false,
            text: None,
            ..event
        }
    ));

    assert!(app.ime(
        &mut engine,
        ImeEvent::Preedit {
            text: "e\u{0301}",
            cursor: Some((0, 3)),
        }
    ));
    assert!(app.ime(&mut engine, ImeEvent::Commit { text: "é" }));
    // The default `context_menu` is the right-click callback, which is what
    // keeps an app that only implemented one of them working.
    assert!(app.context_menu(&mut engine, 12.5, 40.0));
    let first = Path::new("first.txt");
    let second = Path::new("second.txt");
    assert!(app.dropped_file(first));
    assert!(app.dropped_file(second));

    assert_eq!(app.keys[0].logical, logical_tab);
    assert_eq!(
        app.keys[0].physical,
        PhysicalKey::Code(winit::keyboard::KeyCode::KeyI)
    );
    assert_eq!(app.keys[0].text.as_deref(), Some("\t"));
    assert!(app.keys[0].repeat);
    assert_eq!(app.keys[0].modifiers, modifiers);
    assert_eq!(app.keys[1].state, crate::KeyState::Released);
    assert_eq!(app.ime, ["preedit:e\u{0301}:Some((0, 3))", "commit:é"]);
    assert_eq!(app.right_clicks, [(12.5_f32.to_bits(), 40.0_f32.to_bits())]);
    assert_eq!(app.drops, [first, second]);
}

#[test]
fn the_two_new_hooks_are_silent_for_an_app_that_has_not_heard_of_them() {
    // Additive, ADR-230-style: the leave event and the DPI change reach every
    // host, and a host that implements neither behaves exactly as it did.
    let mut app = LegacyKeyRecorder::default();
    assert!(!app.pointer_left());
    assert!(!app.scale_factor_changed(1.5));
    assert!(app.calls.is_empty());
}
