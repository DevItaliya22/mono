// SPDX-License-Identifier: MPL-2.0

//! Separately invoked proof of the new runner primitives. The accepted Agent
//! instrument in `windows::native` is deliberately unchanged.
//!
//! `cargo test -p koru-windows-input --lib workflow::native -- --ignored --nocapture`
//!
//! Run from a foreground console on an exclusive interactive Windows desktop.
//! All input targets are this test's own EDIT windows; failure to obtain their
//! foreground stops the proof. Never automatically invoke this from a suite.
//! This covers neither a hung native provider, mixed DPI, real secure desktop,
//! forced partial injection nor an input race precisely between UIA reads.

use super::*;
use crate::AutomationClient;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{IUIAutomationValuePattern, UIA_ValuePatternId};
use windows::Win32::UI::Controls::EM_GETSEL;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_MINIMIZE,
    SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WM_SETTEXT, WS_BORDER, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::w;

const PHRASE: &str = "M3 native ünïcode 漢字 42";
const REPLACEMENT: &str = "M3 native ünïcode 漢字 42!";
const LIMIT: Duration = Duration::from_secs(20);

#[derive(Debug, Default)]
struct Findings {
    foreground_owned: bool,
    root: isize,
    pid: u32,
    element_pid: Option<i32>,
    element_window: Option<isize>,
    focus: Option<Result<(), Failure>>,
    click: Option<Result<(), Failure>>,
    unicode: Option<Result<(), Failure>>,
    value_after_delivery: Option<String>,
    value_after_settling: Option<String>,
    chord_replacement: Option<String>,
    injected_modifiers_released: bool,
    changed_bounds: bool,
    moved_click: Option<Result<(), Failure>>,
    revoked_text: Option<Result<(), Failure>>,
    revoked_focus: Option<Result<(), Failure>>,
    revoked_click: Option<Result<(), Failure>>,
    revoked_chord: Option<Result<(), Failure>>,
    value_after_revocation: Option<String>,
    dead_owner: Option<Result<(), Failure>>,
    wrong_pid: Option<Result<(), Failure>>,
    changed_foreground: Option<Result<(), Failure>>,
    target_after_refusal: Option<String>,
    decoy_after_refusal: Option<String>,
    minimized: Option<Result<(), Failure>>,
    destroyed: Option<Result<(), Failure>>,
    error: Option<String>,
}

#[test]
#[ignore = "interactive Windows proof: creates owned EDIT windows and sends real clicks, Unicode and chords; needs a foreground console and exclusive desktop"]
fn runner_input_uses_current_targets_and_revocation_prevents_further_delivery() {
    let mut found = Findings::default();
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prove(&mut found)))
    {
        found.error = Some(
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "native proof panicked".into()),
        );
    }
    println!("{found:#?}");
    assert_eq!(
        found.error, None,
        "proof did not finish; partial Findings are not a pass"
    );
    assert!(
        found.foreground_owned,
        "foreground refused; no input proof was performed"
    );
    assert_eq!(found.element_pid, Some(found.pid as i32));
    assert_eq!(found.element_window, Some(found.root));
    assert_eq!(found.focus, Some(Ok(())));
    assert_eq!(found.click, Some(Ok(())));
    assert_eq!(found.unicode, Some(Ok(())));
    assert_eq!(found.value_after_delivery.as_deref(), Some(PHRASE));
    assert_eq!(found.value_after_settling.as_deref(), Some(PHRASE));
    assert_eq!(found.chord_replacement.as_deref(), Some(REPLACEMENT));
    assert!(found.injected_modifiers_released);
    assert!(found.changed_bounds);
    assert_eq!(found.moved_click, Some(Ok(())));
    for result in [
        found.revoked_text,
        found.revoked_focus,
        found.revoked_click,
        found.revoked_chord,
    ] {
        assert_eq!(result, Some(Err(Failure::Cancelled)));
    }
    assert_eq!(found.value_after_revocation.as_deref(), Some(REPLACEMENT));
    assert_eq!(found.dead_owner, Some(Err(Failure::TargetDestroyed)));
    assert_eq!(found.wrong_pid, Some(Err(Failure::TargetChanged)));
    assert_eq!(found.changed_foreground, Some(Err(Failure::TargetChanged)));
    assert_eq!(found.target_after_refusal.as_deref(), Some(REPLACEMENT));
    assert_eq!(found.decoy_after_refusal.as_deref(), Some(""));
    // Minimization may transfer foreground before the iconic-state check.
    assert!(matches!(
        found.minimized,
        Some(Err(Failure::Unavailable | Failure::TargetChanged))
    ));
    assert_eq!(found.destroyed, Some(Err(Failure::TargetDestroyed)));
}

fn prove(found: &mut Findings) {
    let pump = Pump::start();
    let root = pump.create();
    found.root = root.0 as isize;
    found.pid = unsafe { GetCurrentProcessId() };
    found.foreground_owned = pump.foreground(root);
    if !found.foreground_owned {
        return;
    }

    // Client precedes all elements in declaration order so elements drop first.
    // The COM apartment is constructed, used and dropped on this test thread.
    let client = AutomationClient::open_v8().expect("UI Automation client");
    let automation = client.automation();
    let target = unsafe { automation.ElementFromHandle(root) }.expect("owned EDIT element");
    found.element_pid = unsafe { target.CurrentProcessId() }.ok();
    found.element_window = unsafe { target.CurrentNativeWindowHandle() }
        .ok()
        .map(|h| h.0 as isize);
    assert_eq!(
        found.element_pid,
        Some(found.pid as i32),
        "no input into another process"
    );
    assert_eq!(
        found.element_window,
        Some(found.root),
        "no input into another window"
    );
    let desktop = OrdinaryDesktop::capture().expect("ordinary desktop");
    let permit = DeliveryPermit::default();
    let owner_alive = || unsafe { IsWindow(Some(root)) }.as_bool();
    let context = Context {
        automation,
        root,
        pid: found.pid,
        desktop: &desktop,
        permit: &permit,
        owner_alive: &owner_alive,
    };

    found.focus = Some(context.execute(&target, Action::Focus));
    assert_eq!(found.focus, Some(Ok(())));
    wait_until(|| {
        unsafe { automation.GetFocusedElement() }
            .ok()
            .is_some_and(|focused| {
                unsafe { automation.CompareElements(&target, &focused) }
                    .is_ok_and(|same| same.as_bool())
            })
    });
    found.click = Some(context.execute(&target, Action::Click { right: false }));
    assert_eq!(found.click, Some(Ok(())));
    found.unicode = Some(context.execute(&target, Action::Text(PHRASE)));
    found.value_after_delivery = await_value(&target, PHRASE);
    // Observation interval, not synchronization: a duplicate arriving later
    // must change the independently read value and fail the proof.
    std::thread::sleep(Duration::from_millis(400));
    found.value_after_settling = read_value(&target);
    assert_eq!(found.value_after_settling.as_deref(), Some(PHRASE));

    // Ctrl+Home, Ctrl+Shift+End select this single-line EDIT's whole value.
    // A replacement rather than an append proves real chord processing.
    context
        .execute(&target, Action::Chord(&[VK_CONTROL.0, VK_HOME.0]))
        .expect("Ctrl+Home");
    wait_until(|| selection(root) == (0, 0) && modifiers_released());
    context
        .execute(
            &target,
            Action::Chord(&[VK_CONTROL.0, VK_SHIFT.0, VK_END.0]),
        )
        .expect("select to end");
    wait_until(|| {
        selection(root) == (0, PHRASE.encode_utf16().count() as u32) && modifiers_released()
    });
    context
        .execute(&target, Action::Text(REPLACEMENT))
        .expect("replace selection");
    found.chord_replacement = await_value(&target, REPLACEMENT);
    wait_until(|| {
        [VK_CONTROL.0, VK_SHIFT.0, VK_HOME.0, VK_END.0]
            .into_iter()
            .all(|key| unsafe { GetAsyncKeyState(i32::from(key)) } >= 0)
    });
    found.injected_modifiers_released = true;

    let old_bounds = unsafe { target.CurrentBoundingRectangle() }.expect("old bounds");
    pump.order(Order::Move(root.0 as isize));
    wait_until(|| {
        unsafe { target.CurrentBoundingRectangle() }.is_ok_and(|bounds| bounds != old_bounds)
    });
    found.changed_bounds = true;
    found.moved_click = Some(context.execute(&target, Action::Click { right: false }));

    permit.revoke();
    found.revoked_text = Some(context.execute(&target, Action::Text("must not arrive")));
    found.revoked_focus = Some(context.execute(&target, Action::Focus));
    found.revoked_click = Some(context.execute(&target, Action::Click { right: false }));
    found.revoked_chord = Some(context.execute(&target, Action::Chord(&[VK_BACK.0])));
    std::thread::sleep(Duration::from_millis(200));
    found.value_after_revocation = read_value(&target);

    let fresh_permit = DeliveryPermit::default();
    let live_context = Context {
        permit: &fresh_permit,
        ..context
    };
    let dead = || false;
    found.dead_owner = Some(
        Context {
            owner_alive: &dead,
            ..live_context
        }
        .execute(&target, Action::Text("must not arrive")),
    );
    found.wrong_pid = Some(
        Context {
            pid: found.pid.wrapping_add(1),
            ..live_context
        }
        .execute(&target, Action::Text("must not arrive")),
    );

    let decoy_window = pump.create();
    assert!(
        pump.foreground(decoy_window),
        "could not give own decoy foreground"
    );
    let decoy = unsafe { automation.ElementFromHandle(decoy_window) }.expect("own decoy");
    found.changed_foreground = Some(live_context.execute(&target, Action::Text("must not arrive")));
    std::thread::sleep(Duration::from_millis(200));
    found.target_after_refusal = read_value(&target);
    found.decoy_after_refusal = read_value(&decoy);

    assert!(pump.foreground(root), "could not return to owned target");
    pump.order(Order::Minimize(root.0 as isize));
    wait_until(|| unsafe { IsIconic(root) }.as_bool());
    found.minimized = Some(live_context.execute(&target, Action::Text("must not arrive")));
    pump.order(Order::Destroy(root.0 as isize));
    wait_until(|| !unsafe { IsWindow(Some(root)) }.as_bool());
    found.destroyed = Some(live_context.execute(&target, Action::Text("must not arrive")));
}

fn modifiers_released() -> bool {
    [VK_CONTROL.0, VK_SHIFT.0, VK_HOME.0, VK_END.0]
        .into_iter()
        .all(|key| unsafe { GetAsyncKeyState(i32::from(key)) } >= 0)
}

// Physical selection state of the owned native EDIT, not an assumption that
// SendInput returning means the application already processed the chord.
fn selection(window: HWND) -> (u32, u32) {
    let mut start = 0u32;
    let mut end = 0u32;
    unsafe {
        SendMessageW(
            window,
            EM_GETSEL,
            Some(WPARAM((&mut start as *mut u32) as usize)),
            Some(LPARAM((&mut end as *mut u32) as isize)),
        )
    };
    (start, end)
}

fn read_value(element: &IUIAutomationElement) -> Option<String> {
    let pattern: IUIAutomationValuePattern =
        unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId) }.ok()?;
    unsafe { pattern.CurrentValue() }
        .ok()
        .map(|v| v.to_string())
}

fn await_value(element: &IUIAutomationElement, expected: &str) -> Option<String> {
    let deadline = Instant::now() + LIMIT;
    loop {
        let actual = read_value(element);
        if actual.as_deref() == Some(expected) || Instant::now() >= deadline {
            return actual;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + LIMIT;
    loop {
        if condition() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native fixture state deadline expired"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

enum Order {
    Create,
    Foreground(isize),
    Move(isize),
    Minimize(isize),
    Destroy(isize),
}

struct Pump {
    orders: mpsc::Sender<Order>,
    windows: mpsc::Receiver<isize>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Pump {
    fn start() -> Self {
        let (orders, work) = mpsc::channel();
        let (built, windows) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));
        let active = running.clone();
        let thread = std::thread::spawn(move || {
            let mut owned = Vec::new();
            while active.load(Ordering::Acquire) {
                while let Ok(order) = work.try_recv() {
                    match order {
                        Order::Create => {
                            let window = unsafe {
                                CreateWindowExW(
                                    WINDOW_EX_STYLE(0),
                                    w!("EDIT"),
                                    w!(""),
                                    WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_BORDER,
                                    120,
                                    120,
                                    640,
                                    160,
                                    None,
                                    None,
                                    None,
                                    None,
                                )
                            }
                            .expect("owned EDIT window");
                            unsafe {
                                SendMessageW(window, WM_SETTEXT, Some(WPARAM(0)), Some(LPARAM(0)))
                            };
                            owned.push(window);
                            let _ = built.send(window.0 as isize);
                        }
                        Order::Foreground(value) => {
                            let window = HWND(value as *mut core::ffi::c_void);
                            let _ = unsafe { SetForegroundWindow(window) };
                            let _ = unsafe { SetFocus(Some(window)) };
                        }
                        Order::Move(value) => {
                            let _ = unsafe {
                                SetWindowPos(
                                    HWND(value as *mut core::ffi::c_void),
                                    None,
                                    220,
                                    180,
                                    620,
                                    180,
                                    SWP_NOZORDER,
                                )
                            };
                        }
                        Order::Minimize(value) => {
                            let _ = unsafe {
                                ShowWindow(HWND(value as *mut core::ffi::c_void), SW_MINIMIZE)
                            };
                        }
                        Order::Destroy(value) => {
                            let window = HWND(value as *mut core::ffi::c_void);
                            owned.retain(|kept| *kept != window);
                            let _ = unsafe { DestroyWindow(window) };
                        }
                    }
                }
                let mut message = MSG::default();
                while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    let _ = unsafe { TranslateMessage(&message) };
                    unsafe { DispatchMessageW(&message) };
                }
                std::thread::sleep(Duration::from_millis(4));
            }
            for window in owned {
                let _ = unsafe { DestroyWindow(window) };
            }
        });
        Self {
            orders,
            windows,
            running,
            thread: Some(thread),
        }
    }

    fn order(&self, order: Order) {
        self.orders.send(order).expect("native fixture pump");
    }

    fn create(&self) -> HWND {
        self.order(Order::Create);
        HWND(
            self.windows
                .recv_timeout(LIMIT)
                .expect("created fixture window") as *mut core::ffi::c_void,
        )
    }

    fn foreground(&self, window: HWND) -> bool {
        self.order(Order::Foreground(window.0 as isize));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if unsafe { GetForegroundWindow() } == window {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
