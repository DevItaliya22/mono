// SPDX-License-Identifier: MPL-2.0

//! The floating icon (D17): a 48-pixel always-on-top window showing the
//! headset with an unread badge. Click opens the chat; press-and-move drags
//! it; right-click offers *Open*, *Hide until support replies* and *End chat*
//! through a native popup.
//!
//! It draws nothing but the icon and asks the host for everything else: the
//! window is placed and kept on top by `open_pane`'s floating attributes,
//! shown and hidden through `BackgroundIntent::SetVisible` from the tray
//! table the session evaluates, and the popup's choice arrives back through
//! the host's one menu transport.

use std::time::Instant;

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{icons, scene, theme};
use koru_i18n::{Key, Lang, t};
use koru_workspace_host::menus::Popup;
use koru_workspace_host::native_surface::{BackgroundIntent, SurfaceHost};
use vello::Scene;
use vello::kurbo::{Affine, Circle};

use super::session::{Mode, Session};

pub const SIZE: f32 = 48.0;
const ICON: f32 = 22.0;
const RADIUS: f32 = 12.0;
const DRAG_THRESHOLD: f32 = 4.0;

const TAG_OPEN: u32 = 1;
const TAG_QUIET: u32 = 2;
const TAG_END: u32 = 3;

pub struct FabView {
    session: Session,
    lang: Lang,
    host: Option<SurfaceHost>,
    popup: Option<Popup>,
    pressed_at: Option<(f32, f32)>,
    dragging: bool,
    hover: bool,
    intents: Vec<BackgroundIntent>,
    /// Whether this pane asked the host to start a window drag on this press.
    pub wants_drag: bool,
}

impl FabView {
    pub fn new(session: Session, lang: Lang) -> Self {
        Self {
            session,
            lang,
            host: None,
            popup: None,
            pressed_at: None,
            dragging: false,
            hover: false,
            intents: Vec::new(),
            wants_drag: false,
        }
    }

    pub fn attach_host(&mut self, host: SurfaceHost) {
        self.host = Some(host);
    }

    pub fn take_intent(&mut self) -> Option<BackgroundIntent> {
        if self.intents.is_empty() {
            None
        } else {
            Some(self.intents.remove(0))
        }
    }

    // ---------------------------------------------------------------- draw

    pub fn draw(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        size: (f32, f32),
    ) {
        let p = theme::p();
        let (w, h) = size;
        scene::floating_panel(scene, root, 0.0, 0.0, w, h, &scene::Panel::popover(RADIUS));
        let icon = engine.icon_layout(icons::HEADSET, ICON);
        let color = if self.hover { p.accent } else { p.text };
        scene::draw_layout(
            scene,
            root,
            &icon,
            (w - icon.width()) * 0.5,
            (h - icon.height()) * 0.5,
            Some(color),
        );
        let unread = self.session.borrow().badge();
        if unread > 0 {
            let r = 7.0;
            let (cx, cy) = (w - r - 4.0, r + 4.0);
            scene::fill(
                scene,
                root,
                p.close_hover,
                &Circle::new((cx as f64, cy as f64), r as f64),
            );
            let label = if unread > 9 {
                "9+".to_string()
            } else {
                unread.to_string()
            };
            let text = engine.simple_layout(&label, theme::role::hint());
            scene::draw_layout(
                scene,
                root,
                &text,
                cx - text.width() * 0.5,
                cy - text.height() * 0.5,
                Some(p.on_accent),
            );
        }
    }

    // --------------------------------------------------------------- input

    pub fn mouse_down(&mut self, x: f32, y: f32) -> bool {
        self.pressed_at = Some((x, y));
        self.dragging = false;
        true
    }

    /// A move past the threshold while pressed becomes a window drag; the
    /// container turns `wants_drag` into the host's drag request.
    pub fn mouse_move(&mut self, x: f32, y: f32, size: (f32, f32)) -> bool {
        let hover = Rect::new(0.0, 0.0, size.0, size.1).contains(x, y);
        let changed = hover != self.hover;
        self.hover = hover;
        if let Some((px, py)) = self.pressed_at
            && !self.dragging
            && ((x - px).abs() > DRAG_THRESHOLD || (y - py).abs() > DRAG_THRESHOLD)
        {
            self.dragging = true;
            self.wants_drag = true;
        }
        changed
    }

    /// Release without a drag: open the chat and hide the icon.
    pub fn mouse_up(&mut self) -> bool {
        let was_click = self.pressed_at.take().is_some() && !self.dragging;
        self.dragging = false;
        if was_click {
            super::debug::log("fab: click");
            self.intents.push(BackgroundIntent::OpenSupport);
        }
        was_click
    }

    /// Right-click: *Open*, *Hide until support replies*, *End chat* — the
    /// dialog's answers, worded for an icon rather than a window, and short
    /// enough that the native popup stays a popup.
    pub fn context_menu(&mut self) -> bool {
        let Some(host) = self.host.as_ref() else {
            return false;
        };
        let popup = Popup::new(&[
            (t(self.lang, Key::SupportOpen), TAG_OPEN),
            (t(self.lang, Key::SupportHideUntilReply), TAG_QUIET),
            (t(self.lang, Key::SupportEndChat), TAG_END),
        ]);
        popup.show(host.window().as_ref());
        self.popup = Some(popup);
        true
    }

    /// A row the person chose, from the host's menu drain.
    pub fn popup_chosen(&mut self, id: &str) -> bool {
        let Some(tag) = self.popup.as_ref().and_then(|p| p.tag_of_str(id)) else {
            return false;
        };
        match tag {
            TAG_OPEN => {
                super::debug::log("fab: popup open");
                self.intents.push(BackgroundIntent::OpenSupport);
            }
            TAG_QUIET => {
                super::debug::log("fab: popup close and notify");
                self.session.borrow_mut().choose(Mode::Quiet);
            }
            TAG_END => {
                super::debug::log("fab: popup end chat");
                self.session.borrow_mut().end();
            }
            _ => return false,
        }
        true
    }

    // ----------------------------------------------------------- lifecycle

    /// The tray table, applied to this window: shown when the session says
    /// the icon should be on screen, hidden otherwise; closed when the chat
    /// has ended.
    pub fn background_maintain(&mut self, _now: Instant, app_hidden: bool) -> bool {
        let session = self.session.borrow();
        if session.mode == Mode::Ended {
            drop(session);
            self.intents.push(BackgroundIntent::Close);
            return false;
        }
        let visible = session.fab_visible(app_hidden);
        drop(session);
        self.intents.push(BackgroundIntent::SetVisible(visible));
        false
    }

    pub fn visible_after_restore(&self) -> bool {
        self.session.borrow().mode == Mode::Fab
    }
}
