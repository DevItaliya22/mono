// SPDX-License-Identifier: MPL-2.0

use super::modal::{
    Defaults, FRAME_RATES, Hit, MAX_DIM, MIN_DIM, Modal, Outcome, Preset, SizeChoice, suffixed,
};
use koru_draw::text::TextEngine;
use std::path::{Path, PathBuf};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// A clip path that is absolute on the host. The modal refuses a path with no
/// folder, and `C:\videos\…` is a single relative component anywhere but
/// Windows — so a Windows spelling here fails every test on a Mac.
fn clip(name: &str) -> PathBuf {
    let folder = if cfg!(windows) {
        "C:\\videos"
    } else {
        "/videos"
    };
    Path::new(folder).join(name)
}

/// An absolute path whose folder does not exist, spelled for the host.
const MISSING_FOLDER_CLIP: &str = if cfg!(windows) {
    "D:\\no\\such\\folder\\clip.mp4"
} else {
    "/no/such/folder/clip.mp4"
};

fn a_modal() -> (TextEngine, Modal) {
    let mut engine = TextEngine::new();
    let modal = Modal::new(
        &mut engine,
        &Defaults::first_use(),
        &clip("koru-20260916-120000.mp4"),
    );
    (engine, modal)
}

fn type_str(engine: &mut TextEngine, modal: &mut Modal, s: &str) {
    for c in s.chars() {
        let key = Key::Character(c.to_string().into());
        modal.key(engine, &key, Some(&c.to_string()), ModifiersState::empty());
    }
}

/// The first-use modal is valid as it stands: Current size, 60 fps, cursor
/// on, a full path — F9, Enter, recording.
#[test]
fn first_use_commits_immediately() {
    let (_engine, mut modal) = a_modal();
    assert_eq!(modal.problem(), None);
    match modal.activate(Hit::Record) {
        Outcome::Record(request) => {
            assert_eq!(request.size, SizeChoice::Current);
            assert_eq!(request.frame_rate, 60);
            assert!(request.cursor);
            assert_eq!(request.clip, clip("koru-20260916-120000.mp4"));
        }
        other => panic!("expected Record, got {other:?}"),
    }
}

/// A fixed preset carries its exact physical pixels — a portrait preset stays
/// 1080×1920 whatever the display is shaped like.
#[test]
fn a_preset_commits_its_exact_physical_size() {
    let (_engine, mut modal) = a_modal();
    let tiktok = Preset::ALL
        .iter()
        .position(|p| *p == Preset::Tiktok)
        .unwrap();
    modal.activate(Hit::Preset(tiktok));
    match modal.activate(Hit::Record) {
        Outcome::Record(request) => {
            assert_eq!(request.size, SizeChoice::Exact(1080, 1920));
        }
        other => panic!("expected Record, got {other:?}"),
    }
}

/// Letters in the width field disable Record and say so inline — they do not
/// become a 0×0 video.
#[test]
fn an_invalid_custom_size_keeps_record_disabled() {
    let (mut engine, mut modal) = a_modal();
    let custom = Preset::ALL
        .iter()
        .position(|p| *p == Preset::Custom)
        .unwrap();
    modal.activate(Hit::Preset(custom));
    // The defaults are valid; type over the width with nonsense.
    modal.press(&mut engine, Hit::CustomWidth, 0.0);
    modal.custom_width_field().select_all(&mut engine);
    type_str(&mut engine, &mut modal, "abc");
    assert_eq!(modal.problem(), Some(koru_i18n::Key::RecordingProblemSize));
    assert_eq!(modal.activate(Hit::Record), Outcome::Ignored);
}

/// Odd numbers are the same recording, one pixel down — H.264 sizes are even,
/// and the odd WGC report is not a resize (ADR-350).
#[test]
fn an_odd_custom_size_is_adjusted_down_to_even() {
    let (mut engine, mut modal) = a_modal();
    let custom = Preset::ALL
        .iter()
        .position(|p| *p == Preset::Custom)
        .unwrap();
    modal.activate(Hit::Preset(custom));
    modal.press(&mut engine, Hit::CustomWidth, 0.0);
    modal.custom_width_field().select_all(&mut engine);
    type_str(&mut engine, &mut modal, "1281");
    match modal.activate(Hit::Record) {
        Outcome::Record(request) => {
            assert_eq!(request.size, SizeChoice::Exact(1280, 720));
        }
        other => panic!("expected Record, got {other:?}"),
    }
}

/// The floor and the ceiling are refused the same way: 160×160 is the
/// smallest recording, 8192 the largest the encoder takes.
#[test]
fn the_size_bounds_are_enforced() {
    assert!(super::modal::parse_dim(&MIN_DIM.to_string()).is_some());
    assert!(super::modal::parse_dim(&MAX_DIM.to_string()).is_some());
    assert!(super::modal::parse_dim(&(MIN_DIM - 2).to_string()).is_none());
    assert!(super::modal::parse_dim(&(MAX_DIM + 2).to_string()).is_none());
    assert!(super::modal::parse_dim("0").is_none());
    assert!(super::modal::parse_dim("").is_none());
}

/// An empty destination is the one thing F9 → type → Enter cannot paper over.
#[test]
fn an_empty_path_keeps_record_disabled() {
    let (mut engine, mut modal) = a_modal();
    modal.press(&mut engine, Hit::Path, 0.0);
    modal.path_field().select_all(&mut engine);
    modal.path_field().set_text(&mut engine, "");
    // The field edit above bypasses key routing; recompute as a key would.
    modal.key(
        &mut engine,
        &Key::Named(NamedKey::ArrowLeft),
        None,
        ModifiersState::empty(),
    );
    assert_eq!(modal.problem(), Some(koru_i18n::Key::RecordingProblemPath));
    assert_eq!(modal.activate(Hit::Record), Outcome::Ignored);
}

/// A bare file name is a working-directory path, not a destination — refused
/// before anything locks.
#[test]
fn a_bare_file_name_is_not_a_destination() {
    let (mut engine, mut modal) = a_modal();
    modal.press(&mut engine, Hit::Path, 0.0);
    modal.path_field().select_all(&mut engine);
    modal.path_field().set_text(&mut engine, "koru.mp4");
    match modal.commit_with(|_| true) {
        Outcome::Ignored => {}
        other => panic!("expected Ignored, got {other:?}"),
    }
    assert_eq!(modal.problem(), Some(koru_i18n::Key::RecordingProblemPath));
}

/// The plan's "parent exists": a destination in a folder that is not there is
/// refused at *Record* with the inline problem — nothing locks, nothing
/// starts. The existence question is injected; the suite has no disk.
#[test]
fn a_missing_destination_folder_is_refused_at_record() {
    let (mut engine, mut modal) = a_modal();
    modal.press(&mut engine, Hit::Path, 0.0);
    modal.path_field().select_all(&mut engine);
    modal
        .path_field()
        .set_text(&mut engine, MISSING_FOLDER_CLIP);
    match modal.commit_with(|_| false) {
        Outcome::Ignored => {}
        other => panic!("expected Ignored, got {other:?}"),
    }
    assert_eq!(modal.problem(), Some(koru_i18n::Key::RecordingProblemPath));
}

/// Escape cancels, and F9 — which opened the modal — puts it away again.
/// Neither records anything.
#[test]
fn escape_and_f9_both_cancel() {
    let (mut engine, mut modal) = a_modal();
    assert_eq!(
        modal.key(
            &mut engine,
            &Key::Named(NamedKey::Escape),
            None,
            ModifiersState::empty()
        ),
        Outcome::Close
    );
    let (mut engine, mut modal) = a_modal();
    assert_eq!(
        modal.key(
            &mut engine,
            &Key::Named(NamedKey::F9),
            None,
            ModifiersState::empty()
        ),
        Outcome::Close
    );
}

/// Tab walks the whole ring and comes back to where it began; the custom
/// fields join the ring only while *Custom…* is chosen.
#[test]
fn the_focus_ring_matches_the_visible_controls() {
    let (mut engine, mut modal) = a_modal();
    assert_eq!(modal.focus(), Some(Hit::Record));
    let custom = Preset::ALL
        .iter()
        .position(|p| *p == Preset::Custom)
        .unwrap();
    modal.activate(Hit::Preset(custom));
    let home = modal.focus();
    // The ring with *Custom…* chosen: 7 presets, 2 custom fields, 2 frame
    // rates, 2 cursor chips, path, Browse, Record, Cancel.
    let ring_len = Preset::ALL.len() + 2 + FRAME_RATES.len() + 2 + 1 + 1 + 1 + 1;
    let mut seen = vec![home];
    for _ in 0..ring_len {
        modal.key(
            &mut engine,
            &Key::Named(NamedKey::Tab),
            None,
            ModifiersState::empty(),
        );
        seen.push(modal.focus());
    }
    assert!(seen.contains(&Some(Hit::CustomWidth)));
    assert!(seen.contains(&Some(Hit::CustomHeight)));
    assert!(seen.contains(&Some(Hit::Browse)));
    assert!(seen.contains(&Some(Hit::Cancel)));
    // And it is a ring: a full lap lands back where it began.
    assert_eq!(seen.last(), Some(&home));
    // Without *Custom…* the two fields leave the ring — focus can never point
    // at a hidden control.
    let (mut engine, mut modal) = a_modal();
    for _ in 0..(Preset::ALL.len() + FRAME_RATES.len() + 2 + 1 + 1 + 1 + 1) {
        modal.key(
            &mut engine,
            &Key::Named(NamedKey::Tab),
            None,
            ModifiersState::empty(),
        );
        assert_ne!(modal.focus(), Some(Hit::CustomWidth));
        assert_ne!(modal.focus(), Some(Hit::CustomHeight));
    }
}

/// Frame rate and cursor chips hold their choice; the default frame rate is
/// the first of the table, 60.
#[test]
fn chips_hold_their_choice() {
    let (_engine, mut modal) = a_modal();
    assert_eq!(modal.frame_rate(), FRAME_RATES[0]);
    modal.activate(Hit::FrameRate(1));
    assert_eq!(modal.frame_rate(), 30);
    modal.activate(Hit::Cursor(false));
    assert!(!modal.cursor());
    match modal.activate(Hit::Record) {
        Outcome::Record(request) => {
            assert_eq!(request.frame_rate, 30);
            assert!(!request.cursor);
        }
        other => panic!("expected Record, got {other:?}"),
    }
}

/// A taken name is suffixed, never overwritten and never a refusal.
#[test]
fn an_existing_clip_name_is_suffixed() {
    let path = &clip("koru.mp4");
    assert_eq!(suffixed(path, |_| false), clip("koru.mp4"));
    assert_eq!(
        suffixed(path, |p| p == clip("koru.mp4")),
        clip("koru-1.mp4")
    );
    assert_eq!(
        suffixed(path, |p| !p.ends_with("koru-2.mp4")),
        clip("koru-2.mp4")
    );
}

// ---------------------------------------------------------------- the lifecycle

use super::{COUNTDOWN, Lifecycle};
use crate::mainview::MainView;
use crate::mainview::context_menu::MenuAction;
use crate::mainview::harness::*;
use koru_window::WindowRequest;
use koru_workspace_host::{RecordingEvent, View};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Arm a view whose *Record* has just settled: the modal gone, the countdown
/// standing, the lock asked for — the first-use preset, so the lock is the
/// current size.
fn armed_with_recording() -> (MainView, koru_workspace::Workspace) {
    let (mut v, w) = on_document();
    assert!(v.open_recording_modal());
    let outcome = v.recording_modal.as_mut().unwrap().activate(Hit::Record);
    v.settle_recording(outcome);
    assert!(v.recording_modal.is_none(), "Record closed the modal");
    assert!(
        matches!(v.recording_lifecycle, Some(Lifecycle::Countdown { .. })),
        "the countdown is standing"
    );
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::LockCurrentSize),
        "the first-use preset locks the current size"
    );
    (v, w)
}

/// Drive an armed view through the drop and the clean frame into `Starting`,
/// and return the config the start carried.
fn drive_to_starting(v: &mut MainView) -> koru_window::RecordingRequest {
    // At +3 s the overlay drops.
    assert!(v.maintain_recording(Instant::now() + COUNTDOWN + Duration::from_millis(1)));
    assert!(
        matches!(v.recording_lifecycle, Some(Lifecycle::CleanPresent)),
        "the overlay dropped into the clean present"
    );
    // No start yet — the clean frame has not been drawn.
    assert_eq!(View::take_window_request(v), None);
    v.clean_frame_drawn = true;
    // The next maintain lets the start out.
    assert!(!v.maintain_recording(Instant::now() + COUNTDOWN + Duration::from_millis(2)));
    assert!(matches!(v.recording_lifecycle, Some(Lifecycle::Starting)));
    match View::take_window_request(v) {
        Some(WindowRequest::StartRecording(config)) => config,
        other => panic!("expected StartRecording, got {other:?}"),
    }
}

/// The countdown drops at three seconds, and only a drawn clean frame lets
/// the start go out — the race that would put a numeral in the clip.
#[test]
fn the_start_waits_for_a_clean_frame() {
    let (mut v, _w) = armed_with_recording();
    let config = drive_to_starting(&mut v);
    assert_eq!(config.frame_rate, 60, "the first-use default");
    assert!(config.cursor, "the first-use default");
    let name = config
        .clip
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert!(
        name.starts_with("koru-") && name.ends_with(".mp4"),
        "{name}"
    );
}

/// F9 during the countdown aborts: the lock is undone, nothing was ever
/// asked of the shell.
#[test]
fn f9_aborts_the_countdown() {
    let (mut v, _w) = armed_with_recording();
    assert!(v.toggle_recording());
    assert!(v.recording_lifecycle.is_none(), "aborted to Idle");
    assert!(v.recording_request.is_none());
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::UnlockSizeAndRestore),
        "the window is let go"
    );
}

/// Escape is the same take-back, through the key chain this time.
#[test]
fn escape_aborts_the_countdown() {
    let (mut v, mut w) = armed_with_recording();
    let handled = View::key(
        &mut v,
        &mut w.ws,
        &Key::Named(NamedKey::Escape),
        None,
        ModifiersState::empty(),
    );
    assert!(handled, "Escape was the countdown's, not the draft's");
    assert!(v.recording_lifecycle.is_none());
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::UnlockSizeAndRestore)
    );
}

/// Started → Recording → F9 → Stopping → Stopped: the whole arc, with the
/// toast saying where the clip went.
#[test]
fn the_recording_arc_start_stop_saved() {
    let (mut v, _w) = armed_with_recording();
    drive_to_starting(&mut v);
    assert!(v.recording_event_impl(RecordingEvent::Started {
        origin: Instant::now(),
        dpi_scale_milli: 1000,
        size: (1920, 1080),
    }));
    assert!(matches!(
        v.recording_lifecycle,
        Some(Lifecycle::Recording { .. })
    ));
    // F9 stops.
    assert!(v.toggle_recording());
    assert!(matches!(v.recording_lifecycle, Some(Lifecycle::Stopping)));
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::StopRecording)
    );
    // The finalize worker's answer: idle, the request spent, and the saved
    // line is a toast with the path in it.
    let clip = clip("koru.mp4");
    assert!(v.recording_event_impl(RecordingEvent::Stopped {
        clip_path: clip.clone(),
    }));
    assert!(v.recording_lifecycle.is_none());
    assert!(v.recording_request.is_none());
    assert!(v.toast.is_some(), "the saved notice is on screen");
}

/// A stop asked while the shell's start worker held the answer lands the
/// moment `Started` does.
#[test]
fn a_stop_asked_during_starting_lands_on_started() {
    let (mut v, _w) = armed_with_recording();
    drive_to_starting(&mut v);
    assert!(v.toggle_recording(), "F9 during Starting");
    assert!(v.recording_stop_when_started, "the stop is remembered");
    assert_eq!(
        View::take_window_request(&mut v),
        None,
        "nothing is asked of a start still in flight"
    );
    assert!(v.recording_event_impl(RecordingEvent::Started {
        origin: Instant::now(),
        dpi_scale_milli: 1000,
        size: (1920, 1080),
    }));
    assert!(matches!(v.recording_lifecycle, Some(Lifecycle::Stopping)));
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::StopRecording)
    );
}

/// A failed start claims no success: idle, the failure said, and the shell
/// has already unlocked — so the view asks nothing more.
#[test]
fn a_failed_start_is_told_honestly() {
    let (mut v, _w) = armed_with_recording();
    drive_to_starting(&mut v);
    assert!(v.recording_event_impl(RecordingEvent::Failed {
        code: "capture-start-failed",
    }));
    assert!(v.recording_lifecycle.is_none());
    assert!(v.recording_request.is_none());
    assert!(v.toast.is_some(), "the failure is said, not swallowed");
    assert_eq!(
        View::take_window_request(&mut v),
        None,
        "the unlock is the shell's half of the failure path"
    );
}

/// A close mid-recording vetoes, stops and finalizes first — and only then
/// is the close re-asked. The `moov` atom is never sacrificed.
#[test]
fn a_close_during_the_recording_stops_first_then_closes() {
    let (mut v, _w) = armed_with_recording();
    drive_to_starting(&mut v);
    assert!(v.recording_event_impl(RecordingEvent::Started {
        origin: Instant::now(),
        dpi_scale_milli: 1000,
        size: (1920, 1080),
    }));
    assert!(View::request_close(&mut v), "the close is vetoed");
    assert!(matches!(v.recording_lifecycle, Some(Lifecycle::Stopping)));
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::StopRecording)
    );
    assert!(v.recording_event_impl(RecordingEvent::Stopped {
        clip_path: clip("koru.mp4"),
    }));
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::Close),
        "the close is re-asked once the clip is safe"
    );
}

/// A close during the countdown needs no finalize: abort, unlock, and the
/// next maintain re-asks the close — the abort path has no event of its own.
#[test]
fn a_close_during_the_countdown_aborts_then_re_asks() {
    let (mut v, _w) = armed_with_recording();
    assert!(View::request_close(&mut v), "the close is vetoed");
    assert!(v.recording_lifecycle.is_none());
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::UnlockSizeAndRestore)
    );
    assert!(v.maintain_recording(Instant::now()));
    assert_eq!(
        View::take_window_request(&mut v),
        Some(WindowRequest::Close)
    );
}

/// One recording per process: while another window holds it, F9 opens
/// nothing here.
#[test]
fn f9_is_disarmed_while_another_window_records() {
    // RECORDING_ACTIVE is process-global: the lock serializes this flip
    // against the routing suite's parallel reader (TESTING.md's rule).
    let _guard = koru_recording::RECORDING_ACTIVE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (mut v, _w) = on_document();
    koru_recording::RECORDING_ACTIVE.store(true, Ordering::SeqCst);
    assert!(v.toggle_recording());
    assert!(
        v.recording_modal.is_none(),
        "another window's recording dims the gesture"
    );
    koru_recording::RECORDING_ACTIVE.store(false, Ordering::SeqCst);
    assert!(v.toggle_recording());
    assert!(v.recording_modal.is_some(), "free again once it ends");
}

/// The menu row reads off the lifecycle: Start when idle, Stop while any
/// part of the lifecycle stands.
#[test]
fn the_menu_row_reads_start_or_stop_off_the_lifecycle() {
    let (mut v, _w) = on_document();
    // The rows exist only where the gate opens (M31); elsewhere there is no
    // row to read, which the context menu's own suite pins.
    if !(crate::mainview::context_menu::OVERFLOW_HIDDEN_ROWS_SHOWN
        && cfg!(windows)
        && koru_channel::ReleaseChannel::this_build_is_internal())
    {
        return;
    }
    let idle =
        crate::mainview::context_menu::window_items(false, false, v.recording_lifecycle.is_some());
    assert!(idle.contains(&MenuAction::HeaderStartRecording));
    assert!(!idle.contains(&MenuAction::HeaderStopRecording));
    v.recording_lifecycle = Some(Lifecycle::Recording {
        origin: Instant::now(),
    });
    let live =
        crate::mainview::context_menu::window_items(false, false, v.recording_lifecycle.is_some());
    assert!(live.contains(&MenuAction::HeaderStopRecording));
    assert!(!live.contains(&MenuAction::HeaderStartRecording));
}
