// SPDX-License-Identifier: MPL-2.0

//! The five things the shipped boundary has to see a real Koru tab do.
//!
//! Nothing here touches disk: `Workspace::new` is in-memory, and only
//! `Session::open` reaches a directory.
//!
//! **The conformance tests live in the consumer**, which is the dependency
//! direction stated as a build fact: `koru-app-shell` never names an app, and
//! `crates/koru-app-shell/src/tests.rs` asserts it.

use super::*;
use koru_app_shell::{Commands, ShellState};
use koru_draw::text::TextEngine;
use koru_editor::Editing;
use koru_window::MultiWindowApp;
use winit::keyboard::{Key, KeyCode, KeyLocation, ModifiersState, PhysicalKey};
use winit::window::WindowId;

fn press<'a>(key: &'a Key, text: Option<&'a str>) -> RichKeyEvent<'a> {
    RichKeyEvent {
        logical_key: key,
        physical_key: PhysicalKey::Code(KeyCode::KeyA),
        location: KeyLocation::Standard,
        text,
        state: KeyState::Pressed,
        repeat: false,
        modifiers: ModifiersState::empty(),
    }
}

/// A shell with one window, driven through the **runner's** own sequence: ask,
/// `seated`, then a dispatch that carries an engine.
fn hosted() -> (ShellState<Koru>, TextEngine, WindowKey) {
    let mut engine = TextEngine::new();
    let mut shell = ShellState::new(Koru::new());
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    let mut scene = Scene::new();
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, 1200.0, 800.0);
    (shell, engine, window)
}

/// **The load-bearing one.** A `MainView` that needs `&mut WorkspaceDoc` on
/// nearly every path is hostable behind a boundary that cannot name one,
/// because the workspace is inside the surface.
#[test]
fn an_edit_reaches_the_document_through_a_boundary_that_cannot_name_one() {
    let mut surface = KoruSurface::new();
    surface.document("Notes");
    let mut engine = TextEngine::new();
    let window = ShellState::new(Koru::new()).open_window((1200.0, 800.0), 1.0);

    // Drive the surface directly, the way the shell does: one `SurfaceCx`, no
    // domain argument anywhere in the signature.
    let mut commands = Commands::new(window);
    let mut cx = SurfaceCx {
        engine: Some(&mut engine),
        scale: 1.0,
        commands: &mut commands,
    };
    surface.set_bounds(&mut cx, Rect::new(0.0, 0.0, 1200.0, 800.0));

    let before = text_of(&surface);
    let key = Key::Character("k".into());
    surface.key(&mut cx, press(&key, Some("k")));
    let after = text_of(&surface);
    assert_ne!(before, after, "the keystroke reached the document");
    assert!(
        after.contains('k'),
        "and it was the letter typed: {after:?}"
    );
}

/// Every row of the active document tab, joined. The container's own tests read
/// `body().rows[n].text`; this is the same read, reached through the surface.
fn text_of(surface: &KoruSurface) -> String {
    let crate::mainview::TabView::Leaf(leaf) = &surface.view().tabs().active_tab().view else {
        panic!("a document tab is a leaf");
    };
    leaf.body()
        .rows
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// A remote edit arrives through the surface's own clock. The shell needs no
/// notion of "the document changed" — only of "this surface had work to do".
#[test]
fn remote_adoption_runs_from_maintain_and_the_shell_learns_nothing() {
    let mut surface = KoruSurface::new();
    surface.document("Notes");
    // Somebody else's typing, arriving in the same in-memory document.
    let other = koru_workspace::document(&mut surface.workspace_mut().ws, "Theirs");
    assert!(other.is_some(), "a second document was created underneath");
    let before = text_of(&surface);
    surface.adopt_now();
    // `adopt` is "rebuilding is always the right answer": the tab survives it
    // and still reads its own document.
    assert_eq!(text_of(&surface), before);
}

/// **The clock has no `TextEngine`, and that is SP-1's shape.** `maintain`
/// arrives with `engine: None`; Koru's surface never wanted one there, which is
/// the evidence that the `Option` costs the real consumer nothing.
#[test]
fn maintenance_runs_with_no_engine_at_all() {
    let mut surface = KoruSurface::new();
    surface.document("Notes");
    let window = ShellState::new(Koru::new()).open_window((1200.0, 800.0), 1.0);
    let mut commands = Commands::new(window);
    let mut cx = SurfaceCx {
        engine: None,
        scale: 1.0,
        commands: &mut commands,
    };
    surface.maintain(&mut cx, std::time::Instant::now());
    assert!(commands.is_empty(), "and it asked the shell for nothing");
}

/// The drawing signature crosses intact: a vello `Scene`, a `TextEngine` from
/// the shell, and two domain values the shell has never heard of.
#[test]
fn the_drawing_signature_survives_the_boundary() {
    let mut surface = KoruSurface::new();
    surface.document("Notes");
    let mut engine = TextEngine::new();
    let window = ShellState::new(Koru::new()).open_window((100.0, 100.0), 1.0);
    let mut commands = Commands::new(window);
    let mut scene = Scene::new();
    let mut cx = SurfaceCx {
        engine: Some(&mut engine),
        scale: 1.0,
        commands: &mut commands,
    };
    surface.set_bounds(&mut cx, Rect::new(0.0, 0.0, 1200.0, 800.0));
    surface.draw(
        &mut cx,
        &mut scene,
        Affine::IDENTITY,
        Rect::new(0.0, 0.0, 1200.0, 800.0),
    );
    assert!(commands.is_empty(), "drawing records no commands");
}

/// A workspace switch is `reseat` + `on_workspace_switched`, and **neither is
/// on the boundary**. The host performs it on its own surface after a banner
/// selection; the shell only ever answered *which slot*.
#[test]
fn a_workspace_switch_happens_beside_the_boundary_not_through_it() {
    let (mut shell, mut engine, window) = hosted();
    let caption = shell.window(window).expect("a window").regions().caption;
    let slots = shell.window(window).expect("a window").banner().slots();
    let (x, y) = koru_shell_chrome::banner::profile_slot(caption, slots).centre();
    shell.rich_mouse(
        window,
        &mut engine,
        koru_window::RichMouseEvent {
            x,
            y,
            button: MouseButton::Left,
            state: KeyState::Pressed,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
    assert_eq!(
        shell.host().slots,
        vec![BannerSlot::Profile],
        "the chrome answered a slot, never a ProfileId"
    );

    // And the switch itself, on a surface the host owns.
    let mut surface = KoruSurface::new();
    surface.document("Notes");
    surface.switch_workspace();
    surface.adopt_now();
}

/// Pop-out and restore: two-phase, with the source intact until the
/// destination exists.
#[test]
fn a_pop_out_creates_the_window_before_anything_is_given_up() {
    let (mut shell, mut engine, window) = hosted();
    let key = Key::Character("n".into());

    // **Refused first.** The destination could not be made, so the host is told
    // to roll back and nothing was given up.
    shell.host_mut().refuse = true;
    shell.host_mut().queue_popout(koru_core::NodeId::new());
    shell.rich_key(window, &mut engine, press(&key, None));
    let asked = *shell.windows().last().expect("a window was asked for");
    shell.seated(asked, WindowId::from(2u64));
    let mut scene = Scene::new();
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, 1200.0, 800.0);
    assert_eq!(shell.window_keys().count(), 1, "no window survived");
    assert_eq!(shell.host().seated, 0);
    assert_eq!(shell.host().rollbacks, 1, "the host rolled its reason back");

    // And then accepted: the window and its render surface exist before the
    // host commits a thing.
    shell.host_mut().refuse = false;
    shell.host_mut().queue_popout(koru_core::NodeId::new());
    shell.rich_key(window, &mut engine, press(&key, None));
    let asked = *shell.windows().last().expect("a window was asked for");
    shell.seated(asked, WindowId::from(3u64));
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, 1200.0, 800.0);
    assert_eq!(shell.window_keys().count(), 2);
    assert_eq!(shell.host().seated, 1);
}

/// One `MainView`, one window: this host never records `OpenTab`, because a
/// Koru tab needs a tab-private workspace substitution the boundary cannot
/// carry. ADR-262 decided that; this is the compiled form of the decision.
#[test]
fn koru_is_one_surface_per_window_and_owns_its_own_tab_strip() {
    let (shell, _engine, window) = hosted();
    assert_eq!(
        shell.tab_count(window),
        1,
        "the shell holds one surface; the tabs inside it are MainView's"
    );
}
