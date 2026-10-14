// SPDX-License-Identifier: MPL-2.0

//! **Painting the caption row.** The hand-drawn caption dies in here.
//!
//! `koru-shell-chrome` owns the arithmetic — where each rect is — and
//! deliberately draws none of the banner: it has `tabs::draw` and nothing for
//! the chips, because a chip is a *state* (hover, pressed, selected, disabled)
//! and state lives in the container. So this module is the other half of that
//! split — the whole of what Plan 7.3 moved out of koru-terminal's paint
//! module (an app since deleted, TERMINATAB W5).
//!
//! ## What it may know
//!
//! Nothing. Every string arrives on a [`BannerChip`](crate::BannerChip) already
//! localized, the tint is an opaque key hashed into one of the four existing
//! pill palettes, and the mark is whatever the host worked out — initials, an
//! emoji, a colour tile's letter. The four states DESIGN §8 names are
//! presentation and therefore ours; *why* a chip is disabled is a sentence the
//! host hands over.
//!
//! ## The three treatments that must not collapse into each other
//!
//! - **Hover** is a subtle neutral background on the chip's own square.
//! - **Selected** — the chip whose popover is open — keeps a persistent tinted
//!   background, and hovering it *strengthens* that rather than replacing it.
//! - **Pressed** is distinct from both, for the whole time the button is down.
//!
//! DESIGN says so in as many words, and the reason is that a chip which read as
//! hovered while its popover was open would leave the popover looking unowned.

use koru_shell_chrome::regions::Hit;
use koru_shell_chrome::{banner, caption};

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{icons, scene, theme};
/// **The banner's own arithmetic, re-exported.**
///
/// A host that has to assert where a chip lands — the collapse ladder's rungs
/// are DESIGN's, and a test that could not name a slot could not check them —
/// would otherwise have to depend on `koru-shell-chrome` directly, for
/// geometry the shell already owns the state of. One door, and it is this one.
pub use koru_shell_chrome::banner as banner_geometry;
/// The caption's furniture, for the same reason again — SP-12's context toggle
/// is a rect a host has to be able to name to assert that its column's control
/// is offered on one tab and not another.
pub use koru_shell_chrome::caption as caption_geometry;
/// The window's own cut, for the same reason a third time — a host whose left
/// pane has to line up with the caption beside it (DESIGN §10: the caption
/// starts at `sidebar_w`) needs to be able to name the caption that `solve`
/// produced, and `ShellWindow::regions` is only reachable from a live window.
pub use koru_shell_chrome::regions as region_geometry;
/// The lane's arithmetic, for the same reason — a host asserting that its `+`
/// is where it drew it should not have to depend on the chrome crate for one
/// function the shell already calls.
pub use koru_shell_chrome::tabs as tab_geometry;
/// The icon lane's own metric, for the same reason a fourth time. A host's pane
/// rows sit beside Koru's, and `PAGE_ICON_SLOT` is what Koru measures the lane
/// in front of a row's label by — a second number for the same lane is two
/// columns that do not line up.
pub use koru_shell_chrome::title_icon as icon_geometry;
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;

use crate::host::{Banner, BannerChip, BannerSlot};

/// The Settings slot's glyph. `FADERS` rather than a gear: it is the icon the
/// app already uses for preferences, and the banner may not mint a second
/// vocabulary for the same door.
const SETTINGS_ICON: char = icons::FADERS;

/// Radius of a chip's square. `AVATAR_RADIUS` in DESIGN's prose — the same
/// number `koru_ui::profile_picker` insets its identity tile by, because the
/// caption's avatar and the popover's are the same shape at two sizes.
const CHIP_RADIUS: f32 = 6.0;

/// The tile inside a 36-square slot. The slot is the hit area; the tile is what
/// is drawn, and the gap between them is what makes two adjacent chips read as
/// two shapes rather than one bar (DESIGN, "The banner": *there is a gap
/// between them that belongs to neither*).
const TILE: f32 = 22.0;

/// What a dimmed chip draws at. `koru_ui::icon_button`'s own disabled tone, so
/// an unavailable selector reads as the same kind of unavailable as every other
/// control in the row (S9 is quiet, not hidden).
const DIMMED: f32 = 0.45;

/// **A pane toggle is never *active*** (owner, 2026-09-02).
///
/// DESIGN, "Icon-only buttons": *do not mark a caption control active merely
/// because the surface it controls is visibly open* — the pane already
/// communicates that state. That rule stands unqualified; the Terminal exception
/// that briefly qualified it is gone, and Koru's own caption has drawn both
/// toggles this way all along
/// (`apps/koru/src/mainview/draw.rs`, `draw_caption`).
///
/// Named rather than a bare `false` at two call sites, so the rule is legible
/// where it is obeyed and a reader wondering why a toggle never lights finds the
/// answer instead of a literal.
const BARE: bool = false;

/// What the caption is drawing this frame.
///
/// One carrier rather than eleven arguments: every field is answered by the
/// container from state it already holds, and a call site that had to name them
/// positionally is a call site that will get two bools the wrong way round.
pub struct CaptionState<'a> {
    /// The **drawn** caption — the one that follows the sliding pane. Hit
    /// testing uses the settled rect; this is the other one, and they differ
    /// only for the 140 ms of a slide.
    pub caption: Rect,
    pub banner: &'a Banner,
    pub slots: banner::Slots,
    /// True when the leading square is the merged identity slot, so it draws
    /// the workspace's colour as a hairline underline (ladder step 3).
    pub merged: bool,
    pub window_buttons: bool,
    pub context: bool,
    pub host_action: bool,
    pub maximised: bool,
    /// Whether the Settings surface is what the window is showing, so its slot
    /// reads *active*. The host's answer, because what Settings opens is a tab
    /// and tabs are the host's.
    ///
    /// **The only `open` this struct carries**, and deliberately: whether either
    /// pane is showing is not asked, because [`BARE`] means the paint cannot use
    /// the answer, and a field carried for a painter that ignores it is a claim
    /// about the drawing that is not true.
    pub settings_open: bool,
    pub hover: Option<Hit>,
    pub banner_hover: Option<BannerSlot>,
    /// Which slot's popover is open — DESIGN's *selected*, which hover
    /// strengthens rather than replaces.
    pub open: Option<BannerSlot>,
    pub pressed: Option<BannerSlot>,
}

impl CaptionState<'_> {
    fn chip_states(&self) -> ChipStates {
        ChipStates {
            hover: self.banner_hover,
            open: self.open,
            pressed: self.pressed,
        }
    }

    /// **One `State` for every caption icon button.**
    ///
    /// `koru_ui::icon_button`'s `active` is how this app draws a persistent
    /// selected treatment: a low-alpha accent chip, the Phosphor Fill face, the
    /// glyph in `accent` (DESIGN, "Icon-only buttons"). What reaches it is the
    /// caller's business, and DESIGN is unqualified about which controls may ask
    /// for it — **not the ones whose surface you can already see**.
    ///
    /// So the two pane toggles pass [`BARE`]: a column that is open says so by
    /// being open, and Koru's own caption draws both with `active: false`
    /// (`apps/koru/src/mainview/draw.rs`, `draw_caption`). Settings passes its
    /// own answer, because what *it* opens is a tab that looks like every other
    /// tab in the lane — state that would otherwise be hidden, which is exactly
    /// what the persistent treatment is for.
    ///
    /// The `hovered` half matters as much as the `active` one: `icon_button`
    /// strengthens the accent chip from 0.12 to 0.20 under the pointer, which is
    /// DESIGN's *hover strengthens selected rather than replacing it* — and a
    /// control that dropped its hover while active would go the other way.
    fn toggle(&self, hovered: bool, active: bool) -> koru_ui::icon_button::State {
        koru_ui::icon_button::State {
            hovered,
            active,
            disabled: false,
        }
    }
}

/// **DESIGN §8's four states, carried without a band.**
///
/// The identity chips are drawn in two places now — the caption and the pane
/// header — and hover, selected and pressed mean exactly the same thing in each.
/// One carrier, so the second home inherits the three treatments rather than
/// growing its own copy of them.
#[derive(Copy, Clone, Default)]
pub struct ChipStates {
    pub hover: Option<BannerSlot>,
    /// Which slot's popover is open — DESIGN's *selected*.
    pub open: Option<BannerSlot>,
    pub pressed: Option<BannerSlot>,
}

/// **The identity chips in a pane header band** (owner, 2026-09-02).
///
/// The band's own hairline and the two selectors. Everything else in the column
/// is the host's, and everything about the chips — the geometry, the four
/// states, the tint hash, the ellipsis — is the same code the caption runs;
/// only the rects come from `banner::pane_*`.
pub struct PaneHeader<'a> {
    /// The **drawn** band, which follows the sliding column. Hit-testing uses
    /// the settled one, exactly as the caption does.
    pub band: Rect,
    pub banner: &'a Banner,
    pub slots: banner::Slots,
    /// True when the band's leading square is the merged identity slot.
    pub merged: bool,
    pub states: ChipStates,
}

/// Draw a pane header band: its closing hairline, then its chips.
///
/// The hairline sits *inside* the band at `h - 1`, which is where the caption
/// draws its own — the rule that makes the two read as one line across the
/// divide (DESIGN, "The window is cut into four").
pub fn draw_pane_header(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    header: &PaneHeader<'_>,
) {
    let band = header.band;
    if band.w <= 0.0 || band.h <= 0.0 {
        return;
    }
    let rule = band_hairline(band);
    scene::fill(
        scene,
        root,
        theme::p().panel_border,
        &scene::rect(rule.x, rule.y, rule.w, rule.h),
    );
    draw_identity(
        scene,
        root,
        engine,
        header.banner,
        banner::pane_profile_slot(band, header.slots),
        banner::pane_workspace_slot(band, header.slots),
        header.slots.workspace,
        header.merged,
        header.states,
    );
}

/// **What every piece of container chrome is painted on, and why it is not
/// `panel_bg`.**
///
/// One token for the caption band *and* the side columns, because Koru uses one
/// for both: its caption fills with `status_bg`
/// (`apps/koru/src/mainview/draw.rs`, `draw_caption`) and its explore pane
/// fills with `status_bg` (`apps/koru/src/panes/explore.rs`, `draw`). What sits
/// **on** the chrome is `panel_bg` — the active tab pill, the explore pane's
/// open row — each outlined in `panel_border`, which is what makes them read as
/// cards.
///
/// Both were `panel_bg` here, and in the light palette that is the same
/// `#FFFFFF` as the card's own fill. So the pane was a bare white strip with a
/// hairline and the active tab was a bordered white box, which reads as a text
/// input rather than a tab. One token, and it is the reference's.
pub fn chrome_background() -> Color {
    theme::p().status_bg
}

/// **The hairline that closes a chrome band**, inside it at `h - 1`.
///
/// One function for the caption and for the pane header beside it, because that
/// is the whole point of the rule: two rows in the same band whose rules are a
/// pixel apart read as two different heights, which is extremely visible across
/// the divide. It was a literal in each painter, and the pane's was a third copy
/// in the host.
pub fn band_hairline(band: Rect) -> Rect {
    Rect::new(band.x, band.y + band.h - 1.0, band.w, 1.0)
}

/// The caption's own background and hairline. Everything else draws over it.
pub fn draw_band(scene: &mut Scene, root: Affine, caption: Rect) {
    scene::fill(
        scene,
        root,
        chrome_background(),
        &scene::rect(caption.x, caption.y, caption.w, caption.h),
    );
    let rule = band_hairline(caption);
    scene::fill(
        scene,
        root,
        theme::p().panel_border,
        &scene::rect(rule.x, rule.y, rule.w, rule.h),
    );
}

/// The pane toggle, the banner's chips and the Settings slot.
///
/// The tab lane and the window buttons are drawn by the container around this,
/// because both already have a shared function and neither needs a state this
/// module holds.
pub fn draw_banner(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    state: &CaptionState<'_>,
) {
    let caption = state.caption;
    koru_ui::icon_button::draw(
        scene,
        root,
        engine,
        caption::sidebar_toggle(caption),
        icons::PANEL_LEFT,
        state.toggle(state.hover == Some(Hit::ToggleSidebar), BARE),
    );

    draw_identity(
        scene,
        root,
        engine,
        state.banner,
        banner::profile_slot(caption, state.slots),
        banner::workspace_slot(caption, state.slots),
        state.slots.workspace,
        state.merged,
        state.chip_states(),
    );

    let settings = banner::settings_slot(caption, state.window_buttons, state.slots);
    if settings.w > 0.0 {
        koru_ui::icon_button::draw(
            scene,
            root,
            engine,
            settings,
            SETTINGS_ICON,
            state.toggle(
                state.banner_hover == Some(BannerSlot::Settings),
                // **Selected while its own surface is showing**, which is the
                // rule its two neighbours already followed. This was `false`
                // outright: the slot is the only caption control whose surface
                // is a tab, so the shell could not answer it until the host was
                // asked (`ShellHost::settings_open`).
                state.settings_open,
            ),
        );
    }

    // **The context toggle** (SP-12). Its rect is the banner's business because
    // Settings sits outside it, which is why this is drawn here beside the chips
    // rather than with the tab lane. [`BARE`] like the sidebar toggle above —
    // the two are one pair and they read as one pair only if they wear the same
    // state, and the Details column being on screen is already the answer.
    let context = caption::context_toggle_with_banner(
        caption,
        state.window_buttons,
        state.context,
        state.slots,
    );
    if context.w > 0.0 {
        koru_ui::icon_button::draw(
            scene,
            root,
            engine,
            context,
            icons::PANEL_RIGHT,
            state.toggle(state.hover == Some(Hit::ToggleContext), BARE),
        );
    }
}

/// **The two selectors, wherever they are.**
///
/// One painter for both bands (owner, 2026-09-02). The rects are the caller's —
/// `banner::profile_slot` in the caption, `banner::pane_profile_slot` in the
/// pane header — and everything after that is identical, which is what stops the
/// second home becoming a second set of treatments.
#[allow(clippy::too_many_arguments)]
fn draw_identity(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    banner: &Banner,
    profile: Rect,
    workspace: Rect,
    mode: banner::Workspace,
    merged: bool,
    states: ChipStates,
) {
    if profile.w > 0.0
        && let Some(chip) = banner.profile.as_ref()
    {
        let slot = if merged {
            BannerSlot::Identity
        } else {
            BannerSlot::Profile
        };
        draw_mark_chip(scene, root, engine, profile, chip, states, slot);
        if merged && let Some(chip) = banner.workspace.as_ref() {
            // Ladder step 3: one square, with the workspace's colour as a
            // hairline underline, so merging keeps *both* reachable rather than
            // dropping one (DESIGN, "The banner" §2).
            let tile = tile_in(profile);
            scene::fill(
                scene,
                root,
                tint(&chip.tint),
                &scene::rect(tile.x, tile.y + tile.h - 2.0, tile.w, 2.0),
            );
        }
    }
    if workspace.w > 0.0
        && let Some(chip) = banner.workspace.as_ref()
    {
        match mode {
            banner::Workspace::IconOnly => {
                draw_mark_chip(
                    scene,
                    root,
                    engine,
                    workspace,
                    chip,
                    states,
                    BannerSlot::Workspace,
                );
            }
            banner::Workspace::Named(_) => {
                draw_named_chip(scene, root, engine, workspace, chip, states);
            }
            banner::Workspace::Hidden => {}
        }
    }
}

/// The tile a square slot draws inside: centred, and smaller than the slot so
/// two neighbours read as two shapes.
fn tile_in(slot: Rect) -> Rect {
    Rect::new(
        slot.x + (slot.w - TILE) * 0.5,
        slot.y + (slot.h - TILE) * 0.5,
        TILE.min(slot.w),
        TILE.min(slot.h),
    )
}

/// The background a chip's own square wears, folding DESIGN §8's four states
/// into one colour — and answering `None` for a chip at rest, which draws
/// nothing at all.
fn chip_background(states: ChipStates, slot: BannerSlot, enabled: bool) -> Option<Color> {
    let p = theme::p();
    let hovered = enabled && states.hover == Some(slot);
    if states.pressed == Some(slot) {
        return Some(p.row_selected);
    }
    if states.open == Some(slot) {
        // Selected keeps a persistent tint, and hover *strengthens* it rather
        // than replacing it — the one rule that stops an open popover looking
        // unowned.
        return Some(if hovered {
            p.row_selected
        } else {
            p.hover_tint
        });
    }
    hovered.then_some(p.hover_tint)
}

/// A square chip: the profile's avatar, the merged identity slot, or the
/// workspace at ladder step 2.
fn draw_mark_chip(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    slot: Rect,
    chip: &BannerChip,
    states: ChipStates,
    which: BannerSlot,
) {
    if let Some(background) = chip_background(states, which, chip.enabled) {
        let hover = koru_ui::icon_button::hover_chip(slot);
        scene::fill(
            scene,
            root,
            background,
            &scene::rrect(hover.x, hover.y, hover.w, hover.h, CHIP_RADIUS),
        );
    }
    let tile = tile_in(slot);
    let alpha = if chip.enabled { 1.0 } else { DIMMED };
    let palette = palette(&chip.tint);
    scene::fill(
        scene,
        root,
        palette.bg.multiply_alpha(alpha),
        &scene::rrect(tile.x, tile.y, tile.w, tile.h, CHIP_RADIUS),
    );
    if chip.mark.is_empty() {
        return;
    }
    let layout = engine.simple_layout(&chip.mark, theme::role::hint());
    scene::draw_layout_first_line(
        scene,
        root,
        &layout,
        tile.x + (tile.w - layout.width()) * 0.5,
        scene::centre_y_in(&layout, tile),
        // `palette.fg`, exactly as `profile_picker::draw_avatar` does for the
        // same identity in the popover row underneath.
        Some(palette.fg.multiply_alpha(alpha)),
    );
}

/// The workspace at ladder steps 0 and 1: a name and a chevron.
///
/// **The ellipsis is applied here and the width is `banner::workspace_slot`'s.**
/// One clamp, one place, so drawing and hit-testing cannot disagree about where
/// a long name stops (DESIGN, "The banner" §3).
fn draw_named_chip(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    slot: Rect,
    chip: &BannerChip,
    states: ChipStates,
) {
    if let Some(background) = chip_background(states, BannerSlot::Workspace, chip.enabled) {
        // `banner::named_chip_box` rather than four literals, so the pill and a
        // tab across the divide keep the same insets and a test can name it.
        let inner = banner::named_chip_box(slot);
        scene::fill(
            scene,
            root,
            background,
            &scene::rrect(inner.x, inner.y, inner.w, inner.h, CHIP_RADIUS),
        );
    }
    let alpha = if chip.enabled { 1.0 } else { DIMMED };
    let chevron_w = 14.0;
    let label_w = (slot.w - 12.0 - chevron_w).max(0.0);
    let label = engine.simple_layout(&chip.label, theme::role::row());
    let text = if label.width() > label_w {
        let elided = elide(engine, &chip.label, label_w);
        engine.simple_layout(&elided, theme::role::row())
    } else {
        label
    };
    scene::draw_layout_first_line(
        scene,
        root,
        &text,
        slot.x + 6.0,
        scene::centre_y_in(&text, slot),
        Some(theme::p().text.multiply_alpha(alpha)),
    );
    // The chevron **turns over** when the popover is open, mirrored about its
    // own centre with an `Affine` — `ph-caret-up` is not in `icons.rs` and
    // those codepoints are lifted from the pinned stylesheet rather than
    // guessed at.
    let glyph = engine.icon_layout(icons::CARET_DOWN, 12.0);
    let cx = slot.x + slot.w - chevron_w - 4.0 + (chevron_w - glyph.width()) * 0.5;
    let cy = scene::centre_y_in(&glyph, slot);
    let turned = states.open == Some(BannerSlot::Workspace);
    let transform = if turned {
        let mid_x = f64::from(cx + glyph.width() * 0.5);
        let mid_y = f64::from(cy + glyph.height() * 0.5);
        root * Affine::translate((mid_x, mid_y))
            * Affine::scale_non_uniform(1.0, -1.0)
            * Affine::translate((-mid_x, -mid_y))
    } else {
        root
    };
    scene::draw_layout_first_line(
        scene,
        transform,
        &glyph,
        cx,
        cy,
        Some(theme::p().text_dim.multiply_alpha(alpha)),
    );
}

/// The longest prefix of `label` that fits in `width`, with an ellipsis.
///
/// Trims by character rather than by byte, which is not pedantry: a workspace
/// name in `zh-Hans` reaches the clamp at roughly half the character count and
/// slicing it at a byte boundary would panic.
fn elide(engine: &mut TextEngine, label: &str, width: f32) -> String {
    let mut chars: Vec<char> = label.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "\u{2026}";
        if engine.simple_layout(&candidate, theme::role::row()).width() <= width {
            return candidate;
        }
    }
    "\u{2026}".to_owned()
}

/// One of the four existing pill palettes, chosen by hashing an opaque key.
///
/// `koru_ui::profile_picker::chip_slot` is the hash, deliberately: the caption
/// chip and the popover row for the same profile have to land on the same
/// colour, and two hashes would eventually disagree.
///
/// **The whole palette, not just its fill.** `koru_ui::profile_picker::draw_avatar`
/// — the tile Koru draws for the same identity, at the same radius — fills with
/// `palette.bg` and writes the initials in `palette.fg`. This returned only the
/// fill, and the mark was written in `bubble_fg`, which is the *inverted*
/// bubble's foreground: near-white in the light theme, on a pale pill. The
/// initials were drawn and could not be seen, so a profile chip was a blank
/// pale square — `p.date.bg` is `#FBF0DB`, which is the pale yellow that was
/// reported.
fn palette(key: &str) -> &'static theme::PillPalette {
    let p = theme::p();
    match koru_ui::profile_picker::chip_slot(key) {
        0 => &p.document,
        1 => &p.contact,
        2 => &p.date,
        _ => &p.link,
    }
}

/// The fill alone, for the callers that only tint a rule.
fn tint(key: &str) -> Color {
    palette(key).bg
}

/// The tooltip a hovered chip owes, or `None`.
///
/// Both chips carry one, because both are unfamiliar and one of them is
/// icon-only. A disabled chip's tooltip is the host's *reason* — S9's "quiet,
/// not hidden": the chip stays, dimmed, and says why in the person's language.
pub fn tooltip(banner: &Banner, slot: BannerSlot, anchor: Rect) -> Option<TooltipText> {
    let chip = match slot {
        BannerSlot::Profile | BannerSlot::Identity => banner.profile.as_ref(),
        BannerSlot::Workspace => banner.workspace.as_ref(),
        BannerSlot::Settings => return None,
    }?;
    // **The host's word, or none at all.** A chip with nothing to say draws no
    // tooltip rather than echoing its own label back — DESIGN asks for the
    // action, and repeating the name would be pointing at the thing being
    // pointed at.
    (!chip.tooltip.is_empty()).then(|| TooltipText {
        anchor,
        text: chip.tooltip.clone(),
    })
}

pub struct TooltipText {
    pub anchor: Rect,
    pub text: String,
}

#[cfg(test)]
mod tests;
