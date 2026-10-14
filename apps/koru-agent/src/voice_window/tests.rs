// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::dictation_coordinator::{Phase, Refusal, Snapshot};

fn snapshot(phase: Phase, level: u8) -> Snapshot {
    Snapshot {
        phase,
        level: koru_dictation::LevelBucket::new(level),
        display: Some(crate::global_input::TargetDisplay {
            work_area: (0, 0, 1920, 1080),
            dpi: 96,
        }),
        ..Snapshot::default()
    }
}

#[test]
fn hidden_host_has_no_window_draw_level_animation_or_deadline_work() {
    let mut host = VoiceWindowHost::default();
    let now = Instant::now();
    assert!(!host.visible());
    assert!(!host.sync_hud(snapshot(Phase::Idle, 8), false, now));
    assert!(!host.maintain(now));
    assert_eq!(host.maintenance_deadline(), None);
    assert!(!host.animating());
    assert_eq!(host.work_counts(), WorkCounts::default());
}

#[test]
fn hidden_after_a_visible_flight_ignores_repeated_idle_levels_and_maintenance() {
    let mut host = VoiceWindowHost::default();
    let now = Instant::now();
    assert!(host.sync_hud(snapshot(Phase::Listening, 4), false, now));
    assert!(host.sync_hud(snapshot(Phase::Idle, 0), false, now));
    assert!(!host.visible());
    let hidden = host.work_counts();
    for level in 0..=8 {
        assert!(!host.sync_hud(snapshot(Phase::Idle, level), false, now));
        assert!(!host.maintain(now + Duration::from_secs(60)));
    }
    assert_eq!(host.work_counts(), hidden);
    assert_eq!(host.maintenance_deadline(), None);
    assert!(!host.animating());
}

#[test]
fn settings_surface_has_precedence_over_hud() {
    let mut host = VoiceWindowHost::default();
    host.open_surface(koru_voice_ui::VoiceSurface::new(
        koru_i18n::Lang::En,
        koru_voice_ui::ViewModel::default(),
    ));
    assert!(!host.sync_hud(snapshot(Phase::Listening, 4), false, Instant::now()));
    assert!(host.has_surface());
    assert!(!host.has_hud());
    assert_eq!(
        host.window_activation(),
        koru_window::WindowActivation::Focused
    );
}

#[test]
fn active_levels_finalising_and_one_terminal_deadline_drive_only_visible_work() {
    let mut host = VoiceWindowHost::default();
    let now = Instant::now();
    assert!(host.sync_hud(snapshot(Phase::Listening, 0), false, now));
    assert_eq!(
        host.window_activation(),
        koru_window::WindowActivation::Passive
    );
    assert!(host.window(None).transparent);
    let generation = host.generation();
    assert!(!host.sync_hud(snapshot(Phase::Listening, 0), false, now));
    assert_eq!(host.generation(), generation);
    assert!(host.sync_hud(snapshot(Phase::Listening, 8), false, now));
    assert_eq!(host.work_counts().level_updates, 1);
    assert!(host.sync_hud(snapshot(Phase::Finalising, 0), false, now));
    assert!(host.sync_hud(Snapshot::default(), false, now));
    assert!(!host.visible());

    let refused = Snapshot {
        refusal: Some(Refusal::Disabled),
        display: snapshot(Phase::Idle, 0).display,
        ..Snapshot::default()
    };
    assert!(host.sync_hud(refused, true, now));
    let deadline = host.maintenance_deadline().expect("one terminal deadline");
    assert!(!host.maintain(deadline - Duration::from_millis(1)));
    assert!(host.maintain(deadline));
    assert_eq!(host.work_counts().deadline_expirations, 1);
    assert_eq!(host.maintenance_deadline(), None);
    assert!(!host.visible());
}

#[test]
fn settings_window_stays_opaque_while_the_hud_window_is_transparent() {
    let mut host = VoiceWindowHost::default();
    assert!(host.sync_hud(snapshot(Phase::Listening, 4), false, Instant::now()));
    assert!(host.window(None).transparent);

    host.open_surface(koru_voice_ui::VoiceSurface::new(
        koru_i18n::Lang::En,
        koru_voice_ui::ViewModel::default(),
    ));
    assert!(!host.window(None).transparent);
}

#[test]
fn a_live_session_clears_a_previous_terminal_linger() {
    let mut host = VoiceWindowHost::default();
    let now = Instant::now();
    let refused = Snapshot {
        refusal: Some(Refusal::Disabled),
        display: snapshot(Phase::Idle, 0).display,
        ..Snapshot::default()
    };
    assert!(host.sync_hud(refused, true, now));
    let stale = host.maintenance_deadline().expect("terminal deadline");
    assert!(host.sync_hud(snapshot(Phase::Listening, 3), false, now));
    assert_eq!(host.maintenance_deadline(), None);
    assert!(!host.maintain(stale));
    assert!(host.has_hud());
}

#[test]
fn mixed_dpi_secondary_monitor_placement_uses_its_physical_work_area() {
    let placed = placement_for(crate::global_input::TargetDisplay {
        work_area: (-2560, 40, 0, 1400),
        dpi: 144,
    });
    let width = (koru_voice_ui::HUD_WIDTH * 1.5).round() as i32;
    let height = (koru_voice_ui::HUD_HEIGHT * 1.5).round() as i32;
    assert_eq!(placed.position.x, -2560 + (2560 - width) / 2);
    assert_eq!(placed.position.y, 1400 - height - 36);
}

#[cfg(windows)]
#[test]
#[ignore = "interactive Windows proof: focus an editable control on a secondary monitor whose DPI differs from the primary monitor"]
fn passive_hud_lifecycle_preserves_exact_foreground_target_on_mixed_dpi_monitor() {
    use crate::global_input::{InsertBackend, TargetState};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, UpdateWindow,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GA_ROOT, GWL_EXSTYLE, GetAncestor, GetForegroundWindow,
        GetWindowLongPtrW, GetWindowThreadProcessId, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE,
        SWP_NOACTIVATE, SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    eprintln!("Focus the mixed-DPI editable target now; capture begins in 8 seconds.");
    for remaining in (1..=8).rev() {
        eprintln!("  {remaining}...");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let mut backend = crate::global_input::windows::WindowsInsertBackend::new()
        .expect("UI Automation on an ordinary interactive desktop");
    let target = backend
        .capture_target()
        .expect("focus an editable control before starting this proof");
    let foreground = unsafe { GetForegroundWindow() };
    let root = unsafe { GetAncestor(foreground, GA_ROOT) };
    assert!(
        !root.is_null(),
        "the focused control must have a root window"
    );
    let mut pid = 0;
    assert_ne!(unsafe { GetWindowThreadProcessId(root, &mut pid) }, 0);

    let monitor = unsafe { MonitorFromWindow(root, MONITOR_DEFAULTTONEAREST) };
    assert!(!monitor.is_null());
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    assert_ne!(unsafe { GetMonitorInfoW(monitor, &mut info) }, 0);
    let dpi = unsafe { GetDpiForWindow(root) };
    assert_ne!(
        dpi,
        unsafe { GetDpiForSystem() },
        "focus the target on a monitor whose DPI differs from the primary monitor"
    );
    let placement = placement_for(crate::global_input::TargetDisplay {
        work_area: (
            info.rcWork.left,
            info.rcWork.top,
            info.rcWork.right,
            info.rcWork.bottom,
        ),
        dpi,
    });
    let scale = dpi as f64 / 96.0;
    let width = (f64::from(koru_voice_ui::HUD_WIDTH) * scale).round() as i32;
    let height = (f64::from(koru_voice_ui::HUD_HEIGHT) * scale).round() as i32;

    let class = wide("STATIC");
    let listening = wide("Listening");
    let hud = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            class.as_ptr(),
            listening.as_ptr(),
            WS_POPUP,
            placement.position.x,
            placement.position.y,
            width,
            height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    assert!(!hud.is_null(), "create the proof HUD");
    struct HudGuard(windows_sys::Win32::Foundation::HWND);
    impl Drop for HudGuard {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }
    let _guard = HudGuard(hud);
    assert_ne!(
        unsafe { GetWindowLongPtrW(hud, GWL_EXSTYLE) } & WS_EX_NOACTIVATE as isize,
        0
    );
    assert_ne!(
        unsafe {
            SetWindowPos(
                hud,
                HWND_TOPMOST,
                placement.position.x,
                placement.position.y,
                width,
                height,
                SWP_NOACTIVATE,
            )
        },
        0
    );

    let mut prove_target = || {
        let current = unsafe { GetForegroundWindow() };
        assert_eq!(unsafe { GetAncestor(current, GA_ROOT) }, root);
        let mut current_pid = 0;
        assert_ne!(
            unsafe { GetWindowThreadProcessId(root, &mut current_pid) },
            0
        );
        assert_eq!(current_pid, pid);
        assert_eq!(
            backend.target_state(&target),
            TargetState::Editable,
            "the exact captured UIA element must remain focused"
        );
    };

    unsafe { ShowWindow(hud, SW_SHOWNOACTIVATE) };
    prove_target();
    for level in 0..=8 {
        let label = wide(&format!("Listening {level}"));
        assert_ne!(unsafe { SetWindowTextW(hud, label.as_ptr()) }, 0);
        assert_ne!(unsafe { UpdateWindow(hud) }, 0);
        prove_target();
    }
    let finalising = wide("Finishing");
    assert_ne!(unsafe { SetWindowTextW(hud, finalising.as_ptr()) }, 0);
    assert_ne!(unsafe { UpdateWindow(hud) }, 0);
    prove_target();
    unsafe {
        SendMessageW(hud, WM_LBUTTONDOWN, 1, 0);
        SendMessageW(hud, WM_LBUTTONUP, 0, 0);
    }
    prove_target();
    unsafe { ShowWindow(hud, SW_HIDE) };
    prove_target();
}
