// SPDX-License-Identifier: MPL-2.0

//! What the shell hosts.
//!
//! A surface owns its own state outright — including, for a host that needs
//! one, a whole domain object the shell may not name. Every method takes only
//! winit, vello and `koru-draw` values plus `&mut self`, which is what makes
//! `MainView` (which needs a live `&mut WorkspaceDoc` on nearly every path)
//! hostable at all: the workspace lives *inside* the surface, not in an
//! argument the shell would have to carry.
//!
//! ## Coordinates: `set_bounds` says where you are, everything else is yours
//!
//! [`set_bounds`](Surface::set_bounds) hands a rect in **window** space, because
//! both real hosts want to know where they have been put. Every other method
//! speaks the surface's **own** coordinates: pointer events arrive translated,
//! `draw` receives a `root` already carrying the origin, and the `bounds` it is
//! handed is `(0, 0, w, h)`. SP-0 pre-multiplied the root *and* passed a
//! window-space rect beside it, which offsets a drawing surface twice; that is
//! the one thing here that is a correction rather than a copy.

use std::path::Path;
use std::time::Instant;

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_window::{ImeEvent, RichKeyEvent};
use vello::Scene;
use vello::kurbo::Affine;
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;
use winit::window::CursorIcon;

use crate::command::Commands;

/// Everything a surface is lent for one dispatch.
///
/// `engine` and `commands` are separate public fields rather than accessor
/// methods so a surface can borrow the engine and record a command in one
/// expression; two methods on one `&mut self` could not.
pub struct SurfaceCx<'a> {
    /// The runner's one `TextEngine` — **when this dispatch has one**.
    ///
    /// SP-0 promised an engine on every method. SP-1's `MultiWindowApp` lends
    /// one on eleven of its methods and on none of the other sixteen:
    /// `pointer_moved`, `mouse_up`, `scroll_by`, `resized`, `focus_changed`,
    /// `dropped_file`, `maintain` and `close_requested` all arrive without,
    /// because the shared dispatch core has none to lend there. The only way to
    /// keep SP-0's promise would be a second `TextEngine`, which is exactly what
    /// `run_windows` builds its app through a closure to prevent — so SP-1's
    /// shape wins and the promise becomes an `Option`.
    ///
    /// A surface that must measure on a path with no engine marks itself stale
    /// and measures at the next `draw`. Both real adapters already work that
    /// way: `SessionSurface::invalidate` says so in as many words.
    pub engine: Option<&'a mut TextEngine>,
    /// Device pixels per logical pixel. Rides here rather than being pushed
    /// through a `scale_changed` hook because the only consumer that cares —
    /// the terminal's cell metrics — recomputes from it every draw anyway.
    pub scale: f32,
    /// The one way back out. Already stamped with this window.
    pub commands: &'a mut Commands,
}

/// One pointer event, in the surface's own coordinates.
///
/// **One enum, not seven callbacks.** `MultiWindowApp` splits this into
/// `pointer_moved`, `click`, `double_click`, `right_click`, `context_menu`,
/// `drag`, `mouse_up` and `scroll_by`, gated by `wants_drag` and
/// `wants_rich_mouse`; `View` splits it a different way again. Both splits exist
/// because each was grown one consumer at a time, and the two `wants_*` flags
/// exist only to keep the older shape working. The shell opts into **both**
/// flags unconditionally and folds every callback back into this one lossless
/// enum, so a surface that only wants clicks matches on `Pressed` and a surface
/// that wants a middle-button release finally receives one.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum PointerEvent {
    Moved {
        x: f32,
        y: f32,
        mods: ModifiersState,
    },
    Pressed {
        x: f32,
        y: f32,
        button: MouseButton,
        mods: ModifiersState,
        /// 1, 2 or 3. winit reports presses, not clicks, so the shared loop
        /// already does this timing; a surface must never do it again.
        click_count: u8,
    },
    /// The pointer moved with a button held. Delivered unconditionally — the
    /// `wants_drag` opt-in is gone, because a surface that does not drag
    /// answers `false` and costs nothing.
    Dragged {
        x: f32,
        y: f32,
        button: MouseButton,
        mods: ModifiersState,
    },
    Released {
        x: f32,
        y: f32,
        button: MouseButton,
        mods: ModifiersState,
    },
    /// Already converted to logical pixels by the loop; positive is up.
    Wheel {
        x: f32,
        y: f32,
        lines: f32,
        mods: ModifiersState,
    },
    /// The pointer left the window. **Not a winit event the runner forwards** —
    /// `CursorLeft` is dropped in `dispatch`, which SP-0's event matrix recorded
    /// as a gap — so this is synthesised from `MultiWindowApp::pointer_left`,
    /// the one hook that does survive. A surface holding a hover highlight
    /// clears it here; one that does not answers `false`.
    Left,
}

impl PointerEvent {
    /// The point this event happened at, or `None` for one that has no point.
    pub fn position(self) -> Option<(f32, f32)> {
        match self {
            Self::Moved { x, y, .. }
            | Self::Pressed { x, y, .. }
            | Self::Dragged { x, y, .. }
            | Self::Released { x, y, .. }
            | Self::Wheel { x, y, .. } => Some((x, y)),
            Self::Left => None,
        }
    }

    /// The same event, moved by `(dx, dy)`. How the shell turns a window point
    /// into a surface-local one, in one place.
    pub(crate) fn translated(self, dx: f32, dy: f32) -> Self {
        match self {
            Self::Moved { x, y, mods } => Self::Moved {
                x: x + dx,
                y: y + dy,
                mods,
            },
            Self::Pressed {
                x,
                y,
                button,
                mods,
                click_count,
            } => Self::Pressed {
                x: x + dx,
                y: y + dy,
                button,
                mods,
                click_count,
            },
            Self::Dragged { x, y, button, mods } => Self::Dragged {
                x: x + dx,
                y: y + dy,
                button,
                mods,
            },
            Self::Released { x, y, button, mods } => Self::Released {
                x: x + dx,
                y: y + dy,
                button,
                mods,
            },
            Self::Wheel { x, y, lines, mods } => Self::Wheel {
                x: x + dx,
                y: y + dy,
                lines,
                mods,
            },
            Self::Left => Self::Left,
        }
    }
}

/// The answer to "the window was asked to close".
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CloseResponse {
    Close,
    /// The surface will close itself later, by recording
    /// `ShellAction::Window(WindowRequest::Close)` once it is ready. A
    /// popped-out document window's close-choice dialog is why this exists.
    Veto,
}

/// A thing the shell draws inside its chrome.
///
/// Every input method answers **whether anything visible changed**, the rule
/// both existing loops already keep (ADR-26).
pub trait Surface {
    // ----------------------------------------------------------------- layout

    /// This surface's extent changed, in logical pixels, in **window** space.
    ///
    /// **A rect, not a `(w, h)` pair.** A surface hosted inside chrome does not
    /// start at the window's origin, and both real hosts already need to know
    /// where they are: `MainView` translates every click by its content origin,
    /// and the terminal's `GridGeometry` is built from a bounds rect. Handing
    /// only a size would make each of them reconstruct the offset from
    /// something the shell already knows.
    fn set_bounds(&mut self, cx: &mut SurfaceCx<'_>, bounds: Rect) -> bool;

    /// Draw into `scene` under `root`, within `bounds`.
    ///
    /// `root` rather than a scale factor, for `View::draw`'s reason: a surface
    /// no longer owns the window, so the transform carries both wherever it has
    /// been put and, above it, the DPI scale the runner applies. `bounds` is
    /// this surface's own extent — `(0, 0, w, h)` — because `root` has already
    /// carried the origin. The scene arrives already reset and may already hold
    /// chrome; painting starts with your own background over your own extent,
    /// never by clearing.
    fn draw(&mut self, cx: &mut SurfaceCx<'_>, scene: &mut Scene, root: Affine, bounds: Rect);

    /// The theme, the font, the density or the zoom moved. Every cached layout
    /// this surface holds is stale.
    ///
    /// Reaches **every** surface, not only the active one, because those are
    /// process globals: a flip in one window invalidates cached drawing
    /// everywhere.
    fn invalidate(&mut self) {}

    // ------------------------------------------------------------------ input

    /// One key, in the **rich** form.
    ///
    /// Not `(&Key, Option<&str>, ModifiersState)`. That shape is enough for
    /// Koru and provably not enough for the Terminal, which needs the physical
    /// key, the press/release state and the repeat flag to build a `KeyStroke`
    /// — and it is the shape `MultiWindowApp::rich_key` already delivers. A
    /// surface that only edits text reads three of the seven fields and ignores
    /// the rest.
    ///
    /// **This is reached only after the host has declined the key**; see
    /// [`ShellHost::window_key`](crate::host::ShellHost::window_key). The
    /// chrome installs no accelerator table of its own, so the Terminal keeps
    /// swallowing Ctrl+C.
    fn key(&mut self, cx: &mut SurfaceCx<'_>, event: RichKeyEvent<'_>) -> bool {
        let _ = (cx, event);
        false
    }

    /// Whether this surface wants the OS to produce composition events. Asked
    /// when it becomes the active surface, so `set_ime_allowed` follows the tab
    /// rather than the window.
    fn wants_ime(&self) -> bool {
        false
    }

    fn ime(&mut self, cx: &mut SurfaceCx<'_>, event: ImeEvent<'_>) -> bool {
        let _ = (cx, event);
        false
    }

    fn pointer(&mut self, cx: &mut SurfaceCx<'_>, event: PointerEvent) -> bool {
        let _ = (cx, event);
        false
    }

    /// The pointer shape at a logical point inside this surface.
    ///
    /// Only consulted where chrome claims nothing: the resize border and the
    /// caption are decided above, so a surface never has to know they exist.
    fn cursor_icon(&self, x: f32, y: f32) -> CursorIcon {
        let _ = (x, y);
        CursorIcon::Default
    }

    /// One dropped path. winit emits a separate event per file, and both hosts
    /// consume them — Koru imports, the Terminal attaches — so this is on the
    /// boundary against two live consumers rather than one.
    fn dropped_file(&mut self, cx: &mut SurfaceCx<'_>, path: &Path) -> bool {
        let _ = (cx, path);
        false
    }

    // ------------------------------------------------------------- activation

    /// This surface became the active one in its window.
    fn activated(&mut self, cx: &mut SurfaceCx<'_>) -> bool {
        let _ = cx;
        false
    }

    /// Another surface is about to become active in this window.
    ///
    /// **Called before the destination is activated, and that ordering is the
    /// contract**, because ADR-263 requires an unfinished IME preedit to be
    /// cancelled on a tab switch so composition reaches neither terminal.
    fn deactivated(&mut self, cx: &mut SurfaceCx<'_>) -> bool {
        let _ = cx;
        false
    }

    /// The **window** gained or lost the keyboard focus. Routed to the active
    /// surface because it changes what that surface draws — a caret and a
    /// terminal cursor both stop blinking.
    fn window_focus_changed(&mut self, cx: &mut SurfaceCx<'_>, focused: bool) -> bool {
        let _ = (cx, focused);
        false
    }

    /// The window was asked to close while this surface was active.
    fn close_requested(&mut self, cx: &mut SurfaceCx<'_>) -> CloseResponse {
        let _ = cx;
        CloseResponse::Close
    }

    // -------------------------------------------------------------- the clock

    /// The next moment this surface has work no input will trigger.
    fn maintenance_deadline(&self) -> Option<Instant> {
        None
    }

    /// Do whatever is due; answer whether anything visible changed.
    ///
    /// **Every surface's `maintain` runs, active or not.** That is where the
    /// inactive-tab rule lives: an inactive session tab drains its lightweight
    /// status subscription here (R2) and an inactive Koru tab drains its Loro
    /// subscription, or both queue without bound — but neither lays out, and
    /// only the active one is ever drawn.
    fn maintain(&mut self, cx: &mut SurfaceCx<'_>, now: Instant) -> bool {
        let _ = (cx, now);
        false
    }

    /// Motion in flight. **Must return to `false`.** Asked of the active
    /// surface only: an animation nobody can see is not a reason for a frame.
    fn animating(&self) -> bool {
        false
    }

    /// The frame the preceding [`Surface::draw`] built reached the screen.
    /// Presentation acknowledgements and user-visible latency samples; never
    /// drawing, never blocking work.
    fn presented(&mut self) {}
}

/// A surface the host keeps a handle on while the shell owns the tab.
///
/// **The one shape the boundary could not do without.** A host whose tabs are
/// surfaces still receives events for them: a render update arrives on the
/// window thread addressed to a *session*, and the surface showing that session
/// is inside a [`ShellTab`](crate::ShellTab) the host cannot reach — the shell
/// holds a `Box<dyn Surface>` and downcasting it is exactly the `Any` the
/// minimal contract forbids. So the host puts the surface behind one of these,
/// hands a clone to the shell as the tab, and keeps a clone of its own.
///
/// `Rc<RefCell<_>>` rather than a lock: every method here is called from the
/// window thread, and a lock would say otherwise. **The borrow is never held
/// across a call back into the shell** — the shell drains commands *after* a
/// dispatch, never during one, which is what makes that true by construction
/// rather than by care.
pub struct SharedSurface<T: ?Sized>(std::rc::Rc<std::cell::RefCell<T>>);

impl<T> SharedSurface<T> {
    pub fn new(surface: T) -> Self {
        Self(std::rc::Rc::new(std::cell::RefCell::new(surface)))
    }
}

impl<T: ?Sized> SharedSurface<T> {
    pub fn borrow(&self) -> std::cell::Ref<'_, T> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, T> {
        self.0.borrow_mut()
    }

    /// Whether two handles name the same surface — the identity guard ADR-263
    /// wants beside the epoch, so a late frame for a deactivated surface is
    /// dropped rather than presented into the active one.
    pub fn is(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<T: ?Sized> Clone for SharedSurface<T> {
    fn clone(&self) -> Self {
        Self(std::rc::Rc::clone(&self.0))
    }
}

impl<T: Surface + ?Sized> Surface for SharedSurface<T> {
    fn set_bounds(&mut self, cx: &mut SurfaceCx<'_>, bounds: Rect) -> bool {
        self.0.borrow_mut().set_bounds(cx, bounds)
    }

    fn draw(&mut self, cx: &mut SurfaceCx<'_>, scene: &mut Scene, root: Affine, bounds: Rect) {
        self.0.borrow_mut().draw(cx, scene, root, bounds);
    }

    fn invalidate(&mut self) {
        self.0.borrow_mut().invalidate();
    }

    fn key(&mut self, cx: &mut SurfaceCx<'_>, event: RichKeyEvent<'_>) -> bool {
        self.0.borrow_mut().key(cx, event)
    }

    fn wants_ime(&self) -> bool {
        self.0.borrow().wants_ime()
    }

    fn ime(&mut self, cx: &mut SurfaceCx<'_>, event: ImeEvent<'_>) -> bool {
        self.0.borrow_mut().ime(cx, event)
    }

    fn pointer(&mut self, cx: &mut SurfaceCx<'_>, event: PointerEvent) -> bool {
        self.0.borrow_mut().pointer(cx, event)
    }

    fn cursor_icon(&self, x: f32, y: f32) -> CursorIcon {
        self.0.borrow().cursor_icon(x, y)
    }

    fn dropped_file(&mut self, cx: &mut SurfaceCx<'_>, path: &Path) -> bool {
        self.0.borrow_mut().dropped_file(cx, path)
    }

    fn activated(&mut self, cx: &mut SurfaceCx<'_>) -> bool {
        self.0.borrow_mut().activated(cx)
    }

    fn deactivated(&mut self, cx: &mut SurfaceCx<'_>) -> bool {
        self.0.borrow_mut().deactivated(cx)
    }

    fn window_focus_changed(&mut self, cx: &mut SurfaceCx<'_>, focused: bool) -> bool {
        self.0.borrow_mut().window_focus_changed(cx, focused)
    }

    fn close_requested(&mut self, cx: &mut SurfaceCx<'_>) -> CloseResponse {
        self.0.borrow_mut().close_requested(cx)
    }

    fn maintenance_deadline(&self) -> Option<Instant> {
        self.0.borrow().maintenance_deadline()
    }

    fn maintain(&mut self, cx: &mut SurfaceCx<'_>, now: Instant) -> bool {
        self.0.borrow_mut().maintain(cx, now)
    }

    fn animating(&self) -> bool {
        self.0.borrow().animating()
    }

    fn presented(&mut self) {
        self.0.borrow_mut().presented();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host keeps its handle, the shell holds the tab, and the two are one
    /// surface — which is what lets a wake reach a tab the shell owns.
    #[test]
    fn a_shared_surface_is_one_surface_behind_two_handles() {
        struct Counter(u32);
        impl Surface for Counter {
            fn set_bounds(&mut self, _cx: &mut SurfaceCx<'_>, _bounds: Rect) -> bool {
                self.0 += 1;
                true
            }
            fn draw(
                &mut self,
                _cx: &mut SurfaceCx<'_>,
                _scene: &mut Scene,
                _root: Affine,
                _bounds: Rect,
            ) {
            }
        }
        let mine = SharedSurface::new(Counter(0));
        let mut theirs: Box<dyn Surface> = Box::new(mine.clone());
        let mut commands = crate::command::Commands::new(koru_window::WindowKey(1));
        let mut cx = SurfaceCx {
            engine: None,
            scale: 1.0,
            commands: &mut commands,
        };
        theirs.set_bounds(&mut cx, Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(mine.borrow().0, 1, "the shell's call moved the host's copy");
        assert!(mine.is(&mine.clone()));
    }

    #[test]
    fn a_translated_event_keeps_everything_but_its_point() {
        let press = PointerEvent::Pressed {
            x: 100.0,
            y: 50.0,
            button: MouseButton::Left,
            mods: ModifiersState::empty(),
            click_count: 3,
        };
        let local = press.translated(-10.0, -20.0);
        assert_eq!(local.position(), Some((90.0, 30.0)));
        assert!(matches!(
            local,
            PointerEvent::Pressed {
                click_count: 3,
                button: MouseButton::Left,
                ..
            }
        ));
    }

    /// A leave has no point, so translating it is a no-op — and a caller that
    /// asks for one is told there is none rather than handed a zero.
    #[test]
    fn a_leave_carries_no_point_to_translate() {
        assert_eq!(PointerEvent::Left.position(), None);
        assert_eq!(
            PointerEvent::Left.translated(-10.0, -10.0),
            PointerEvent::Left
        );
    }
}
