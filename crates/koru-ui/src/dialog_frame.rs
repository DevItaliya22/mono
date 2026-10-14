// SPDX-License-Identifier: MPL-2.0

//! The shared compact name-prompt frame. Content, editing and modal policy
//! stay with the host; this component owns the existing layout and paint.

use koru_draw::{geometry::Rect, inspect, scene, theme};
use vello::{Scene, kurbo::Affine};

const WIDTH: f32 = 320.0;
const HEIGHT: f32 = 84.0;
const RADIUS: f32 = 8.0;
const INSET: f32 = 8.0;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum KoruDialogFrameState {
    #[default]
    Unknown,
    Open,
    Closing,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct KoruDialogFrameInspection {
    /// Existing host opening identity, if one is actually available.
    pub occurrence: Option<u64>,
    pub state: KoruDialogFrameState,
}

/// A resolved frame. Drawing and the host's hit testing read the same panel.
/// Only the approved compact name-prompt recipe is offered initially.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct KoruDialogFrame {
    viewport: Rect,
    panel: Rect,
}

impl KoruDialogFrame {
    pub fn name_prompt(viewport: (f32, f32)) -> Self {
        Self {
            viewport: Rect::new(0.0, 0.0, viewport.0, viewport.1),
            panel: Rect::new(
                ((viewport.0 - WIDTH) * 0.5).max(INSET),
                ((viewport.1 - HEIGHT) * 0.5).max(INSET),
                WIDTH.min((viewport.0 - INSET * 2.0).max(1.0)),
                HEIGHT,
            ),
        }
    }

    pub const fn panel(&self) -> Rect {
        self.panel
    }

    /// The callback always paints, even when capture is off. It receives the
    /// resolved panel used by the frame, not another geometry calculation.
    pub fn draw<R>(
        &self,
        canvas: &mut Scene,
        root: Affine,
        association: KoruDialogFrameInspection,
        content: impl FnOnce(&mut Scene, Affine, Rect) -> R,
    ) -> R {
        let panel = self.panel;
        let _inspection = inspect::scope(
            "KoruDialogFrame",
            "component",
            root,
            scene::rect(panel.x, panel.y, panel.w, panel.h),
            |r| {
                r.property(
                    "component ID",
                    "C-DIALOG-FRAME",
                    "docs/features/koruui/COMPONENTS.md",
                );
                r.property("component family", "C-OVERLAY", "catalog");
                r.property(
                    "recipe",
                    "compact name prompt",
                    "KoruDialogFrame::name_prompt",
                );
                match association.occurrence {
                    Some(id) => r.property("occurrence", id, "existing host opening identity"),
                    None => r.property(
                        "occurrence",
                        "unavailable",
                        "host supplies no per-opening UI identity",
                    ),
                }
                r.property(
                    "state",
                    format!("{:?}", association.state),
                    "explicit host state; paint does not infer lifecycle",
                );
                r.property(
                    "recipe width / height",
                    format!("{WIDTH} / {HEIGHT}"),
                    "current compact name-prompt recipe; GP-V04",
                );
                r.property("radius", RADIUS, "compact name-prompt recipe; GP-V04");
                r.property(
                    "minimum inset",
                    INSET,
                    "existing name-prompt clamp, not a universal spacing token",
                );
                r.property(
                    "vertical overflow",
                    panel.y + panel.h > self.viewport.h,
                    "fixed height retained; GP-L01",
                );
                r.property(
                    "responsive limitation",
                    "fixed height; child overflow and scrolling are host-owned and unresolved",
                    "GP-L01; no new clipping/shrink policy",
                );
                r.property(
                    "native semantics",
                    "host-owned modal projection, input, focus and dismissal",
                    "frame paint does not implement modal behavior",
                );
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        {
            let viewport = self.viewport;
            let _scrim = inspect::scope(
                "Dialog scrim",
                "part",
                root,
                scene::rect(viewport.x, viewport.y, viewport.w, viewport.h),
                |_| {},
            );
            scene::fill(
                canvas,
                root,
                theme::p().scrim,
                &scene::rect(viewport.x, viewport.y, viewport.w, viewport.h),
            );
        }
        scene::floating_panel(
            canvas,
            root,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            &scene::Panel::popover(RADIUS),
        );
        content(canvas, root, panel)
    }
}

#[cfg(test)]
mod tests;
