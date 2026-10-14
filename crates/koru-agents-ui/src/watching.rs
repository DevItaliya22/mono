// SPDX-License-Identifier: MPL-2.0

//! **The foot of a session somebody else is driving.**
//!
//! A delegated run's session belongs to the orchestrator: it holds the
//! controller lease for as long as the run works. A window can still *read*
//! that session — the observer verbs need no lease — and the maintainers' ask is
//! exactly that: open the chat, watch the agent work, and have one way to
//! step in.
//!
//! What that needs on screen is two things and no more:
//!
//! - **A sentence saying why nothing can be typed.** A surface that were
//!   merely inert would leave a person pressing keys into a session that
//!   silently ignores them and concluding the app was broken. So the
//!   read-only state is *said*, in ordinary text, under the transcript.
//! - **One control that changes it.** [`Watching::Yes`] draws *Take the
//!   session* — the same word and the same request the run views' chip
//!   makes, because it is the same act. [`Watching::Asked`] draws no control
//!   at all and says the request is with the agent (ADR-354: **absent, not
//!   disabled** — a control present but refused is a promise the surface
//!   cannot keep, and §7.4 forbids the surface claiming a takeover it has
//!   not been granted).
//!
//! **This band never ends anything.** Taking a session ends the agent's step
//! and the run carries on; ending a run is a different act, wearing a
//! different face, behind a confirmation, and it lives on the run views
//! (ADR-354). There is no path from here to it.
//!
//! One arithmetic, shared: [`layout`] is what the draw pass paints into and
//! what the hit test asks, so the button cannot be pressable anywhere but
//! where it was drawn.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_i18n::{Key as I18nKey, Lang, t};
use koru_ui::button::{ActionButton, ActionStyle};
use vello::Scene;
use vello::kurbo::Affine;

/// Whether a surface is watching its session rather than driving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Watching {
    /// The ordinary session: this window holds the controller, or is about
    /// to. Nothing here is drawn.
    #[default]
    No,
    /// Read-only. Somebody else has the session and this window is reading
    /// its event stream over a connection that took no lease.
    Yes,
    /// Read-only, and the session has been asked for. Still read-only: the
    /// handover is the host's to report, and the surface says it is waiting
    /// rather than pretending it already happened.
    Asked,
}

impl Watching {
    /// Whether the band is drawn at all.
    pub fn watching(self) -> bool {
        !matches!(self, Self::No)
    }

    /// The sentence under the transcript.
    pub fn line(self, lang: Lang) -> &'static str {
        match self {
            Self::No | Self::Yes => t(lang, I18nKey::TerminalWatchingAgent),
            Self::Asked => t(lang, I18nKey::RunTakeoverPending),
        }
    }

    /// The word on the control, when there is one. `None` once the session
    /// has been asked for: there is nothing left to press.
    pub fn action(self, lang: Lang) -> Option<&'static str> {
        matches!(self, Self::Yes).then(|| t(lang, I18nKey::RunTakeControl))
    }
}

/// The band's height: the sentence, the control beside it, and air.
pub const FOOTER_H: f32 = 44.0;
/// Air at each end of the band, matching the transcript's own side inset so
/// the sentence lines up under the last message rather than beside it.
const PAD_X: f32 = koru_session_view::CONTENT_INSET_X;
/// Between the sentence and the control.
const GAP: f32 = 12.0;

/// Where the band's two pieces go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The rect the sentence is drawn in, vertically centred.
    pub line: Rect,
    /// The control, when the state has one.
    pub action: Option<Rect>,
}

/// A band that was not laid out: nothing to draw and nothing to press.
/// Written rather than derived because `Rect` has no `Default` and a zero
/// rect is the honest empty value here.
impl Default for Layout {
    fn default() -> Self {
        Self {
            line: Rect::new(0.0, 0.0, 0.0, 0.0),
            action: None,
        }
    }
}

impl Layout {
    /// Whether a point lands on the control. The one question the host asks
    /// of this module, and the reason the geometry is a value.
    pub fn hits_action(&self, x: f32, y: f32) -> bool {
        self.action.is_some_and(|rect| rect.contains(x, y))
    }
}

/// Measure the band. **The only arithmetic**: `draw` paints into what this
/// returns and the host hit-tests against the same value.
pub fn layout(band: Rect, engine: &mut TextEngine, lang: Lang, watching: Watching) -> Layout {
    if !watching.watching() || band.h <= 0.0 {
        return Layout::default();
    }
    let action = watching.action(lang).map(|label| {
        let button = ActionButton::new(label, ActionStyle::Secondary, None);
        let width = button.width(engine).max(MIN_ACTION_W);
        Rect::new(
            band.x + band.w - PAD_X - width,
            band.y + (band.h - ActionButton::H) * 0.5,
            width,
            ActionButton::H,
        )
    });
    let right = action.map_or(band.x + band.w - PAD_X, |rect| rect.x - GAP);
    Layout {
        line: Rect::new(
            band.x + PAD_X,
            band.y,
            (right - (band.x + PAD_X)).max(0.0),
            band.h,
        ),
        action,
    }
}

/// So *Take the session* and a longer translation of it are the same size of
/// target, and a row of one button does not look like a fragment.
const MIN_ACTION_W: f32 = 120.0;

/// One draw of the band: where it goes, what it says, and where the pointer
/// and the keyboard are. A value rather than five more arguments, because
/// the last four travel together and only ever change together.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub rect: Rect,
    pub lang: Lang,
    pub watching: Watching,
    pub hovered: bool,
    pub focused: bool,
}

/// Paint the band: its ground, its hairline, the sentence and the control.
pub fn draw(canvas: &mut Scene, root: Affine, engine: &mut TextEngine, band: Band) -> Layout {
    let Band {
        rect,
        lang,
        watching,
        hovered,
        focused,
    } = band;
    let band = rect;
    let laid = layout(band, engine, lang, watching);
    if !watching.watching() || band.h <= 0.0 {
        return laid;
    }
    let p = theme::p();
    scene::fill(
        canvas,
        root,
        p.panel_bg,
        &scene::rect(band.x, band.y, band.w, band.h),
    );
    scene::fill(
        canvas,
        root,
        p.panel_border,
        &scene::rect(band.x, band.y, band.w, theme::BORDER_W as f32),
    );
    let text = watching.line(lang);
    let style = theme::role::meta();
    let measured = engine.simple_layout_wrapped(text, style, laid.line.w.max(1.0));
    scene::draw_layout(
        canvas,
        root,
        &measured,
        laid.line.x,
        laid.line.y + (laid.line.h - measured.height()) * 0.5,
        Some(p.text_dim),
    );
    if let (Some(rect), Some(label)) = (laid.action, watching.action(lang)) {
        ActionButton::new(label, ActionStyle::Secondary, None)
            .draw_focused(canvas, root, engine, rect, hovered, focused);
    }
    laid
}

#[cfg(test)]
mod tests;
