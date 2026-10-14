// SPDX-License-Identifier: MPL-2.0

//! The support session: one per process, on the UI thread, owned by the app.
//!
//! `Session` is an `Rc<RefCell<..>>`, and `Rc` is `!Send`, so the thread
//! requirement is a compile error rather than a comment: the engine's surface
//! inside is `!Send` too. The root reference lives in the app's `Host`
//! (`Manager`), which outlives every window; the support window holds a clone
//! while it shows the chat. "Parked" is not a place — it is the state in which
//! no window shows the session.
//!
//! The state machine (ADR-276):
//!
//! ```text
//! Idle ─open─▶ Creating ─Ok─▶ Loading ─PageFinished─▶ Booting ─ready─▶ Live
//!   │             │               │                       │
//!   └─ no runtime / Create ───────┴──── deadline ─────────┴──▶ Failed(reason)
//!                                            Failed ─Try again─▶ Creating (generation + 1)
//! ```
//!
//! *Ready* means Intercom's `onShow` fired after the page called `show`.
//! Creation is deferred one pass after the window has drawn *Loading
//! support…*, because the constructor stalls the UI thread
//! (guide/contributing/deps/WRY.md).

use koru_workspace_host::native_surface::SharedSurface;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use koru_webview::{WebConfig, WebError, WebEvent, WebSurface};
use koru_workspace_host::native_surface::{PhysicalRect, SurfaceHost};

use super::surface::HostedWebSurface;
use super::url;

pub type Session = Rc<RefCell<SupportSession>>;

/// How long the page may take to finish loading, and then to say the
/// Messenger is on screen, before the window shows the error state.
pub const PAGE_DEADLINE: Duration = Duration::from_secs(15);
pub const READY_DEADLINE: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    /// The window has drawn its loading frame; the next pass creates.
    Creating,
    Loading,
    Booting,
    Live,
    Failed(Failure),
    /// `end()` ran: the surface is gone and the session will not come back.
    Closed,
}

/// Why the chat is not on screen, in the classes the UI distinguishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No web runtime on this machine — the browser is the answer.
    NoRuntime(String),
    /// The engine refused to create the surface.
    Create(String),
    /// The page did not finish, or the Messenger never said it was shown.
    Timeout(&'static str),
    /// A placement or hide call failed.
    Surface(String),
    /// The page reported an error of its own.
    Page(String),
}

/// What the person asked for when they closed the window (D14′), and what
/// the icon and the tray table read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The chat window is (or should be) on screen.
    Open,
    /// Keep chat with a floating icon: window hidden, icon shown.
    Fab,
    /// Close and tell me when support replies: window hidden, no icon until
    /// a reply.
    Quiet,
    /// End chat: nothing comes back.
    Ended,
}

pub struct SupportSession {
    pub state: State,
    pub generation: u32,
    pub mode: Mode,
    /// The count against which "support replied" is judged: taken at
    /// registration, at the window becoming visible and at each close
    /// choice; lowered on a decrease.
    baseline: u32,
    /// Permission to surface the icon during the current application-hide
    /// interval: set by a qualifying reply while hidden, cleared when a hide
    /// interval begins, on Quiet, on showing the chat, on restore, on end.
    replied: bool,
    app_hidden: bool,
    /// Whether the chat window is on screen, as the view last reported.
    window_visible: bool,
    /// A reply notice is owed to the focused Koru window.
    notice_owed: bool,
    origin: String,
    lang: koru_i18n::Lang,
    host: Option<SurfaceHost>,
    surface: Option<Rc<HostedWebSurface>>,
    receiver: Option<Receiver<WebEvent>>,
    deadline: Option<Instant>,
    /// URLs the policy sent outside; the view opens them in the browser.
    external: VecDeque<String>,
    /// Intercom's count, always the last value the page reported.
    pub unread: u32,
}

impl SupportSession {
    pub fn new(origin: String, lang: koru_i18n::Lang) -> Session {
        Rc::new(RefCell::new(Self {
            state: State::Idle,
            generation: 0,
            mode: Mode::Open,
            baseline: 0,
            replied: false,
            app_hidden: false,
            // The session is created as its window opens.
            window_visible: true,
            notice_owed: false,
            origin,
            lang,
            host: None,
            surface: None,
            receiver: None,
            deadline: None,
            external: VecDeque::new(),
            unread: 0,
        }))
    }

    pub fn page_url(&self) -> String {
        url::page_url(&self.origin, self.lang)
    }

    pub fn attach_host(&mut self, host: SurfaceHost) {
        self.host = Some(host);
    }

    #[cfg(test)]
    pub fn surface(&self) -> Option<&HostedWebSurface> {
        self.surface.as_deref()
    }

    /// The handle the host places, cloned per reconcile; the session stays
    /// the owner, so `end()` destroys the surface whatever a view holds.
    pub fn shared_surface(&self) -> Option<SharedSurface> {
        self.surface.as_ref().map(|s| s.clone() as SharedSurface)
    }

    pub fn is_live_or_loading(&self) -> bool {
        matches!(self.state, State::Loading | State::Booting | State::Live)
    }

    /// The close choice, from the dialog or the icon's popup. Taking the
    /// baseline here means a reply that arrives *after* the choice counts;
    /// clearing `replied` means one that arrived before it does not.
    pub fn choose(&mut self, mode: Mode) {
        self.mode = mode;
        self.baseline = self.unread;
        self.replied = false;
        match mode {
            Mode::Open => self.window_visible = true,
            Mode::Fab | Mode::Quiet => self.window_visible = false,
            Mode::Ended => {}
        }
        super::debug::log(&format!("choice: {mode:?}"));
    }

    /// The chat window is on screen again: the badge clears, the baseline is
    /// the count now, and any hide-interval permission is spent.
    pub fn shown(&mut self) {
        self.mode = Mode::Open;
        self.window_visible = true;
        self.baseline = self.unread;
        self.replied = false;
    }

    /// The tray table's inputs, once per background pass. The transition into
    /// a hide interval clears `replied`; restoring the application does too.
    pub fn observe_app_hidden(&mut self, app_hidden: bool) {
        if app_hidden != self.app_hidden {
            self.app_hidden = app_hidden;
            self.replied = false;
        }
    }

    /// Whether the floating icon should be on screen right now — the tray
    /// table's column for this mode and hide state. Only the icon mode has
    /// an icon: *hide until support replies* answers a reply by opening the
    /// chat window itself (maintainers, 2026-09-05), so it never shows one.
    pub fn fab_visible(&self, app_hidden: bool) -> bool {
        self.mode == Mode::Fab && (!app_hidden || self.replied)
    }

    /// A reply arrived while the chat was hidden until one: the window
    /// opens by itself. Read where `take_notice` is, by the view.
    pub fn opens_on_reply(&self) -> bool {
        self.mode == Mode::Quiet
    }

    /// The badge: Intercom's count while the chat is not on screen.
    pub fn badge(&self) -> u32 {
        if self.window_visible { 0 } else { self.unread }
    }

    /// The reply notice owed to the focused window, taken once.
    pub fn take_notice(&mut self) -> bool {
        std::mem::take(&mut self.notice_owed)
    }

    /// The next URL to open in the browser, if the page asked for one.
    pub fn take_external(&mut self) -> Option<String> {
        self.external.pop_front()
    }

    /// The window has drawn *Loading support…*; create on the next pass.
    pub fn request_open(&mut self, now: Instant) {
        if matches!(self.state, State::Idle | State::Failed(_)) {
            self.state = State::Creating;
            self.deadline = Some(now);
        }
    }

    /// Try again after a failure: a new generation, a new surface.
    pub fn retry(&mut self, now: Instant) {
        self.drop_surface();
        self.generation += 1;
        self.state = State::Idle;
        self.request_open(now);
    }

    /// The deadline the window's maintenance folds into the loop's sleep.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Drain the engine's events and advance the state. Returns whether the
    /// window should redraw.
    pub fn maintain(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self.state == State::Creating && self.deadline.is_some_and(|at| now >= at) {
            self.deadline = None;
            self.create(now);
            changed = true;
        }
        let events: Vec<WebEvent> = match self.receiver.as_ref() {
            Some(receiver) => receiver.try_iter().collect(),
            None => Vec::new(),
        };
        for event in events {
            changed |= self.apply(event, now);
        }
        if let Some(at) = self.deadline
            && now >= at
            && matches!(self.state, State::Loading | State::Booting)
        {
            let stage = if self.state == State::Loading {
                "page"
            } else {
                "ready"
            };
            self.fail(Failure::Timeout(stage));
            changed = true;
        }
        changed
    }

    fn create(&mut self, now: Instant) {
        let Some(host) = self.host.clone() else {
            self.fail(Failure::Create("no window to host the chat".into()));
            return;
        };
        let origin = self.origin.clone();
        let config = WebConfig {
            url: self.page_url(),
            data_directory: url::data_directory(),
            policy: Box::new(move |url| url::navigation(&origin, url)),
            devtools: cfg!(debug_assertions) && koru_webview::debug_enabled(),
            user_agent_suffix: Some(format!("KoruDesktop/{}", env!("CARGO_PKG_VERSION"))),
        };
        // Reconcile places it on this same pass; a zero rect until then.
        let bounds = PhysicalRect::default();
        let engine_bounds = koru_webview::PhysicalRect {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        };
        match WebSurface::open(host.window().as_ref(), engine_bounds, config, host.waker()) {
            Ok((surface, receiver)) => {
                self.surface = Some(Rc::new(HostedWebSurface::new(surface)));
                self.receiver = Some(receiver);
                self.state = State::Loading;
                self.deadline = Some(now + PAGE_DEADLINE);
                super::debug::log(&format!("created generation {}", self.generation));
            }
            Err(WebError::RuntimeUnavailable(why)) => {
                tracing::warn!(target: "koru::support", "webview_unavailable: {why}");
                self.fail(Failure::NoRuntime(why));
            }
            Err(error) => {
                tracing::warn!(target: "koru::support", "load_failed stage=create: {error}");
                self.fail(Failure::Create(error.to_string()));
            }
        }
    }

    fn apply(&mut self, event: WebEvent, now: Instant) -> bool {
        match event {
            WebEvent::PageStarted { .. } => false,
            WebEvent::PageFinished { .. } => {
                if self.state == State::Loading {
                    self.state = State::Booting;
                    self.deadline = Some(now + READY_DEADLINE);
                    super::debug::log("page finished; waiting for the Messenger");
                    true
                } else {
                    false
                }
            }
            WebEvent::Ipc { body, source } => {
                if !url::same_origin_for_ipc(&self.origin, &source) {
                    super::debug::log("ipc from a foreign origin dropped");
                    return false;
                }
                self.apply_ipc(&body)
            }
            WebEvent::External { url } | WebEvent::NewWindow { url } => {
                self.external.push_back(url);
                true
            }
            WebEvent::Refused { .. } => false,
        }
    }

    /// The page's messages are small JSON objects with one key each:
    /// `{"ready":true}`, `{"unread":n}`, `{"error":"…"}`, `{"hidden":true}`.
    /// Read without a JSON dependency: the keys are ours and the values are
    /// a bool, a number or a short string.
    fn apply_ipc(&mut self, body: &str) -> bool {
        if body.contains("\"ready\"") {
            if matches!(self.state, State::Loading | State::Booting) {
                self.state = State::Live;
                self.deadline = None;
                super::debug::log("ready");
                return true;
            }
            return false;
        }
        if let Some(n) = json_u32(body, "unread") {
            return self.unread_changed(n);
        }
        if let Some(what) = json_string(body, "error") {
            tracing::warn!(target: "koru::support", "load_failed stage=page: {what}");
            self.fail(Failure::Page(what));
            return true;
        }
        false
    }

    /// The unread rule (D18, corrected per the rev7 and rev9 reviews). The
    /// count is authoritative; the baseline is what "replied" is judged
    /// against; the badge is presentation. A decrease lowers the baseline so
    /// a later increase counts again; an equal count does nothing.
    fn unread_changed(&mut self, n: u32) -> bool {
        let previous = self.unread;
        self.unread = n;
        if n < self.baseline {
            self.baseline = n;
        }
        if n > self.baseline && !self.window_visible && self.mode != Mode::Ended {
            self.baseline = n;
            if self.app_hidden {
                self.replied = true;
            }
            // In Quiet mode the view turns this notice into opening the
            // window (`opens_on_reply`); the mode stays Quiet until the
            // window is shown, which is what `shown` records.
            self.notice_owed = true;
            super::debug::log(&format!("support replied ({n})"));
            return true;
        }
        previous != n
    }

    pub fn surface_failed(&mut self, what: &str, reason: String) {
        tracing::warn!(target: "koru::support", "load_failed stage={what}: {reason}");
        self.fail(Failure::Surface(reason));
    }

    fn fail(&mut self, failure: Failure) {
        self.deadline = None;
        self.state = State::Failed(failure);
    }

    /// End the chat, D19: the surface and its receiver are dropped — the
    /// surface before the window it is a child of, which is still alive
    /// because this runs from the window's own close path or from
    /// `Host::exiting` — and the state is terminal. The visitor cookie in the
    /// data directory is untouched, so the next open shows the history.
    /// Idempotent.
    pub fn end(&mut self) {
        if self.state == State::Closed {
            return;
        }
        self.generation += 1;
        self.drop_surface();
        // The host holds an `Arc` of the chat window. The pane is about to go
        // (`BackgroundIntent::Close`), and this session outlives it in the
        // app's `Manager`: keeping the handle here kept the window itself
        // alive after its pane -- a dead window nothing painted or answered.
        self.host = None;
        self.state = State::Closed;
        self.mode = Mode::Ended;
        self.replied = false;
        self.notice_owed = false;
        tracing::info!(target: "koru::support", "chat_ended");
    }

    /// Run a script on the page: the `window.koruSupport` calls the page
    /// exposes. Nothing to run on is not an error; the row that asked is
    /// only offered while the chat is live.
    pub fn call_js(&self, js: &str) {
        if let Some(surface) = self.surface.as_ref()
            && let Err(error) = surface.eval(js)
        {
            tracing::warn!(target: "koru::support", "page call failed: {error}");
        }
    }

    fn drop_surface(&mut self) {
        // The receiver first: a late event has nowhere to go.
        self.receiver = None;
        self.surface = None;
        self.deadline = None;
    }
}

fn json_u32(body: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{key}\":");
    let rest = &body[body.find(&needle)? + needle.len()..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn json_string(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let rest = &body[body.find(&needle)? + needle.len()..];
    Some(rest.split('"').next()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_messages_are_read_by_key() {
        assert_eq!(json_u32(r#"{"unread":3}"#, "unread"), Some(3));
        assert_eq!(json_u32(r#"{"unread": 12}"#, "unread"), Some(12));
        assert_eq!(json_u32(r#"{"ready":true}"#, "unread"), None);
        assert_eq!(
            json_string(r#"{"error":"intercom-missing"}"#, "error").as_deref(),
            Some("intercom-missing")
        );
    }

    fn session() -> Session {
        SupportSession::new("https://www.koru.do".into(), koru_i18n::Lang::En)
    }

    #[test]
    fn open_waits_one_pass_and_a_ready_message_makes_it_live() {
        let s = session();
        let now = Instant::now();
        let mut s = s.borrow_mut();
        s.request_open(now);
        assert_eq!(s.state, State::Creating);
        // No host attached: creation fails honestly rather than panicking.
        s.maintain(now);
        assert!(matches!(s.state, State::Failed(Failure::Create(_))));
        // Retry bumps the generation and goes round again.
        s.retry(now);
        assert_eq!(s.generation, 1);
        assert_eq!(s.state, State::Creating);
        // Simulate the engine: state Loading, then the page's messages.
        s.state = State::Loading;
        s.deadline = Some(now + PAGE_DEADLINE);
        assert!(s.apply(WebEvent::PageFinished { url: "x".into() }, now));
        assert_eq!(s.state, State::Booting);
        assert!(!s.apply(
            WebEvent::Ipc {
                body: r#"{"ready":true}"#.into(),
                source: "https://evil.example/".into()
            },
            now
        ));
        assert_eq!(
            s.state,
            State::Booting,
            "a foreign origin cannot make it live"
        );
        assert!(s.apply(
            WebEvent::Ipc {
                body: r#"{"ready":true}"#.into(),
                source: "https://www.koru.do/en/support/desktop".into()
            },
            now
        ));
        assert_eq!(s.state, State::Live);
        assert_eq!(s.deadline, None);
    }

    #[test]
    fn a_deadline_that_passes_fails_with_its_stage() {
        let s = session();
        let mut s = s.borrow_mut();
        let now = Instant::now();
        s.state = State::Loading;
        s.deadline = Some(now);
        assert!(s.maintain(now + Duration::from_millis(1)));
        assert_eq!(s.state, State::Failed(Failure::Timeout("page")));
        s.state = State::Booting;
        s.deadline = Some(now);
        s.maintain(now + Duration::from_millis(1));
        assert_eq!(s.state, State::Failed(Failure::Timeout("ready")));
    }

    #[test]
    fn end_is_terminal_and_idempotent() {
        let s = session();
        let mut s = s.borrow_mut();
        s.state = State::Live;
        s.end();
        assert_eq!(s.state, State::Closed);
        let generation = s.generation;
        s.end();
        assert_eq!(s.generation, generation, "a second end changes nothing");
        assert!(s.surface().is_none());
    }

    #[test]
    fn the_unread_rule_notifies_on_a_rise_over_the_baseline_only() {
        let s = session();
        let mut s = s.borrow_mut();
        s.state = State::Live;
        // Registration, with the chat on screen: count 1, nothing owed.
        s.unread_changed(1);
        assert!(!s.take_notice());
        // Close and notify at 1: the baseline is 1. Then 2 arrives: a notice
        // is owed, and in this mode the view opens the window on it.
        s.choose(Mode::Quiet);
        assert_eq!(s.baseline, 1);
        assert!(s.unread_changed(2));
        assert_eq!(s.mode, Mode::Quiet);
        assert!(s.opens_on_reply());
        assert!(s.take_notice());
        // 2 again: nothing.
        assert!(!s.unread_changed(2));
        assert!(!s.take_notice());
        // 2 → 1 lowers the baseline; 1 → 2 counts again.
        s.unread_changed(1);
        assert_eq!(s.baseline, 1);
        assert!(s.unread_changed(2));
        assert!(s.take_notice());
        // While the chat is on screen, no notice and no badge.
        s.shown();
        assert!(!s.unread_changed(3) || !s.take_notice());
        assert_eq!(s.badge(), 0);
    }

    #[test]
    fn replied_is_permission_for_the_current_hide_interval_only() {
        let s = session();
        let mut s = s.borrow_mut();
        s.state = State::Live;
        s.choose(Mode::Fab);
        // In the tray: a reply shows the icon.
        s.observe_app_hidden(true);
        assert!(
            !s.fab_visible(true),
            "a hide interval starts with the icon hidden"
        );
        s.unread_changed(1);
        assert!(s.fab_visible(true));
        // Quiet from the popup clears it; a wake with no change keeps it hidden.
        s.choose(Mode::Quiet);
        assert!(!s.fab_visible(true));
        assert!(!s.unread_changed(1));
        assert!(!s.fab_visible(true));
        // A later reply opens the window instead of showing the icon.
        s.unread_changed(2);
        assert!(!s.fab_visible(true));
        assert!(s.opens_on_reply());
        // A reply while visible, then a hide: no icon from the old reply.
        s.observe_app_hidden(false);
        s.choose(Mode::Fab);
        s.unread_changed(3);
        s.observe_app_hidden(true);
        assert!(!s.fab_visible(true));
        // Restore: Fab mode shows the icon, Quiet does not.
        s.observe_app_hidden(false);
        assert!(s.fab_visible(false));
        s.choose(Mode::Quiet);
        assert!(!s.fab_visible(false));
    }

    #[test]
    fn external_urls_queue_for_the_browser_and_unread_is_kept() {
        let s = session();
        let mut s = s.borrow_mut();
        let now = Instant::now();
        s.state = State::Live;
        assert!(s.apply(
            WebEvent::External {
                url: "https://help.example/a".into()
            },
            now
        ));
        assert!(s.apply(
            WebEvent::NewWindow {
                url: "https://help.example/b".into()
            },
            now
        ));
        assert_eq!(s.take_external().as_deref(), Some("https://help.example/a"));
        assert_eq!(s.take_external().as_deref(), Some("https://help.example/b"));
        assert_eq!(s.take_external(), None);
        let ipc = |n: u32| WebEvent::Ipc {
            body: format!(r#"{{"unread":{n}}}"#),
            source: "https://www.koru.do/en/support/desktop".into(),
        };
        assert!(s.apply(ipc(2), now));
        assert!(!s.apply(ipc(2), now));
        assert_eq!(s.unread, 2);
    }
}
