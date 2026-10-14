// SPDX-License-Identifier: MPL-2.0

//! The session recorder's surface: the F9/caption-menu setup modal, and the
//! lifecycle its *Record* starts (the internal session-recorder design).
//!
//! [`modal`] is what is true, [`paint`] is where it is on screen, and the
//! container (`MainView`) performs everything the modal asks for. Internal
//! Windows builds only — the menu row and the chord are gated before any of
//! this is reached.
//!
//! The lifecycle itself:
//!
//! ```text
//! Record ──▶ Countdown ──3 s──▶ CleanPresent ──clean frame drawn──▶ Starting
//!                │ abort (F9 / Escape / close): unlock, restore, Idle  │ RecordingEvent
//!                ▼                                                      ▼
//!              Idle                          Recording ──F9 / menu / close──▶ Stopping ──▶ Idle
//! ```
//!
//! The view owns what the person sees; the shell owns the capture, and speaks
//! back only through [`RecordingEvent`].

pub(crate) mod modal;
pub(crate) mod paint;
pub(crate) mod telemetry;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use koru_workspace_host::{PaneRequest, RecordingEvent};
// For `show_notice_reveal` — the saved toast with its *Show* action.
use koru_window::{RecordingRequest, WindowRequest};
use koru_workspace_host::View as _;
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key, ModifiersState};

use super::MainView;
use modal::{Defaults, Outcome, SizeChoice};

/// The countdown's length. Three seconds: long enough to see the window take
/// its recording size, short enough that a dev take is not a wait.
pub(crate) const COUNTDOWN: Duration = Duration::from_secs(3);

/// Where the recorder stands once *Record* has settled. The modal is gone by
/// then — it and a lifecycle never stand at once.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Lifecycle {
    /// 3-2-1 over a dim, from the *Record* press's instant. Abortable: F9,
    /// Escape or a close undoes the lock and nothing ever reaches the shell.
    Countdown { started: Instant },
    /// The overlay is down and the next drawn frame is clean; only then does
    /// the start go out, so no numeral or scrim can race into the clip's
    /// first encoded frame.
    CleanPresent,
    /// `StartRecording` issued; the shell's start worker holds the answer.
    /// Not abortable — the answer lands in well under a second and a stop is
    /// simply remembered for it.
    Starting,
    /// Live. `origin` is the encoder's first-frame instant, relayed by the
    /// shell — the zero every telemetry stamp is measured from (I5).
    Recording { origin: Instant },
    /// Stop issued; the finalize worker holds the answer.
    Stopping,
}

#[cfg(test)]
mod tests;

impl MainView {
    /// **Open the setup modal, pre-filled.** The values are what the person
    /// last recorded with, as the settings authority's device-local section
    /// has them — the first-use set until a recording has settled.
    pub(crate) fn open_recording_modal(&mut self) -> bool {
        if self.recording_modal.is_some() {
            return true;
        }
        let defaults = self.recording_defaults();
        let suggested = defaults
            .folder
            .clone()
            .or_else(koru_workspace_host::settings::videos_dir)
            .unwrap_or_default()
            .join(modal::timestamped_name());
        self.recording_modal = Some(modal::Modal::new(&mut self.engine, &defaults, &suggested));
        true
    }

    /// The values the modal opens with. The authority's section starts as the
    /// registry's defaults until its first answer lands, which maps to the
    /// same first-use set — so an early F9 is never *wrong*, only unprefilled.
    fn recording_defaults(&self) -> Defaults {
        let prefs = &self.delegate_folder.authoritative().recording;
        let first = Defaults::first_use();
        Defaults {
            preset: modal::Preset::from_token(&prefs.preset).unwrap_or(first.preset),
            custom_width: prefs
                .custom_width
                .map(|w| w.to_string())
                .unwrap_or(first.custom_width),
            custom_height: prefs
                .custom_height
                .map(|h| h.to_string())
                .unwrap_or(first.custom_height),
            frame_rate: prefs.frame_rate.unwrap_or(first.frame_rate),
            cursor: prefs.cursor.unwrap_or(first.cursor),
            folder: prefs.folder.as_ref().map(PathBuf::from),
        }
    }

    /// **F9, and the menu row with it.** What the toggle does is where the
    /// recorder stands: open the modal, abort the countdown, remember a stop
    /// for a start in flight, stop a recording — and nothing at all while a
    /// stop is already being finalized or another window holds the process's
    /// one recording.
    pub(crate) fn toggle_recording(&mut self) -> bool {
        // The modal's own F9 closes it (the blocker routes the key to
        // `recording_key`); this arm can only be reached by the menu row.
        if self.recording_modal.take().is_some() {
            return true;
        }
        match self.recording_lifecycle {
            Some(Lifecycle::Countdown { .. } | Lifecycle::CleanPresent) => {
                // Abort: nothing exists to finalize, so there is nothing to
                // wait for — unlock and go Idle.
                self.recording_lifecycle = None;
                self.recording_request = None;
                self.ask_window(WindowRequest::UnlockSizeAndRestore)
            }
            Some(Lifecycle::Starting) => {
                // The start is on the shell's worker; the stop lands the
                // moment its answer does.
                self.recording_stop_when_started = true;
                true
            }
            Some(Lifecycle::Recording { .. }) => {
                self.recording_lifecycle = Some(Lifecycle::Stopping);
                self.ask_window(WindowRequest::StopRecording)
            }
            Some(Lifecycle::Stopping) => true,
            None => {
                if koru_recording::RECORDING_ACTIVE.load(Ordering::SeqCst) {
                    // Another window of this process holds the one recording;
                    // its own row and F9 are the way to stop it.
                    return true;
                }
                self.open_recording_modal()
            }
        }
    }

    /// **The window was asked to close mid-lifecycle** — the veto half of
    /// `request_close`. The window never dies holding a capture: veto, drive
    /// the lifecycle toward its end, and re-ask Close when the shell's answer
    /// lands — or, for the countdown abort that has no answer, on the next
    /// maintenance pass.
    pub(crate) fn recording_close_veto(&mut self) -> bool {
        self.recording_close_after_stop = true;
        match self.recording_lifecycle {
            Some(Lifecycle::Countdown { .. } | Lifecycle::CleanPresent) => {
                self.recording_lifecycle = None;
                self.recording_request = None;
                self.ask_window(WindowRequest::UnlockSizeAndRestore);
            }
            Some(Lifecycle::Starting) => {
                // The Started answer will find the close already owed.
            }
            Some(Lifecycle::Recording { .. }) => {
                self.recording_lifecycle = Some(Lifecycle::Stopping);
                self.ask_window(WindowRequest::StopRecording);
            }
            Some(Lifecycle::Stopping) | None => {}
        }
        true
    }

    /// **The shell's answer.** The only way the lifecycle moves once a worker
    /// holds it: Started means frames are being encoded, Stopped means the
    /// clip is on disk and the window is restored, Failed means no success is
    /// claimed and whatever finalized is told honestly.
    pub(crate) fn recording_event_impl(&mut self, event: RecordingEvent) -> bool {
        match event {
            RecordingEvent::Started {
                origin,
                dpi_scale_milli,
                size,
            } => {
                if !matches!(self.recording_lifecycle, Some(Lifecycle::Starting)) {
                    // A stale answer — a stop or abort already landed. The
                    // shell owns the session; the view owes nothing back.
                    return true;
                }
                if self.recording_close_after_stop || self.recording_stop_when_started {
                    // A stop was asked for before the first frame: take it now.
                    self.recording_stop_when_started = false;
                    self.recording_lifecycle = Some(Lifecycle::Stopping);
                    self.ask_window(WindowRequest::StopRecording);
                    return true;
                }
                self.recording_lifecycle = Some(Lifecycle::Recording { origin });
                // The recorder opens its eyes with the capture: the encoder's
                // origin as the clock, the clip's own pixels as the map.
                let clip_name = self
                    .recording_request
                    .as_ref()
                    .and_then(|request| {
                        request
                            .clip
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "koru.mp4".into());
                self.recording_telemetry = Some(telemetry::Recorder::new(
                    origin,
                    dpi_scale_milli,
                    size,
                    clip_name,
                ));
                true
            }
            RecordingEvent::Failed { code } => {
                self.recording_lifecycle = None;
                self.recording_request = None;
                self.recording_stop_when_started = false;
                let line = if code == RecordingEvent::ALREADY_RECORDING {
                    // A refusal, not a capture failure — the sentence says
                    // why, and the other window's own row is the way to stop.
                    koru_i18n::t(crate::format::lang(), koru_i18n::Key::RecordingBusy).to_owned()
                } else {
                    koru_i18n::fill(
                        koru_i18n::t(crate::format::lang(), koru_i18n::Key::RecordingFailed),
                        &[code],
                    )
                };
                self.toast = Some((
                    koru_ui::Toast::new(line, None, Instant::now()),
                    super::ToastPayload::None,
                ));
                self.close_if_owed()
            }
            RecordingEvent::Stopped { clip_path } => {
                self.recording_lifecycle = None;
                self.recording_request = None;
                self.recording_stop_when_started = false;
                let line = koru_i18n::fill(
                    koru_i18n::t(crate::format::lang(), koru_i18n::Key::RecordingSaved),
                    &[&clip_path.to_string_lossy()],
                );
                self.show_notice_reveal(line, clip_path);
                self.close_if_owed()
            }
        }
    }

    /// Re-ask the close a veto held back. The event paths call it directly;
    /// the countdown abort's call is `maintain_recording`'s, because an abort
    /// has no event of its own.
    fn close_if_owed(&mut self) -> bool {
        if std::mem::take(&mut self.recording_close_after_stop) {
            return self.ask_window(WindowRequest::Close);
        }
        true
    }

    /// The countdown's clock, the clean-present handshake, and the abort
    /// path's close re-ask — the recorder's share of `maintain`.
    pub(crate) fn maintain_recording(&mut self, now: Instant) -> bool {
        match self.recording_lifecycle {
            Some(Lifecycle::Countdown { started }) => {
                if now.duration_since(started) >= COUNTDOWN {
                    // The overlay drops now; the frame drawn after this pass
                    // is the clean one the start waits for.
                    self.recording_lifecycle = Some(Lifecycle::CleanPresent);
                    self.clean_frame_drawn = false;
                }
                // A numeral boundary — or the drop itself — is a redraw. The
                // deadline below keeps this to four wakes, never a spin.
                true
            }
            Some(Lifecycle::CleanPresent) => {
                if !self.clean_frame_drawn {
                    return false;
                }
                let Some(request) = self.recording_request.clone() else {
                    // Nothing settled means nothing to start — refuse rather
                    // than issue half a config.
                    self.recording_lifecycle = None;
                    return true;
                };
                self.recording_lifecycle = Some(Lifecycle::Starting);
                self.ask_window(WindowRequest::StartRecording(RecordingRequest {
                    clip: request.clip,
                    frame_rate: request.frame_rate,
                    cursor: request.cursor,
                }));
                false
            }
            Some(Lifecycle::Recording { .. }) => {
                if let Some(recorder) = self.recording_telemetry.as_mut() {
                    // The heartbeat is observation, never a frame: it moves
                    // no pixels, so it asks for no redraw.
                    recorder.heartbeat(
                        now,
                        self.pointer.0,
                        self.pointer.1,
                        koru_draw::theme::zoom(),
                    );
                }
                false
            }
            _ => {
                if self.recording_lifecycle.is_none() && self.recording_close_after_stop {
                    self.recording_close_after_stop = false;
                    return self.ask_window(WindowRequest::Close);
                }
                false
            }
        }
    }

    /// The countdown's next wake, the heartbeat's next beat, or `None` — an
    /// idle window is never woken by the recorder.
    pub(crate) fn recording_deadline(&self) -> Option<Instant> {
        match self.recording_lifecycle {
            Some(Lifecycle::Countdown { started }) => {
                // One wake per numeral, plus the drop: four in all.
                Some(started + Duration::from_secs(started.elapsed().as_secs() + 1))
            }
            // The flip already asked for the frame; this is only the backstop
            // for a draw that never came.
            Some(Lifecycle::CleanPresent) => Some(Instant::now() + Duration::from_millis(100)),
            Some(Lifecycle::Recording { .. }) => self
                .recording_telemetry
                .as_ref()
                .map(telemetry::Recorder::heartbeat_due),
            _ => None,
        }
    }

    /// **A press the recorder should see.** Every click is recorded with its
    /// position, view-logical mapped to capture-local; the leaf dispatch
    /// reclassifies the ones that reached the document. A no-op whenever no
    /// capture is live.
    pub(crate) fn observe_recording_click(&mut self, x: f32, y: f32) {
        if let Some(recorder) = self.recording_telemetry.as_mut() {
            recorder.click(Instant::now(), x, y, koru_draw::theme::zoom());
        }
    }

    /// The press just recorded reached the document's content — one row,
    /// reclassified, never two rows for one gesture.
    pub(crate) fn observe_recording_node_click(&mut self) {
        if let Some(recorder) = self.recording_telemetry.as_mut() {
            recorder.last_click_was_a_node();
        }
    }

    /// A menu opened, from whichever gesture opens one.
    pub(crate) fn observe_recording_menu_open(&mut self) {
        if let Some(recorder) = self.recording_telemetry.as_mut() {
            recorder.menu_open(Instant::now());
        }
    }

    /// The active tab changed.
    pub(crate) fn observe_recording_tab_switch(&mut self) {
        if let Some(recorder) = self.recording_telemetry.as_mut() {
            recorder.tab_switch(Instant::now());
        }
    }

    /// A character was typed — the kind arrives, the character never does.
    pub(crate) fn observe_recording_text_input(&mut self) {
        if let Some(recorder) = self.recording_telemetry.as_mut() {
            recorder.text_input(Instant::now());
        }
    }

    /// **The shell's finalize worker takes the observations with the stop** —
    /// the one-writer rule: the same worker that writes the clip writes the
    /// `.actions.json` beside it, so the two artifacts cannot disagree about
    /// existing.
    pub(crate) fn take_recording_telemetry_impl(
        &mut self,
    ) -> Option<koru_recording::SessionTelemetry> {
        self.recording_telemetry
            .take()
            .map(telemetry::Recorder::finish)
    }

    /// Every key, while the modal stands. A blocker swallows whatever it does
    /// not use — the `Confirm` precedent — so this always answers `true`.
    pub(crate) fn recording_key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> bool {
        let Some(mut modal) = self.recording_modal.take() else {
            return true;
        };
        let outcome = modal.key(&mut self.engine, key, text, mods);
        self.recording_modal = Some(modal);
        self.settle_recording(outcome);
        true
    }

    /// The press. A control activates; a press outside the panel is swallowed
    /// but dismisses **nothing** — the modal holds values somebody typed, the
    /// delegation modal's own rule.
    pub(crate) fn recording_mouse_down(&mut self, x: f32, y: f32) -> bool {
        let Some(mut modal) = self.recording_modal.take() else {
            return true;
        };
        let viewport = self.viewport;
        let hits = paint::layout(&mut self.engine, &modal, viewport);
        let found = hits.iter().find(|(_, rect)| rect.contains(x, y)).copied();
        let outcome = match found {
            Some((hit, rect)) => {
                let tx = x - paint::text_x(rect);
                modal.press(&mut self.engine, hit, tx)
            }
            None => Outcome::Keep,
        };
        self.recording_modal = Some(modal);
        self.settle_recording(outcome);
        true
    }

    /// The move: report the hovered control (the cursor is asked with no
    /// engine in hand) and extend a field selection while the button is held.
    pub(crate) fn recording_mouse_move(&mut self, x: f32, y: f32) -> bool {
        let Some(modal) = self.recording_modal.as_mut() else {
            return true;
        };
        let viewport = self.viewport;
        let hits = paint::layout(&mut self.engine, modal, viewport);
        let found = hits.iter().find(|(_, rect)| rect.contains(x, y)).copied();
        modal.set_hover(found.map(|(hit, _)| hit));
        // The drag's x is text-local to the field it began in — the focused
        // one — wherever the pointer has got to since.
        if let Some((_, rect)) = modal
            .focus()
            .and_then(|hit| hits.iter().find(|(candidate, _)| *candidate == hit))
        {
            let tx = x - paint::text_x(*rect);
            modal.drag(&mut self.engine, tx);
        }
        true
    }

    /// The release ends a field drag. It activates nothing: activation was the
    /// press's, the same convention the delegation modal keeps.
    pub(crate) fn recording_mouse_up(&mut self) -> bool {
        if let Some(modal) = self.recording_modal.as_mut() {
            modal.release();
        }
        true
    }

    /// **The answer to the save-file picker.** The shell calls only on a real
    /// choice; a cancel said nothing, so the field keeps what it had.
    pub(crate) fn recording_path_chosen_impl(&mut self, chosen: PathBuf) -> bool {
        let Some(modal) = self.recording_modal.as_mut() else {
            return false;
        };
        modal.chose_path(&mut self.engine, &chosen);
        true
    }

    /// Perform what the modal asked for. Everything here touches the world —
    /// the native picker, the window lock, the settings link — which is
    /// exactly what the modal itself may not do.
    pub(crate) fn settle_recording(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Keep | Outcome::Ignored => {}
            Outcome::Close => {
                self.recording_modal = None;
            }
            Outcome::Browse => {
                // A view may not open a blocking native dialog, so the shell
                // performs the picker off this press (the delegation folder
                // picker's rule) and the answer returns through
                // [`Self::recording_path_chosen_impl`].
                let (start, file_name) = match self.recording_modal.as_mut() {
                    Some(modal) => {
                        let current = PathBuf::from(modal.path_field().value());
                        let start = current
                            .parent()
                            .filter(|parent| !parent.as_os_str().is_empty())
                            .map(Path::to_path_buf)
                            .or_else(koru_workspace_host::settings::videos_dir);
                        let file_name = current
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(modal::timestamped_name);
                        (start, file_name)
                    }
                    None => (
                        koru_workspace_host::settings::videos_dir(),
                        modal::timestamped_name(),
                    ),
                };
                self.ask_pane(PaneRequest::ChooseRecordingPath {
                    title: koru_i18n::t(crate::format::lang(), koru_i18n::Key::RecordingTitle)
                        .to_owned(),
                    start,
                    file_name,
                });
            }
            Outcome::Record(request) => {
                let Some(mut modal) = self.recording_modal.take() else {
                    return;
                };
                // Never overwrite a clip somebody kept; never refuse a start
                // over a name collision either.
                let clip = modal::suffixed(&request.clip, clip_taken);
                // Persist the working set — applied locally first, so the next
                // modal pre-fills before the authority's reply even lands.
                let defaults = Defaults {
                    preset: modal.preset(),
                    custom_width: modal.custom_width_field().value(),
                    custom_height: modal.custom_height_field().value(),
                    frame_rate: request.frame_rate,
                    cursor: request.cursor,
                    folder: modal.clip_folder(),
                };
                self.delegate_folder.save(
                    koru_settings::GlobalPatch {
                        recording: Some(prefs_of(&defaults)),
                        ..koru_settings::GlobalPatch::default()
                    },
                    |patch, section| patch.clone().apply(section),
                );
                let lock = match request.size {
                    SizeChoice::Current => WindowRequest::LockCurrentSize,
                    SizeChoice::Exact(width, height) => {
                        WindowRequest::PlaceAndLockExact { width, height }
                    }
                };
                // What the countdown starts when it ends: kept so the size
                // lock and the capture that follows cannot disagree about
                // what was asked for.
                self.recording_request = Some(modal::Request { clip, ..request });
                self.recording_lifecycle = Some(Lifecycle::Countdown {
                    started: Instant::now(),
                });
                self.ask_window(lock);
            }
        }
    }

    /// The modal, the countdown, or neither — over the menu that opened them.
    pub(crate) fn draw_recording(&mut self, scene: &mut Scene, root: Affine) {
        let viewport = self.viewport;
        let pointer = self.pointer;
        if let Some(modal) = self.recording_modal.as_mut() {
            paint::draw(scene, root, &mut self.engine, modal, viewport, pointer);
            return;
        }
        if let Some(Lifecycle::Countdown { started }) = self.recording_lifecycle {
            // 3 for the first second, 2 for the second, 1 for the third; the
            // flip to CleanPresent is `maintain_recording`'s, at three.
            let numeral = 3u32
                .saturating_sub(started.elapsed().as_secs() as u32)
                .max(1);
            paint::draw_countdown(scene, root, &mut self.engine, viewport, numeral);
            return;
        }
        if matches!(self.recording_lifecycle, Some(Lifecycle::CleanPresent)) {
            // This frame has no overlay: it is the clean present the start
            // was waiting for, and the next maintain lets the start go out.
            self.clean_frame_drawn = true;
        }
    }
}

/// The settings-section form of a settled set — one patch, written whole on
/// every *Record*.
fn prefs_of(defaults: &Defaults) -> koru_settings::RecordingPrefs {
    koru_settings::RecordingPrefs {
        preset: defaults.preset.token().to_owned(),
        // The custom pair is stored as the encoder would take it — validated,
        // even-adjusted — or as absent when the field does not parse.
        custom_width: modal::parse_dim(&defaults.custom_width),
        custom_height: modal::parse_dim(&defaults.custom_height),
        frame_rate: Some(defaults.frame_rate),
        cursor: Some(defaults.cursor),
        folder: defaults
            .folder
            .as_ref()
            .map(|folder| folder.to_string_lossy().into_owned()),
    }
}

/// Whether a clip path is already taken — the one filesystem question the
/// settle asks, named so the suite answers it without one. A dependency is
/// never compiled with `cfg(test)`, but this module is the binary's own, so
/// the test build gets the inert half: collision-suffixing itself is covered
/// with an injected `exists` in `recording::tests`, and the container's tests
/// only need determinism (the `run_index` precedent in `MainView::new_with`).
#[cfg(not(test))]
fn clip_taken(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
fn clip_taken(_path: &Path) -> bool {
    false
}

/// Whether a destination's folder is there — asked once at the Record press,
/// never per keystroke, and named so the suite answers it without a disk
/// (the `clip_taken` precedent above). The refusal rule itself is covered
/// with an injected `dir_exists` through `modal::commit_with`.
#[cfg(not(test))]
fn dir_exists(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(test)]
fn dir_exists(_path: &Path) -> bool {
    true
}
