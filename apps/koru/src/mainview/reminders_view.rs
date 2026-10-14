// SPDX-License-Identifier: MPL-2.0

//! The All Reminders page — the `reminder_management` component's second
//! host, filling a tab in the strip instead of the bell's popover.
//!
//! One component, two hosts. The bell's shell (`reminder_shell.rs`) shows the
//! management surface modally inside its popover, with a back arrow, a close
//! ×, a 400px clamp and a page slide. This view shows the same surface as an
//! ordinary page: it owns a [`Management`], a [`Presentation`] and the inputs
//! both borrow, feeds them on the container's maintain pass exactly the way
//! the shell is fed, and hands the component the whole content rect with
//! [`Chrome::Page`] — no back arrow, no close ×, no clamp; the tab strip's ×
//! is the close, and Esc never navigates.
//!
//! The page is **not modal**. Keys, scroll and the pointer arrive through the
//! ordinary tab dispatch, the way the Tasks page's do; `Layer::ReminderOverlay`
//! stays exclusively the bell popover's. Intents the component reports queue
//! here and drain through the container's existing
//! `apply_reminder_shell_intent`, which is host-agnostic.
//!
//! **A background Reminders tab costs a bool**, the Tasks page's rule: the
//! container feeds this view only while its tab is in front, and
//! [`maintain`](Self::maintain) answers `false` for every other tab.

use koru_core::schema::WorkspaceDoc;
use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_editor::reminders::GlobalManagement;
use koru_workspace_host::View;
use vello::Scene;
use vello::kurbo::Affine;
use winit::event::MouseScrollDelta;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

use std::time::Instant;

use crate::reminder_settings::HealthPresentation;
use koru_i18n::Lang;

use super::reminder_management::{Chrome, Management};
use super::reminder_shell::{Intent, Presentation, PresentationEnvironment, banner_text};

/// What the tab strip reads. A fixed word, like the Trash tab's: the page is
/// one place, not a queue's five.
pub(crate) struct RemindersView {
    management: Management,
    presentation: Presentation,
    global: GlobalManagement,
    health: Option<HealthPresentation>,
    mutation_failure: bool,
    edit_available: bool,
    recreate_available: bool,
    /// The tab views' own engine, for the empty state's centered label —
    /// measuring for geometry happens with the container's, in
    /// [`prepare_action_widths`](Self::prepare_action_widths).
    engine: TextEngine,
    /// The content rect's size, pushed by the layout. The component's content
    /// rect is the whole of it — this host has no chrome of its own.
    viewport: (f32, f32),
    geometry_viewport: Option<(f32, f32)>,
    geometry_dirty: bool,
    /// One intent the component reported, waiting for the container's drain.
    pending: Option<Intent>,
    /// Whether the last pointer position is over a press target, for
    /// [`View::cursor_icon`].
    hover_press: bool,
}

impl RemindersView {
    pub(super) fn new() -> Self {
        Self {
            management: Management::default(),
            presentation: Presentation::default(),
            global: GlobalManagement::default(),
            health: None,
            mutation_failure: false,
            edit_available: false,
            recreate_available: false,
            engine: TextEngine::new(),
            viewport: (0.0, 0.0),
            geometry_viewport: None,
            geometry_dirty: true,
            pending: None,
            hover_press: false,
        }
    }

    /// The container's one question between gestures.
    pub(super) fn take_intent(&mut self) -> Option<Intent> {
        self.pending.take()
    }

    // ---------------------------------------------------------- the feeding
    //
    // The same contract the bell's shell is fed with, in the same order, from
    // the same maintain pass — and only while this tab is in front.

    pub(super) fn set_global(
        &mut self,
        global: &GlobalManagement,
        environment: PresentationEnvironment,
    ) -> bool {
        if &self.global == global && !self.presentation.environment_changed(&environment) {
            return false;
        }
        self.global = global.clone();
        self.presentation.set_environment(environment);
        self.management.sync_filter(global.filter);
        self.presentation.rebuild_rows(&self.global);
        self.presentation
            .rebuild_snooze(self.management.snooze.as_ref(), self.management.now_ms);
        self.geometry_dirty = true;
        true
    }

    pub(super) fn set_health(&mut self, health: HealthPresentation) -> bool {
        if self.health.as_ref() == Some(&health) {
            return false;
        }
        self.health = Some(health);
        self.geometry_dirty = true;
        true
    }

    pub(super) fn set_mutation_failure(&mut self, failed: bool) -> bool {
        if self.mutation_failure == failed {
            return false;
        }
        self.mutation_failure = failed;
        self.geometry_dirty = true;
        true
    }

    pub(super) fn set_edit_available(&mut self, available: bool) -> bool {
        if self.edit_available == available {
            return false;
        }
        self.edit_available = available;
        self.geometry_dirty = true;
        true
    }

    pub(super) fn set_recreate_available(&mut self, available: bool) -> bool {
        if self.recreate_available == available {
            return false;
        }
        self.recreate_available = available;
        self.geometry_dirty = true;
        true
    }

    pub(super) fn prepare_action_widths(&mut self, engine: &mut TextEngine, lang: Lang) -> bool {
        let mut changed = self.presentation.measure_actions(engine, lang);
        let text = banner_text(lang, self.health.as_ref(), self.mutation_failure);
        // The page's own width, with no popover to stand in for it.
        changed |= self
            .presentation
            .measure_banner(engine, lang, text, self.viewport.0.max(1.0));
        self.geometry_dirty |= changed;
        changed
    }

    /// Refresh wall-clock presentation and arm the snooze chooser's minute
    /// boundary, the shell's clock contract carried over whole.
    pub(super) fn maintain_clock(&mut self, now: Instant, now_ms: i64) -> bool {
        let refresh_chooser = self.management.clock_refresh_due(now);
        let mut changed = self.set_now_ms(now_ms);
        if refresh_chooser {
            self.presentation
                .rebuild_snooze(self.management.snooze.as_ref(), self.management.now_ms);
            changed = true;
        }
        self.management.arm_clock_deadline(now, now_ms);
        changed
    }

    fn set_now_ms(&mut self, now_ms: i64) -> bool {
        let changed = self.management.set_now_ms(now_ms);
        if changed {
            self.presentation.rebuild_rows(&self.global);
            self.presentation
                .rebuild_snooze(self.management.snooze.as_ref(), self.management.now_ms);
        }
        changed
    }

    /// Rebuild the component's geometry snapshot from the whole content rect.
    /// All collection growth happens here, during maintenance — never while a
    /// frame is drawn or an event is routed, the shell's rule carried over.
    pub(super) fn maintain_geometry(&mut self) -> bool {
        if !self.geometry_dirty
            && !self.management.geometry_dirty
            && self.geometry_viewport == Some(self.viewport)
        {
            return false;
        }
        let rect = (self.viewport.0 > 0.0 && self.viewport.1 > 0.0)
            .then(|| Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1));
        self.management.rebuild_geometry(
            rect,
            Chrome::Page,
            &self.global,
            &self.presentation,
            self.health.as_ref(),
            self.mutation_failure,
            self.edit_available,
            self.recreate_available,
        );
        self.geometry_viewport = Some(self.viewport);
        self.geometry_dirty = false;
        true
    }

    /// An intent the component just reported, remembered for the drain.
    fn report(&mut self, intent: Option<Intent>) {
        if intent.is_some() {
            self.pending = intent;
        }
    }

    /// The content rect the component is filling, for container tests that
    /// drive the production layout path.
    #[cfg(test)]
    pub(super) fn content_rect_for_test(&self) -> Option<Rect> {
        self.management.content_rect()
    }

    /// What the surface answers at a point, for the same tests.
    #[cfg(test)]
    pub(super) fn hit_for_test(&self, x: f32, y: f32) -> Option<super::reminder_management::Hit> {
        self.management.hit(x, y)
    }
}

impl View for RemindersView {
    /// The component's keys, and only those: arrows and Tab walk its focus,
    /// Enter and Space activate, PageUp/PageDown scroll, Esc steps one level
    /// back *inside* the surface — an open chooser, an armed confirmation,
    /// the focus — and never closes the tab. Everything else falls through to
    /// the document's own bindings.
    fn key(
        &mut self,
        _ws: &mut WorkspaceDoc,
        key: &Key,
        _text: Option<&str>,
        _mods: ModifiersState,
    ) -> bool {
        if !matches!(
            key,
            Key::Named(
                NamedKey::Escape
                    | NamedKey::PageUp
                    | NamedKey::PageDown
                    | NamedKey::ArrowLeft
                    | NamedKey::ArrowUp
                    | NamedKey::ArrowRight
                    | NamedKey::ArrowDown
                    | NamedKey::Tab
                    | NamedKey::Enter
                    | NamedKey::Space
            )
        ) {
            return false;
        }
        let (intent, _request) = self.management.key(
            key,
            Instant::now(),
            &self.global,
            &self.presentation,
            self.health.as_ref(),
        );
        self.report(intent);
        // A page has nowhere to go back to and nothing to close: a `Request`
        // is the popover host's vocabulary, and `Chrome::Page` never raises
        // one the component could mean here.
        true
    }

    fn mouse_down(&mut self, _ws: &mut WorkspaceDoc, x: f32, y: f32, _ctrl: bool) -> bool {
        let hit = self.management.hit(x, y);
        let (intent, _request) =
            self.management
                .mouse_down(x, y, Instant::now(), &self.global, self.health.as_ref());
        self.report(intent);
        hit.is_some()
    }

    fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        let hit = self.management.hit(x, y);
        self.hover_press = self.management.cursor_press(x, y);
        self.management.set_hover(hit)
    }

    fn mouse_up(&mut self, _ws: &mut WorkspaceDoc) -> bool {
        false
    }

    fn mouse_double_click(&mut self, _ws: &WorkspaceDoc, _x: f32, _y: f32) -> bool {
        // A single press already opens the row; the second click has no
        // stronger gesture here.
        false
    }

    fn scroll_by(&mut self, delta: MouseScrollDelta) {
        // The popover's wheel conversion, so the page scrolls the way the
        // compact surface does.
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => -y * 32.0,
            MouseScrollDelta::PixelDelta(p) => -p.y as f32,
        };
        self.management.scroll_by(dy, &self.presentation);
    }

    fn set_viewport(&mut self, w: f32, h: f32) {
        if self.viewport == (w, h) {
            return;
        }
        self.viewport = (w, h);
        self.geometry_dirty = true;
    }

    /// Every hit is [`PRESS`], the plain arrow — the same rule the Tasks page
    /// cites, and the popover's own answer through `cursor_press`.
    ///
    /// [`PRESS`]: koru_ui::cursor::PRESS
    fn cursor_icon(&self) -> CursorIcon {
        if self.hover_press {
            koru_ui::cursor::PRESS
        } else {
            koru_ui::cursor::IDLE
        }
    }

    /// Nothing to adopt: the page's rows are the container's fed presentation
    /// data, not a document this view reads.
    fn adopt(&mut self, _ws: &WorkspaceDoc) {}

    fn reseat(&mut self, _ws: &WorkspaceDoc) {
        self.geometry_dirty = true;
    }

    /// The theme is process-global and baked into the measured widths, so a
    /// flip remeasures once at the next maintain pass.
    fn invalidate(&mut self) {
        self.presentation.invalidate_metrics();
        self.geometry_dirty = true;
    }

    /// The snooze chooser's minute tick — only ever asked of the tab in
    /// front, which is what keeps a hidden page costing no wake.
    fn maintenance_deadline(&self) -> Option<Instant> {
        self.management.clock_deadline()
    }

    fn maintain(&mut self, _ws: &mut WorkspaceDoc, _now: Instant) -> bool {
        self.maintain_geometry()
    }

    fn draw(
        &mut self,
        scene: &mut Scene,
        _workspace: &koru_workspace::Workspace,
        _images: &koru_draw::ImageCache,
        root: Affine,
        _caret_on: bool,
        _pointer: (f32, f32),
    ) {
        // `bg`, not `canvas`: a sheet you read a list on, the same surface the
        // Tasks page and the Trash tab draw.
        scene::fill(
            scene,
            root,
            theme::p().bg,
            &scene::rect(0.0, 0.0, self.viewport.0, self.viewport.1),
        );
        let lang = crate::format::lang();
        self.management.draw(
            scene,
            root,
            &mut self.engine,
            lang,
            &self.global,
            &self.presentation,
            self.health.as_ref(),
            self.mutation_failure,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_core::WorkspaceId;
    use koru_editor::reminders::GlobalItemState;

    use super::super::reminder_management::Hit;
    use super::super::reminder_shell::tests::item;

    fn doc() -> WorkspaceDoc {
        WorkspaceDoc::create(WorkspaceId::new(), "Test").expect("in-memory workspace")
    }

    fn page(rows: Vec<koru_editor::reminders::GlobalItem>) -> RemindersView {
        let mut view = RemindersView::new();
        view.set_global(&GlobalManagement::new(rows), Lang::En.into());
        view.set_viewport(800.0, 900.0);
        assert!(view.maintain_geometry(), "the first feed builds geometry");
        view
    }

    /// The page host hands the component the whole content rect: no 400px
    /// clamp, no bell to anchor to, and no back arrow or close × where the
    /// popover's chrome would sit.
    #[test]
    fn the_page_fills_the_whole_content_rect_with_no_popover_chrome() {
        let view = page(vec![item("one", GlobalItemState::ComingUp, 1)]);
        assert_eq!(
            view.management.content_rect(),
            Some(Rect::new(0.0, 0.0, 800.0, 900.0)),
            "taller than the popover's clamp, at the content rect's own origin"
        );
        // (24, 24) is the back arrow's seat in the popover; (w - 24, 24) the
        // close ×'s. Here both are plain content.
        assert_ne!(view.management.hit(24.0, 24.0), Some(Hit::Back));
        assert_ne!(view.management.hit(800.0 - 24.0, 24.0), Some(Hit::Close));
        // …and a row is hit-testable, so pointer and keys land on the page.
        let first_row = 52.0 + koru_ui::Chip::H + 12.0 + 10.0;
        assert_eq!(view.management.hit(400.0, first_row), Some(Hit::Row(0)));
    }

    /// **Esc never navigates.** It walks one level back inside the surface —
    /// the focus first — and the tab stays exactly where it was: nothing is
    /// reported, because there is nothing for the container to do.
    #[test]
    fn esc_steps_back_inside_the_page_and_never_navigates() {
        let mut view = page(vec![item("one", GlobalItemState::ComingUp, 1)]);
        let mut doc = doc();
        let down = Key::Named(NamedKey::ArrowDown);
        assert!(View::key(
            &mut view,
            &mut doc,
            &down,
            None,
            ModifiersState::empty()
        ));
        assert!(
            view.management.focus.is_some(),
            "the arrows walked the focus"
        );

        let esc = Key::Named(NamedKey::Escape);
        assert!(View::key(
            &mut view,
            &mut doc,
            &esc,
            None,
            ModifiersState::empty()
        ));
        assert!(view.management.focus.is_none(), "Esc stepped back inside");
        assert!(view.take_intent().is_none(), "and reported nothing");
        assert_eq!(
            view.management.content_rect(),
            Some(Rect::new(0.0, 0.0, 800.0, 900.0)),
            "the page is still the page"
        );
    }

    /// A row press reports the row's Open intent in the one vocabulary both
    /// hosts speak, for the container's host-agnostic drain.
    #[test]
    fn a_row_press_reports_the_open_intent() {
        let mut view = page(vec![item("one", GlobalItemState::ComingUp, 1)]);
        let mut doc = doc();
        let first_row = 52.0 + koru_ui::Chip::H + 12.0 + 10.0;
        assert!(View::mouse_down(
            &mut view, &mut doc, 400.0, first_row, false
        ));
        assert!(matches!(
            view.take_intent(),
            Some(Intent::Open(row)) if row.reminder_id == "one"
        ));
        assert!(view.take_intent().is_none(), "one press, one intent");
    }
}
