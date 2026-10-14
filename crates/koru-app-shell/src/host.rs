// SPDX-License-Identifier: MPL-2.0

//! What an application supplies. The shell can open nothing itself.
//!
//! Everything a tab, a pane, a profile or a workspace *means* is here. The
//! shell holds windows, chrome geometry, a tab manager and a drain loop, and
//! asks this trait for every decision it is not entitled to make.

use koru_draw::geometry::Rect;
use koru_window::{ClosingWindowState, ImeEvent, RichKeyEvent, WindowKey};
use vello::Scene;
use vello::kurbo::Affine;
use winit::dpi::PhysicalPosition;
use winit::event_loop::EventLoopProxy;
use winit::window::{CursorIcon, WindowAttributes};

use crate::command::ShellCommand;
use crate::keys::{ProfileKey, WorkspaceKey};
use crate::state::Side;
use crate::surface::{PointerEvent, Surface};

/// Which banner slot a press landed in.
///
/// Mirrors `koru_shell_chrome::banner::Hit` rather than re-exporting it, so the
/// shell's vocabulary is one enum a host matches on and the chrome crate stays
/// free to grow a slot the shell has not routed yet.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BannerSlot {
    Profile,
    Workspace,
    Settings,
    /// The merged identity square the collapse ladder ends in — one chip
    /// opening a two-section popover (DESIGN, "The banner" §2, step 3). A host
    /// that receives this offers *Profiles* and *Workspaces* in one popover;
    /// nothing else about it differs, which is why it is a fourth slot rather
    /// than a mode flag on the other two.
    Identity,
}

/// What the caption's banner shows in one window.
///
/// **Host-supplied, localized text.** The neutral chrome renders a string; it
/// does not know what "No profile" says, and it may not — the whole point of
/// `koru-shell-chrome` shedding the status chip's hard-coded copy (the module
/// `offline.rs` became `status_chip.rs` when the words left).
///
/// `None` on either selector is a slot that is not offered, which the chrome
/// draws as a zero-width rect and the tab strip reclaims with no second code
/// path.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Banner {
    pub profile: Option<BannerChip>,
    pub workspace: Option<BannerChip>,
    pub settings: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BannerChip {
    /// Already localized. The chrome measures it and clamps it; it never
    /// decides what it says.
    pub label: String,
    /// What the square shows when the slot draws as a square — initials, an
    /// emoji, a single glyph. **The host computes it**, because "the first two
    /// letters of a name" is a fact about names in eight languages and the
    /// shell has no `koru-i18n`. Empty draws a bare tile.
    pub mark: String,
    /// An opaque key the chip's tint is derived from. The shell hashes it into
    /// one of the four existing pill palettes and learns nothing else; DESIGN
    /// says the profile's is keyed on the email rather than the name, so
    /// renaming yourself does not change your colour — which only the host can
    /// know.
    pub tint: String,
    /// What the host will recognise when the slot is pressed. Sixteen bytes;
    /// the shell learns nothing from them.
    pub key: BannerKey,
    /// A quiet, non-interactive slot — the Gateway is unavailable, so the menus
    /// are (R3/S9). Presentation is chrome's; the reason is the host's.
    pub enabled: bool,
    /// **What the chip's tooltip says**, already localized — the *action*
    /// ("Switch workspace"), or the reason it will not open when it is dimmed.
    ///
    /// DESIGN, "The banner" §8: both chips carry a tooltip, because both are
    /// unfamiliar and one of them is icon-only, and it names the action rather
    /// than repeating the label — the label is already the thing being pointed
    /// at. The presentation and the clock are chrome's; every word is the
    /// host's. Empty draws none.
    pub tooltip: String,
}

impl BannerChip {
    /// An enabled chip with no mark and no tint key of its own.
    ///
    /// The builders below are what a host reaches for; this is the floor, and
    /// it exists so a host that has only a name is not forced to invent an
    /// avatar and a palette key to say so.
    pub fn new(label: impl Into<String>, key: BannerKey) -> Self {
        let label = label.into();
        Self {
            tint: label.clone(),
            label,
            mark: String::new(),
            key,
            enabled: true,
            tooltip: String::new(),
        }
    }

    pub fn with_mark(mut self, mark: impl Into<String>) -> Self {
        self.mark = mark.into();
        self
    }

    /// The key the tint is hashed from — DESIGN says the profile's is the
    /// email rather than the name, so renaming yourself does not change your
    /// colour.
    pub fn with_tint(mut self, tint: impl Into<String>) -> Self {
        self.tint = tint.into();
        self
    }

    /// What the tooltip says while the chip works: the action, and the shortcut
    /// when there is one.
    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = tooltip.into();
        self
    }

    /// Dimmed, and the tooltip becomes the reason. S9's quiet state: it stays,
    /// it says why in the person's language, and it does not open.
    pub fn unavailable(mut self, reason: impl Into<String>) -> Self {
        self.enabled = false;
        self.tooltip = reason.into();
        self
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BannerKey {
    Profile(ProfileKey),
    Workspace(WorkspaceKey),
}

/// What a host is lent while the shell asks it something.
///
/// Unlike [`SurfaceCx`](crate::surface::SurfaceCx), commands here take an
/// explicit window: a host legitimately addresses windows other than the one
/// the event arrived in — closing every window of a removed profile, focusing
/// the window that owns a session the Agent named.
pub struct HostCx<'a> {
    /// The runner's `TextEngine`, when this dispatch has one — see
    /// [`SurfaceCx::engine`](crate::surface::SurfaceCx::engine) for why it is an
    /// `Option` and not a promise.
    pub engine: Option<&'a mut koru_draw::text::TextEngine>,
    queued: &'a mut Vec<ShellCommand>,
}

impl<'a> HostCx<'a> {
    pub(crate) fn new(
        engine: Option<&'a mut koru_draw::text::TextEngine>,
        queued: &'a mut Vec<ShellCommand>,
    ) -> Self {
        Self { engine, queued }
    }

    pub fn command(&mut self, window: WindowKey, action: crate::command::ShellAction) {
        self.queued.push(ShellCommand { window, action });
    }
}

/// Why a window could not be seated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SeatRefused(pub String);

/// **Where a popover's rows go this frame.**
///
/// The rect has always been the whole of it. `travel` is what a panel showing
/// two views at once needs: while one view replaces another
/// ([`ShellAction::PopoverSlide`](crate::ShellAction::PopoverSlide)) both are on
/// screen, and this is how far each is drawn from the panel's own left edge —
/// `(leaving, arriving)`, in logical pixels, one panel width apart. `None` is
/// the ordinary case: one view, at zero.
///
/// **The host owns which view is which and the shell owns the clock**, which is
/// the arrangement DESIGN's chevron already has with the panel it reads: two
/// clocks for one event is two things that can drift apart.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct PopoverFrame {
    /// The panel's own rect, in window space, under `root`.
    pub bounds: Rect,
    pub travel: Option<(f32, f32)>,
}

/// How the tab a seated window opens on is *presented*.
///
/// [`ShellAction::OpenTab`](crate::ShellAction::OpenTab) has carried a title and
/// a `closable` since SP-9, and [`ShellHost::seat_window`] — the same act, for a
/// window's first tab — carried neither, because until SP-13 nothing seated a
/// window. A popped-out session is a titled, closable tab wherever it lands, and
/// a window whose one tab had no ✕ would be a window whose tab strip changed
/// meaning depending on how the tab got there.
///
/// The default is the never-empty floor's own presentation — no title, no ✕ —
/// so a host that does not answer keeps exactly what it had.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SeatedTab {
    /// Already localized, like every other string crossing this boundary.
    pub title: String,
    pub closable: bool,
}

impl SeatedTab {
    pub fn new(title: impl Into<String>, closable: bool) -> Self {
        Self {
            title: title.into(),
            closable,
        }
    }
}

/// The application behind the chrome.
pub trait ShellHost {
    /// What this host's background threads send in.
    ///
    /// **SP-9 left the shell's wake type as `()` and named it SP-10's
    /// question** (the closed UIREFACTOR program's decision log, 2026-09-01,
    /// in git history). This is
    /// the answer, and it is the only one that does not add a dispatcher:
    /// [`ShellState`](crate::ShellState) forwards its own
    /// [`MultiWindowApp::Event`](koru_window::MultiWindowApp::Event) straight
    /// through to [`wake`](Self::wake) without inspecting it. A host with no
    /// threads sets `()` and never implements either method. There is still
    /// exactly one event loop and exactly one proxy.
    type Event: Send + 'static;

    /// The proxy this host's threads will send on, handed over once before the
    /// loop starts.
    fn attach(&mut self, proxy: EventLoopProxy<Self::Event>) {
        let _ = proxy;
    }

    /// Something a background thread sent. Answers whether anything visible
    /// changed.
    ///
    /// **The host, not a surface.** A wake carries no window and no tab: which
    /// surface an event belongs to is a fact about the host's own model, and a
    /// shell that routed one would be a shell that knew what a session is.
    fn wake(&mut self, cx: &mut HostCx<'_>, event: Self::Event) -> bool {
        let _ = (cx, event);
        false
    }

    /// The attributes this window opens with — title, size, position,
    /// maximised.
    ///
    /// The shell always applies its own two: undecorated, because it draws the
    /// caption, and never smaller than [`MIN_WINDOW_SIZE`](crate::MIN_WINDOW_SIZE).
    /// Everything else — a restored geometry row, a localized title — is the
    /// host's, and `None` keeps the shell's own defaults.
    fn window_attributes(&self, window: WindowKey) -> Option<WindowAttributes> {
        let _ = window;
        None
    }

    /// The surface a brand-new window opens on, and the one a never-empty tab
    /// strip mints when its last tab closes.
    ///
    /// **A factory, not a stored prototype.** ADR-262's floor "mints through a
    /// host factory", and a prototype would be payload the shell persists —
    /// which the minimal contract forbids.
    fn floor_surface(&mut self, cx: &mut HostCx<'_>, window: WindowKey) -> Box<dyn Surface>;

    /// How the floor tab is **presented**, which is not the same question as
    /// what surface is behind it.
    ///
    /// [`floor_surface`](Self::floor_surface) has always answered the second and
    /// nothing answered the first, so every floor tab was minted with
    /// [`SeatedTab::default`] — an empty title. A window that opens on a Welcome
    /// tab therefore drew a pill with no label in it, and a tab with no label is
    /// a tab nobody can name.
    ///
    /// It is a second question rather than a wider return type from
    /// `floor_surface` for the reason [`SeatedTab`] gives about
    /// [`seat_window`](Self::seat_window): the strip mints floor *tabs* in one
    /// borrow and seats *surfaces* in another (see `ShellTab`), so the
    /// presentation has to be askable without a `&mut` and without a surface.
    ///
    /// The default is what every host had before, so a host that does not answer
    /// keeps its silent floor.
    fn floor_tab(&self, window: WindowKey) -> SeatedTab {
        let _ = window;
        SeatedTab::default()
    }

    /// The banner's current content for one window.
    ///
    /// Asked at draw and at hit, so the two cannot disagree about how wide a
    /// workspace label is. Cheap: a host that fetched a catalog here would be
    /// fetching it per frame, and R3 says fetch on menu open.
    fn banner(&self, window: WindowKey) -> Banner {
        let _ = window;
        Banner::default()
    }

    /// A banner slot was pressed. The chrome has answered everything it knows.
    ///
    /// What opens, what it lists, and whether the catalog is even reachable are
    /// all the host's. The shell does not learn that a workspace exists.
    ///
    /// A host that wants the shell's popover records
    /// [`ShellAction::Popover`](crate::ShellAction::Popover) here; one that
    /// opens something of its own simply returns `true`.
    fn slot_pressed(&mut self, cx: &mut HostCx<'_>, window: WindowKey, slot: BannerSlot) -> bool {
        let _ = (cx, window, slot);
        false
    }

    /// How tall the popover for `slot` wants to be, and how wide.
    ///
    /// The shell owns the anchor, the flip and the clamp; the size is the only
    /// thing it cannot work out, because the rows are the host's. DESIGN's
    /// metrics — `PANEL_W` for profiles, 230 for workspaces, row pitch 32,
    /// pad 16 — are the host's to apply.
    fn popover_size(&self, window: WindowKey, slot: BannerSlot) -> (f32, f32) {
        let _ = (window, slot);
        (0.0, 0.0)
    }

    /// Draw the popover's rows into the panel the shell has already placed and
    /// filled. See [`PopoverFrame`] for the rect and the travel.
    fn draw_popover(
        &mut self,
        cx: &mut HostCx<'_>,
        scene: &mut Scene,
        root: Affine,
        window: WindowKey,
        slot: BannerSlot,
        frame: PopoverFrame,
    ) {
        let _ = (cx, scene, root, window, slot, frame);
    }

    /// A press inside the open popover. Outside it the shell dismisses and the
    /// host is not asked.
    fn popover_pressed(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        slot: BannerSlot,
        x: f32,
        y: f32,
    ) -> bool {
        let _ = (cx, window, slot, x, y);
        false
    }

    /// **The wheel over an open popover, in lines, positive up.**
    ///
    /// The shell owns the panel and therefore owns the question *is the pointer
    /// on it*; how far a list may scroll is the host's, because only the host
    /// knows how many rows there are and what the panel pins above and below
    /// them. Answering `false` — the default — is a popover that does not
    /// scroll, and the shell then simply swallows the wheel rather than letting
    /// it reach the column underneath the panel.
    fn popover_scrolled(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        slot: BannerSlot,
        lines: f32,
    ) -> bool {
        let _ = (cx, window, slot, lines);
        false
    }

    /// The host's chance at a key **before any surface sees it**.
    ///
    /// Returning `true` consumes. This exists because the chrome must not
    /// install an accelerator table above the host (ADR-262) *and* because a
    /// window-layer chord has to outrank a surface that wants raw bytes: the
    /// Terminal keeps swallowing Ctrl+C precisely because this hook is the
    /// host's own key map rather than the shell's.
    ///
    /// Ctrl+Tab never reaches here. **The shell resolves it** from
    /// `physical_key` and turns it into a tab selection, for the reason
    /// `View::cycle_tab` already records: Ctrl+Tab and Ctrl+I are the same
    /// logical key on Windows. SP-0 expected the runner to do this; SP-1's
    /// runner does not (a runner that knew about tabs would be a runner with
    /// shell policy in it), so it lands here — one step lower, same guarantee.
    fn window_key(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        event: RichKeyEvent<'_>,
    ) -> bool {
        let _ = (cx, window, event);
        false
    }

    /// A window the host asked for now exists, with a live render surface.
    ///
    /// **The commit point of two-phase pop-out**, and it is the runner's own:
    /// `MultiWindowApp::seated` fires once the window *and* its surface exist.
    /// Until this returns `Ok`, the source tab is untouched; a `SeatRefused` and
    /// the shell drops the new window with nothing lost.
    fn seat_window(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
    ) -> Result<Box<dyn Surface>, SeatRefused>;

    /// How that window's first tab is presented — its title, and whether it has
    /// a ✕. Asked immediately after [`seat_window`](Self::seat_window) answered
    /// `Ok`, and never for a window opened on the host's floor.
    ///
    /// See [`SeatedTab`] for why it is a second question rather than a wider
    /// return type: `seat_window` is implemented by every host, and the answer
    /// that leaves them all unchanged is the floor's.
    fn seated_tab(&self, window: WindowKey) -> SeatedTab {
        let _ = window;
        SeatedTab::default()
    }

    /// The shell could not create the window the host asked for. Roll back.
    fn window_open_failed(&mut self, cx: &mut HostCx<'_>) {
        let _ = cx;
    }

    /// A window closed. The host drops whatever it kept for it.
    fn window_closed(&mut self, cx: &mut HostCx<'_>, window: WindowKey) {
        let _ = (cx, window);
    }

    /// This window is about to be dropped, with its last live OS state.
    ///
    /// **The moment a per-window geometry row has to be written**, since the
    /// position and size are unrecoverable a line later. ADR-226 is *caller owns
    /// the key*: the runner supplies the state and never touches `windows.conf`
    /// itself, and the row name and value schema are the host's — which is what
    /// lets two hosts share one file.
    fn window_closing(&mut self, window: WindowKey, state: ClosingWindowState) {
        let _ = (window, state);
    }

    /// Whether the process should end. Asked after every event, and the honest
    /// default is "when the last window has gone" — but what a last close means
    /// is the host's (a menu-bar app outlives its windows).
    fn should_exit(&self, windows: usize) -> bool {
        windows == 0
    }

    // -------------------------------------------------------- the tab's life
    //
    // Three notifications, and the reason they are the host's rather than the
    // surface's is the same each time: the answer needs the host's model. A
    // surface knows it was activated; only the host knows that the surface
    // activated is the one whose session must now take the controller lease,
    // and that the one deactivated must give it up (R2, ADR-263).

    /// A tab now exists, with the key the shell minted for it.
    ///
    /// **The host has no other way to learn the key.**
    /// [`ShellAction::OpenTab`](crate::ShellAction::OpenTab) is recorded and
    /// drained, so it cannot return one, and a host that could not name its own
    /// tab could not retitle it, close it, or answer which session it shows.
    fn tab_opened(&mut self, cx: &mut HostCx<'_>, window: WindowKey, tab: crate::TabKey) {
        let _ = (cx, window, tab);
    }

    /// A tab is gone. Whatever the host held for it goes too.
    fn tab_closed(&mut self, cx: &mut HostCx<'_>, window: WindowKey, tab: crate::TabKey) {
        let _ = (cx, window, tab);
    }

    /// This tab is now the active one, and `previous` is the one that was.
    ///
    /// Called **after** the surfaces have been deactivated and activated, so an
    /// unfinished IME preedit is already cancelled by the time a host attaches
    /// anything (ADR-263, and the ordering `Surface::deactivated` documents).
    fn tab_activated(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        tab: crate::TabKey,
        previous: Option<crate::TabKey>,
    ) {
        let _ = (cx, window, tab, previous);
    }

    // ------------------------------------------------------------- the panes
    //
    // The shell owns the *slots* — a [`Panel`](crate::Panel) a side, feeding
    // `regions::solve`. What is drawn in one is the host's, and these three are
    // the whole of that seam. SP-9 deferred `SidePaneSurface` "until SP-11
    // supplies a consumer"; SP-10's left pane is a consumer that arrived first,
    // and it wanted three methods rather than a trait object with a lifecycle.

    /// Whether this pane starts open in a new window.
    ///
    /// Asked once, at creation, because a pane that animated open on the first
    /// frame would be motion nobody asked for. A host that restores a per-window
    /// width answers from the row it read.
    fn pane_open(&self, window: WindowKey, side: Side) -> bool {
        let _ = (window, side);
        false
    }

    /// **How wide this pane opens**, asked once beside
    /// [`pane_open`](Self::pane_open) at creation.
    ///
    /// `None` — the default — keeps the shell's own `theme::SIDEBAR_W` /
    /// `theme::CONTEXT_W`. A host that remembers a per-window width answers from
    /// the row it read, and it is clamped to the same limits a drag is
    /// ([`crate::panel::SIDEBAR_MIN_W`]..[`crate::panel::SIDEBAR_MAX_W`]) — a
    /// stored width is not more trustworthy than a dragged one, and a file
    /// somebody edited by hand must not be able to hand the content less room
    /// than `regions::solve` protects.
    ///
    /// Applied through `Panel::set_immediately`, so a restored width is laid out
    /// against from the first frame: motion on startup is motion nobody asked
    /// for.
    fn pane_width(&self, window: WindowKey, side: Side) -> Option<f32> {
        let _ = (window, side);
        None
    }

    /// **The person let go of a pane's resize grip**, and this is the width it
    /// settled at.
    ///
    /// Once per drag, on release, rather than once per mouse-move: the same
    /// moment the content is allowed to reflow (`Panel::commit_width`), for the
    /// same reason. A host that persists it writes the row here; one that does
    /// not simply forgets the width when the window closes.
    fn pane_resized(&mut self, window: WindowKey, side: Side, width: f32) {
        let _ = (window, side, width);
    }

    /// **Whether the context column is offered at all**, re-asked wherever the
    /// answer can have moved — every draw, every tab activation, every resize.
    ///
    /// SP-12's hook, and the last dead wire of the right slot: the shell has had
    /// `Side::Right` on all four pane methods, `Hit::ToggleContext` routed to
    /// `toggle_pane`, and a `Panel` at `theme::CONTEXT_W` since SP-9 — with
    /// nothing able to turn the toggle on. This is that, and it is a question
    /// rather than a command because the answer is a fact about the host's own
    /// model: R7 says the toggle is visible **only while Details has content**,
    /// and only the host knows whether the active tab is a session.
    ///
    /// **Cheap by contract, and the contract is load-bearing**: it is a `bool`
    /// off state the host already holds, asked once per window per frame. A host
    /// that built its column's content to answer it would be building it twice a
    /// frame and throwing one away.
    ///
    /// `false` makes the toggle a zero-width rect (the caption's idiom) *and*
    /// gives the column no layout width. It does not shut the pane: the person's
    /// open/shut choice stays in the `Panel`, so switching to a Welcome tab and
    /// back leaves the column where they left it. Nothing animates on the change
    /// either — the gate is instant, which is the same rule
    /// `Panel::layout_width` follows and the reason a tab switch is not 140 ms
    /// of motion.
    ///
    /// A host answering `true` must have something to draw. A toggle that opens
    /// a blank column advertises something that is not there, which is the whole
    /// of DESIGN's *"The context column's caption toggle is hidden"*.
    fn context_offered(&self, window: WindowKey) -> bool {
        let _ = window;
        false
    }

    /// **Where this window's identity selectors live** (owner, 2026-09-02).
    ///
    /// `false` — the default, and every window built before this — puts the
    /// profile avatar and the workspace name/chevron in the caption, where they
    /// collapse through DESIGN's ladder against the tab lane.
    ///
    /// `true` puts them in the **left pane's header band** instead: the
    /// `CAPTION_H` strip at the top of the explore column, which is where Koru
    /// itself has always drawn them. The caption's banner slots then go
    /// zero-width — *except while the pane is shut*, when the ladder's step-3
    /// merged identity square appears there so the selectors stay reachable in a
    /// window with no pane on screen.
    ///
    /// A question rather than a build flag, and for the same reason
    /// `window_buttons` and `context_offered` are: **which furniture a window
    /// has is the container's decision**, and the shell has to be able to host
    /// both arrangements in one process. It buys no second code path — the same
    /// `banner::identity_slots` arithmetic lays the chips out in either band,
    /// and the ladder is the same ladder against a different budget.
    fn identity_in_pane(&self, window: WindowKey) -> bool {
        let _ = window;
        false
    }

    /// **Whether the Settings slot's own surface is what this window is
    /// showing**, so the slot reads *selected* while it is.
    ///
    /// The two pane toggles have been able to answer this since they existed —
    /// the shell owns the columns, so it knows when one is open — and Settings
    /// could not, because what it opens is a *tab* and tabs are the host's. So
    /// the caption had one control that never lit and two that did, which is a
    /// row of three that does not read as a row of three (DESIGN, "Hover,
    /// Pressed, and Selected States": a control's state must not be a fact only
    /// some of its neighbours can express).
    ///
    /// A question rather than a flag, for the reason every other question here
    /// is one: *what* the Settings slot opens is the container's decision, and a
    /// host that opens no Settings surface answers `false` and loses nothing.
    fn settings_open(&self, window: WindowKey) -> bool {
        let _ = window;
        false
    }

    /// Draw one open pane. `bounds` is where it is *drawn*, which during a
    /// slide is not where it was laid out — the shell has already applied
    /// `Panel::slide_offset`.
    fn draw_pane(
        &mut self,
        cx: &mut HostCx<'_>,
        scene: &mut Scene,
        root: Affine,
        window: WindowKey,
        side: Side,
        bounds: Rect,
    ) {
        let _ = (cx, scene, root, window, side, bounds);
    }

    /// A pointer event inside an open pane, in **window** coordinates — the
    /// pane's own rect travels with the slide and a surface-local point would
    /// move under the pointer mid-animation.
    fn pane_pointer(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        side: Side,
        event: PointerEvent,
    ) -> bool {
        let _ = (cx, window, side, event);
        false
    }

    fn pane_cursor_icon(
        &self,
        window: WindowKey,
        side: Side,
        x: f32,
        y: f32,
    ) -> Option<CursorIcon> {
        let _ = (window, side, x, y);
        None
    }

    // ---------------------------------------------------------- the overlays
    //
    // A menu, a modal and a tooltip are the host's — ADR-262 is explicit that
    // "the overflow menu is the host's: what is in it is product, and a shell
    // that opened one would be a shell with a menu model". So the shell gives
    // the host the top of the frame and the first look at input, and owns
    // neither.

    /// **The host's own tick, at the top of a frame, with an engine.**
    ///
    /// A host has work that is neither a surface's nor a pane's and cannot wait
    /// for input: an authoritative settings section that arrived on another
    /// thread has to be re-measured against a `TextEngine`, and a backpressure
    /// flag a sender raised has to be noticed. Both used to happen in the
    /// window's own `draw`, which no longer exists.
    ///
    /// It is *not* a place to lay anything out: the chrome and the surfaces are
    /// drawn after it, and doing work here that a surface will redo is how a
    /// frame budget disappears.
    fn frame(&mut self, cx: &mut HostCx<'_>, window: WindowKey) {
        let _ = (cx, window);
    }

    /// The **window** gained or lost the keyboard focus.
    ///
    /// The active surface is told separately, because it changes what that
    /// surface draws. This is for what the *host* does with it: the Terminal
    /// records focus order with its Agent, and every session surface — not only
    /// the active one — has to stop believing the window is focused.
    fn window_focus_changed(&mut self, window: WindowKey, focused: bool) -> bool {
        let _ = (window, focused);
        false
    }

    /// The frame the preceding draw built reached the screen.
    ///
    /// The active surface is told separately. This is the host's half:
    /// presentation acknowledgements it aggregates across tabs, and the
    /// switch-to-input sample, which is only a sample once the destination is
    /// genuinely on screen.
    fn presented(&mut self, window: WindowKey) {
        let _ = window;
    }

    /// Draw whatever floats above everything: menus, confirmations, tooltips.
    /// Called last, after the surface, the chrome and the popover.
    fn draw_overlay(
        &mut self,
        cx: &mut HostCx<'_>,
        scene: &mut Scene,
        root: Affine,
        window: WindowKey,
        viewport: Rect,
    ) {
        let _ = (cx, scene, root, window, viewport);
    }

    /// The host's chance at a pointer event **before the chrome and before any
    /// surface**, in window coordinates. Returning `true` consumes.
    ///
    /// This is what makes a modal modal: the Terminal's terminate confirmation
    /// has to win the click over the grid beneath it, and it can only do that
    /// from above the chrome.
    fn overlay_pointer(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        event: PointerEvent,
    ) -> bool {
        let _ = (cx, window, event);
        false
    }

    /// The same for composition. **S16's modal half**: an open modal consumes
    /// IME so a commit never reaches the PTY behind it.
    fn overlay_ime(&mut self, cx: &mut HostCx<'_>, window: WindowKey, event: ImeEvent<'_>) -> bool {
        let _ = (cx, window, event);
        false
    }

    fn overlay_cursor_icon(&self, window: WindowKey, x: f32, y: f32) -> Option<CursorIcon> {
        let _ = (window, x, y);
        None
    }

    /// A surface asked its host for something, by a number the host minted.
    /// See [`ShellAction::HostRequest`](crate::ShellAction::HostRequest).
    fn host_request(&mut self, cx: &mut HostCx<'_>, window: WindowKey, request: u64) {
        let _ = (cx, window, request);
    }

    // ------------------------------------------------- the caption's actions

    /// The `+` was pressed. `anchor` is its rect, for a menu to hang from.
    fn new_tab_pressed(&mut self, cx: &mut HostCx<'_>, window: WindowKey, anchor: Rect) -> bool {
        let _ = (cx, window, anchor);
        false
    }

    /// The overflow control was pressed. `anchor` is its rect.
    fn window_menu_pressed(
        &mut self,
        cx: &mut HostCx<'_>,
        window: WindowKey,
        anchor: Rect,
    ) -> bool {
        let _ = (cx, window, anchor);
        false
    }

    // --------------------------------------------------------- window state

    /// The OS maximised or restored this window.
    fn set_maximised(&mut self, window: WindowKey, maximised: bool) -> bool {
        let _ = (window, maximised);
        false
    }

    fn set_pinned(&mut self, window: WindowKey, pinned: bool) -> bool {
        let _ = (window, pinned);
        false
    }

    fn pinned(&self, window: WindowKey) -> bool {
        let _ = window;
        false
    }

    /// Whether this window should be brought forward, drained after every
    /// event. An Agent asking a host to come forward is why it exists.
    fn take_activation_request(&mut self, window: WindowKey) -> bool {
        let _ = window;
        false
    }

    /// The compositor stopped or resumed presenting a window.
    ///
    /// **On the host, not on `Surface`.** It changes nothing a surface draws;
    /// it only stops metrics counting time nobody could see. Routing it to the
    /// active surface would be the shell deciding a surface has metrics.
    fn window_occluded(&mut self, window: WindowKey, occluded: bool) {
        let _ = (window, occluded);
    }

    /// The window moved. One event per pixel during a drag, which is exactly
    /// why it must never reach a surface: it changes nothing one draws.
    fn window_moved(&mut self, window: WindowKey, position: PhysicalPosition<i32>) {
        let _ = (window, position);
    }
}
