// SPDX-License-Identifier: MPL-2.0

//! Something only the window itself can do.
//!
//! A view that draws its own caption row has to be able to move, minimise,
//! maximise and close the window — none of which it can reach, since a `Window`
//! is the loop's to hold. So it asks, the same way it asks to open a document.
//!
//! ## Two of these the loop does not perform
//!
//! [`WindowRequest::Close`] and [`WindowRequest::HideToTray`] are decisions
//! only the application can make: whether closing this window ends the process,
//! leaves a tray icon behind, or merely puts a panel away is a fact about the
//! app, not about the window. So the loop performs the other six and honours
//! these two by re-reading [`WindowApp::should_exit`] and
//! [`WindowApp::wants_window`], which the app has already answered.
//!
//! [`WindowApp::should_exit`]: crate::WindowApp::should_exit
//! [`WindowApp::wants_window`]: crate::WindowApp::wants_window

use std::path::PathBuf;

use crate::snap;

/// A window-level action a view asked for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WindowRequest {
    /// Begin an OS window drag. **Must be performed in the same press that
    /// asked for it** — winit documents that `drag_window` only works when the
    /// left button was pressed immediately before.
    Drag,
    /// Restore this window if minimized and make its application active.
    /// Used when another process asks an existing surface to come forward.
    Focus,
    Minimise,
    ToggleMaximise,
    /// The window should go. **The loop does not act on this**; see the module
    /// note. The app has already recorded what it means.
    Close,
    /// Set this window's `WindowLevel` to `AlwaysOnTop` or back to `Normal`.
    /// Per-window: only the window that asked is affected.
    TogglePin,
    /// Put the application in the notification area instead of quitting it —
    /// Ctrl and the ✕. Every window goes, and the tray icon is the way back.
    ///
    /// **Falls back to [`Close`] if there is no tray to go to.** An app with no
    /// window and no icon is a process nobody can reach, still holding whatever
    /// it opened. Like `Close`, the loop does not act on this.
    ///
    /// [`Close`]: WindowRequest::Close
    HideToTray,
    /// Launch the updater's inert progress window, then close this Koru window.
    /// Only the `--test-upgrade` banner asks for this; a real install remains
    /// owned by the Agent and reaches the updater after every window has saved.
    TestUpgrade,
    /// Persist a new choice for what ✕ on the last window does. **The loop does
    /// not act on this either**: the shell holds the live
    /// [`DesktopPreferences`](crate::DesktopPreferences) copy it saves on exit,
    /// so the value has to land there — a view writing the file itself would be
    /// clobbered by that stale copy. The standalone window runner has no such
    /// copy and declines it.
    SetCloseBehavior(crate::desktop_preferences::CloseBehavior),
    /// Move or resize this window — half-screen snap, across displays, centred,
    /// or back to its default. See [`crate::snap`].
    Place(snap::Placement),
    /// Hold this window at exactly `width`×`height` **physical** pixels — a
    /// recording size, which is why the size is never clamped to the work
    /// area: a 1920×1080 preset means those pixels on any display scale.
    /// **The loop does not act on this**: remembering the previous geometry,
    /// lifting the minimum size so a size like 640×360 is not clamped back to
    /// it, and refusing the resize border, snap rows and maximise while held
    /// are app policy (`koru-workspace-host`), not windowing.
    PlaceAndLockExact {
        width: u32,
        height: u32,
    },
    /// The same hold at the window's **current** size. Separate from
    /// [`PlaceAndLockExact`](WindowRequest::PlaceAndLockExact) rather than a
    /// sentinel pair of zeroes: only the shell can read the real inner size,
    /// and it reads it at apply time, so an in-flight resize cannot hand the
    /// lock a stale rectangle.
    LockCurrentSize,
    /// Undo [`PlaceAndLockExact`](WindowRequest::PlaceAndLockExact): the
    /// remembered geometry — or the maximised state — returns, the minimum
    /// size is re-applied, and the resize border works again. The loop does
    /// not act on this either, for the same reason.
    UnlockSizeAndRestore,
    /// **Start capturing this window** (SESSIONRECORD). The loop does not act
    /// on this: binding the HWND to a Windows Graphics Capture session,
    /// waiting for the first encoded frame on a worker, and answering the
    /// view are the shell's (`koru-workspace-host`) — which is also what
    /// refuses a second start while one is live, the enforcement half of
    /// `koru_recording::RECORDING_ACTIVE`.
    StartRecording(RecordingRequest),
    /// Stop the live capture and finalize the clip. The shell's finalize
    /// worker writes the file and only then restores the geometry the lock
    /// remembered, so a restore can never resize the window into the clip's
    /// last frames. The view's telemetry snapshot joins this payload with
    /// I5; today the stop is the whole message.
    StopRecording,
}

/// What a session recording needs from the shell (SESSIONRECORD).
///
/// **The size is deliberately not here.** The lock holds the window at the
/// chosen size and the shell reads the real geometry off the HWND at capture
/// start — a view-side copy could have been made stale by the resize the
/// lock itself just performed. The bitrate is derived from that geometry the
/// same way (`koru_recording::bitrate_bps`), not carried.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecordingRequest {
    /// Where the clip goes, already collision-suffixed by the view.
    pub clip: PathBuf,
    /// 60 or 30, from the modal.
    pub frame_rate: u32,
    /// Whether the mouse cursor is visible in the clip.
    pub cursor: bool,
}
