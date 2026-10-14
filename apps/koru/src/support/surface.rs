// SPDX-License-Identifier: MPL-2.0

//! The adapter: the host's `NativeSurface` over `koru-webview`'s surface.
//!
//! The one file that names both crates. The host places, asks about focus
//! and hands focus back; the engine does what it is told. The adapter keeps
//! the last applied placement so a reconcile that changes nothing costs
//! nothing, and a freshly created object always receives its first
//! placement (ADR-276).

use std::cell::Cell;

use koru_webview::{PhysicalRect as EnginePhysicalRect, WebError, WebSurface};
use koru_workspace_host::native_surface::{NativeSurface, PhysicalRect, SurfaceError};

pub struct HostedWebSurface {
    surface: WebSurface,
    applied: Cell<Option<(PhysicalRect, bool)>>,
}

impl HostedWebSurface {
    pub fn new(surface: WebSurface) -> Self {
        Self {
            surface,
            applied: Cell::new(None),
        }
    }
}

impl HostedWebSurface {
    /// Run a script on the page.
    pub fn eval(&self, js: &str) -> Result<(), WebError> {
        self.surface.eval(js)
    }
}

fn engine_rect(r: PhysicalRect) -> EnginePhysicalRect {
    EnginePhysicalRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

impl NativeSurface for HostedWebSurface {
    fn place(&self, bounds: PhysicalRect, visible: bool) -> Result<(), SurfaceError> {
        if self.applied.get() == Some((bounds, visible)) {
            return Ok(());
        }
        let was_visible = self.applied.get().is_some_and(|(_, v)| v);
        // Bounds first, then visibility, so a surface that is about to be
        // shown appears where it belongs rather than where it was.
        self.surface
            .set_bounds(engine_rect(bounds))
            .map_err(|e| SurfaceError {
                what: "place",
                reason: e.to_string(),
            })?;
        if was_visible != visible || self.applied.get().is_none() {
            self.surface
                .set_visible(visible)
                .map_err(|e| SurfaceError {
                    what: if visible { "place" } else { "hide" },
                    reason: e.to_string(),
                })?;
        }
        self.applied.set(Some((bounds, visible)));
        Ok(())
    }

    fn has_focus(&self) -> bool {
        self.surface.has_focus()
    }

    fn focus_parent(&self) {
        let _ = self.surface.focus_parent();
    }
}
