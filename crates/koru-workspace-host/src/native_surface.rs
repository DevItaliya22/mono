// SPDX-License-Identifier: MPL-2.0

//! One native child surface per pane: placed by the host, owned by the view.
//!
//! The mechanism, in platform vocabulary: a view may own a native child of
//! its window (a WebView, today; anything with a native handle, later), and
//! the host — which owns the window, its scale factor and Koru's display
//! zoom — is the only party that can say where that child goes in physical
//! pixels. So the view answers [`View::native_surface`] with the object and
//! a [`Placement`] in **window-relative, unzoomed logical pixels**, and the
//! host converts once and calls [`NativeSurface::place`]. The host never
//! names the engine behind the trait; the app's adapter implements it.
//!
//! **Coordinate spaces, and the one conversion at each boundary.**
//!
//! | space | who produces it | unit |
//! | --- | --- | --- |
//! | view | the view's own content area | unzoomed logical |
//! | window, unzoomed | the view: content + its own chrome's origin — this is `Placement.rect` | unzoomed logical |
//! | window, OS-logical | the host: × Koru zoom | what winit calls logical; the resize band lives here |
//! | physical | the host: × OS scale | device pixels — what `place` receives |
//!
//! `physical = rect × zoom × scale`, in [`to_physical`], and the origin is
//! added exactly once, by the view. The tests pin a non-zero origin at zoom
//! 1.5 and scale 1.25 so the two factors cannot be swapped.
//!
//! **The resize band.** Where the window's resize border is the host's own
//! pointer band (`koru_window::resize::is_ours()`, Windows, not maximised),
//! the child must not cover it or it eats the press that starts a resize.
//! The band's widths are logical, **OS-DPI** pixels (`resize::BORDER`,
//! `resize::CORNER`), so the physical inset is `BORDER × scale` and Koru zoom
//! plays no part — the same rule `Pane::resize_edge_at` follows.
//!
//! **Reconcile, never per frame.** [`reconcile`] runs after a resize, a
//! scale change, a zoom change, and once per maintenance pass so a state
//! change the view made (a dialog opened, a new object after Retry) is
//! applied within one wake. The adapter keeps the last applied pair and
//! skips a no-op, so the host holds no cache and a freshly created object
//! always receives its first placement.

use std::rc::Rc;
use std::sync::Arc;

use koru_draw::geometry::Rect;
use koru_window::resize;
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::View;
use crate::app::Wake;

/// A rectangle in the window's physical pixels, origin top-left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Why a placement or hide call failed. `what` is the stage — `"place"` or
/// `"hide"` — so the view can say which; `reason` is the engine's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceError {
    pub what: &'static str,
    pub reason: String,
}

/// Implemented by the app's adapter over the engine's surface. The host can
/// place it, ask whether it holds keyboard focus, and hand focus back —
/// nothing else.
pub trait NativeSurface {
    /// Apply bounds and visibility. The implementation keeps the last
    /// applied pair and skips a no-op itself, so a freshly created object
    /// always receives its first placement and the host holds no cache.
    fn place(&self, bounds: PhysicalRect, visible: bool) -> Result<(), SurfaceError>;
    /// Whether keyboard focus is inside the surface — the platform's answer,
    /// never inferred from visibility.
    fn has_focus(&self) -> bool;
    fn focus_parent(&self);
}

/// What a view wants for its surface this instant, in window-relative,
/// unzoomed logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub rect: Rect,
    pub visible: bool,
}

/// The host's side of the handshake: the window a surface would be a child
/// of, and a way to wake the loop from an engine callback. Supplied once per
/// pane, before the view can create a surface; the view keeps it.
#[derive(Clone)]
pub struct SurfaceHost {
    window: Arc<Window>,
    proxy: EventLoopProxy<Wake>,
}

impl SurfaceHost {
    pub(crate) fn new(window: Arc<Window>, proxy: EventLoopProxy<Wake>) -> Self {
        Self { window, proxy }
    }

    /// The parent window. A surface created as its child must be dropped
    /// before it.
    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }

    /// A cheap, `Send` ping for the loop: the engine's callbacks call it after
    /// queueing an event, and the view drains on its next `maintain`.
    pub fn waker(&self) -> impl Fn() + Send + Sync + 'static {
        let proxy = self.proxy.clone();
        move || {
            let _ = proxy.send_event(Wake::Surface);
        }
    }
}

/// What the host knows about the window that the conversion needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowGeometry {
    /// Koru's display zoom (`App::zoom`), never the OS scale.
    pub zoom: f32,
    /// The OS scale factor (`Window::scale_factor`), never Koru's zoom.
    pub scale: f64,
    /// The window's inner size in physical pixels.
    pub inner: (u32, u32),
    /// Whether the window fills the screen — a maximised window has no band.
    pub maximized: bool,
    /// Whether the resize band is the host's own (`resize::is_ours()`); a
    /// parameter so the geometry stays tested on every platform.
    pub band_is_ours: bool,
}

/// `physical = rect × zoom × scale`. The origin is already in `rect`.
pub fn to_physical(rect: Rect, zoom: f32, scale: f64) -> PhysicalRect {
    let k = zoom as f64 * scale;
    let x = (rect.x as f64 * k).round() as i32;
    let y = (rect.y as f64 * k).round() as i32;
    let right = ((rect.x + rect.w) as f64 * k).round() as i32;
    let bottom = ((rect.y + rect.h) as f64 * k).round() as i32;
    PhysicalRect {
        x,
        y,
        width: (right - x).max(0) as u32,
        height: (bottom - y).max(0) as u32,
    }
}

/// Keep the child off the window's own resize band. Logical OS-DPI widths
/// scaled by the OS factor only; nothing on a maximised window or where the
/// band is not ours.
pub fn inset_for_band(bounds: PhysicalRect, geometry: WindowGeometry) -> PhysicalRect {
    if !geometry.band_is_ours || geometry.maximized {
        return bounds;
    }
    let band = (resize::BORDER as f64 * geometry.scale).round() as i32;
    let (w, h) = (geometry.inner.0 as i32, geometry.inner.1 as i32);
    let left = bounds.x;
    let top = bounds.y;
    let right = bounds.x + bounds.width as i32;
    let bottom = bounds.y + bounds.height as i32;
    // Only the edges the child actually reaches are inset; a child that ends
    // 40 px short of the right edge is not over the band there.
    let new_left = if left < band { band } else { left };
    let new_top = if top < band { band } else { top };
    let new_right = if right > w - band { w - band } else { right };
    let new_bottom = if bottom > h - band { h - band } else { bottom };
    PhysicalRect {
        x: new_left,
        y: new_top,
        width: (new_right - new_left).max(0) as u32,
        height: (new_bottom - new_top).max(0) as u32,
    }
}

/// Ask the view where its surface should be and put it there. Returns the
/// error, if any, for the caller to hand back through
/// [`View::native_surface_failed`] — after the borrow of the view ends.
pub fn reconcile<V: View + ?Sized>(view: &mut V, geometry: WindowGeometry) -> Option<SurfaceError> {
    let (surface, placement) = view.native_surface()?;
    let bounds = inset_for_band(
        to_physical(placement.rect, geometry.zoom, geometry.scale),
        geometry,
    );
    surface.place(bounds, placement.visible).err()
}

/// The shared handle a view hands the host: `Rc` because the object lives
/// on the UI thread and is owned by whoever outlives the window's views (a
/// session held by the app), and the host only needs it for one call.
pub type SharedSurface = Rc<dyn NativeSurface>;

/// What an auxiliary pane wants done to itself, decided off the input path.
///
/// The host's two request channels do not reach a hidden window: window
/// requests are drained inside a press, pane requests after the tray
/// branch's early return. A floating icon that appears because a reply
/// arrived, or a support window that closes because a popup said so, is
/// neither. The host drains these in **both** loop paths and applies them
/// after the iteration, bounded like the agent's own drain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundIntent {
    /// Remove this pane now — the view chose it, so no dialog and no veto.
    Close,
    /// Show or hide this pane's window, without activating it when showing.
    SetVisible(bool),
    /// Open (or reveal) the support window.
    OpenSupport,
    /// Open (or reveal) the floating support icon's window, without focus.
    ShowFab,
    /// Support replied: offer the notice to the focused window.
    NotifyReply,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn zoom_and_scale_multiply_and_the_origin_is_not_added_twice() {
        // A rect the view already made window-relative: origin (20, 40).
        let rect = Rect::new(20.0, 40.0, 400.0, 600.0);
        let p = to_physical(rect, 1.5, 1.25);
        // 20 × 1.875 = 37.5 → 38; 40 × 1.875 = 75; 420 × 1.875 = 787.5 → 788; 640 × 1.875 = 1200
        assert_eq!(
            p,
            PhysicalRect {
                x: 38,
                y: 75,
                width: 750,
                height: 1125
            }
        );
        // Swapping the factors would give a different x: 20 × 1.25 × 1.5 is
        // the same product, so pin one factor at 1 instead.
        assert_eq!(to_physical(rect, 1.0, 1.25).x, 25);
        assert_eq!(to_physical(rect, 1.5, 1.0).x, 30);
    }

    #[test]
    fn the_band_inset_scales_with_os_dpi_only() {
        let full = PhysicalRect {
            x: 0,
            y: 0,
            width: 1000,
            height: 800,
        };
        let at = |scale: f64, zoom: f32| WindowGeometry {
            zoom,
            scale,
            inner: (1000, 800),
            maximized: false,
            band_is_ours: true,
        };
        // 100 % DPI: 6 px on every edge the child reaches.
        assert_eq!(
            inset_for_band(full, at(1.0, 1.0)),
            PhysicalRect {
                x: 6,
                y: 6,
                width: 988,
                height: 788
            }
        );
        // 200 % DPI: 12 px — and the same at Koru zoom 1.5, which plays no part.
        assert_eq!(
            inset_for_band(full, at(2.0, 1.0)),
            PhysicalRect {
                x: 12,
                y: 12,
                width: 976,
                height: 776
            }
        );
        assert_eq!(
            inset_for_band(full, at(2.0, 1.5)),
            inset_for_band(full, at(2.0, 1.0))
        );
    }

    #[test]
    fn a_child_that_does_not_reach_an_edge_is_not_inset_there() {
        // Below a 40 px caption: the top edge is not the child's.
        let below = PhysicalRect {
            x: 0,
            y: 40,
            width: 1000,
            height: 760,
        };
        let g = WindowGeometry {
            zoom: 1.0,
            scale: 1.0,
            inner: (1000, 800),
            maximized: false,
            band_is_ours: true,
        };
        assert_eq!(
            inset_for_band(below, g),
            PhysicalRect {
                x: 6,
                y: 40,
                width: 988,
                height: 754
            }
        );
    }

    #[test]
    fn maximised_or_not_ours_means_no_inset() {
        let full = PhysicalRect {
            x: 0,
            y: 0,
            width: 1000,
            height: 800,
        };
        let g = WindowGeometry {
            zoom: 1.0,
            scale: 2.0,
            inner: (1000, 800),
            maximized: true,
            band_is_ours: true,
        };
        assert_eq!(inset_for_band(full, g), full);
        let g = WindowGeometry {
            maximized: false,
            band_is_ours: false,
            ..g
        };
        assert_eq!(inset_for_band(full, g), full);
    }

    /// A fake adapter that records every `place` it receives.
    struct Fake {
        placed: RefCell<Vec<(PhysicalRect, bool)>>,
        fail: bool,
    }

    impl NativeSurface for Fake {
        fn place(&self, bounds: PhysicalRect, visible: bool) -> Result<(), SurfaceError> {
            self.placed.borrow_mut().push((bounds, visible));
            if self.fail {
                Err(SurfaceError {
                    what: "place",
                    reason: "fake".into(),
                })
            } else {
                Ok(())
            }
        }
        fn has_focus(&self) -> bool {
            false
        }
        fn focus_parent(&self) {}
    }

    /// A view that owns a fake surface and says where it wants it.
    struct Owner {
        surface: Option<Rc<Fake>>,
        want: Placement,
    }

    impl View for Owner {
        fn native_surface(&mut self) -> Option<(SharedSurface, Placement)> {
            self.surface
                .as_ref()
                .map(|s| (s.clone() as SharedSurface, self.want))
        }
        // The rest of the trait is defaulted; a fake needs none of it.
        fn key(
            &mut self,
            _ws: &mut koru_core::schema::WorkspaceDoc,
            _key: &winit::keyboard::Key,
            _text: Option<&str>,
            _mods: winit::keyboard::ModifiersState,
        ) -> bool {
            false
        }
        fn mouse_down(
            &mut self,
            _ws: &mut koru_core::schema::WorkspaceDoc,
            _x: f32,
            _y: f32,
            _ctrl: bool,
        ) -> bool {
            false
        }
        fn mouse_move(&mut self, _x: f32, _y: f32) -> bool {
            false
        }
        fn mouse_up(&mut self, _ws: &mut koru_core::schema::WorkspaceDoc) -> bool {
            false
        }
        fn mouse_double_click(
            &mut self,
            _ws: &koru_core::schema::WorkspaceDoc,
            _x: f32,
            _y: f32,
        ) -> bool {
            false
        }
        fn scroll_by(&mut self, _delta: winit::event::MouseScrollDelta) {}
        fn set_viewport(&mut self, _w: f32, _h: f32) {}
        fn cursor_icon(&self) -> winit::window::CursorIcon {
            winit::window::CursorIcon::Default
        }
        fn adopt(&mut self, _ws: &koru_core::schema::WorkspaceDoc) {}
        fn reseat(&mut self, _ws: &koru_core::schema::WorkspaceDoc) {}
        fn invalidate(&mut self) {}
        fn draw(
            &mut self,
            _scene: &mut vello::Scene,
            _workspace: &koru_workspace::Workspace,
            _images: &koru_draw::ImageCache,
            _root: vello::kurbo::Affine,
            _caret_on: bool,
            _pointer: (f32, f32),
        ) {
        }
    }

    fn geometry() -> WindowGeometry {
        WindowGeometry {
            zoom: 1.0,
            scale: 1.0,
            inner: (1000, 800),
            maximized: false,
            band_is_ours: true,
        }
    }

    #[test]
    fn reconcile_places_what_the_view_asks_for_and_nothing_without_a_surface() {
        let mut owner = Owner {
            surface: None,
            want: Placement {
                rect: Rect::new(0.0, 40.0, 1000.0, 760.0),
                visible: true,
            },
        };
        assert_eq!(reconcile(&mut owner, geometry()), None);
        owner.surface = Some(Rc::new(Fake {
            placed: RefCell::new(Vec::new()),
            fail: false,
        }));
        assert_eq!(reconcile(&mut owner, geometry()), None);
        owner.want.visible = false;
        assert_eq!(reconcile(&mut owner, geometry()), None);
        let placed = owner.surface.as_ref().unwrap().placed.borrow().clone();
        assert_eq!(
            placed,
            vec![
                (
                    PhysicalRect {
                        x: 6,
                        y: 40,
                        width: 988,
                        height: 754
                    },
                    true
                ),
                (
                    PhysicalRect {
                        x: 6,
                        y: 40,
                        width: 988,
                        height: 754
                    },
                    false
                ),
            ]
        );
    }

    #[test]
    fn a_failed_placement_is_returned_to_the_caller() {
        let mut owner = Owner {
            surface: Some(Rc::new(Fake {
                placed: RefCell::new(Vec::new()),
                fail: true,
            })),
            want: Placement {
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                visible: true,
            },
        };
        let error = reconcile(&mut owner, geometry()).expect("the fake fails");
        assert_eq!(error.what, "place");
    }
}
