// SPDX-License-Identifier: MPL-2.0

//! What the support window shows around the chat: *Loading support…* until
//! the Messenger is on screen, the error state with **Try again** and **Open
//! support in browser** (D10), and the close-choice dialog the ✕ opens. The
//! chat has the whole content area; the caption's only extra is the pin.
//!
//! Coordinates arrive in window space; the container hands over its content
//! rect (below the caption row) and this view lays itself out inside it. The
//! surface's placement is reported in the same window space, which is what
//! the host's `native_surface` seam expects.

use std::time::Instant;

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_i18n::{Key, Lang, t};
use koru_ui::button::{ButtonRole, button_width, draw_button, row_from_right};
use koru_ui::choice::{Choice, ChoiceOutcome};
use koru_workspace_host::native_surface::BackgroundIntent;
use koru_workspace_host::native_surface::{Placement, SharedSurface, SurfaceError, SurfaceHost};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::Key as WinitKey;
use winit::window::CursorIcon;

use super::session::{Failure, Mode, Session, State};

const ANSWER_END: usize = 0;
const ANSWER_FAB: usize = 1;
const ANSWER_QUIET: usize = 2;

const PAD: f32 = 24.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Button {
    TryAgain,
    FailedOpenInBrowser,
}

/// Where the buttons landed on the last draw, for hit-testing.
#[derive(Default, Clone, Copy)]
struct Laid {
    try_again: Option<Rect>,
    failed_open: Option<Rect>,
}

impl Laid {
    fn hit(&self, x: f32, y: f32) -> Option<Button> {
        let inside = |r: Option<Rect>| r.is_some_and(|r| r.contains(x, y));
        if inside(self.try_again) {
            Some(Button::TryAgain)
        } else if inside(self.failed_open) {
            Some(Button::FailedOpenInBrowser)
        } else {
            None
        }
    }
}

pub struct SupportView {
    session: Session,
    lang: Lang,
    dialog: Option<Choice>,
    hover: Option<Button>,
    laid: Laid,
    /// What the container should ask the host for, after a choice.
    intents: Vec<BackgroundIntent>,
}

impl SupportView {
    pub fn new(session: Session, lang: Lang) -> Self {
        Self {
            session,
            lang,
            dialog: None,
            hover: None,
            laid: Laid::default(),
            intents: Vec::new(),
        }
    }

    pub fn attach_host(&mut self, host: SurfaceHost) {
        self.session.borrow_mut().attach_host(host);
    }

    fn text(&self, key: Key) -> &'static str {
        t(self.lang, key)
    }

    /// The chat's rectangle: the whole content area below the caption.
    fn surface_rect(content: Rect) -> Rect {
        content
    }

    // ---------------------------------------------------------------- draw

    pub fn draw(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        content: Rect,
        viewport: (f32, f32),
    ) {
        let p = theme::p();
        scene::fill(
            scene,
            root,
            p.bg,
            &scene::rect(content.x, content.y, content.w, content.h),
        );
        self.laid = Laid::default();

        // Ask for the surface one frame *after* the loading text is on
        // screen: the constructor stalls the loop (guide/contributing/deps/WRY.md).
        let state = self.session.borrow().state.clone();
        match state {
            State::Idle => {
                self.draw_centered(scene, root, engine, content, self.text(Key::SupportLoading));
                self.session.borrow_mut().request_open(Instant::now());
            }
            State::Creating | State::Loading | State::Booting => {
                self.draw_centered(scene, root, engine, content, self.text(Key::SupportLoading));
            }
            State::Live | State::Closed => {}
            State::Failed(failure) => self.draw_failed(scene, root, engine, content, &failure),
        }

        if let Some(dialog) = &self.dialog {
            dialog.draw(scene, root, engine, viewport);
        }
    }

    fn draw_centered(
        &self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        content: Rect,
        text: &str,
    ) {
        let layout = engine.simple_layout(text, theme::role::body());
        let area = Self::surface_rect(content);
        let x = area.x + (area.w - layout.width()) * 0.5;
        let y = area.y + (area.h - layout.height()) * 0.5;
        scene::draw_layout(scene, root, &layout, x, y, Some(theme::p().text_dim));
    }

    fn draw_failed(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        content: Rect,
        failure: &Failure,
    ) {
        let p = theme::p();
        let area = Self::surface_rect(content);
        let w = (area.w - PAD * 2.0).max(80.0);
        let x = area.x + PAD;
        let title = engine.simple_layout_wrapped(
            self.text(Key::SupportFailedTitle),
            theme::role::subtitle(),
            w,
        );
        let body_key = match failure {
            Failure::NoRuntime(_) => Key::SupportNoRuntimeBody,
            _ => Key::SupportFailedBody,
        };
        let body = engine.simple_layout_wrapped(self.text(body_key), theme::role::meta(), w);
        let block_h = title.height() + 8.0 + body.height() + 18.0 + koru_ui::button::H;
        let mut y = area.y + ((area.h - block_h) * 0.4).max(PAD);
        scene::draw_layout(scene, root, &title, x, y, Some(p.text));
        y += title.height() + 8.0;
        scene::draw_layout(scene, root, &body, x, y, Some(p.text_dim));
        y += body.height() + 18.0;

        let open_label = self.text(Key::SupportOpenInBrowser);
        let retry_label = self.text(Key::SupportTryAgain);
        let can_retry = !matches!(failure, Failure::NoRuntime(_));
        let open_w = button_width(engine, open_label);
        let mut widths = vec![open_w];
        if can_retry {
            widths.push(button_width(engine, retry_label));
        }
        let mut rects = row_from_right(x + w, y, &widths);
        // Left-align the row instead: this is a message, not a dialog corner.
        let shift = x - rects[0].x;
        for r in &mut rects {
            r.x += shift;
        }
        let open = rects[0];
        draw_button(
            scene,
            root,
            engine,
            open,
            open_label,
            ButtonRole::Secondary,
            self.hover == Some(Button::FailedOpenInBrowser),
            false,
        );
        self.laid.failed_open = Some(open);
        if can_retry {
            let retry = rects[1];
            draw_button(
                scene,
                root,
                engine,
                retry,
                retry_label,
                ButtonRole::Primary,
                self.hover == Some(Button::TryAgain),
                false,
            );
            self.laid.try_again = Some(retry);
        }
    }

    // --------------------------------------------------------------- input

    /// Returns whether the press was handled (a redraw is owed either way).
    pub fn mouse_down(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> bool {
        if let Some(dialog) = self.dialog.as_mut() {
            let outcome = dialog.mouse_down(engine, viewport, x, y);
            self.answer(outcome);
            return true;
        }
        match self.laid.hit(x, y) {
            Some(Button::FailedOpenInBrowser) => {
                self.open_in_browser();
                true
            }
            Some(Button::TryAgain) => {
                self.session.borrow_mut().retry(Instant::now());
                true
            }
            None => false,
        }
    }

    pub fn mouse_move(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> bool {
        if let Some(dialog) = self.dialog.as_mut() {
            return dialog.mouse_move(engine, viewport, x, y);
        }
        let hover = self.laid.hit(x, y);
        let changed = hover != self.hover;
        self.hover = hover;
        changed
    }

    pub fn key(&mut self, key: &WinitKey) -> bool {
        let Some(dialog) = self.dialog.as_mut() else {
            return false;
        };
        let outcome = dialog.key(key);
        self.answer(outcome);
        true
    }

    pub fn cursor_icon(&self) -> CursorIcon {
        if self.hover.is_some() {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        }
    }

    // ----------------------------------------------------------- lifecycle

    /// The ✕: ask what closing means. Vetoes the close; the dialog's answer
    /// either ends the chat (and the container closes the window on the next
    /// `take_wants_close`) or leaves everything as it was.
    pub fn request_close(&mut self) -> bool {
        if self.session.borrow().state == State::Closed {
            return false;
        }
        if self.dialog.is_none() {
            self.dialog = Some(
                Choice::new(
                    self.text(Key::SupportCloseTitle),
                    self.text(Key::SupportCloseBody),
                )
                .answer(self.text(Key::SupportEndChat))
                .answer(self.text(Key::SupportKeepWithIcon))
                .answer(self.text(Key::SupportCloseNotify))
                .defaulting_to(ANSWER_FAB),
            );
            super::debug::log("close asked");
        }
        true
    }

    /// The dialog's answer (D14′): End chat closes the window; the two
    /// keep-alive answers hide it and let the icon follow the tray table.
    fn answer(&mut self, outcome: ChoiceOutcome) {
        match outcome {
            ChoiceOutcome::Keep => {}
            ChoiceOutcome::Dismiss => self.dialog = None,
            ChoiceOutcome::Chosen(ANSWER_END) => self.end_chat(),
            ChoiceOutcome::Chosen(ANSWER_FAB) => self.hide_as(Mode::Fab),
            ChoiceOutcome::Chosen(ANSWER_QUIET) => self.hide_as(Mode::Quiet),
            ChoiceOutcome::Chosen(_) => self.dialog = None,
        }
    }

    fn hide_as(&mut self, mode: Mode) {
        self.dialog = None;
        self.session.borrow_mut().choose(mode);
        // The Messenger hides with the window, or Intercom marks every reply
        // read on arrival and the badge never holds (page: `koruSupport.hide`).
        self.session
            .borrow()
            .call_js("window.koruSupport && window.koruSupport.hide()");
        self.intents.push(BackgroundIntent::SetVisible(false));
        if mode == Mode::Fab {
            self.intents.push(BackgroundIntent::ShowFab);
        }
    }

    /// The chat window is on screen (again): the session's badge and
    /// baseline follow, and any icon hides itself on its next pass.
    pub fn shown(&mut self) {
        self.session.borrow_mut().shown();
        self.session
            .borrow()
            .call_js("window.koruSupport && window.koruSupport.show()");
    }

    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub fn take_intent(&mut self) -> Option<BackgroundIntent> {
        if self.intents.is_empty() {
            None
        } else {
            Some(self.intents.remove(0))
        }
    }

    pub fn end_chat(&mut self) {
        super::debug::log("choice: end chat");
        self.dialog = None;
        self.session.borrow_mut().end();
        self.intents.push(BackgroundIntent::Close);
    }

    /// The tray table, applied to the chat window: the session observes the
    /// hide state, services its receiver even while hidden, and reports a
    /// reply as a notice for the focused window. The window itself is only
    /// ever shown by Contact support and hidden by the dialog.
    pub fn background_maintain(&mut self, now: Instant, app_hidden: bool) -> bool {
        self.session.borrow_mut().observe_app_hidden(app_hidden);
        let changed = self.maintain(now);
        let (notice, ended) = {
            let mut s = self.session.borrow_mut();
            (s.take_notice(), s.mode == Mode::Ended)
        };
        if notice {
            let session = self.session.borrow();
            if session.opens_on_reply() {
                // Hidden until support replies: the reply opens the window
                // itself, no icon, no notice to click through.
                self.intents.push(BackgroundIntent::OpenSupport);
            } else {
                self.intents.push(BackgroundIntent::NotifyReply);
                if !app_hidden && session.mode == Mode::Fab {
                    self.intents.push(BackgroundIntent::ShowFab);
                }
            }
        }
        if ended && self.session.borrow().state == State::Closed && self.dialog.is_none() {
            // Ended from the icon's popup: this window goes too.
            self.intents.push(BackgroundIntent::Close);
        }
        changed
    }

    pub fn visible_after_restore(&self) -> bool {
        self.session.borrow().mode == Mode::Open
    }

    pub fn open_in_browser(&mut self) {
        let url = self.session.borrow().page_url();
        tracing::info!(target: "koru::support", "fallback_browser");
        if let Err(error) = koru_editor::browser::open(&url) {
            tracing::warn!(target: "koru::support", "fallback_browser failed: {error}");
        }
    }

    pub fn maintenance_deadline(&self) -> Option<Instant> {
        self.session.borrow().deadline()
    }

    /// Drain the engine, advance the state, open what the page sent outside.
    pub fn maintain(&mut self, now: Instant) -> bool {
        let changed = self.session.borrow_mut().maintain(now);
        loop {
            let next = self.session.borrow_mut().take_external();
            let Some(url) = next else { break };
            if let Err(error) = koru_editor::browser::open(&url) {
                tracing::warn!(target: "koru::support", "external open failed: {error}");
            }
        }
        changed
    }

    /// The surface and where it goes, in window space: the whole content
    /// area, hidden while the dialog or the error state needs the clicks.
    /// `None` when there is no surface to place.
    pub fn native_surface(&mut self, content: Rect) -> Option<(SharedSurface, Placement)> {
        let session = self.session.borrow();
        let surface = session.shared_surface()?;
        let visible = self.dialog.is_none() && session.is_live_or_loading();
        Some((
            surface,
            Placement {
                rect: Self::surface_rect(content),
                visible,
            },
        ))
    }

    pub fn native_surface_failed(&mut self, error: SurfaceError) {
        self.session
            .borrow_mut()
            .surface_failed(error.what, error.reason);
    }
}
