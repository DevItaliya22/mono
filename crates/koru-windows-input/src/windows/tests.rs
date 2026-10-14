// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn unicode_input_pairs_preserve_utf16_units_and_flags() {
    let units: Vec<u16> = "A\u{1f600}\n".encode_utf16().collect();
    let inputs = unicode_inputs("A\u{1f600}\n");
    assert_eq!(inputs.len(), units.len() * 2);
    for (pair, unit) in inputs.chunks_exact(2).zip(units) {
        let down = unsafe { pair[0].Anonymous.ki };
        let up = unsafe { pair[1].Anonymous.ki };
        assert_eq!(pair[0].r#type, INPUT_KEYBOARD);
        assert_eq!(pair[1].r#type, INPUT_KEYBOARD);
        assert_eq!(down.wVk.0, 0);
        assert_eq!(down.wScan, unit);
        assert_eq!(down.dwFlags, KEYEVENTF_UNICODE);
        assert_eq!(down.time, 0);
        assert_eq!(down.dwExtraInfo, 0);
        assert_eq!(up.wVk.0, 0);
        assert_eq!(up.wScan, unit);
        assert_eq!(up.dwFlags, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        assert_eq!(up.time, 0);
        assert_eq!(up.dwExtraInfo, 0);
    }
}

#[test]
fn send_input_counts_classify_complete_zero_and_partial() {
    assert_eq!(classify_direct_insertion(8, 8), DirectInsertion::Complete);
    assert_eq!(classify_direct_insertion(0, 8), DirectInsertion::Rejected);
    assert_eq!(classify_direct_insertion(3, 8), DirectInsertion::Partial);
}

#[test]
fn a_restore_hint_distinguishes_a_closed_window_from_one_another_process_took_over() {
    // The hint is captured before consent and is the only thing held across
    // a Voice session, so what it can and cannot tell apart is the whole of
    // what restoration is allowed to assume.
    assert_eq!(classify_restore_hint(true, Some(11), 11), Ok(()));
    assert_eq!(
        classify_restore_hint(false, Some(11), 11),
        Err(Refusal::TargetDestroyed)
    );
    // A handle Windows recycled into another process's window. The pid
    // catches it; the handle alone would have said yes.
    assert_eq!(
        classify_restore_hint(true, Some(99), 11),
        Err(Refusal::TargetChanged)
    );
    // A window whose thread has gone while the handle lingers: no pid at
    // all is not the pid that was recorded.
    assert_eq!(
        classify_restore_hint(true, None, 11),
        Err(Refusal::TargetChanged)
    );
}

#[test]
fn a_root_handle_reused_inside_one_process_passes_the_hint_and_is_caught_at_the_target() {
    // **The limit, written down rather than left to be discovered.** A
    // `WindowsRestoreHint` is a root HWND and a PID. If that window closes
    // and Windows hands the same handle to a *new window of the same
    // process*, both fields still agree and the hint cannot tell.
    //
    // That is not a hole in dictation, because a hint never authorises
    // insertion by itself: the target capture that follows re-derives the
    // focused control and its root, and a focused control that no longer
    // hangs from the recorded root is Changed. The hint corroborates; the
    // element is the identity. `guide/contributing/deps/WIN32.md`.
    assert_eq!(classify_restore_hint(true, Some(11), 11), Ok(()));
    assert_eq!(
        classify_target_identity(true, Some(11), 11, true, Some(11), 11, false),
        TargetState::Changed
    );
}

#[test]
fn restore_identity_rejects_reused_or_reparented_handles_without_equating_pids() {
    assert_eq!(
        classify_target_identity(true, Some(11), 11, true, Some(22), 22, true,),
        TargetState::Editable
    );
    for missing in [
        classify_target_identity(false, Some(11), 11, true, Some(22), 22, true),
        classify_target_identity(true, Some(11), 11, false, Some(22), 22, true),
    ] {
        assert_eq!(missing, TargetState::Destroyed);
    }
    for changed in [
        classify_target_identity(true, Some(99), 11, true, Some(22), 22, true),
        classify_target_identity(true, Some(11), 11, true, Some(99), 22, true),
        classify_target_identity(true, Some(11), 11, true, Some(22), 22, false),
    ] {
        assert_eq!(changed, TargetState::Changed);
    }
}

#[test]
fn a_named_process_requires_both_the_element_and_the_captured_root() {
    assert_eq!(classify_named_process(11, 11, 11), Ok(()));
    for (element, root, expected) in [(22, 11, 11), (11, 22, 11), (0, 0, 0), (-1, 11, 11)] {
        assert_eq!(
            classify_named_process(element, root, expected),
            Err(Refusal::TargetChanged)
        );
    }
}

#[test]
fn only_a_stale_uia_element_is_classified_as_destroyed() {
    use windows::core::{Error, HRESULT};
    assert_eq!(
        element_refusal(Error::from_hresult(HRESULT(
            UIA_E_ELEMENTNOTAVAILABLE as i32
        ))),
        Refusal::TargetDestroyed,
    );
    assert_eq!(
        element_refusal(Error::from_hresult(windows::Win32::Foundation::E_FAIL)),
        Refusal::TargetUnavailable,
    );
}
