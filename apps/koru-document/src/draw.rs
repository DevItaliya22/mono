// SPDX-License-Identifier: MPL-2.0

//! The document's own chrome: the caption row when it owns the window, and the
//! selection bubble.
//!
//! There is much less of it than there used to be. A formatting toolbar, a
//! view-mode control, a page swatch and the document's name all lived here and
//! were on screen whether or not they had anything to do; what is left appears
//! when it is asked for and nowhere else. The row-level affordances — the `+`
//! and the title's `…` — are drawn by the editor's row loop instead, because
//! they scroll with their rows and are gated on a row's own properties, the way
//! the checkbox and the drag grip already are.
//!
//! Geometry comes from `bubble::buttons` and `koru_ui::caption::controls`, which
//! the click path also calls, so a button is clickable exactly where it is
//! drawn.

use vello::Scene;
use vello::kurbo::Affine;

use koru_core::schema::WorkspaceDoc;

use crate::caption;
use crate::{blockmenu, bubble, docview};
use koru_editor::Body;
use koru_editor::ViewState;

/// The state the window's own caption needs when this view owns the window
/// chrome: whether the window fills the screen, and whether it is pinned above
/// the others.
///
/// `None` is a document hosted in a container tab: the caption above is already
/// the container's, and a second one would not even reach the window's edge.
#[derive(PartialEq, Debug)]
pub struct WindowChrome {
    pub maximized: bool,
    pub pinned: bool,
    /// How far the ⋮ button has turned into an ✕: 0 a ⋮ button, 1 an ✕.
    pub menu_morph: f32,
    /// Whether its menu is showing — which is what makes the button a close
    /// button, red on hover.
    pub menu_open: bool,
    /// The document's name, drawn in the bar.
    ///
    /// A standalone window turns `Body::title_row` off, so the name is not the
    /// first line of the page here — it is the window's, and the window already
    /// has somewhere to put it.
    pub title: String,
    /// A local-search match in the standalone caption title, if any.
    pub search: Option<koru_editor::SearchHighlight>,
    /// What the **Offline** chip should say, and whether its popover is open.
    ///
    /// `None` while the store is being reached, which is when there is nothing
    /// to say. Worded by `koru_workspace::Unconfirmed`, so this window and the
    /// container cannot describe the same state two ways.
    pub offline: Option<(String, bool)>,
}

/// Everything this view draws over its rows: its title bar, if it has one, and
/// the selection bubble, if there is a selection.
#[allow(clippy::too_many_arguments)]
pub fn draw_chrome(
    scene: &mut Scene,
    root: Affine,
    body: &Body,
    view: &mut ViewState,
    ws: &WorkspaceDoc,
    chrome: Option<WindowChrome>,
    bubble_panel: Option<koru_draw::geometry::Rect>,
    tooltip: f32,
) {
    let vw = view.viewport.0;
    if let Some(state) = chrome {
        // **The bar wears the page's colour.** A window that is one document is
        // the document — a separate grey strip across the top made it look like
        // a frame around a page rather than the page's own top edge. Page mode
        // tints the canvas, so the bar takes whatever the user chose there;
        // Full and Narrow have no canvas, so it takes the plain background.
        // Both read straight off the same fields `koru_editor::draw` fills
        // from, which is what stops the two drifting apart.
        let bar_bg = if view.view_mode == koru_draw::theme::ViewMode::Page {
            view.canvas_bg
                .map(|c| vello::peniko::Color::from_rgba8(c[0], c[1], c[2], c[3]))
                .unwrap_or(koru_draw::theme::p().canvas)
        } else {
            koru_draw::theme::p().bg
        };
        caption::draw(
            scene,
            root,
            &mut view.engine,
            vw,
            view.pointer,
            state.maximized,
            state.pinned,
            state.menu_morph,
            state.menu_open,
            &state.title,
            state.search.as_ref(),
            bar_bg,
            state.offline.as_ref().map(|(d, open)| (d.as_str(), *open)),
        );
    }

    let Some(panel) = bubble_panel else {
        return;
    };
    let state = docview::State::read(ws, body, body.armed());
    let icon = blockmenu::block_icon(&state);
    let pointer = view.pointer;
    bubble::draw(scene, root, &mut view.engine, panel, &state, icon, pointer);
    if tooltip > 0.0 {
        bubble::draw_tooltip(
            scene,
            root,
            &mut view.engine,
            panel,
            (vw, view.viewport.1),
            pointer,
            tooltip,
            &state,
        );
    }
}
