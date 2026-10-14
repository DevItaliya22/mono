// SPDX-License-Identifier: MPL-2.0

//! **The exact-target mechanism, against a real window, with real `SendInput`.**
//!
//! ```text
//! cargo test -p koru-windows-input --lib windows::native -- --ignored --nocapture
//! ```
//!
//! ## Why it is `#[ignore]`d, and why that is not the usual reason
//!
//! Every other interactive proof in this tree is ignored because it opens a
//! window. This one is ignored because it **types**. `SendInput` delivers to
//! whatever has keyboard focus at the instant it runs, so a proof of this
//! mechanism is a proof that puts characters somewhere on a real desktop. Run it
//! on a machine somebody is working at and the somewhere may be their editor.
//!
//! So it does two things no other proof here has to:
//!
//! - **It owns its target end to end.** It creates the window, it knows the
//!   `HWND`, it knows the process id, and the control it types into is a plain
//!   Win32 `EDIT` of its own. Nothing it touches belongs to anyone else.
//! - **It refuses rather than guesses.** Before a single input event is built,
//!   it proves the focused window is one *it created*, in *this* process. If
//!   Windows declines to give it the foreground — which is exactly what happens
//!   when somebody else's application is in front, because the foreground lock
//!   denies a background process — the proof **fails and types nothing**. A
//!   failure here means *run this when you have the machine*, and never *the
//!   mechanism is broken*.
//!
//! ## What it establishes
//!
//! That the shipped mechanism, unchanged, delivers a phrase **once** into the
//! exact control it captured, and refuses in each of the ways it claims to:
//!
//! | | |
//! | --- | --- |
//! | identity | the captured target's focus window, root window and both pids are the ones this proof created |
//! | delivery | the phrase arrives in that control, complete, exactly once, with its non-ASCII characters intact |
//! | foreground change | focus moves to a second window of ours and the same target is `Changed`; `insert_unicode` refuses and the first control is untouched |
//! | destroyed target | the window is destroyed and the target is `Destroyed`; `insert_unicode` refuses |
//! | COM lifetime | the apartment is initialised, used and dropped on one thread, and that thread is not the Agent's event loop |
//!
//! ## What it does **not** establish, and what is still owed
//!
//! **It is not the roadmap's baseline.** Plan 3.1 requires the existing Agent
//! dictation path driven into a *real third-party editable application* in an
//! exclusive interactive session, before and after the extraction, compared. A
//! self-owned `EDIT` control is not a third-party application: it has no web
//! view, no custom provider, no hosted control with its own focus `HWND`, and no
//! IME. It is the lower-level harness the plan asks for *in addition*, not the
//! gate. `docs/features/accessibility/README.md` holds the gate.
//!
//! Also uncovered here, each for a reason rather than an oversight:
//!
//! - **Secure desktop.** A UAC prompt or the lock screen cannot be armed from a
//!   test, and must not be simulated: the whole value of that refusal is that
//!   it is the *real* desktop switch. `WindowsKeyState` and
//!   `OrdinaryDesktop::is_current` are the code; a person is the evidence.
//! - **Mixed DPI.** `apps/koru-agent/src/voice_window/tests.rs` already carries
//!   an interactive proof for a secondary monitor whose DPI differs.
//! - **A partial or rejected `SendInput` count.** Neither can be induced
//!   deliberately without lying about what happened; `classify_direct_insertion`
//!   is pinned by unit test and the outcomes are a person's to see in the wild.
//! - **Held modifiers on cancellation.** Structurally not applicable to *this*
//!   mechanism: `unicode_inputs` builds `KEYEVENTF_UNICODE` scan-code pairs and
//!   presses no modifier at all, so there is none to leave down. It becomes a
//!   real case the moment Plan 3.2 adds chord primitives.
//! - **A hung provider.** The fake half is `apps/koru-demo/src/supervise/tests.rs`,
//!   which proves bounded regain of control against a mechanism that never
//!   returns. Wedging a *real* UI Automation provider on demand is not something
//!   this proof can arrange, and pretending otherwise would be worse than the
//!   gap.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{IUIAutomationValuePattern, UIA_ValuePatternId};
// `SetFocus` is a keyboard-and-mouse binding rather than a windowing one: it is
// about which window receives key events, which is the same distinction
// `GetGUIThreadInfo`'s `hwndFocus` draws against `GetForegroundWindow`.
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetForegroundWindow, MSG, PM_REMOVE,
    PeekMessageW, SendMessageW, SetForegroundWindow, TranslateMessage, WINDOW_EX_STYLE, WM_SETTEXT,
    WS_BORDER, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::w;

use super::*;

/// The phrase this proof types. **Synthetic, and it says so.**
///
/// §7.4: automated artifacts use synthetic fixtures. It also carries a
/// non-ASCII letter and a CJK pair so that a UTF-16 stream that lost a code
/// unit fails the read-back rather than passing on ASCII.
const PHRASE: &str = "koru a11y proof ünïcode 漢字 42";

/// Generous, and for the documented reason: the first `WM_GETOBJECT` against a
/// window starts UI Automation itself on this machine.
const LIMIT: Duration = Duration::from_secs(20);

/// What the proof saw. Printed whole before any assertion, so a failing run is
/// readable rather than one line about the first thing that went wrong.
#[derive(Debug, Default)]
struct Findings {
    /// Whether Windows let this process take the foreground at all. `false`
    /// means the proof stopped before building a single input event.
    foreground_taken: bool,
    /// The window this proof created, and the one the mechanism captured.
    /// Equal, or the proof refuses.
    created_window: isize,
    captured_focus_window: isize,
    captured_root_window: isize,
    this_process: u32,
    captured_focus_pid: u32,
    captured_root_pid: u32,
    /// The restore hint, captured the way the Agent captures it before consent.
    hint_root_window: isize,
    hint_pid: u32,
    state_before_delivery: Option<TargetState>,
    delivery: Option<DirectInsertion>,
    /// Read back through the captured element's own `ValuePattern`, so what is
    /// checked is what the control holds rather than what was sent.
    value_after_delivery: Option<String>,
    /// The same read, later, with nothing else sent. A second copy here would
    /// be a duplicate delivery, which is the failure that matters most.
    value_after_settling: Option<String>,
    state_after_foreground_change: Option<TargetState>,
    delivery_after_foreground_change: Option<DirectInsertion>,
    value_after_refused_delivery: Option<String>,
    state_after_target_destroyed: Option<TargetState>,
    delivery_after_target_destroyed: Option<DirectInsertion>,
    /// The thread COM was initialised, used and released on.
    com_thread: Option<String>,
    error: Option<String>,
}

#[test]
#[ignore = "interactive Windows proof: creates its own editable window and delivers real synthetic keystrokes into it with SendInput"]
fn the_mechanism_delivers_once_into_the_exact_control_it_captured_and_refuses_otherwise() {
    let mut found = Findings::default();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prove(&mut found)));
    if let Err(panic) = outcome {
        found.error = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .or(Some("the proof panicked".to_owned()));
    }
    println!("{found:#?}");

    assert_eq!(found.error, None, "the proof could not finish");
    assert!(
        found.foreground_taken,
        "Windows would not give this process the foreground, so nothing was typed and nothing \
         was proved. That is the safe outcome, not a defect: run this proof on a machine you \
         have to yourself, from a console that is already in front."
    );

    // Identity: everything the mechanism captured is the window this proof made.
    assert_eq!(found.captured_focus_window, found.created_window);
    assert_eq!(found.captured_root_window, found.created_window);
    assert_eq!(found.hint_root_window, found.created_window);
    assert_eq!(found.captured_focus_pid, found.this_process);
    assert_eq!(found.captured_root_pid, found.this_process);
    assert_eq!(found.hint_pid, found.this_process);

    // Delivery: once, complete, into that control, with its characters intact.
    assert_eq!(found.state_before_delivery, Some(TargetState::Editable));
    assert_eq!(found.delivery, Some(DirectInsertion::Complete));
    assert_eq!(
        found.value_after_delivery.as_deref(),
        Some(PHRASE),
        "the control holds exactly the phrase — not a prefix, not two copies"
    );
    assert_eq!(
        found.value_after_settling.as_deref(),
        Some(PHRASE),
        "a second copy here is a duplicate delivery, which is the failure that would put \
         somebody's words in twice"
    );

    // Refusals.
    assert_eq!(
        found.state_after_foreground_change,
        Some(TargetState::Changed),
        "focus moved to another window and the captured target must not still be editable"
    );
    assert_eq!(
        found.delivery_after_foreground_change,
        Some(DirectInsertion::TargetUnavailable),
        "the platform's own re-proof, immediately before SendInput, is what has to refuse here"
    );
    assert_eq!(
        found.value_after_refused_delivery.as_deref(),
        Some(PHRASE),
        "a refused delivery must leave the control exactly as it was"
    );
    assert_eq!(
        found.state_after_target_destroyed,
        Some(TargetState::Destroyed)
    );
    assert_eq!(
        found.delivery_after_target_destroyed,
        Some(DirectInsertion::TargetUnavailable)
    );

    assert!(found.com_thread.is_some());
}

fn prove(found: &mut Findings) {
    found.this_process = unsafe { GetCurrentProcessId() };
    found.com_thread = Some(format!("{:?}", std::thread::current().id()));

    // Be polite about the desktop we borrowed.
    let previous_foreground = unsafe { GetForegroundWindow() };

    let pump = Pump::start();
    let target_window = pump.create("Koru global-input proof");
    found.created_window = target_window;

    // **The guard.** Everything below this point can type, so nothing below it
    // runs until the foreground is provably ours.
    found.foreground_taken = pump.take_foreground(target_window);
    if !found.foreground_taken {
        pump.stop();
        restore_foreground(previous_foreground);
        return;
    }

    // The mechanism, constructed on this thread, exactly as the dictation
    // worker constructs it.
    let mut backend = WindowsInsertBackend::new().expect("a UI Automation client");

    let hint = backend
        .capture_restore_hint()
        .expect("the foreground is ours, so a hint must be capturable");
    found.hint_root_window = hint.root.0 as isize;
    found.hint_pid = hint.pid;

    let target = backend
        .capture_target_for_hint(&hint)
        .expect("the focused control is ours");
    found.captured_focus_window = target.focus_window.0 as isize;
    found.captured_root_window = target.restore_window.0 as isize;
    found.captured_focus_pid = target.focus_pid;
    found.captured_root_pid = target.restore_pid;

    // **The second guard, and the one that matters.** The mechanism refuses on
    // its own if focus moved, but this proof must never be the thing that finds
    // out. If what was captured is not ours, stop here having typed nothing.
    assert_eq!(
        target.focus_pid, found.this_process,
        "the captured control belongs to another process; refusing to send input"
    );
    assert_eq!(
        target.focus_window.0 as isize, target_window,
        "the captured control is not the window this proof created; refusing to send input"
    );

    found.state_before_delivery = Some(backend.target_state(&target));
    found.delivery = Some(backend.insert_unicode(&target, PHRASE));

    found.value_after_delivery = await_value(&target, PHRASE);
    // Nothing is sent in this window of time; a second copy appearing would be
    // the duplicate that matters.
    std::thread::sleep(Duration::from_millis(400));
    found.value_after_settling = read_value(&target);

    // A foreground change, made by us, to a window that is also ours.
    let decoy = pump.create("Koru global-input proof — decoy");
    let _ = pump.take_foreground(decoy);
    found.state_after_foreground_change = Some(backend.target_state(&target));
    found.delivery_after_foreground_change =
        Some(backend.insert_unicode(&target, "THIS MUST NOT ARRIVE"));
    found.value_after_refused_delivery = read_value(&target);

    // A destroyed target.
    pump.destroy(target_window);
    let deadline = Instant::now() + LIMIT;
    loop {
        let state = backend.target_state(&target);
        if state == TargetState::Destroyed || Instant::now() >= deadline {
            found.state_after_target_destroyed = Some(state);
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    found.delivery_after_target_destroyed =
        Some(backend.insert_unicode(&target, "THIS MUST NOT ARRIVE EITHER"));

    // The apartment goes when the backend does, on this thread.
    drop(target);
    drop(backend);

    pump.destroy(decoy);
    pump.stop();
    restore_foreground(previous_foreground);
}

/// Read the captured element's value through its own `ValuePattern`.
///
/// Deliberately *through the element the mechanism captured*, rather than by
/// asking the window for its text: the claim is "the phrase went into that
/// control", and a `WM_GETTEXT` to the window would be a different question with
/// the same answer only by luck.
fn read_value(target: &WindowsTarget) -> Option<String> {
    let pattern: IUIAutomationValuePattern =
        unsafe { target.element.GetCurrentPatternAs(UIA_ValuePatternId) }.ok()?;
    unsafe { pattern.CurrentValue() }
        .ok()
        .map(|v| v.to_string())
}

fn await_value(target: &WindowsTarget, wanted: &str) -> Option<String> {
    let deadline = Instant::now() + LIMIT;
    let mut last = None;
    while Instant::now() < deadline {
        last = read_value(target);
        if last.as_deref() == Some(wanted) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    last
}

fn restore_foreground(window: HWND) {
    if !window.0.is_null() {
        let _ = unsafe { SetForegroundWindow(window) };
    }
}

/// A thread that owns every window this proof creates, and pumps their messages.
///
/// The windows have to be on a thread with a message loop or `SendInput`'s
/// keystrokes are never translated into `WM_CHAR` and the control stays empty —
/// which would look exactly like a delivery failure. It is a *second* thread
/// because the mechanism's COM apartment lives on the test thread, the same
/// split the dictation worker has.
struct Pump {
    orders: std::sync::mpsc::Sender<Order>,
    made: std::sync::mpsc::Receiver<isize>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

enum Order {
    Create(&'static str),
    Destroy(isize),
}

impl Pump {
    fn start() -> Self {
        let (orders, work) = channel::<Order>();
        let (built, made) = channel::<isize>();
        let running = Arc::new(AtomicBool::new(true));
        let thread = std::thread::Builder::new()
            .name("koru-agent global-input proof windows".to_owned())
            .spawn({
                let running = Arc::clone(&running);
                move || {
                    let mut windows: Vec<isize> = Vec::new();
                    while running.load(Ordering::SeqCst) {
                        while let Ok(order) = work.try_recv() {
                            match order {
                                Order::Create(title) => {
                                    let window = create_edit_window(title);
                                    windows.push(window);
                                    let _ = built.send(window);
                                }
                                Order::Destroy(window) => {
                                    windows.retain(|kept| *kept != window);
                                    let _ = unsafe {
                                        DestroyWindow(HWND(window as *mut core::ffi::c_void))
                                    };
                                }
                            }
                        }
                        let mut message = MSG::default();
                        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool()
                        {
                            // Required: `SendInput`'s `KEYEVENTF_UNICODE` events
                            // arrive as `WM_KEYDOWN` with `VK_PACKET`, and it is
                            // `TranslateMessage` that turns them into the
                            // `WM_CHAR` an EDIT control acts on.
                            let _ = unsafe { TranslateMessage(&message) };
                            unsafe { DispatchMessageW(&message) };
                        }
                        std::thread::sleep(Duration::from_millis(4));
                    }
                    for window in windows {
                        let _ = unsafe { DestroyWindow(HWND(window as *mut core::ffi::c_void)) };
                    }
                }
            })
            .expect("a thread");
        Self {
            orders,
            made,
            running,
            thread: Some(thread),
        }
    }

    fn create(&self, title: &'static str) -> isize {
        self.orders.send(Order::Create(title)).expect("the pump");
        let window = self
            .made
            .recv_timeout(LIMIT)
            .expect("the pump made a window");
        assert!(window != 0, "the window could not be created");
        window
    }

    fn destroy(&self, window: isize) {
        let _ = self.orders.send(Order::Destroy(window));
        // Give the pump a pass to act on it, so the caller's next question is
        // about a window that has actually gone.
        std::thread::sleep(Duration::from_millis(100));
    }

    /// Ask Windows for the foreground, and answer honestly about whether it
    /// arrived.
    ///
    /// `SetForegroundWindow`'s `BOOL` reports whether the attempt was honoured,
    /// not whether focus is ours *now*, so the answer is read back from
    /// `GetForegroundWindow` instead — the same distinction
    /// `InsertBackend::restore_target` makes and for the same reason.
    fn take_foreground(&self, window: isize) -> bool {
        let handle = HWND(window as *mut core::ffi::c_void);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let _ = unsafe { SetForegroundWindow(handle) };
            let _ = unsafe { SetFocus(Some(handle)) };
            if unsafe { GetForegroundWindow() } == handle {
                // One pass for the focus change to settle in the pump.
                std::thread::sleep(Duration::from_millis(150));
                return unsafe { GetForegroundWindow() } == handle;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A top-level, visible, empty `EDIT` control.
///
/// The predefined `EDIT` window class rather than one of our own: it needs no
/// `RegisterClassExW` and no window procedure, it is a real Win32 editable
/// control with a real UI Automation `ValuePattern`, and its whole existence is
/// this test's. What it is **not** is a third-party application — see the module
/// note on what this proof does not establish.
fn create_edit_window(title: &'static str) -> isize {
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            &windows::core::HSTRING::from(title),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_BORDER,
            120,
            120,
            640,
            120,
            None,
            None,
            None,
            None,
        )
    };
    let Ok(window) = window else {
        return 0;
    };
    // Start empty, so a read-back that finds the phrase found only the phrase.
    unsafe { SendMessageW(window, WM_SETTEXT, Some(WPARAM(0)), Some(LPARAM(0))) };
    window.0 as isize
}
