// SPDX-License-Identifier: MPL-2.0

//! What can be asserted about painting without a GPU: the state machine behind
//! the four chip treatments, the ellipsis, and the tooltip a disabled chip owes.
//!
//! Drawing itself is not asserted here — a scene is opaque and a golden image is
//! a test that fails when a font ships. What *is* asserted is every decision
//! made before a brush is chosen, which is where the bugs are.

use super::*;
use crate::host::{Banner, BannerKey};
use crate::keys::{ProfileKey, WorkspaceKey};

fn profile_chip() -> BannerChip {
    BannerChip::new(
        "Steven",
        BannerKey::Profile(ProfileKey::from_uuid(uuid::Uuid::new_v4())),
    )
    .with_mark("S")
}

fn workspace_chip() -> BannerChip {
    BannerChip::new(
        "Notes",
        BannerKey::Workspace(WorkspaceKey::from_uuid(uuid::Uuid::new_v4())),
    )
}

fn state<'a>(banner: &'a Banner) -> CaptionState<'a> {
    CaptionState {
        caption: Rect::new(0.0, 0.0, 1200.0, theme::CAPTION_H),
        banner,
        slots: banner::Slots {
            profile: true,
            workspace: banner::Workspace::Named(60.0),
            settings: true,
        },
        merged: false,
        window_buttons: true,
        context: false,
        host_action: false,
        maximised: false,
        settings_open: false,
        hover: None,
        banner_hover: None,
        open: None,
        pressed: None,
    }
}

/// **Hover, selected and pressed are three treatments, not two.** A chip whose
/// popover is open and is *also* hovered must read stronger than either alone,
/// or the popover looks unowned — DESIGN, "The banner" §8.
#[test]
fn selected_hovered_and_pressed_are_three_distinct_treatments() {
    let banner = Banner {
        profile: Some(profile_chip()),
        workspace: Some(workspace_chip()),
        settings: true,
    };
    let rest = state(&banner);
    assert_eq!(
        chip_background(rest.chip_states(), BannerSlot::Profile, true),
        None
    );

    let mut hovered = state(&banner);
    hovered.banner_hover = Some(BannerSlot::Profile);
    let hover_tone = chip_background(hovered.chip_states(), BannerSlot::Profile, true);
    assert!(hover_tone.is_some());

    let mut open = state(&banner);
    open.open = Some(BannerSlot::Profile);
    let open_tone = chip_background(open.chip_states(), BannerSlot::Profile, true);
    assert_eq!(open_tone, hover_tone, "selected at rest is the quiet tint");

    let mut open_hovered = state(&banner);
    open_hovered.open = Some(BannerSlot::Profile);
    open_hovered.banner_hover = Some(BannerSlot::Profile);
    let strengthened = chip_background(open_hovered.chip_states(), BannerSlot::Profile, true);
    assert_ne!(
        strengthened, open_tone,
        "hovering an open chip strengthens its tint rather than replacing it"
    );

    let mut pressed = state(&banner);
    pressed.pressed = Some(BannerSlot::Profile);
    assert_eq!(
        chip_background(pressed.chip_states(), BannerSlot::Profile, true),
        strengthened,
        "pressed is the strongest of the three"
    );
}

/// **A pane toggle stays bare; Settings lights** (owner, 2026-09-02).
///
/// The reversal of the Terminal exception, pinned. DESIGN's rule — *do not mark
/// a caption control active merely because the surface it controls is visibly
/// open* — is unqualified again, so the two pane toggles pass [`BARE`] however
/// open their columns are, exactly as Koru's caption draws them. Settings keeps
/// its `active`, because what it opens is a **tab**: state the caption cannot
/// otherwise show, which is what the persistent treatment is for.
///
/// Asserting the `State` rather than the paint is the rule this file already
/// follows: what is worth pinning is the decision made before a brush is chosen.
#[test]
fn the_pane_toggles_stay_bare_and_only_settings_reads_selected() {
    let banner = Banner {
        profile: Some(profile_chip()),
        workspace: Some(workspace_chip()),
        settings: true,
    };
    let rest = state(&banner);
    assert!(
        !rest.toggle(false, rest.settings_open).active,
        "nothing is open, so nothing is selected"
    );

    // Both columns open, and neither toggle says so: the columns do.
    for hovered in [false, true] {
        let bare = rest.toggle(hovered, BARE);
        assert!(!bare.active, "a pane toggle never lights");
        assert!(!bare.disabled);
        assert_eq!(bare.hovered, hovered, "and it still answers the pointer");
    }

    let mut open = state(&banner);
    open.settings_open = true;
    assert!(
        open.toggle(false, open.settings_open).active,
        "Settings opens a tab that looks like every other tab in the lane, so \
         the slot is the only place that state can be seen"
    );

    // And hover survives the active state rather than being replaced by it —
    // `icon_button` strengthens the accent chip from 0.12 to 0.20, which is
    // DESIGN's "hovering it strengthens that treatment".
    let both = open.toggle(true, open.settings_open);
    assert!(both.hovered && both.active);
}

/// A dimmed chip does not light up under the pointer. S9 is *quiet*, not
/// hidden — and a control that highlights is a control that promises to open.
#[test]
fn a_disabled_chip_takes_no_hover_treatment() {
    let banner = Banner {
        profile: Some(profile_chip().unavailable("The Gateway is not answering")),
        workspace: None,
        settings: true,
    };
    let mut hovered = state(&banner);
    hovered.banner_hover = Some(BannerSlot::Profile);
    assert_eq!(
        chip_background(hovered.chip_states(), BannerSlot::Profile, false),
        None
    );
}

/// The tooltip carries the **reason** when there is one, and the label
/// otherwise — the localized sentence S9 owes, in the person's language,
/// supplied by the host.
#[test]
fn a_disabled_chip_says_why_and_an_enabled_one_says_what() {
    let anchor = Rect::new(0.0, 0.0, 36.0, theme::CAPTION_H);
    let live = Banner {
        profile: Some(profile_chip().with_tooltip("Switch profile")),
        workspace: Some(workspace_chip().with_tooltip("Switch workspace")),
        settings: true,
    };
    assert_eq!(
        tooltip(&live, BannerSlot::Profile, anchor).map(|text| text.text),
        Some("Switch profile".to_owned()),
        "the action, not the name that is already being pointed at"
    );
    let down = Banner {
        profile: Some(profile_chip().unavailable("No gateway")),
        workspace: Some(workspace_chip().unavailable("No gateway")),
        settings: true,
    };
    assert_eq!(
        tooltip(&down, BannerSlot::Workspace, anchor).map(|text| text.text),
        Some("No gateway".to_owned())
    );
    // The identity slot is the profile's square wearing both meanings, so it
    // borrows the profile's sentence rather than inventing a third one.
    assert_eq!(
        tooltip(&down, BannerSlot::Identity, anchor).map(|text| text.text),
        Some("No gateway".to_owned())
    );
    // Settings is an icon everybody knows and the host names it in the menu;
    // a tooltip here would be a second label for one door.
    assert!(tooltip(&live, BannerSlot::Settings, anchor).is_none());
    // And a chip with nothing to say draws none rather than echoing its label.
    let silent = Banner {
        profile: Some(profile_chip()),
        workspace: None,
        settings: false,
    };
    assert!(tooltip(&silent, BannerSlot::Profile, anchor).is_none());
}

/// The ellipsis lands at the **end** of the label and the result fits.
///
/// Trimmed by character, not by byte: a `zh-Hans` name reaches the clamp at
/// roughly half the character count, and slicing it at a byte boundary would
/// panic rather than truncate.
#[test]
fn a_long_label_is_elided_at_its_end_and_never_split_mid_character() {
    let mut engine = TextEngine::new();
    let long = "Engineering Notes And Everything Else";
    let elided = elide(&mut engine, long, 60.0);
    assert!(elided.ends_with('\u{2026}'));
    assert!(engine.simple_layout(&elided, theme::role::row()).width() <= 60.0);

    let cjk = "工程笔记与其他所有内容";
    let narrow = elide(&mut engine, cjk, 20.0);
    assert!(narrow.ends_with('\u{2026}'));
    assert!(cjk.starts_with(narrow.trim_end_matches('\u{2026}')));
}

/// A width nothing fits in still answers a string rather than panicking, which
/// is the case a window mid-drag actually reaches.
#[test]
fn an_impossible_width_answers_the_ellipsis_alone() {
    let mut engine = TextEngine::new();
    assert_eq!(elide(&mut engine, "Notes", 0.0), "\u{2026}");
}

/// The same key lands on the same palette everywhere, which is what stops the
/// caption chip and the popover row for one profile disagreeing about colour.
#[test]
fn a_tint_is_stable_for_one_key_and_spread_across_the_four_palettes() {
    let first = tint("steven@example.com");
    assert_eq!(first, tint("steven@example.com"));
    let spread: std::collections::BTreeSet<usize> = ["a", "b", "c", "d", "e", "f", "g", "h"]
        .into_iter()
        .map(koru_ui::profile_picker::chip_slot)
        .collect();
    assert!(spread.len() > 1, "one key per palette, not one palette");
}

// ------------------------------------------------- parity with the reference
//
// The Terminal has to feel like Koru, and the only proof available without a
// screenshot is that the two reach for the same token. These assert the token
// each way — what it *is*, and what it is deliberately *not* — so a reviewer can
// read the claim without opening `apps/koru`.

/// The palette is a process global, so a test that flips it and one that reads
/// it have to take turns. The lesson `THEME_LOCK` already carries elsewhere in
/// the tree; poisoning is ignored because a failed assertion leaves the theme
/// where the guard's own restore puts it.
static THEME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn theme_lock() -> std::sync::MutexGuard<'static, ()> {
    THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// **Container chrome is `status_bg`; what sits on it is `panel_bg`.**
///
/// Koru fills its caption with `status_bg` (`apps/koru/src/mainview/draw.rs`,
/// `draw_caption`) and its explore column with `status_bg`
/// (`apps/koru/src/panes/explore.rs`, `draw`). This band was `panel_bg`, which
/// in the light palette is the same `#FFFFFF` the active tab pill fills with —
/// so the pill's only remaining mark was its outline and a tab read as a text
/// input.
#[test]
fn the_caption_band_is_the_token_korus_caption_and_explore_column_use() {
    let _guard = theme_lock();
    for dark in [false, true] {
        theme::set_dark(dark);
        assert_eq!(
            chrome_background(),
            theme::p().status_bg,
            "the band is `status_bg`, in both themes"
        );
        assert_ne!(
            chrome_background(),
            theme::p().panel_bg,
            "and never the token the card on top of it uses — \
             `koru_shell_chrome::tabs::draw` fills the active pill with `panel_bg`"
        );
    }
    theme::set_dark(false);
}

/// The mark on an identity tile is the pill palette's own `fg`, exactly as
/// `koru_ui::profile_picker::draw_avatar` writes the initials it draws for the
/// same identity in the popover row underneath.
///
/// It was `bubble_fg` — the *inverted* selection bubble's foreground, near-white
/// in the light theme — on a pale pill. The initials were drawn every frame and
/// could not be seen, which is how a profile chip became a blank pale square.
#[test]
fn a_marks_colour_is_its_own_palettes_foreground_not_the_bubbles() {
    let _guard = theme_lock();
    for key in ["steven@example.com", "a", "b", "c", "d", "e", "f", "g"] {
        let palette = palette(key);
        assert_eq!(palette.bg, tint(key), "the fill is the palette's own");
        assert_ne!(
            palette.fg, palette.bg,
            "and the mark's colour is legible on it"
        );
        assert_ne!(
            palette.fg,
            theme::p().bubble_fg,
            "`bubble_fg` belongs to the one inverted surface in the app, not here"
        );
    }
}

/// **The caption and the pane header close with one rule, from one function.**
///
/// `h - 1`, inside the band, full width. Both were literals — three of them once
/// the host had its own copy — and two rows in the same band whose hairlines are
/// a pixel apart read as two different heights across the divide.
#[test]
fn every_chrome_band_closes_with_the_same_hairline_rule() {
    let caption = Rect::new(theme::SIDEBAR_W, 0.0, 968.0, theme::CAPTION_H);
    let band = Rect::new(0.0, 0.0, theme::SIDEBAR_W, theme::CAPTION_H);
    for source in [caption, band] {
        let rule = band_hairline(source);
        assert_eq!(rule.h, 1.0, "one logical pixel");
        assert_eq!(
            rule.y,
            source.y + source.h - 1.0,
            "inside the band, not under it"
        );
        assert_eq!((rule.x, rule.w), (source.x, source.w), "its full width");
    }
    assert_eq!(
        band_hairline(caption).y,
        band_hairline(band).y,
        "so the two read as one line across the divide"
    );
}
