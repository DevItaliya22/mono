// SPDX-License-Identifier: MPL-2.0

//! The shell's half of session recording (the internal session-recorder design):
//! the pieces only the owner of the `Window` can perform — the save picker,
//! the capture lifecycle, and the finalize.
//!
//! **Nothing here runs on the loop thread longer than a lookup.** Starting
//! capture blocks (bind the HWND, create the encoder, wait for the first
//! encoded frame) and finishing blocks (flush and write the `moov` atom), so
//! each runs on its own worker and answers through the [`Answer`] channel;
//! [`Wake::Recording`](super::Wake::Recording) ends the loop's wait and the
//! maintenance pass drains. That is the delegate-folder check's own shape.
//!
//! The lock's bookkeeping is deliberately *here* and not in the view: a start
//! that fails unlocks before the view is told, and the geometry restore waits
//! for the finalize worker, so a restore can never resize the window into the
//! clip's last frames.

use super::{App, Host, Path, View, WindowId};
use std::path::PathBuf;

/// The answer a recording worker hands the loop. The capture-carrying variant
/// is Windows-only like the engine; the rest of the vocabulary exists
/// everywhere so the channel type compiles on any platform.
#[allow(dead_code)] // variants are constructed only by the Windows workers
pub(super) enum Answer {
    /// The start worker: capture is live and producing encoded frames. The
    /// session travels to the pane, which parks it until the stop.
    #[cfg(windows)]
    Started {
        window: WindowId,
        session: koru_recording::capture::Session,
        origin: std::time::Instant,
        dpi_scale_milli: u32,
        size: (u32, u32),
        clip: PathBuf,
    },
    /// The start worker: capture never got going, with the contract's code.
    /// No session exists, so there is nothing to finalize.
    StartFailed {
        window: WindowId,
        code: &'static str,
    },
    /// The finalize worker: the clip is written, or the code it failed with.
    Finished {
        window: WindowId,
        clip: PathBuf,
        result: Result<(), &'static str>,
    },
}

/// A live capture parked in a [`super::Pane`] between `Started` and the stop.
pub(super) struct Live {
    #[cfg(windows)]
    session: koru_recording::capture::Session,
    #[allow(dead_code)] // read by the stop path, which is Windows-only
    clip: PathBuf,
}

/// `KORU_RECORDING_DEBUG`: lifecycle detail — geometry, fps, bitrate,
/// counts. Failures log unconditionally without it.
#[cfg(windows)]
fn debug(args: std::fmt::Arguments) {
    if koru_env::switch(koru_env::name::KORU_RECORDING_DEBUG) {
        eprintln!("[koru:recording] {args}");
    }
}

/// Write the session's second artifact beside the clip (SESSIONRECORD §4
/// dec 7). A failure is said, not hidden — but it is never a capture
/// failure: the clip is written either way, and that is the artifact the
/// person asked for.
#[cfg(windows)]
fn write_telemetry(clip: &std::path::Path, telemetry: Option<&koru_recording::SessionTelemetry>) {
    let Some(telemetry) = telemetry else {
        return;
    };
    let path = clip.with_extension("actions.json");
    match std::fs::write(&path, telemetry.to_json()) {
        Ok(()) => debug(format_args!(
            "wrote {} ({} actions, {} heartbeats)",
            path.display(),
            telemetry.actions.len(),
            telemetry.heartbeats.len()
        )),
        Err(error) => tracing::warn!(%error, "could not write {}", path.display()),
    }
}

/// Flush and write the clip, and say honestly whether it is what was asked
/// for. `windows-capture` does not fail a mid-clip size change — it pads or
/// crops the frame into the configured dimensions, so an OS-level resize the
/// app lock cannot see (Win+Arrow, aero snap, a DPI change on monitor move)
/// warps the clip *silently*. The stamps are the only place it shows;
/// report it with the capture contract's own code.
#[cfg(windows)]
fn finalize(session: koru_recording::capture::Session) -> Result<(), &'static str> {
    let (frames, result) = session.finish();
    match result {
        Ok(()) if koru_recording::frames_hold_size(&frames) => Ok(()),
        Ok(()) => {
            tracing::warn!(
                "the window's size changed mid-clip; the recording is padded, not clean"
            );
            Err("capture-geometry-changed")
        }
        Err(code) => Err(code),
    }
}

impl<H: Host> App<H> {
    /// Perform the recording modal's save-file picker. Synchronous, like every
    /// picker here: the native dialog blocks and the shell is the one allowed
    /// to block. A choice goes straight back to the asking view; a cancel
    /// answers nothing at all.
    pub(super) fn choose_recording_path(
        &mut self,
        source: WindowId,
        title: &str,
        start: Option<&Path>,
        file_name: &str,
    ) {
        let Some(pane) = self.panes.get(&source) else {
            return;
        };
        let filters = [crate::dialogs::FileFilter {
            name: "H.264 video",
            extensions: &["mp4"],
        }];
        let Some(chosen) = crate::dialogs::pick_save_file(
            title,
            file_name,
            &filters,
            start,
            Some(pane.window.as_ref()),
        ) else {
            return;
        };
        if let Some(pane) = self.panes.get_mut(&source)
            && pane.view.recording_path_chosen(chosen)
        {
            pane.window.request_redraw();
        }
    }

    /// Hand an event to the window's view and redraw if it changed something.
    fn answer_recording(&mut self, window: WindowId, event: crate::RecordingEvent) {
        if let Some(pane) = self.panes.get_mut(&window)
            && pane.view.recording_event(event)
        {
            pane.window.request_redraw();
        }
    }
}

#[cfg(windows)]
impl<H: Host> App<H> {
    /// Perform [`koru_window::WindowRequest::StartRecording`]: bind this
    /// window's HWND and wait for the first encoded frame, **on a worker**.
    ///
    /// One recording per process is enforced here — the menu row and F9 read
    /// [`koru_recording::RECORDING_ACTIVE`] to dim themselves, but a request
    /// that arrives anyway is refused, not raced. A refusal *answers*: the
    /// asking view is waiting in `Starting`, and a refusal that said nothing
    /// would strand it there — size-locked, unclosable — until process exit.
    pub(super) fn start_recording(&mut self, id: WindowId, config: koru_window::RecordingRequest) {
        use std::sync::atomic::Ordering;
        let Some(pane) = self.panes.get(&id) else {
            return;
        };
        if pane.recording.is_some() {
            // This window is already the recorder — its view is in
            // Recording/Stopping and cannot have asked, so this is a corrupted
            // request stream, not a race. Ignore it: answering Failed would
            // tear down a healthy lifecycle while the capture runs on.
            tracing::warn!("a second recording start arrived from the recording window itself");
            return;
        }
        if koru_recording::RECORDING_ACTIVE.load(Ordering::SeqCst) {
            // Another window holds the one recording — or its start is in
            // flight (busy is marked at request time below, so an in-flight
            // start is visible here). Refuse exactly like a failed start:
            // undo this window's lock first, then tell it.
            debug(format_args!(
                "start refused: a recording is already live in this process"
            ));
            self.unlock_size_and_restore(id);
            self.answer_recording(
                id,
                crate::RecordingEvent::Failed {
                    code: crate::RecordingEvent::ALREADY_RECORDING,
                },
            );
            return;
        }
        // Busy from this moment, not from the first frame: the bind, the
        // encoder and the first-frame wait below take seconds on the worker,
        // and a second window's start landing in that window must be
        // refused, not raced. Every failure path below clears the flag;
        // `Started` keeps it until the finalize (or retire) answers.
        koru_recording::RECORDING_ACTIVE.store(true, Ordering::SeqCst);
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let hwnd = pane
            .window
            .window_handle()
            .ok()
            .and_then(|handle| match handle.as_raw() {
                RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                _ => None,
            });
        let Some(hwnd) = hwnd else {
            koru_recording::RECORDING_ACTIVE.store(false, Ordering::SeqCst);
            self.answer_recording(
                id,
                crate::RecordingEvent::Failed {
                    code: "window-bind-failed",
                },
            );
            return;
        };
        // The bitrate is derived from the real geometry — the size the lock
        // is holding — never from the preset the view remembers.
        let size = pane.window.inner_size();
        let (width, height) = (
            koru_recording::even_dim(size.width),
            koru_recording::even_dim(size.height),
        );
        let bitrate = koru_recording::bitrate_bps(width, height);
        debug(format_args!(
            "start {width}x{height} @ {}fps, {} Mbps, cursor={} → {}",
            config.frame_rate,
            bitrate / 1_000_000,
            config.cursor,
            config.clip.display()
        ));
        let tx = self.recording_tx.clone();
        let proxy = self.wake_proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("koru-recording-start".into())
            .spawn(move || {
                let answer =
                    match koru_recording::capture::start(koru_recording::capture::StartConfig {
                        window_handle: hwnd as i64,
                        process_id: std::process::id(),
                        clip: config.clip.clone(),
                        stills: None,
                        cursor: config.cursor,
                        frame_rate: Some(config.frame_rate),
                        bitrate_bps: Some(bitrate),
                    }) {
                        Ok((session, geometry)) => {
                            match session.wait_first_frame(std::time::Duration::from_secs(5)) {
                                Ok(origin) => Answer::Started {
                                    window: id,
                                    session,
                                    origin: origin.instant,
                                    dpi_scale_milli: geometry.dpi_scale_milli,
                                    size: (geometry.width, geometry.height),
                                    clip: config.clip,
                                },
                                Err(code) => Answer::StartFailed { window: id, code },
                            }
                        }
                        Err(code) => Answer::StartFailed { window: id, code },
                    };
                // Queue first, wake second — the delegate-folder rule.
                if tx.send(answer).is_ok() {
                    let _ = proxy.send_event(super::Wake::Recording);
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not spawn the recording start worker");
            koru_recording::RECORDING_ACTIVE.store(false, Ordering::SeqCst);
            self.answer_recording(
                id,
                crate::RecordingEvent::Failed {
                    code: "capture-start-failed",
                },
            );
        }
    }

    /// Perform [`koru_window::WindowRequest::StopRecording`]: the parked
    /// session goes to a finalize worker. The geometry restore waits for the
    /// worker's answer — see the module note.
    pub(super) fn stop_recording(&mut self, id: WindowId) {
        let Some(live) = self
            .panes
            .get_mut(&id)
            .and_then(|pane| pane.recording.take())
        else {
            // No live session. A stop for a start still in flight is the
            // view's deferred stop; it lands when `Started` does.
            debug(format_args!("stop with no live session on {id:?}; ignored"));
            return;
        };
        debug(format_args!("stop; finalizing {}", live.clip.display()));
        // The observations travel with the stop — the one-writer rule: the
        // worker that writes the clip writes the `.actions.json` beside it,
        // so the two artifacts cannot disagree about existing. Shared rather
        // than moved, so the inline fallback below can answer the same way.
        let telemetry = std::sync::Arc::new(
            self.panes
                .get_mut(&id)
                .and_then(|pane| pane.view.take_recording_telemetry()),
        );
        // The session travels in a slot rather than the closure, so a thread
        // that could not be started hands it back instead of dropping it —
        // an unfinished session leaks the capture and loses the clip.
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(live.session)));
        let tx = self.recording_tx.clone();
        let proxy = self.wake_proxy.clone();
        let clip = live.clip.clone();
        let spawned = std::thread::Builder::new()
            .name("koru-recording-finish".into())
            .spawn({
                let slot = slot.clone();
                let telemetry = telemetry.clone();
                move || {
                    let Some(session) = slot.lock().unwrap().take() else {
                        return;
                    };
                    let result = finalize(session);
                    write_telemetry(&clip, telemetry.as_ref().as_ref());
                    if tx
                        .send(Answer::Finished {
                            window: id,
                            clip,
                            result,
                        })
                        .is_ok()
                    {
                        let _ = proxy.send_event(super::Wake::Recording);
                    }
                }
            });
        if let Err(error) = spawned {
            // The thread never started and the slot still holds the session,
            // so finalize inline on this should-never-happen path and answer
            // exactly as the worker would have.
            tracing::warn!(%error, "could not spawn the recording finalize worker; finalizing inline");
            if let Some(session) = slot.lock().unwrap().take() {
                let result = finalize(session);
                write_telemetry(&live.clip, telemetry.as_ref().as_ref());
                self.recording_rx_handled(Answer::Finished {
                    window: id,
                    clip: live.clip,
                    result,
                });
            }
        }
    }

    /// Drain the recording workers' answers, from the maintenance pass.
    pub(super) fn drain_recording_answers(&mut self) {
        while let Ok(answer) = self.recording_rx.try_recv() {
            self.recording_rx_handled(answer);
        }
    }

    /// One worker answer, applied. Separated from the drain so the inline
    /// fallback of a failed spawn takes the same path.
    fn recording_rx_handled(&mut self, answer: Answer) {
        use std::sync::atomic::Ordering;
        match answer {
            Answer::Started {
                window,
                session,
                origin,
                dpi_scale_milli,
                size,
                clip,
            } => {
                if !self.panes.contains_key(&window) {
                    // The window went away mid-start. An abort leaves no
                    // file: finalize on a worker so the capture is not
                    // leaked, then remove the partial clip.
                    koru_recording::RECORDING_ACTIVE.store(false, Ordering::SeqCst);
                    std::thread::spawn(move || {
                        let (_frames, _result) = session.finish();
                        let _ = std::fs::remove_file(clip);
                    });
                    return;
                }
                koru_recording::RECORDING_ACTIVE.store(true, Ordering::SeqCst);
                debug(format_args!("recording live; first frame is the origin"));
                if let Some(pane) = self.panes.get_mut(&window) {
                    pane.recording = Some(Live { session, clip });
                }
                self.answer_recording(
                    window,
                    crate::RecordingEvent::Started {
                        origin,
                        dpi_scale_milli,
                        size,
                    },
                );
            }
            Answer::StartFailed { window, code } => {
                // No session ever existed. Clear the busy the request took,
                // and undo the lock *before* the view is told, so its Idle
                // and the window's freedom agree.
                tracing::warn!(code, "session recording failed to start");
                koru_recording::RECORDING_ACTIVE.store(false, Ordering::SeqCst);
                self.unlock_size_and_restore(window);
                self.answer_recording(window, crate::RecordingEvent::Failed { code });
            }
            Answer::Finished {
                window,
                clip,
                result,
            } => {
                koru_recording::RECORDING_ACTIVE.store(false, Ordering::SeqCst);
                self.unlock_size_and_restore(window);
                match &result {
                    Ok(()) => debug(format_args!("saved {}", clip.display())),
                    Err(code) => tracing::warn!(code, "session recording finalize failed"),
                }
                let event = match result {
                    Ok(()) => crate::RecordingEvent::Stopped { clip_path: clip },
                    Err(code) => crate::RecordingEvent::Failed { code },
                };
                self.answer_recording(window, event);
            }
        }
    }

    /// **A window may not die holding a capture.** The view vetoes a close
    /// while a recording is live, so this is the belt-and-braces half: a pane
    /// removed anyway takes its session to a finalize worker first — the
    /// `moov` atom is not sacrificed and the capture thread is not leaked.
    pub(super) fn retire_recording(&mut self, id: WindowId) {
        use std::sync::atomic::Ordering;
        let Some(live) = self
            .panes
            .get_mut(&id)
            .and_then(|pane| pane.recording.take())
        else {
            return;
        };
        tracing::warn!("a window closed holding a recording; finalizing anyway");
        koru_recording::RECORDING_ACTIVE.store(false, Ordering::SeqCst);
        let telemetry = self
            .panes
            .get_mut(&id)
            .and_then(|pane| pane.view.take_recording_telemetry());
        let tx = self.recording_tx.clone();
        let proxy = self.wake_proxy.clone();
        let clip = live.clip.clone();
        std::thread::spawn(move || {
            let (_frames, result) = live.session.finish();
            write_telemetry(&clip, telemetry.as_ref());
            // The window is gone; the drain tolerates an answer with no pane.
            let _ = tx.send(Answer::Finished {
                window: id,
                clip,
                result,
            });
            let _ = proxy.send_event(super::Wake::Recording);
        });
    }
}

#[cfg(not(windows))]
impl<H: Host> App<H> {
    /// Windows Graphics Capture is Windows-only. A start that arrives anyway
    /// — the gate is internal *and* Windows, so this should be unreachable —
    /// is refused with the honest code rather than panicking.
    pub(super) fn start_recording(&mut self, id: WindowId, _config: koru_window::RecordingRequest) {
        tracing::warn!("session recording is Windows-only; a start reached this shell anyway");
        // The view locked the size before asking to start; a refusal that
        // left it locked would strand the window, as every Windows failure
        // path already knows.
        self.unlock_size_and_restore(id);
        self.answer_recording(
            id,
            crate::RecordingEvent::Failed {
                code: "capture-start-failed",
            },
        );
    }

    pub(super) fn stop_recording(&mut self, _id: WindowId) {}

    pub(super) fn drain_recording_answers(&mut self) {}

    pub(super) fn retire_recording(&mut self, _id: WindowId) {}
}
