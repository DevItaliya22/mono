// SPDX-License-Identifier: MPL-2.0

//! The **branch pill** — a short name at the end of the caption band.
//!
//! Several worktrees of this repo are open at once and their windows are
//! otherwise identical on screen. Welcome has said the branch for a while, but
//! Welcome is a tab somebody closes; the caption is on screen for the whole
//! session, which is where the question is actually asked — *is this the window
//! I just rebuilt?*
//!
//! **It draws whatever name it is given, and it is never given one it should
//! not draw.** Which branches are worth naming is a judgement about a
//! particular repository's habits — `dev` and `main` are where Koru's work
//! usually is, so a pill on them would be permanent furniture that stops being
//! read — and a crate chartered to hold "only rects, indices, slots and
//! drawing primitives" cannot hold a judgement about `dev`
//! ([ADR-262](../../../docs/decisions/README.md)). The gate used to live here
//! as `worth_showing`; it is `apps/koru/src/build_stamp.rs`'s now, beside the
//! `option_env!` that reads the stamp in the first place. Both windows that
//! draw a caption go through that one function, so they still cannot disagree
//! about it.
//!
//! **Here rather than in a view, for the reason [`crate::status_chip`] gives.**
//! Two windows draw a caption; a second copy drifted once already.
//!
//! **`chip_bg` and `text_dim`, and no new colour.** This is ambient, not an
//! alarm: the offline chip's inverted surface is how the app says something is
//! wrong, and a branch name is not. `DESIGN.md` records a standing preference
//! for not adding `Palette` fields, and the group-row chip is already this
//! shape at this size.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

/// Padding inside the pill, each side of the label.
const PAD_X: f32 = 9.0;

/// The widest the pill may grow, before the label is ellipsised into it.
///
/// A branch here is a topic name — `import-google-docs`, `mac-catch` — and the
/// strip it takes room from holds the tabs. Long names are the ones this is
/// least useful for anyway: the first few characters are what tells two
/// worktrees apart.
pub const MAX_W: f32 = 148.0;

/// How wide the pill wants to be for this name.
///
/// Measured rather than fixed, unlike [`crate::status_chip::CHIP_W`]: that chip
/// is given a fixed slot and this one says whatever the branch is called. The
/// caller is expected to measure once and keep the answer — the name is fixed
/// at compile time, so it only moves when the font or the density does.
pub fn width(engine: &mut TextEngine, name: &str) -> f32 {
    let text = engine.simple_layout(name, theme::role::hint());
    (text.width() + PAD_X * 2.0).min(MAX_W)
}

/// The pill's rect at the right-hand end of a band.
///
/// The caller decides the band and has already measured `w`, so this is the
/// same arithmetic as [`crate::status_chip::chip`] and stays beside it.
pub fn pill(band: Rect, w: f32) -> Rect {
    let w = w.min(band.w);
    let h = theme::CAPTION_H * 0.55;
    Rect::new(band.x + band.w - w, band.y + (band.h - h) * 0.5, w, h)
}

/// Draw the pill. There is no popover: the name is the whole of what it says.
pub fn draw(scene: &mut Scene, root: Affine, engine: &mut TextEngine, pill: Rect, name: &str) {
    let p = theme::p();
    let _inspection = koru_draw::inspect::scope(
        "KoruBranchChip",
        "component",
        root,
        scene::rect(pill.x, pill.y, pill.w, pill.h),
        |r| {
            r.property(
                "component ID",
                "C-BRANCH-CHIP",
                "docs/features/koruui/COMPONENTS.md",
            );
            r.property("component family", "C-CHROME", "catalog");
            r.property(
                "variant",
                "ambient build branch",
                "shared branch chip recipe",
            );
            r.property("interaction", "read-only / no hit target", "branch::draw");
            r.property("horizontal padding", PAD_X, "branch chip recipe");
            r.property("radius", pill.h * 0.5, "resolved pill height / 2");
            r.property(
                "background token",
                "theme::p().chip_bg",
                "branch chip recipe",
            );
            r.property(
                "foreground token",
                "theme::p().text_dim",
                "branch chip recipe",
            );
            r.property(
                "typography role",
                "theme::role::hint()",
                "branch chip recipe",
            );
            r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
        },
    );
    scene::fill(
        scene,
        root,
        p.chip_bg,
        &scene::rrect(pill.x, pill.y, pill.w, pill.h, pill.h * 0.5),
    );
    // Ellipsised into the pill rather than clipped, so a name too long to fit
    // still reads as a name rather than as a word cut in half.
    let text = engine.simple_layout(name, theme::role::hint());
    let inner = pill.w - PAD_X * 2.0;
    scene.push_clip_layer(
        vello::peniko::Fill::NonZero,
        root,
        &scene::rrect(pill.x, pill.y, pill.w, pill.h, pill.h * 0.5),
    );
    scene::draw_layout_first_line(
        scene,
        root,
        &text,
        pill.x + PAD_X + (inner - text.width()).max(0.0) * 0.5,
        scene::centre_y_in(&text, pill),
        Some(p.text_dim),
    );
    scene.pop_layer();
}

#[cfg(test)]
mod tests;
