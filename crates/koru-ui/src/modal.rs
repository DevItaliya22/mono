// SPDX-License-Identifier: MPL-2.0

//! A modal root: the full-window scrim that owns everything painted above it.
//!
//! The scrim is the modal's claim that nothing beneath answers until it is
//! gone, and the inspector tree makes the same claim: a KoruModal subtree is
//! hoisted to lead the hierarchy although the modal paints last. Picking still
//! follows paint order, so a point on the modal never selects the window
//! content it covers.

use koru_draw::{inspect, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

/// Paint the scrim over the whole viewport, then the content closure on top.
///
/// The closure receives the same scene: the modal owns the dimming and the
/// tree position, never what floats on it — that is the hosted surface's
/// (a KoruPrompt, a form) own business.
#[track_caller]
pub fn draw(
    canvas: &mut Scene,
    root: Affine,
    viewport: (f32, f32),
    content: impl FnOnce(&mut Scene),
) {
    let _modal = inspect::scope_hoisted(
        "KoruModal",
        "component",
        root,
        scene::rect(0.0, 0.0, viewport.0, viewport.1),
        |r| {
            r.property("implementation", "koru_ui::modal", "shared implementation");
            r.property(
                "scrim",
                inspect::color(theme::p().scrim),
                "theme::p().scrim",
            );
            r.property(
                "order",
                "first in the hierarchy by hoist; last in paint by z",
                "inspect::scope_hoisted",
            );
            r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
        },
    );
    // The opacity is the modal's one claim, so it is a named part: hovering
    // the dimmed window answers "Scrim", not an anonymous fill.
    {
        let _scrim = inspect::scope(
            "Scrim",
            "part",
            root,
            scene::rect(0.0, 0.0, viewport.0, viewport.1),
            |r| {
                r.property(
                    "color",
                    inspect::color(theme::p().scrim),
                    "theme::p().scrim",
                );
            },
        );
        scene::fill(
            canvas,
            root,
            theme::p().scrim,
            &scene::rect(0.0, 0.0, viewport.0, viewport.1),
        );
    }
    content(canvas);
}
