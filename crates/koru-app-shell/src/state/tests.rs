// SPDX-License-Identifier: MPL-2.0

//! What the boundary has to be able to say, asserted rather than described.
//!
//! Nothing here needs a GPU or a window: the runner's methods are plain calls,
//! so the whole contract is drivable from a test.
//!
//! **No downcast anywhere.** A test surface reports through a shared log it was
//! built with, not through `Any` — because the moment the shell can ask a
//! surface what it *is*, surface-kind policy has arrived, and ADR-262 says that
//! waits for a consumer.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use koru_draw::geometry::Rect;
use koru_window::KeyState;
use winit::keyboard::{KeyLocation, NamedKey};
use winit::window::WindowId;

use super::*;
use crate::host::{Banner, BannerChip, BannerKey, SeatRefused};
use crate::keys::{ProfileKey, WorkspaceKey};

// ------------------------------------------------------------------ fixtures

#[derive(Default)]
struct Log {
    bounds: Vec<Rect>,
    keys: Vec<String>,
    pointers: Vec<PointerEvent>,
    activated: u32,
    deactivated: u32,
    /// True while a composition is in flight on *some* surface.
    live_preedit: bool,
    dropped: Vec<String>,
    invalidated: u32,
    maintained: u32,
}

type Shared = Rc<RefCell<Log>>;

struct Recorder {
    log: Shared,
}

impl Surface for Recorder {
    fn set_bounds(&mut self, _cx: &mut SurfaceCx<'_>, bounds: Rect) -> bool {
        self.log.borrow_mut().bounds.push(bounds);
        true
    }

    fn draw(&mut self, _cx: &mut SurfaceCx<'_>, _scene: &mut Scene, _root: Affine, _bounds: Rect) {}

    fn invalidate(&mut self) {
        self.log.borrow_mut().invalidated += 1;
    }

    fn key(&mut self, _cx: &mut SurfaceCx<'_>, event: RichKeyEvent<'_>) -> bool {
        self.log
            .borrow_mut()
            .keys
            .push(format!("{:?}", event.logical_key));
        true
    }

    fn ime(&mut self, _cx: &mut SurfaceCx<'_>, event: ImeEvent<'_>) -> bool {
        if let ImeEvent::Preedit { text, .. } = event {
            self.log.borrow_mut().live_preedit = !text.is_empty();
        }
        true
    }

    fn pointer(&mut self, _cx: &mut SurfaceCx<'_>, event: PointerEvent) -> bool {
        self.log.borrow_mut().pointers.push(event);
        true
    }

    fn dropped_file(&mut self, _cx: &mut SurfaceCx<'_>, path: &Path) -> bool {
        self.log
            .borrow_mut()
            .dropped
            .push(path.display().to_string());
        true
    }

    fn activated(&mut self, _cx: &mut SurfaceCx<'_>) -> bool {
        self.log.borrow_mut().activated += 1;
        true
    }

    fn deactivated(&mut self, _cx: &mut SurfaceCx<'_>) -> bool {
        let mut log = self.log.borrow_mut();
        log.deactivated += 1;
        // The whole reason `deactivated` exists and runs first.
        log.live_preedit = false;
        true
    }

    fn maintain(&mut self, _cx: &mut SurfaceCx<'_>, _now: Instant) -> bool {
        self.log.borrow_mut().maintained += 1;
        true
    }
}

/// What the host should do the next time it is asked something.
#[derive(Default, Clone)]
enum Armed {
    #[default]
    Nothing,
    OpenTab,
    OpenWindow,
    PopOut(TabKey),
}

struct Host {
    log: Shared,
    floors: u32,
    seats: u32,
    refuse_seat: bool,
    slots: Vec<BannerSlot>,
    claim_keys: bool,
    claimed: Vec<String>,
    banner: Banner,
    open_failures: u32,
    armed: Armed,
    /// What this host calls its floor tab. Empty is the default every host had
    /// before `ShellHost::floor_tab` existed.
    floor_title: String,
    /// Whether this host's left pane opens with the window, and whether its
    /// identity lives in that pane's header band. Two of the trait's questions,
    /// answered from fields so one fixture can drive both arrangements.
    left_pane: bool,
    identity_in_pane: bool,
}

impl Host {
    fn new(log: Shared) -> Self {
        Self {
            log,
            floors: 0,
            seats: 0,
            refuse_seat: false,
            slots: Vec::new(),
            claim_keys: false,
            claimed: Vec::new(),
            banner: Banner::default(),
            open_failures: 0,
            armed: Armed::Nothing,
            floor_title: String::new(),
            left_pane: false,
            identity_in_pane: false,
        }
    }
}

impl ShellHost for Host {
    type Event = ();

    fn floor_surface(&mut self, _cx: &mut HostCx<'_>, _window: WindowKey) -> Box<dyn Surface> {
        self.floors += 1;
        Box::new(Recorder {
            log: Rc::clone(&self.log),
        })
    }

    fn floor_tab(&self, _window: WindowKey) -> crate::host::SeatedTab {
        crate::host::SeatedTab::new(self.floor_title.clone(), false)
    }

    fn banner(&self, _window: WindowKey) -> Banner {
        self.banner.clone()
    }

    fn slot_pressed(&mut self, _cx: &mut HostCx<'_>, _window: WindowKey, slot: BannerSlot) -> bool {
        self.slots.push(slot);
        true
    }

    fn pane_open(&self, _window: WindowKey, side: Side) -> bool {
        self.left_pane && side == Side::Left
    }

    fn identity_in_pane(&self, _window: WindowKey) -> bool {
        self.identity_in_pane
    }

    fn window_key(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        event: RichKeyEvent<'_>,
    ) -> bool {
        match std::mem::take(&mut self.armed) {
            Armed::Nothing => {}
            Armed::OpenTab => {
                cx.command(
                    window,
                    ShellAction::OpenTab {
                        title: "second".into(),
                        closable: true,
                        surface: Box::new(Recorder {
                            log: Rc::clone(&self.log),
                        }),
                    },
                );
                return true;
            }
            Armed::OpenWindow => {
                cx.command(window, ShellAction::OpenWindow);
                return true;
            }
            Armed::PopOut(tab) => {
                cx.command(window, ShellAction::PopOutTab(tab));
                return true;
            }
        }
        if self.claim_keys {
            self.claimed.push(format!("{:?}", event.logical_key));
            return true;
        }
        false
    }

    fn seat_window(
        &mut self,
        _cx: &mut HostCx<'_>,
        _window: WindowKey,
    ) -> Result<Box<dyn Surface>, SeatRefused> {
        if self.refuse_seat {
            return Err(SeatRefused("the destination could not be made".into()));
        }
        self.seats += 1;
        Ok(Box::new(Recorder {
            log: Rc::clone(&self.log),
        }))
    }

    fn window_open_failed(&mut self, _cx: &mut HostCx<'_>) {
        self.open_failures += 1;
    }
}

fn press(key: &Key) -> RichKeyEvent<'_> {
    RichKeyEvent {
        logical_key: key,
        physical_key: PhysicalKey::Code(KeyCode::KeyA),
        location: KeyLocation::Standard,
        text: None,
        state: KeyState::Pressed,
        repeat: false,
        modifiers: ModifiersState::empty(),
    }
}

fn ctrl_tab(key: &Key, shift: bool) -> RichKeyEvent<'_> {
    let mut modifiers = ModifiersState::CONTROL;
    if shift {
        modifiers |= ModifiersState::SHIFT;
    }
    RichKeyEvent {
        logical_key: key,
        physical_key: PhysicalKey::Code(KeyCode::Tab),
        location: KeyLocation::Standard,
        text: None,
        state: KeyState::Pressed,
        repeat: false,
        modifiers,
    }
}

/// A shell with one window open and seated, plus the log its surfaces report
/// through.
///
/// The window is driven through the **runner's** own sequence — ask, `seated`,
/// then a dispatch that carries an engine — because that is the sequence the
/// product has and a back door would prove nothing about it.
fn shell() -> (ShellState<Host>, Shared, TextEngine, WindowKey) {
    let log: Shared = Rc::default();
    let engine = TextEngine::new();
    let mut shell = ShellState::new(Host::new(Rc::clone(&log)));
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1200, 800), 1.0);
    (shell, log, engine, window)
}

/// Seat whatever the runner has opened, the way the first engine-bearing
/// dispatch does.
fn settle(shell: &mut ShellState<Host>, engine: &mut TextEngine) {
    let mut scene = Scene::new();
    let keys: Vec<WindowKey> = shell.windows().to_vec();
    for key in keys {
        MultiWindowApp::draw(shell, key, &mut scene, engine, 1200.0, 800.0);
    }
}

/// Open a second tab **the way a host opens one** — by recording a command
/// during a dispatch — rather than through a back door the product would not
/// have.
fn open_second_tab(shell: &mut ShellState<Host>, engine: &mut TextEngine, window: WindowKey) {
    shell.host_mut().armed = Armed::OpenTab;
    let key = Key::Named(NamedKey::F1);
    shell.rich_key(window, engine, press(&key));
}

fn banner_with_labels() -> Banner {
    Banner {
        profile: Some(BannerChip::new(
            "Steven",
            BannerKey::Profile(ProfileKey::from_uuid(uuid::Uuid::new_v4())),
        )),
        workspace: Some(BannerChip::new(
            "Notes",
            BannerKey::Workspace(WorkspaceKey::from_uuid(uuid::Uuid::new_v4())),
        )),
        settings: true,
    }
}

// --------------------------------------------------------------------- tests

#[test]
fn a_window_the_runner_seats_opens_on_the_hosts_surface_and_is_activated() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    assert_eq!(
        shell.host().floors,
        1,
        "a window the application opened for itself takes the host's floor, once"
    );
    assert_eq!(
        shell.host().seats,
        0,
        "and never troubles the two-phase seat, which exists to be refusable"
    );
    assert_eq!(shell.tab_count(window), 1);
    assert_eq!(log.borrow().activated, 1);
}

/// **The runner is the authority.** A window the host asked for is in
/// `windows()` before it exists, which is what makes `run_windows` open it.
#[test]
fn a_window_is_asked_for_before_it_exists() {
    let log: Shared = Rc::default();
    let mut shell = ShellState::new(Host::new(log));
    let key = shell.open_window((1200.0, 800.0), 1.0);
    assert_eq!(shell.windows(), &[key]);
    assert!(
        shell.window(key).is_none(),
        "asked for, not yet seated — and the host has not been troubled"
    );
    assert_eq!(shell.host().seats, 0);
}

/// The shell hands a surface a rect, not a size. Both real hosts translate
/// their own input by it, so handing only `(w, h)` would make each of them
/// rebuild an offset the shell already knows.
#[test]
fn a_surface_is_told_where_it_is_and_receives_input_in_its_own_coordinates() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let content = shell.content_rect(window).expect("a content rect");
    assert!(
        content.y >= theme::CAPTION_H,
        "content starts below the caption, at y = {}",
        content.y
    );
    assert_eq!(log.borrow().bounds.last().copied(), Some(content));

    shell.rich_mouse(
        window,
        &mut engine,
        RichMouseEvent {
            x: content.x + 10.0,
            y: content.y + 20.0,
            button: MouseButton::Left,
            state: KeyState::Pressed,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
    let seen = log.borrow().pointers.last().copied().expect("a press");
    assert!(
        matches!(seen, PointerEvent::Pressed { x, y, .. } if (x - 10.0).abs() < f32::EPSILON
            && (y - 20.0).abs() < f32::EPSILON),
        "the press arrived surface-local, not window-local: {seen:?}"
    );
}

/// **The drag the runner stopped producing.** With `wants_rich_mouse` on, the
/// dispatch core never sets its own `dragging` flag, so a move with a button
/// held would arrive as a plain move. The shell remembers the button.
#[test]
fn a_move_with_a_button_held_is_reported_as_a_drag() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let content = shell.content_rect(window).expect("a content rect");
    shell.rich_mouse(
        window,
        &mut engine,
        RichMouseEvent {
            x: content.x + 5.0,
            y: content.y + 5.0,
            button: MouseButton::Left,
            state: KeyState::Pressed,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
    shell.pointer_moved(window, content.x + 40.0, content.y + 40.0);
    assert!(
        matches!(
            log.borrow().pointers.last(),
            Some(PointerEvent::Dragged { .. })
        ),
        "a held button turns the move into a drag: {:?}",
        log.borrow().pointers.last()
    );
    shell.rich_mouse(
        window,
        &mut engine,
        RichMouseEvent {
            x: content.x + 40.0,
            y: content.y + 40.0,
            button: MouseButton::Left,
            state: KeyState::Released,
            modifiers: ModifiersState::empty(),
            click_count: 0,
        },
    );
    shell.pointer_moved(window, content.x + 60.0, content.y + 60.0);
    assert!(
        matches!(
            log.borrow().pointers.last(),
            Some(PointerEvent::Moved { .. })
        ),
        "and the release puts it back"
    );
}

#[test]
fn the_host_sees_a_key_before_any_surface_and_may_consume_it() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    shell.host_mut().claim_keys = true;
    let key = Key::Character("c".into());
    assert!(shell.rich_key(window, &mut engine, press(&key)));
    assert_eq!(shell.host().claimed.len(), 1, "the host claimed it");
    assert!(
        log.borrow().keys.is_empty(),
        "and the surface never saw it — which is how the Terminal keeps \
         swallowing Ctrl+C without the chrome owning an accelerator table"
    );
}

#[test]
fn a_key_the_host_declines_reaches_the_active_surface() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let key = Key::Character("c".into());
    assert!(shell.rich_key(window, &mut engine, press(&key)));
    assert_eq!(log.borrow().keys.len(), 1);
}

/// ADR-263: switching tabs cancels an unfinished IME preedit before the
/// destination attaches, so composition reaches neither terminal. That is an
/// ordering fact, and this is the ordering.
#[test]
fn deactivation_runs_before_activation_so_a_preedit_can_be_cancelled() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    shell.ime(
        window,
        &mut engine,
        ImeEvent::Preedit {
            text: "にほ",
            cursor: None,
        },
    );
    assert!(log.borrow().live_preedit);

    open_second_tab(&mut shell, &mut engine, window);
    assert_eq!(shell.tab_count(window), 2);
    assert!(
        !log.borrow().live_preedit,
        "the outgoing surface cancelled its composition before the incoming \
         one was activated"
    );
    assert_eq!(log.borrow().deactivated, 1);
}

/// **Ctrl+Tab is the shell's, resolved from the physical key**, and it never
/// reaches the host's key map — where it would collide with Ctrl+I on Windows.
#[test]
fn ctrl_tab_cycles_and_never_reaches_the_host() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    open_second_tab(&mut shell, &mut engine, window);
    let second = shell.active_tab(window).expect("a tab");
    shell.host_mut().claim_keys = true;

    // Windows reports Ctrl+Tab's *logical* key as Tab and Ctrl+I's as Tab too;
    // only the physical key tells them apart, so that is what is resolved on.
    let tab = Key::Named(NamedKey::Tab);
    assert!(shell.rich_key(window, &mut engine, ctrl_tab(&tab, true)));
    assert_ne!(shell.active_tab(window), Some(second));
    assert!(shell.rich_key(window, &mut engine, ctrl_tab(&tab, false)));
    assert_eq!(shell.active_tab(window), Some(second));
    assert!(
        shell.host().claimed.is_empty(),
        "the host's key map never saw it"
    );
}

#[test]
fn a_banner_press_reaches_the_host_as_a_slot_and_never_as_an_identity() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    shell.host_mut().banner = banner_with_labels();
    shell.settle_banner(window);
    let caption = shell.window(window).expect("a window").regions().caption;
    let slots = shell.window(window).expect("a window").banner().slots();
    let (x, y) = banner::profile_slot(caption, slots).centre();
    shell.rich_mouse(
        window,
        &mut engine,
        RichMouseEvent {
            x,
            y,
            button: MouseButton::Left,
            state: KeyState::Pressed,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
    assert_eq!(shell.host().slots, vec![BannerSlot::Profile]);
}

/// **The shell owns open, dismiss, anchor and clamp; the rows are the host's.**
/// A press inside the panel is the host's business; a press outside it is a
/// dismissal, and the shell decides which is which without asking.
#[test]
fn the_shell_opens_and_dismisses_the_popover_and_the_host_owns_only_its_rows() {
    struct Popovers {
        opened: Vec<BannerSlot>,
        pressed: Vec<BannerSlot>,
    }
    impl ShellHost for Popovers {
        type Event = ();

        fn floor_surface(&mut self, _cx: &mut HostCx<'_>, _w: WindowKey) -> Box<dyn Surface> {
            Box::new(Bare)
        }
        fn banner(&self, _window: WindowKey) -> Banner {
            Banner {
                profile: Some(BannerChip::new(
                    "Steven",
                    BannerKey::Profile(ProfileKey::from_uuid(uuid::Uuid::new_v4())),
                )),
                workspace: None,
                settings: false,
            }
        }
        fn slot_pressed(
            &mut self,
            cx: &mut HostCx<'_>,
            window: WindowKey,
            slot: BannerSlot,
        ) -> bool {
            self.opened.push(slot);
            cx.command(window, ShellAction::Popover(Some(slot)));
            true
        }
        fn popover_size(&self, _window: WindowKey, _slot: BannerSlot) -> (f32, f32) {
            (theme::PANEL_W, 180.0)
        }
        fn popover_pressed(
            &mut self,
            _cx: &mut HostCx<'_>,
            _window: WindowKey,
            slot: BannerSlot,
            _x: f32,
            _y: f32,
        ) -> bool {
            self.pressed.push(slot);
            true
        }
        fn seat_window(
            &mut self,
            _cx: &mut HostCx<'_>,
            _w: WindowKey,
        ) -> Result<Box<dyn Surface>, SeatRefused> {
            Ok(Box::new(Bare))
        }
    }
    struct Bare;
    impl Surface for Bare {
        fn set_bounds(&mut self, _cx: &mut SurfaceCx<'_>, _bounds: Rect) -> bool {
            false
        }
        fn draw(
            &mut self,
            _cx: &mut SurfaceCx<'_>,
            _scene: &mut Scene,
            _root: Affine,
            _bounds: Rect,
        ) {
        }
    }

    let mut engine = TextEngine::new();
    let mut shell = ShellState::new(Popovers {
        opened: Vec::new(),
        pressed: Vec::new(),
    });
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(9u64));
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1200, 800), 1.0);
    let mut scene = Scene::new();
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, 1200.0, 800.0);

    let press = |shell: &mut ShellState<Popovers>, engine: &mut TextEngine, x: f32, y: f32| {
        shell.rich_mouse(
            window,
            engine,
            RichMouseEvent {
                x,
                y,
                button: MouseButton::Left,
                state: KeyState::Pressed,
                modifiers: ModifiersState::empty(),
                click_count: 1,
            },
        )
    };

    let caption = shell.window(window).expect("a window").regions().caption;
    let slots = shell.window(window).expect("a window").banner().slots();
    let (x, y) = banner::profile_slot(caption, slots).centre();
    press(&mut shell, &mut engine, x, y);
    let popover = shell
        .window(window)
        .and_then(ShellWindow::popover)
        .expect("the shell opened one");
    assert_eq!(shell.host().opened, vec![BannerSlot::Profile]);
    let chip = banner::chip_band(banner::profile_slot(caption, slots));
    assert_eq!(
        popover.bounds().y - (chip.y + chip.h),
        crate::popover::GAP,
        "hung below the chip it came from, by Koru's own gap: {:?}",
        popover.bounds()
    );

    // Inside: the host's rows answer.
    let (px, py) = popover.bounds().centre();
    press(&mut shell, &mut engine, px, py);
    assert_eq!(shell.host().pressed, vec![BannerSlot::Profile]);
    assert!(
        shell
            .window(window)
            .and_then(ShellWindow::popover)
            .is_some()
    );

    // Outside: the shell dismisses, and the host is not asked.
    let content = shell.content_rect(window).expect("content");
    press(
        &mut shell,
        &mut engine,
        content.x + content.w - 4.0,
        content.y + content.h - 4.0,
    );
    assert!(
        shell
            .window(window)
            .and_then(ShellWindow::popover)
            .is_none()
    );
    assert_eq!(
        shell.host().pressed.len(),
        1,
        "a dismissal is the shell's, not a row press"
    );
}

/// Two-phase: the destination has to exist before the source loses anything.
#[test]
fn a_refused_seat_rolls_back_with_the_source_intact() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    open_second_tab(&mut shell, &mut engine, window);
    let doomed = shell.active_tab(window).expect("a tab");
    let before = shell.tab_count(window);

    shell.host_mut().refuse_seat = true;
    shell.host_mut().armed = Armed::PopOut(doomed);
    let key = Key::Named(NamedKey::F2);
    shell.rich_key(window, &mut engine, press(&key));
    // The runner opens the window it was asked for, then reports it.
    let asked = *shell.windows().last().expect("a pending window");
    assert_ne!(asked, window);
    shell.seated(asked, WindowId::from(1u64));
    settle(&mut shell, &mut engine);

    assert_eq!(shell.window_keys().count(), 1, "no window survived");
    assert_eq!(shell.tab_count(window), before, "the tab is still here");
    assert_eq!(shell.active_tab(window), Some(doomed));
    assert_eq!(
        shell.host().open_failures,
        1,
        "the host was told to roll back"
    );
    assert!(!shell.windows().contains(&asked), "and the runner drops it");
}

#[test]
fn a_successful_pop_out_moves_the_tab_only_after_the_window_exists() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    open_second_tab(&mut shell, &mut engine, window);
    let moved = shell.active_tab(window).expect("a tab");

    shell.host_mut().armed = Armed::PopOut(moved);
    let key = Key::Named(NamedKey::F2);
    shell.rich_key(window, &mut engine, press(&key));
    let asked = *shell.windows().last().expect("a pending window");
    assert_eq!(
        shell.tab_count(window),
        2,
        "asked for, not seated: the source has given up nothing"
    );

    shell.seated(asked, WindowId::from(1u64));
    settle(&mut shell, &mut engine);
    assert_eq!(shell.window_keys().count(), 2);
    assert_eq!(shell.tab_count(window), 1, "the source gave one up");
}

/// The list is never empty: closing the last tab mints a replacement through
/// the host factory rather than leaving a window with nothing in it.
#[test]
fn closing_the_last_tab_mints_a_new_floor_from_the_host() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let only = shell.active_tab(window).expect("a tab");
    assert!(shell.close_tab(Some(&mut engine), window, only));
    assert_eq!(shell.tab_count(window), 1);
    assert_eq!(
        shell.host().floors,
        2,
        "one for the window, one for the tab that replaced the last"
    );
    assert!(
        shell.active_tab(window).is_some_and(|key| key != only),
        "a fresh tab, with a key of its own"
    );
}

#[test]
fn a_command_a_host_records_is_performed_after_the_dispatch_not_during_it() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    shell.host_mut().armed = Armed::OpenWindow;
    let key = Key::Named(NamedKey::Enter);
    shell.rich_key(window, &mut engine, press(&key));
    assert_eq!(
        shell.windows().len(),
        2,
        "the drain ran, once, afterwards — and asked the runner for a window"
    );
}

#[test]
fn a_theme_flip_invalidates_every_surface_in_the_window() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    open_second_tab(&mut shell, &mut engine, window);
    log.borrow_mut().invalidated = 0;
    shell.system_theme_changed(window, true);
    assert_eq!(
        log.borrow().invalidated,
        2,
        "background tabs too — the theme is a process global, and the runner \
         asks this of every window"
    );
}

#[test]
fn a_dropped_file_reaches_the_active_surface() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    assert!(shell.dropped_file(window, Path::new("a.png")));
    assert_eq!(log.borrow().dropped, vec!["a.png".to_owned()]);
}

/// Every surface's maintenance runs; only the active one's answer is a reason
/// for a frame. That is the inactive-tab rule (R2) with an enforcement point.
#[test]
fn every_surface_maintains_but_only_the_visible_one_asks_for_a_frame() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    open_second_tab(&mut shell, &mut engine, window);
    log.borrow_mut().maintained = 0;
    assert!(shell.maintain(window, Instant::now()));
    assert_eq!(log.borrow().maintained, 2, "every surface drained");
}

/// A close the surface vetoes leaves the window in `windows()`, which is the
/// only thing that keeps it on screen.
#[test]
fn a_vetoed_close_keeps_the_window() {
    struct Sticky;
    impl Surface for Sticky {
        fn set_bounds(&mut self, _cx: &mut SurfaceCx<'_>, _bounds: Rect) -> bool {
            false
        }
        fn draw(
            &mut self,
            _cx: &mut SurfaceCx<'_>,
            _scene: &mut Scene,
            _root: Affine,
            _bounds: Rect,
        ) {
        }
        fn close_requested(&mut self, _cx: &mut SurfaceCx<'_>) -> CloseResponse {
            CloseResponse::Veto
        }
    }
    struct Refuser;
    impl ShellHost for Refuser {
        type Event = ();

        fn floor_surface(&mut self, _cx: &mut HostCx<'_>, _w: WindowKey) -> Box<dyn Surface> {
            Box::new(Sticky)
        }
        fn seat_window(
            &mut self,
            _cx: &mut HostCx<'_>,
            _w: WindowKey,
        ) -> Result<Box<dyn Surface>, SeatRefused> {
            Ok(Box::new(Sticky))
        }
    }

    let mut engine = TextEngine::new();
    let mut shell = ShellState::new(Refuser);
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    let mut scene = Scene::new();
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, 1200.0, 800.0);

    MultiWindowApp::close_requested(&mut shell, window);
    assert_eq!(shell.windows(), &[window], "the veto held");
}

// -------------------------------------------------- the context column (SP-12)

/// A host that offers the right slot only while it says it has something to
/// draw, and records every pane call it is given. Its own double rather than an
/// arm on [`Host`], which is the point: adding a hook must not force every
/// existing host to know about it.
struct Contextual {
    offered: bool,
    drawn: Vec<(Side, Rect)>,
    pointed: Vec<Side>,
    /// The surfaces' shared log — the same [`Recorder`] the other fixtures use,
    /// so "what was this surface last sized to" is answered without a downcast.
    log: Shared,
}

impl ShellHost for Contextual {
    type Event = ();

    fn floor_surface(&mut self, _cx: &mut HostCx<'_>, _w: WindowKey) -> Box<dyn Surface> {
        Box::new(Recorder {
            log: Rc::clone(&self.log),
        })
    }

    fn seat_window(
        &mut self,
        _cx: &mut HostCx<'_>,
        _w: WindowKey,
    ) -> Result<Box<dyn Surface>, SeatRefused> {
        Ok(Box::new(Recorder {
            log: Rc::clone(&self.log),
        }))
    }

    fn banner(&self, _window: WindowKey) -> Banner {
        banner_with_labels()
    }

    fn context_offered(&self, _window: WindowKey) -> bool {
        self.offered
    }

    fn pane_open(&self, _window: WindowKey, _side: Side) -> bool {
        true
    }

    fn draw_pane(
        &mut self,
        _cx: &mut HostCx<'_>,
        _scene: &mut Scene,
        _root: Affine,
        _window: WindowKey,
        side: Side,
        bounds: Rect,
    ) {
        self.drawn.push((side, bounds));
    }

    fn pane_pointer(
        &mut self,
        _cx: &mut HostCx<'_>,
        _window: WindowKey,
        side: Side,
        _event: PointerEvent,
    ) -> bool {
        self.pointed.push(side);
        true
    }
}

fn contextual_at(
    offered: bool,
    size: (f32, f32),
    id: u64,
) -> (ShellState<Contextual>, Shared, TextEngine, WindowKey) {
    let log: Shared = Rc::default();
    let mut engine = TextEngine::new();
    let mut shell = ShellState::new(Contextual {
        offered,
        drawn: Vec::new(),
        pointed: Vec::new(),
        log: Rc::clone(&log),
    });
    let window = shell.open_window(size, 1.0);
    shell.seated(window, WindowId::from(id));
    MultiWindowApp::resized(
        &mut shell,
        window,
        PhysicalSize::new(size.0 as u32, size.1 as u32),
        1.0,
    );
    let mut scene = Scene::new();
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, size.0, size.1);
    (shell, log, engine, window)
}

fn contextual(offered: bool) -> (ShellState<Contextual>, TextEngine, WindowKey) {
    let (shell, _log, engine, window) = contextual_at(offered, (1200.0, 800.0), 12);
    (shell, engine, window)
}

fn draw_once(shell: &mut ShellState<Contextual>, engine: &mut TextEngine, window: WindowKey) {
    let mut scene = Scene::new();
    MultiWindowApp::draw(shell, window, &mut scene, engine, 1200.0, 800.0);
}

fn left_press(
    shell: &mut ShellState<Contextual>,
    engine: &mut TextEngine,
    window: WindowKey,
    x: f32,
    y: f32,
) {
    shell.rich_mouse(
        window,
        engine,
        RichMouseEvent {
            x,
            y,
            button: MouseButton::Left,
            state: KeyState::Pressed,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
}

/// **The toggle exists only while the host offers the column**, and pressing it
/// is what shuts and opens the pane — the wire `Hit::ToggleContext` has had
/// since SP-9, reachable at last.
#[test]
fn the_context_toggle_is_offered_by_the_host_and_toggles_the_right_pane() {
    let (mut shell, mut engine, window) = contextual(true);
    let caption = shell.window(window).expect("a window").regions().caption;
    let slots = shell.window(window).expect("a window").banner().slots();
    let toggle = caption::context_toggle_with_banner(caption, true, true, slots);
    assert!(toggle.w > 0.0, "offered means a real rect");
    assert!(
        shell.window(window).expect("a window").regions().context.w > 0.0,
        "and the column has room"
    );

    let (x, y) = toggle.centre();
    left_press(&mut shell, &mut engine, window, x, y);
    assert_eq!(
        shell.window(window).expect("a window").regions().context.w,
        0.0,
        "the press shut the column"
    );
}

/// **Not offered is a zero-width rect, not a shut pane.** The person's choice
/// stays in the `Panel`, so a tab that has nothing to show takes the column's
/// room and gives it straight back when the offer returns — with no slide, which
/// is what stops a tab switch being 140 ms of motion.
#[test]
fn withdrawing_the_offer_takes_the_room_without_forgetting_the_choice() {
    let (mut shell, mut engine, window) = contextual(true);
    let open_w = shell.window(window).expect("a window").regions().context.w;
    assert!(open_w > 0.0);

    shell.host_mut().offered = false;
    draw_once(&mut shell, &mut engine, window);
    let window_ref = shell.window(window).expect("a window");
    assert_eq!(window_ref.regions().context.w, 0.0);
    assert_eq!(
        window_ref.drawn_pane(Side::Right).w,
        0.0,
        "and nothing is painted over the content that reclaimed it"
    );

    shell.host_mut().offered = true;
    draw_once(&mut shell, &mut engine, window);
    assert_eq!(
        shell.window(window).expect("a window").regions().context.w,
        open_w,
        "and the column comes back where it was left"
    );
}

/// The pane hooks were always `Side`-parameterised; this is the first host to
/// answer `Right`, so the draw and the pointer are asserted to reach it.
#[test]
fn the_right_pane_is_drawn_and_pointed_at_like_the_left_one() {
    let (mut shell, mut engine, window) = contextual(true);
    let context = shell.window(window).expect("a window").regions().context;
    assert!(
        shell
            .host()
            .drawn
            .iter()
            .any(|(side, bounds)| *side == Side::Right && bounds.w == context.w),
        "the shell drew the right column: {:?}",
        shell.host().drawn
    );

    let (x, y) = context.centre();
    left_press(&mut shell, &mut engine, window, x, y);
    assert_eq!(shell.host().pointed, vec![Side::Right]);
}

/// **A column never paints wider than the window granted it.**
///
/// `Panel::drawn_width` answers what the column *wants* — `full`, unclamped —
/// and `regions::solve` protects content by giving way in order. At the shared
/// 720 floor with the sidebar open, `solve(720, h, 232, 268)` grants the context
/// column 168. Drawing at 268 would paint 100 px over content the layout never
/// gave up, and — worse — put the column's own rows outside `pane_at`'s region,
/// so a row you can see is a row that types into the surface behind it.
#[test]
fn a_column_is_drawn_at_the_width_the_window_granted_not_the_one_it_asked_for() {
    // The sidebar has to be open for the floor to bite, which is what
    // `Contextual::pane_open` answering both sides gives us.
    let (shell, _log, _engine, window) = contextual_at(true, MIN_WINDOW_SIZE, 13);

    let window_ref = shell.window(window).expect("a seated window");
    let granted = window_ref.regions().context;
    assert!(
        granted.w < theme::CONTEXT_W,
        "the floor is the case worth testing: {granted:?}"
    );
    assert_eq!(
        window_ref.drawn_pane(Side::Right),
        granted,
        "drawn is what was granted"
    );
    assert!(
        shell
            .host()
            .drawn
            .iter()
            .filter(|(side, _)| *side == Side::Right)
            .all(|(_, bounds)| bounds.w == granted.w),
        "and the host was handed exactly that: {:?}",
        shell.host().drawn
    );
    // The point that used to be inside the drawn column but outside the hit
    // region is now in neither — it belongs to the content, and it looks like it.
    let (x, y) = granted.centre();
    assert!(
        shell
            .window(window)
            .expect("a seated window")
            .regions()
            .context
            .contains(x, y)
    );
}

/// **Opening a column moves the content, so the active surface is told.**
/// A surface that was not would keep laying out against the width it last heard
/// — for a terminal, a PTY still 268 px wide inside a view that is not.
#[test]
fn toggling_the_column_re_sizes_the_active_surface() {
    let (mut shell, log, mut engine, window) = contextual_at(true, (1200.0, 800.0), 14);
    let open = *log.borrow().bounds.last().expect("a sized surface");
    assert_eq!(open, shell.window(window).expect("a window").content());

    // Shut it: the content gets the column's width back, and hears about it.
    let toggle = {
        let window_ref = shell.window(window).expect("a window");
        caption::context_toggle_with_banner(
            window_ref.regions().caption,
            true,
            true,
            window_ref.banner().slots(),
        )
    };
    let (x, y) = toggle.centre();
    left_press(&mut shell, &mut engine, window, x, y);

    let shut = *log.borrow().bounds.last().expect("a re-sized surface");
    assert!(
        shut.w > open.w,
        "the content took the column's width back: {open:?} → {shut:?}"
    );
    assert_eq!(
        shut,
        shell.window(window).expect("a window").content(),
        "and it was told the rect the shell will draw it into"
    );
}

/// A host that never offers it is the shell as it was: no toggle, no column, and
/// the lane keeps every pixel the Settings slot does not take.
#[test]
fn a_host_that_offers_no_context_column_has_neither_toggle_nor_room() {
    let (shell, _engine, window) = contextual(false);
    let window_ref = shell.window(window).expect("a window");
    let caption = window_ref.regions().caption;
    let slots = window_ref.banner().slots();
    let strip = window_ref.strip();
    assert_eq!(window_ref.regions().context.w, 0.0);
    assert_eq!(
        caption::context_toggle_with_banner(caption, true, false, slots).w,
        0.0
    );
    assert_eq!(
        strip.x + strip.w,
        banner::settings_slot(caption, true, slots).x,
        "the lane runs up to Settings and no further"
    );
}

// ------------------------------------------ the size a window opens at
//
// The owner's report: a maximised ~1920 window drew its session several hundred
// pixels narrower than the window, with a hairline where the surface stopped.

/// **A size the window learns before it is seated is the size it opens at.**
///
/// The runner reports `Resized` the moment the OS window exists — for a window
/// restored maximised, that is the *maximised* size — and that is several
/// events before the first engine-bearing dispatch runs `commit_seats`. This
/// used to answer `window_mut` with `None` and drop the event, and the window
/// was then built at the size `open_window` had *asked* for. A maximised window
/// laid its content out at the remembered restored width and stayed there until
/// somebody dragged an edge.
#[test]
fn a_resize_that_arrives_before_seating_is_the_size_the_window_opens_at() {
    let log: Shared = Rc::default();
    let mut engine = TextEngine::new();
    let mut shell = ShellState::new(Host::new(Rc::clone(&log)));
    // Asked for at the remembered restored size…
    let window = shell.open_window((1100.0, 720.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    // …and the OS opens it maximised, before anything can draw.
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1920, 1080), 1.0);
    settle(&mut shell, &mut engine);

    let content = shell.content_rect(window).expect("a content rect");
    assert_eq!(
        shell.window(window).expect("a window").viewport(),
        Rect::new(0.0, 0.0, 1920.0, 1080.0),
        "the window opened at the size it was told, not the size it asked for"
    );
    assert_eq!(
        content.x + content.w,
        1920.0,
        "and the content reaches the window's right edge"
    );
    assert_eq!(
        log.borrow().bounds.last().copied(),
        Some(content),
        "the surface was sized to that rect in the same seating"
    );
}

/// The whole of what the surface is owed, at several sizes and across a
/// simulated maximise: **`set_bounds` is `regions.content`, and it arrives in
/// the same event as the resize** — not on the frame after it.
#[test]
fn every_resize_hands_the_active_surface_the_content_rect_at_once() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    // A restored size, a portrait snap, a maximise, an ultrawide, and the
    // shared 720x600 floor where the columns have already given way.
    for (w, h) in [
        (1100.0_f32, 720.0_f32),
        (960.0, 1040.0),
        (1920.0, 1080.0),
        (3440.0, 1440.0),
        (720.0, 600.0),
    ] {
        let before = log.borrow().bounds.len();
        MultiWindowApp::resized(
            &mut shell,
            window,
            PhysicalSize::new(w as u32, h as u32),
            1.0,
        );
        let solved = {
            let window = shell.window(window).expect("a window");
            regions::solve(w, h, window.regions().sidebar.w, window.regions().context.w)
        };
        let content = shell.content_rect(window).expect("a content rect");
        assert_eq!(
            content, solved.content,
            "the content rect is `regions::solve`'s and nothing else's at {w}x{h}"
        );
        assert!(
            log.borrow().bounds.len() > before,
            "the resize reached the surface in its own event at {w}x{h}"
        );
        assert_eq!(
            log.borrow().bounds.last().copied(),
            Some(content),
            "and what it was handed is the content rect at {w}x{h}"
        );
        assert_eq!(
            content.x + content.w,
            w,
            "which reaches the window's right edge at {w}x{h}"
        );
    }
}

/// **The floor tab wears the host's label.** It was minted with
/// `SeatedTab::default()` — an empty title — so a window that opens on a
/// Welcome tab drew a pill with nothing in it.
#[test]
fn the_floor_tab_takes_the_hosts_presentation() {
    let log: Shared = Rc::default();
    let mut engine = TextEngine::new();
    let mut host = Host::new(Rc::clone(&log));
    host.floor_title = "Welcome".to_owned();
    let mut shell = ShellState::new(host);
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1200, 800), 1.0);
    settle(&mut shell, &mut engine);

    let tab = shell.active_tab(window).expect("a floor tab");
    assert_eq!(shell.tab_title(window, tab), Some("Welcome"));

    // And the floor the strip mints when the last tab closes is the same tab,
    // so it is asked the same question.
    open_second_tab(&mut shell, &mut engine, window);
    let second = shell.active_tab(window).expect("the second tab");
    shell.close_tab(Some(&mut engine), window, tab);
    shell.close_tab(Some(&mut engine), window, second);
    let floor = shell.active_tab(window).expect("a re-minted floor");
    assert_eq!(
        shell.tab_title(window, floor),
        Some("Welcome"),
        "a re-minted floor is not a nameless one"
    );
}

// ------------------------------------------------ parity with the reference
//
// Koru is the reference and the Terminal has to feel like it. These say so at
// the level a test can: the same shared function answers, so the two windows
// cannot have picked different numbers.

/// **The lane is `koru_shell_chrome`'s, measured by its own function.**
///
/// A tab's cached width is `tabs::preferred_width` — the same measure
/// `apps/koru/src/mainview/draw.rs` caches before it calls `tabs::draw` — so
/// the pill's clamp, its padding, its reserved close chrome and its
/// `[TAB_MIN_W, TAB_MAX_W]` range are one implementation, not two.
#[test]
fn a_tabs_width_is_the_shared_chromes_measure_and_not_the_shells_own() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    open_second_tab(&mut shell, &mut engine, window);
    settle(&mut shell, &mut engine);
    let strip = shell.window(window).expect("a window").strip();
    let measured: Vec<(f32, f32)> = shell
        .window(window)
        .expect("a window")
        .tabs()
        .iter()
        .map(|tab| {
            (
                tab.width(),
                tabs::preferred_width(&mut engine, tab.title(), false, tab.closable()),
            )
        })
        .collect();
    for (cached, shared) in measured {
        assert_eq!(
            cached, shared,
            "the shell measures through the shared crate"
        );
    }
    assert_eq!(
        strip,
        caption::tab_strip_with_banner(
            shell.window(window).expect("a window").regions().caption,
            true,
            false,
            false,
            shell.window(window).expect("a window").banner().slots(),
        ),
        "and the lane it lays them out in is the shared crate's too"
    );
}

/// **The right-hand cluster is `koru_ui::caption`'s**, which is the same
/// function `apps/koru/src/mainview/draw.rs` calls for the same four buttons —
/// so overflow, minimise, maximise and close are one pitch, one size and one
/// set of glyphs across both windows.
#[test]
fn the_window_controls_are_korus_own_rects() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let caption = shell.window(window).expect("a window").regions().caption;
    let controls = koru_ui::caption::controls(caption);
    assert_eq!(
        koru_ui::caption::reserved_width(caption),
        theme::CAPTION_BTN * 4.0,
        "four buttons at `CAPTION_BTN`, edge to edge"
    );
    for pair in controls.windows(2) {
        assert_eq!(
            pair[1].rect.x - pair[0].rect.x,
            theme::CAPTION_BTN,
            "no gap between them; the air is inside each slot's own chip"
        );
    }
    let last = controls.last().expect("four controls");
    assert_eq!(
        last.rect.x + last.rect.w,
        caption.x + caption.w,
        "and close owns the corner exactly"
    );
    let settings = banner::settings_slot(
        caption,
        true,
        shell.window(window).expect("a window").banner().slots(),
    );
    assert_eq!(
        settings.x + settings.w,
        controls[0].rect.x,
        "Settings keeps its square against them, and a slot that is not \
         offered is a zero-width rect rather than an absence"
    );
}

/// A host that answers nothing keeps the silent floor it always had.
#[test]
fn a_host_that_does_not_name_its_floor_keeps_the_silent_one() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let tab = shell.active_tab(window).expect("a floor tab");
    assert_eq!(shell.tab_title(window, tab), Some(""));
}

// ------------------------------------------- collapsing the pane, and the slide
//
// The one rule the chrome exists to keep: **what is drawn is what is pressed**.
// `drawn_caption` subtracted `Panel::slide_offset`, which is `full` at rest with
// the pane shut rather than zero — so a shut pane drew its whole caption 232 px
// left of where every hit test looked for it, permanently. The banner walked off
// the window's left edge, the lane moved under the pointer, and hovering the `+`
// reported the profile chip's tooltip.

/// A pane opens and shuts with a shell whose left column is offered.
fn with_left_pane() -> (ShellState<Host>, TextEngine, WindowKey) {
    paned(false)
}

/// The same, in whichever of the two identity arrangements. The flag is set
/// **before** the window exists, because where a window's furniture lives is a
/// fact about the host and not something that moves under a live window.
fn paned(identity_in_pane: bool) -> (ShellState<Host>, TextEngine, WindowKey) {
    let log: Shared = Rc::default();
    let mut shell = ShellState::new(Host::new(Rc::clone(&log)));
    shell.host_mut().left_pane = true;
    shell.host_mut().identity_in_pane = identity_in_pane;
    shell.host_mut().banner = banner_with_labels();
    let mut engine = TextEngine::new();
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1200, 800), 1.0);
    settle(&mut shell, &mut engine);
    (shell, engine, window)
}

/// Run a slide to its end, the way the runner does — the clock is an argument,
/// not a sleep.
fn finish_slide(shell: &mut ShellState<Host>, window: WindowKey) {
    let settled = Instant::now() + std::time::Duration::from_millis(1000);
    if let Some(w) = shell.window_mut(window) {
        w.left.advance(settled);
    }
}

/// **At rest the drawn caption is the settled caption, open or shut.** They may
/// differ only while a slide is in flight (DESIGN, "The banner" §6).
#[test]
fn a_settled_caption_is_drawn_exactly_where_it_is_hit_tested() {
    let (mut shell, _engine, window) = with_left_pane();
    let open = shell.window(window).expect("a window");
    assert_eq!(
        open.drawn_caption(),
        open.regions().caption,
        "pane open, at rest"
    );

    shell.toggle_pane(window, Side::Left);
    finish_slide(&mut shell, window);
    let shut = shell.window(window).expect("a window");
    assert_eq!(
        shut.regions().caption.x,
        0.0,
        "the caption reclaimed the column's room"
    );
    assert_eq!(
        shut.drawn_caption(),
        shut.regions().caption,
        "pane shut, at rest — this is the divergence that hid the banner"
    );
}

/// **Every caption slot hit-tests where it is drawn, with the pane collapsed.**
///
/// The reported symptom was hovering the `+` and being shown the profile chip's
/// tooltip, which is exactly what a caption drawn 232 px off does. Asserted over
/// the furniture rather than at one point, so the next thing added to the row is
/// covered too.
#[test]
fn with_the_pane_collapsed_every_caption_slot_hit_tests_where_it_is_drawn() {
    let (mut shell, mut engine, window) = with_left_pane();
    shell.toggle_pane(window, Side::Left);
    finish_slide(&mut shell, window);
    settle(&mut shell, &mut engine);

    let w = shell.window(window).expect("a window");
    let drawn = w.drawn_caption();
    let slots = w.banner().slots();
    let widths: Vec<f32> = w.tabs().iter().map(|tab| tab.width).collect();

    // The `+`, drawn in the lane the drawn caption produced.
    let plus = tabs::new_tab(
        caption::tab_strip_with_banner(drawn, w.window_buttons, w.context, w.host_action, slots),
        &widths,
        w.tab_scroll,
    );
    let (x, y) = plus.centre();
    assert_eq!(
        shell.chrome_hit(window, x, y),
        Some(regions::Hit::NewTab),
        "the `+` answers for itself where it is drawn"
    );
    assert_eq!(
        shell.banner_hit(window, x, y),
        None,
        "and no banner chip claims it — the tooltip that appeared there was the \
         profile's, 232 px away"
    );

    // The sidebar toggle, likewise, and it is the one at the window's own edge
    // when the column is gone.
    let toggle = caption::sidebar_toggle(drawn);
    let (x, y) = toggle.centre();
    assert_eq!(
        shell.chrome_hit(window, x, y),
        Some(regions::Hit::ToggleSidebar)
    );

    // And the profile chip, which is what the `+`'s tooltip was reporting.
    let chip = banner::profile_slot(drawn, slots);
    assert!(chip.w > 0.0, "the identity stayed reachable");
    let (x, y) = chip.centre();
    assert_eq!(shell.banner_hit(window, x, y), Some(BannerSlot::Profile));
}

/// **Mid-slide, hit-testing uses the settled layout.** The drawn caption travels
/// with the column so the chips do not jump at the ends of the slide; the press
/// arithmetic does not follow it, which is the "no third code path" rule.
#[test]
fn mid_slide_the_caption_travels_and_hit_testing_stays_settled() {
    let (mut shell, _engine, window) = with_left_pane();
    shell.toggle_pane(window, Side::Left);
    // Half a slide: the layout width is already zero, the column is still there.
    let midway = Instant::now() + std::time::Duration::from_millis(70);
    if let Some(w) = shell.window_mut(window) {
        w.left.advance(midway);
    }
    let w = shell.window(window).expect("a window");
    assert_eq!(
        w.regions().caption.x,
        0.0,
        "the layout settled at the toggle"
    );
    let drawn = w.drawn_caption();
    assert!(
        drawn.x > 0.0 && drawn.x < theme::SIDEBAR_W,
        "and the drawing is somewhere between: {drawn:?}"
    );
    assert_eq!(
        drawn.x + drawn.w,
        w.viewport().w,
        "the caption still reaches the window's right edge"
    );

    finish_slide(&mut shell, window);
    let w = shell.window(window).expect("a window");
    assert_eq!(
        w.drawn_caption(),
        w.regions().caption,
        "and they meet again"
    );
}

/// The chord's path: a host records `TogglePane` and the shell performs it after
/// the dispatch, through the same call the toggle button reaches.
#[test]
fn a_host_can_toggle_a_pane_by_command() {
    let (mut shell, mut engine, window) = with_left_pane();
    assert!(shell.window(window).expect("a window").regions().sidebar.w > 0.0);
    let mut commands = Commands::new(window);
    commands.push(ShellAction::TogglePane(Side::Left));
    shell.queued.extend(commands.drain());
    shell.drain(Some(&mut engine));
    assert_eq!(
        shell.window(window).expect("a window").regions().sidebar.w,
        0.0,
        "the command shut the column"
    );
}

// ------------------------------------------------- the identity in the pane
//
// Owner, 2026-09-02. The selectors live in the pane header band; the caption
// keeps them only as the merged square while the pane is shut.

fn with_identity_in_pane() -> (ShellState<Host>, TextEngine, WindowKey) {
    paned(true)
}

#[test]
fn with_the_pane_open_the_selectors_are_in_its_header_and_the_caption_slots_are_zero_width() {
    let (shell, _engine, window) = with_identity_in_pane();
    let w = shell.window(window).expect("a window");
    let band = w.pane_header();
    assert_eq!(band.h, theme::CAPTION_H, "the caption's own band");
    assert_eq!(band.w, w.regions().sidebar.w);

    let pane_slots = w.banner().pane_slots();
    for (slot, expected) in [
        (
            banner::pane_profile_slot(band, pane_slots),
            BannerSlot::Profile,
        ),
        (
            banner::pane_workspace_slot(band, pane_slots),
            BannerSlot::Workspace,
        ),
    ] {
        assert!(slot.w > 0.0, "{expected:?} is in the band");
        let (x, y) = slot.centre();
        assert_eq!(shell.banner_hit(window, x, y), Some(expected));
    }

    let caption = w.regions().caption;
    let slots = w.banner().slots();
    assert_eq!(
        banner::profile_slot(caption, slots).w,
        0.0,
        "the caption's banner slots went to zero"
    );
    assert_eq!(banner::workspace_slot(caption, slots).w, 0.0);
    assert!(slots.settings, "Settings never moved");
}

#[test]
fn shutting_the_pane_puts_the_merged_identity_chip_in_the_caption() {
    let (mut shell, mut engine, window) = with_identity_in_pane();
    shell.toggle_pane(window, Side::Left);
    finish_slide(&mut shell, window);
    settle(&mut shell, &mut engine);

    let w = shell.window(window).expect("a window");
    assert_eq!(w.pane_header().w, 0.0, "no band left to draw in");
    assert_eq!(w.banner().pane_slots(), banner::Slots::NONE);
    let chip = banner::profile_slot(w.regions().caption, w.banner().slots());
    assert!(chip.w > 0.0, "the identity is reachable in the caption");
    let (x, y) = chip.centre();
    assert_eq!(shell.banner_hit(window, x, y), Some(BannerSlot::Identity));
}

/// A press on a chip in the band reaches the host as a slot, and its popover
/// hangs under the column rather than being clamped across the divider.
#[test]
fn a_press_in_the_pane_header_opens_its_popover_under_the_column() {
    struct Popovers {
        inner: Host,
    }
    impl ShellHost for Popovers {
        type Event = ();
        fn floor_surface(&mut self, cx: &mut HostCx<'_>, window: WindowKey) -> Box<dyn Surface> {
            self.inner.floor_surface(cx, window)
        }
        fn banner(&self, window: WindowKey) -> Banner {
            self.inner.banner(window)
        }
        fn pane_open(&self, window: WindowKey, side: Side) -> bool {
            self.inner.pane_open(window, side)
        }
        fn identity_in_pane(&self, window: WindowKey) -> bool {
            self.inner.identity_in_pane(window)
        }
        fn slot_pressed(
            &mut self,
            cx: &mut HostCx<'_>,
            window: WindowKey,
            slot: BannerSlot,
        ) -> bool {
            cx.command(window, ShellAction::Popover(Some(slot)));
            self.inner.slots.push(slot);
            true
        }
        fn popover_size(&self, _window: WindowKey, _slot: BannerSlot) -> (f32, f32) {
            (theme::PANEL_W, 180.0)
        }
        fn seat_window(
            &mut self,
            cx: &mut HostCx<'_>,
            window: WindowKey,
        ) -> Result<Box<dyn Surface>, SeatRefused> {
            self.inner.seat_window(cx, window)
        }
    }

    let log: Shared = Rc::default();
    let mut inner = Host::new(log);
    inner.left_pane = true;
    inner.identity_in_pane = true;
    inner.banner = banner_with_labels();
    let mut shell = ShellState::new(Popovers { inner });
    let mut engine = TextEngine::new();
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1200, 800), 1.0);
    let mut scene = Scene::new();
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, 1200.0, 800.0);

    let w = shell.window(window).expect("a window");
    let band = w.pane_header();
    let chip = banner::pane_profile_slot(band, w.banner().pane_slots());
    let (x, y) = chip.centre();
    shell.rich_mouse(
        window,
        &mut engine,
        RichMouseEvent {
            x,
            y,
            button: MouseButton::Left,
            state: KeyState::Pressed,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
    let popover = shell
        .window(window)
        .and_then(ShellWindow::popover)
        .expect("the press opened one");
    assert_eq!(popover.slot, BannerSlot::Profile);
    let bounds = popover.bounds();
    // The chip it came from, not the band behind it: the panel clears the chip
    // by `popover::GAP`, which is Koru's own number and lands it on the band's
    // closing hairline exactly as the reference's does.
    let drawn = banner::chip_band(chip);
    assert_eq!(
        bounds.y - (drawn.y + drawn.h),
        crate::popover::GAP,
        "it cleared the chip it came from: {bounds:?}"
    );
    assert!(
        bounds.x < shell.window(window).expect("a window").regions().content.x,
        "and it hangs under the column, not across the divider: {bounds:?}"
    );
}

// -------------------------------------------------------------- the pane's edge

/// A host whose left pane opens, restores a width, and records the one it is
/// told on release.
struct Resizable {
    inner: Host,
    /// What `pane_width` answers — a restored row, or nothing on a first run.
    restore: Option<f32>,
    /// Every width `pane_resized` was handed, in order. **The length is the
    /// assertion**: a drag that told the host per mouse-move would be a drag
    /// that wrote a file per mouse-move.
    committed: Vec<f32>,
}

impl ShellHost for Resizable {
    type Event = ();
    fn floor_surface(&mut self, cx: &mut HostCx<'_>, window: WindowKey) -> Box<dyn Surface> {
        self.inner.floor_surface(cx, window)
    }
    fn seat_window(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
    ) -> Result<Box<dyn Surface>, SeatRefused> {
        self.inner.seat_window(cx, window)
    }
    fn pane_open(&self, _window: WindowKey, side: Side) -> bool {
        side == Side::Left
    }
    fn pane_width(&self, _window: WindowKey, side: Side) -> Option<f32> {
        (side == Side::Left).then_some(self.restore).flatten()
    }
    fn pane_resized(&mut self, _window: WindowKey, side: Side, width: f32) {
        assert_eq!(side, Side::Left, "only the left column has a grip");
        self.committed.push(width);
    }
}

fn resizable(restore: Option<f32>) -> (ShellState<Resizable>, TextEngine, WindowKey, Shared) {
    let log: Shared = Rc::default();
    let mut shell = ShellState::new(Resizable {
        inner: Host::new(Rc::clone(&log)),
        restore,
        committed: Vec::new(),
    });
    let mut engine = TextEngine::new();
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1200, 800), 1.0);
    let mut scene = Scene::new();
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, 1200.0, 800.0);
    (shell, engine, window, log)
}

fn grip_button(
    shell: &mut ShellState<Resizable>,
    engine: &mut TextEngine,
    window: WindowKey,
    x: f32,
    y: f32,
    state: KeyState,
) {
    MultiWindowApp::rich_mouse(
        shell,
        window,
        engine,
        RichMouseEvent {
            x,
            y,
            button: MouseButton::Left,
            state,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
}

fn finish_left_slide<H: ShellHost>(shell: &mut ShellState<H>, window: WindowKey) {
    let settled = Instant::now() + std::time::Duration::from_millis(1000);
    if let Some(w) = shell.window_mut(window) {
        w.left.advance(settled);
    }
}

/// **The grip is on the column's drawn edge and straddles it**, which is Koru's
/// arrangement (`GRIP` in `apps/koru/src/mainview/layout.rs`): half the band is
/// inside the column and half is over the content, so there is a target either
/// side of the hairline rather than only inside the pane.
///
/// Both halves have to answer, which is the whole reason the shell resolves the
/// grip before `pane_at` and before the content: either of those would claim its
/// own half first.
#[test]
fn the_grip_straddles_the_columns_drawn_edge_and_both_halves_answer() {
    let (mut shell, mut engine, window, _log) = resizable(None);
    let w = shell.window(window).expect("a window");
    let column = w.drawn_pane(Side::Left);
    let grip = w.pane_grip(Side::Left);
    let edge = column.x + column.w;
    assert_eq!(grip.w, crate::panel::GRIP);
    assert_eq!(
        grip.x,
        edge - crate::panel::GRIP * 0.5,
        "centred on the edge"
    );
    assert_eq!((grip.y, grip.h), (column.y, column.h), "the whole edge");

    // A point either side of the hairline says "resize", and both start a drag
    // rather than reaching the pane or the surface behind it.
    let y = column.y + column.h * 0.5;
    for x in [edge - 2.0, edge + 2.0] {
        assert_eq!(
            MultiWindowApp::cursor_icon(&mut shell, window, x, y),
            CursorIcon::ColResize,
            "at {x}"
        );
        grip_button(&mut shell, &mut engine, window, x, y, KeyState::Pressed);
        assert!(
            shell.window(window).expect("a window").pane_drag.is_some(),
            "the press at {x} grabbed the edge"
        );
        grip_button(&mut shell, &mut engine, window, x, y, KeyState::Released);
    }

    // And a shut column has nothing to grab.
    shell.toggle_pane(window, Side::Left);
    finish_left_slide(&mut shell, window);
    assert_eq!(
        shell
            .window(window)
            .expect("a window")
            .pane_grip(Side::Left)
            .w,
        0.0
    );
}

/// **One reflow for the whole drag.** `Panel`'s `full`/`committed` split is what
/// buys it: the column draws at the live width and the content keeps the one it
/// was laid out against until the button comes up. A drag that reflowed per
/// mouse-move would be `Editing::reflow` several times a second, which
/// AGENTS.md's responsiveness rules treat as a bug rather than a trade.
#[test]
fn a_drag_reflows_the_content_once_on_release() {
    let (mut shell, mut engine, window, log) = resizable(None);
    let start = shell
        .window(window)
        .expect("a window")
        .drawn_pane(Side::Left)
        .w;
    let grip = shell
        .window(window)
        .expect("a window")
        .pane_grip(Side::Left);
    let (x, y) = grip.centre();
    log.borrow_mut().bounds.clear();

    grip_button(&mut shell, &mut engine, window, x, y, KeyState::Pressed);
    for step in 1..=20 {
        MultiWindowApp::pointer_moved(&mut shell, window, x + step as f32 * 4.0, y);
    }
    let mid = shell.window(window).expect("a window");
    let drawn = mid.drawn_pane(Side::Left);
    assert!(drawn.w > start, "the column follows the pointer");
    assert_eq!(
        mid.regions().sidebar.w,
        start,
        "and the layout does not, until it is let go"
    );
    // **And the grip is on the edge you can see, not the one the layout still
    // believes in.** They are the same rect at rest and a whole drag apart in
    // the middle of one; a grip on the settled edge would be a band you cannot
    // grab twice without letting go.
    assert_eq!(
        mid.pane_grip(Side::Left).x,
        drawn.x + drawn.w - crate::panel::GRIP * 0.5,
        "the drawn edge, which regions has not caught up with"
    );
    assert!(
        log.borrow().bounds.is_empty(),
        "twenty moves reflowed nothing: {:?}",
        log.borrow().bounds
    );

    grip_button(
        &mut shell,
        &mut engine,
        window,
        x + 80.0,
        y,
        KeyState::Released,
    );
    assert_eq!(
        log.borrow().bounds.len(),
        1,
        "and the release reflowed exactly once"
    );
    let settled = shell.window(window).expect("a window");
    assert_eq!(
        settled.regions().sidebar.w,
        settled.drawn_pane(Side::Left).w
    );
    assert!(settled.pane_drag.is_none());
    assert_eq!(
        shell.host().committed.len(),
        1,
        "the host was told once, on release"
    );
    assert_eq!(shell.host().committed[0], settled.regions().sidebar.w);
}

/// **The width round-trips through the host, and it is clamped at both ends** —
/// on the way out of a drag and on the way back in from a row, because a file
/// somebody edited by hand is not more trustworthy than a pointer. The limits
/// are Koru's `SIDEBAR_MIN_W`/`SIDEBAR_MAX_W`.
#[test]
fn the_width_is_clamped_and_a_restored_one_is_laid_out_against_from_the_first_frame() {
    for (target, expected) in [
        (
            crate::panel::SIDEBAR_MIN_W - 400.0,
            crate::panel::SIDEBAR_MIN_W,
        ),
        (
            crate::panel::SIDEBAR_MAX_W + 400.0,
            crate::panel::SIDEBAR_MAX_W,
        ),
    ] {
        let (mut shell, mut engine, window, _log) = resizable(None);
        let grip = shell
            .window(window)
            .expect("a window")
            .pane_grip(Side::Left);
        let (x, y) = grip.centre();
        grip_button(&mut shell, &mut engine, window, x, y, KeyState::Pressed);
        MultiWindowApp::pointer_moved(&mut shell, window, target, y);
        grip_button(
            &mut shell,
            &mut engine,
            window,
            target,
            y,
            KeyState::Released,
        );
        assert_eq!(shell.host().committed, vec![expected], "asked for {target}");
    }

    // Restored, and the content is laid out against it immediately: nothing is
    // animating and `regions` already has it, so there is no snap on the first
    // release and no motion on startup.
    let (shell, _engine, window, _log) = resizable(Some(340.0));
    let w = shell.window(window).expect("a window");
    assert_eq!(w.regions().sidebar.w, 340.0);
    assert_eq!(w.drawn_pane(Side::Left).w, 340.0);
    assert!(!w.left.animating());

    // And a row nobody could have written by dragging is clamped too.
    let (shell, _engine, window, _log) = resizable(Some(4000.0));
    assert_eq!(
        shell.window(window).expect("a window").regions().sidebar.w,
        crate::panel::SIDEBAR_MAX_W
    );
}

// --------------------------------------------------- the popover's two views

/// A host whose popover has two views of different heights, asking for the swap
/// the way the Terminal's merged identity square does.
struct Views {
    inner: Host,
    tall: bool,
}

const SHORT_H: f32 = 120.0;
const TALL_H: f32 = 300.0;

impl ShellHost for Views {
    type Event = ();
    fn floor_surface(&mut self, cx: &mut HostCx<'_>, window: WindowKey) -> Box<dyn Surface> {
        self.inner.floor_surface(cx, window)
    }
    fn seat_window(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
    ) -> Result<Box<dyn Surface>, SeatRefused> {
        self.inner.seat_window(cx, window)
    }
    fn banner(&self, window: WindowKey) -> Banner {
        self.inner.banner(window)
    }
    fn slot_pressed(&mut self, cx: &mut HostCx<'_>, window: WindowKey, slot: BannerSlot) -> bool {
        cx.command(window, ShellAction::Popover(Some(slot)));
        true
    }
    fn popover_size(&self, _window: WindowKey, _slot: BannerSlot) -> (f32, f32) {
        (theme::PANEL_W, if self.tall { TALL_H } else { SHORT_H })
    }
    /// The press that swaps the view: **the host switches first, then asks**, so
    /// the size the shell goes back for is the arriving view's.
    fn popover_pressed(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        _slot: BannerSlot,
        _x: f32,
        _y: f32,
    ) -> bool {
        self.tall = !self.tall;
        cx.command(window, ShellAction::PopoverSlide(SlideFrom::Left));
        true
    }
}

fn press_at(
    shell: &mut ShellState<Views>,
    engine: &mut TextEngine,
    window: WindowKey,
    x: f32,
    y: f32,
) {
    MultiWindowApp::rich_mouse(
        shell,
        window,
        engine,
        RichMouseEvent {
            x,
            y,
            button: MouseButton::Left,
            state: KeyState::Pressed,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
}

fn with_views() -> (ShellState<Views>, TextEngine, WindowKey) {
    let log: Shared = Rc::default();
    let mut inner = Host::new(log);
    inner.banner = banner_with_labels();
    let mut shell = ShellState::new(Views { inner, tall: false });
    let mut engine = TextEngine::new();
    let window = shell.open_window((1200.0, 800.0), 1.0);
    shell.seated(window, WindowId::from(1u64));
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1200, 800), 1.0);
    let mut scene = Scene::new();
    MultiWindowApp::draw(&mut shell, window, &mut scene, &mut engine, 1200.0, 800.0);
    let w = shell.window(window).expect("a window");
    let chip = banner::profile_slot(w.regions().caption, w.banner().slots());
    let (x, y) = chip.centre();
    press_at(&mut shell, &mut engine, window, x, y);
    (shell, engine, window)
}

/// **A view swap travels, takes the panel's height with it, and settles.**
///
/// The two views are drawn one panel width apart — the arriving one entering
/// from the named edge and the leaving one going the other way — the height
/// interpolates between what the two asked for rather than snapping or being
/// fixed to the taller, and `animating` comes back to `false` on a deadline so
/// the shell's loop stops asking for frames.
#[test]
fn a_view_swap_slides_the_panel_and_settles_at_the_new_height() {
    let (mut shell, mut engine, window) = with_views();
    let opened = shell
        .window(window)
        .and_then(ShellWindow::popover)
        .expect("the chip opened one");
    assert_eq!(opened.bounds().h, SHORT_H);
    assert_eq!(opened.view_offsets(), None, "at rest there is one view");

    // Press a row: the host switches its view and asks for the slide.
    let (x, y) = opened.bounds().centre();
    press_at(&mut shell, &mut engine, window, x, y);
    let start = shell
        .window(window)
        .and_then(ShellWindow::popover)
        .expect("it navigates rather than dismissing");
    assert!(start.animating());
    assert_eq!(start.bounds().h, SHORT_H, "the height starts where it was");
    let (leaving, arriving) = start.view_offsets().expect("two views are on screen");
    let width = start.bounds().w;
    assert_eq!(leaving, 0.0, "the one that was here has not moved yet");
    assert_eq!(
        arriving, -width,
        "and the new one is a panel width off to its left"
    );

    // Half way: both have moved, they are still exactly a width apart, and the
    // height is between the two.
    let half =
        Instant::now() + std::time::Duration::from_millis((theme::POPOVER_FADE_MS / 2.0) as u64);
    MultiWindowApp::maintain(&mut shell, window, half);
    let mid = shell
        .window(window)
        .and_then(ShellWindow::popover)
        .expect("still open");
    let (leaving, arriving) = mid.view_offsets().expect("still travelling");
    assert!(
        leaving > 0.0 && arriving < 0.0,
        "the old one goes right as the new one comes from the left: \
         {leaving} / {arriving}"
    );
    assert!(
        (leaving - arriving - width).abs() < 0.01,
        "one width apart: {leaving} / {arriving}"
    );
    assert!(
        mid.bounds().h > SHORT_H && mid.bounds().h < TALL_H,
        "the height travels with them: {}",
        mid.bounds().h
    );

    // **And it stops.** `animating` must return to false or the loop spins.
    let done = Instant::now() + std::time::Duration::from_millis(theme::POPOVER_FADE_MS as u64 + 8);
    MultiWindowApp::maintain(&mut shell, window, done);
    let settled = shell
        .window(window)
        .and_then(ShellWindow::popover)
        .expect("still open");
    assert!(!settled.animating());
    assert_eq!(settled.view_offsets(), None);
    assert_eq!(settled.bounds().h, TALL_H);
    assert!(!MultiWindowApp::animating(&shell, window));
}

/// **A panel mid-swap is deaf, and it still eats the press.**
///
/// DESIGN's rule for a dismissed popover — it is on screen for the length of
/// its fade and must not answer a click landing on a menu that is visibly
/// leaving — applies to one that is visibly *changing*: half the rows under the
/// pointer belong to a view that is going, and the rest are not where they will
/// be. Swallowing rather than falling through is the other half of it: the
/// press must not reach the column underneath either.
#[test]
fn a_press_on_a_travelling_panel_chooses_nothing_and_does_not_fall_through() {
    let (mut shell, mut engine, window) = with_views();
    let bounds = shell
        .window(window)
        .and_then(ShellWindow::popover)
        .expect("open")
        .bounds();
    let (x, y) = bounds.centre();
    press_at(&mut shell, &mut engine, window, x, y);
    let tall = shell.host().tall;
    assert!(
        shell
            .window(window)
            .and_then(ShellWindow::popover)
            .is_some_and(|popover| popover.animating())
    );

    // A second press while it travels reaches nothing.
    press_at(&mut shell, &mut engine, window, x, y);
    assert_eq!(shell.host().tall, tall, "the host was not asked");
    assert!(
        shell
            .window(window)
            .and_then(ShellWindow::popover)
            .is_some(),
        "and it was not dismissed either"
    );

    // Once it settles the panel answers again.
    let done = Instant::now() + std::time::Duration::from_millis(theme::POPOVER_FADE_MS as u64 + 8);
    MultiWindowApp::maintain(&mut shell, window, done);
    press_at(&mut shell, &mut engine, window, x, y);
    assert_ne!(shell.host().tall, tall, "the settled panel answered");
}

// ---------------------------------------------------------------- display zoom

/// **The zoom is a transform over a divided viewport, and that is the whole of
/// it.**
///
/// Koru's window scales this way and nothing in its chrome reads a zoom: the
/// layout viewport is the logical size over the factor, every region is solved
/// against that, and one affine multiplies the lot back up. A shell that scaled
/// only its content — which is what the Terminal did while its zoom was a font
/// size in `koru-session-view` — leaves the caption, the tabs and the panes at
/// 100%.
#[test]
fn a_zoom_divides_the_layout_viewport_and_scales_the_root() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let before = shell.window(window).expect("a window").viewport();
    assert_eq!(before.w, 1200.0);
    assert_eq!(
        shell.window(window).expect("a window").root(),
        Affine::IDENTITY
    );

    assert!(shell.set_zoom(Some(&mut engine), 2.0));
    let window_state = shell.window(window).expect("a window");
    assert_eq!(window_state.viewport(), Rect::new(0.0, 0.0, 600.0, 400.0));
    assert_eq!(window_state.root(), Affine::scale(2.0));
    assert_eq!(shell.zoom(), 2.0);

    // And it is reversible without drift: the logical size is what was kept.
    assert!(shell.set_zoom(Some(&mut engine), 1.0));
    assert_eq!(shell.window(window).expect("a window").viewport(), before);
}

/// A resize while zoomed re-derives from the **logical** size, not from the
/// already-divided viewport — or every resize would shrink the window again.
#[test]
fn a_resize_while_zoomed_divides_the_new_size_rather_than_the_old_viewport() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    assert!(shell.set_zoom(Some(&mut engine), 2.0));
    MultiWindowApp::resized(&mut shell, window, PhysicalSize::new(1600, 1000), 1.0);
    assert_eq!(
        shell.window(window).expect("a window").viewport(),
        Rect::new(0.0, 0.0, 800.0, 500.0)
    );
}

/// **Every window, and every surface in it** — the reach `system_theme_changed`
/// has, because what a background tab cached was measured against the old
/// space. The active surface is re-bounded on top, so it lays out for the room
/// it can actually show.
#[test]
fn a_zoom_reaches_every_window_and_every_surface() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    open_second_tab(&mut shell, &mut engine, window);
    shell.host_mut().armed = Armed::OpenWindow;
    let key = Key::Named(NamedKey::F1);
    shell.rich_key(window, &mut engine, press(&key));
    let second = *shell
        .windows()
        .iter()
        .find(|candidate| **candidate != window)
        .expect("a second window");
    shell.seated(second, WindowId::from(2u64));
    MultiWindowApp::resized(&mut shell, second, PhysicalSize::new(1200, 800), 1.0);
    settle(&mut shell, &mut engine);

    let invalidated = log.borrow().invalidated;
    let bounds = log.borrow().bounds.len();
    assert!(shell.set_zoom(Some(&mut engine), 1.5));

    // Two tabs in the first window and one in the second: three surfaces, one
    // call each, active or not.
    assert_eq!(
        log.borrow().invalidated - invalidated,
        3,
        "every surface in every window, exactly once"
    );
    // One re-bound active surface per window.
    assert_eq!(log.borrow().bounds.len() - bounds, 2);
    assert_eq!(shell.window(second).expect("a window").zoom(), 1.5);
    assert_eq!(
        shell.window(second).expect("a window").viewport(),
        Rect::new(0.0, 0.0, 800.0, 800.0 / 1.5)
    );
}

/// **The four doors.** A pointer, a caption test and a cursor query all arrive
/// in the window's own logical pixels and are compared against regions solved
/// in the zoomed-down space, so each is divided once — or half the chrome would
/// be unreachable at 200%.
#[test]
fn every_coordinate_the_runner_reports_is_divided_by_the_zoom() {
    let (mut shell, log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let content = shell.content_rect(window).expect("a content rect");
    let (x, y) = (content.x + 40.0, content.y + 30.0);

    // Somewhere on the caption's own draggable band at 100% — found rather
    // than guessed, because which parts of it are furniture is the ladder's
    // business and not this test's.
    let caption = shell.window(window).expect("a window").drawn_caption();
    let band_y = caption.y + caption.h * 0.5;
    let band_x = (0..)
        .map(|step| caption.x + 2.0 * step as f32)
        .take_while(|x| *x < caption.x + caption.w)
        .find(|x| MultiWindowApp::is_caption(&shell, window, *x, band_y))
        .expect("some point of the caption is draggable");
    assert!(!MultiWindowApp::is_caption(&shell, window, band_x, y));

    assert!(shell.set_zoom(Some(&mut engine), 2.0));

    // The same band, twice as far down the glass — which is the point: at 200%
    // the caption is twice as tall and its drag region moved with it.
    assert!(MultiWindowApp::is_caption(
        &shell,
        window,
        band_x * 2.0,
        band_y * 2.0
    ));
    assert!(
        !MultiWindowApp::is_caption(&shell, window, band_x, band_y * 2.0 + caption.h * 2.0),
        "and it does not reach past its own doubled height"
    );

    // A press at twice the coordinates lands on the same point of the surface.
    log.borrow_mut().pointers.clear();
    MultiWindowApp::rich_mouse(
        &mut shell,
        window,
        &mut engine,
        RichMouseEvent {
            x: x * 2.0,
            y: y * 2.0,
            button: MouseButton::Left,
            state: KeyState::Pressed,
            modifiers: ModifiersState::empty(),
            click_count: 1,
        },
    );
    let local = log
        .borrow()
        .pointers
        .last()
        .and_then(|event| event.position())
        .expect("a press reached the surface");
    let zoomed_content = shell.content_rect(window).expect("a content rect");
    assert!(
        (local.0 - (x - zoomed_content.x)).abs() < 0.01
            && (local.1 - (y - zoomed_content.y)).abs() < 0.01,
        "the press landed at {local:?}"
    );
}

/// Koru's numbers, and Koru's snap. The bounds are the shell's clamp; the 0.1
/// grid is what a stepping chord asks for before it sends one.
#[test]
fn the_clamp_and_the_step_are_korus() {
    assert_eq!((ZOOM_MIN, ZOOM_MAX, ZOOM_STEP), (0.5, 2.0, 0.1));
    assert_eq!(stepped_zoom(3.0), ZOOM_MAX);
    assert_eq!(stepped_zoom(0.1), ZOOM_MIN);
    // Snapped to the grid, which is what stops drift accumulating across many
    // in/out cycles.
    assert!((stepped_zoom(1.04) - 1.0).abs() < 1e-6);
    assert!((stepped_zoom(1.06) - 1.1).abs() < 1e-6);

    let (mut shell, _log, mut engine, _window) = shell();
    settle(&mut shell, &mut engine);
    // A *chosen* value — the Settings row offers 125% — is clamped and kept,
    // never snapped: the shell is not the thing that steps.
    assert!(shell.set_zoom(Some(&mut engine), 1.25));
    assert_eq!(shell.zoom(), 1.25);
    assert!(shell.set_zoom(Some(&mut engine), 9.0));
    assert_eq!(shell.zoom(), ZOOM_MAX);
    assert!(
        !shell.set_zoom(Some(&mut engine), ZOOM_MAX),
        "no move, no work"
    );
    assert!(!shell.set_zoom(Some(&mut engine), f32::NAN));
    assert_eq!(shell.zoom(), ZOOM_MAX);
}

/// **The `120%` flash is armed by the zoom, not by the host** (owner,
/// 2026-09-02) — and it lets go of the frame budget when it is over.
///
/// The per-step arithmetic is `zoom_label`'s own tests; this is the wiring:
/// a window nobody has zoomed asks for nothing, a zoom arms it, and it is the
/// shell's `maintain` that eventually puts the window back to sleep.
#[test]
fn a_zoom_arms_the_indicator_and_a_window_at_rest_asks_for_no_frames() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    let now = Instant::now();

    assert!(
        !MultiWindowApp::animating(&shell, window),
        "an untouched window is not animating"
    );
    let idle = MultiWindowApp::maintenance_deadline(&shell, window);

    assert!(shell.set_zoom(Some(&mut engine), 1.5));
    let armed = shell.window(window).expect("a window");
    assert!(armed.zoom_label.showing(), "the zoom armed the flash");
    assert_eq!(armed.zoom_label.alpha(now), Some(1.0));

    // The hold asks for a wake rather than frames — the toast rule.
    assert!(!MultiWindowApp::animating(&shell, window));
    let held = MultiWindowApp::maintenance_deadline(&shell, window);
    assert!(
        held.is_some() && held != idle,
        "the end of the hold is a deadline the window did not have before"
    );

    // A move that changes nothing does not re-arm, because it never runs.
    let deadline = shell
        .window(window)
        .expect("a window")
        .zoom_label
        .maintenance_deadline();
    assert!(!shell.set_zoom(Some(&mut engine), 1.5));
    assert_eq!(
        shell
            .window(window)
            .expect("a window")
            .zoom_label
            .maintenance_deadline(),
        deadline
    );

    // And it is `maintain` that drops it, which is what returns the window to
    // the idle wake.
    assert!(MultiWindowApp::maintain(
        &mut shell,
        window,
        now + Duration::from_secs(3)
    ));
    assert!(!shell.window(window).expect("a window").zoom_label.showing());
    assert!(!MultiWindowApp::animating(&shell, window));
    assert_eq!(MultiWindowApp::maintenance_deadline(&shell, window), idle);
}

/// **The shared floor, zoomed in.** A 720×600 window at 200% lays out in
/// 360×300 — below the floor the caption arithmetic was worked for — and the
/// answer is `regions::solve`'s existing give-way order, not a special case.
/// Koru's window has exactly the same property for exactly the same reason, so
/// this pins that it degrades rather than producing a negative rect.
#[test]
fn the_smallest_window_at_the_largest_zoom_still_solves() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    MultiWindowApp::resized(
        &mut shell,
        window,
        PhysicalSize::new(MIN_WINDOW_SIZE.0 as u32, MIN_WINDOW_SIZE.1 as u32),
        1.0,
    );
    assert!(shell.set_zoom(Some(&mut engine), ZOOM_MAX));
    let content = shell.content_rect(window).expect("a content rect");
    assert!(content.w >= 0.0 && content.h >= 0.0, "{content:?}");
    for side in [Side::Left, Side::Right] {
        let pane = shell.window(window).expect("a window").drawn_pane(side);
        assert!(pane.w >= 0.0 && pane.h >= 0.0, "{side:?}: {pane:?}");
    }
    // And it still draws, which is where a bad rect would show up.
    settle(&mut shell, &mut engine);
}

/// A window opened while the process is zoomed arrives zoomed, rather than
/// laying its first frame out at 100% and snapping.
#[test]
fn a_window_seated_while_zoomed_opens_at_the_zoom() {
    let (mut shell, _log, mut engine, window) = shell();
    settle(&mut shell, &mut engine);
    assert!(shell.set_zoom(Some(&mut engine), 1.5));
    shell.host_mut().armed = Armed::OpenWindow;
    let key = Key::Named(NamedKey::F1);
    shell.rich_key(window, &mut engine, press(&key));
    let second = *shell
        .windows()
        .iter()
        .find(|candidate| **candidate != window)
        .expect("a second window");
    shell.seated(second, WindowId::from(2u64));
    settle(&mut shell, &mut engine);
    assert_eq!(shell.window(second).expect("a window").zoom(), 1.5);
    assert_eq!(
        shell.window(second).expect("a window").viewport().w,
        1200.0 / 1.5
    );
}
