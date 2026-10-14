// SPDX-License-Identifier: MPL-2.0

//! The banner's slot state in one window, and the collapse ladder.
//!
//! Chrome owns the arithmetic — where a slot is, how wide it may be, whether
//! what is left is enough for the lane. What is here is the *state* that
//! arithmetic reads: which slots the host offered, the label width measured at
//! the last draw, and which rung of DESIGN's ladder the banner has settled on.
//!
//! ## The ladder is walked once per toggle, never per frame
//!
//! `DESIGN.md`, "The banner" §6: the collapse ladder is evaluated against the
//! **settled** caption — the one `regions::solve` produced from the pane's
//! `layout_width()`, which is final the instant the pane is toggled — and never
//! against the sliding one. A chip that changed step halfway through a 140 ms
//! slide would flicker, and re-measuring a label per frame is the
//! reflow-per-frame mistake in a smaller hat. So [`settle`](BannerState::settle)
//! is called when the caption changes or the banner's content changes, and
//! drawing only reads [`slots`](BannerState::slots).
//!
//! ## Two homes, one ladder (owner, 2026-09-02)
//!
//! [`ShellHost::identity_in_pane`] moves the profile and workspace selectors out
//! of the caption and into the **pane header band**, which is where Koru itself
//! draws them. Nothing about the ladder changes; only its budget does. In the
//! caption the trigger is what the tab lane keeps (`banner::fits`); in the pane
//! header the band is the whole budget (`banner::pane_fits`), because there is no
//! lane in a column.
//!
//! What the caption keeps in that arrangement is the *fallback*: while the pane
//! is shut there is no band to draw in, so the caption shows ladder **step 3** —
//! the merged identity square — and nothing else. Not step 0: the caption is not
//! the identity's home any more, and a window that grew a full avatar-and-name
//! banner every time you shut the pane would be two arrangements fighting over
//! one row.
//!
//! [`ShellHost::identity_in_pane`]: crate::host::ShellHost::identity_in_pane

use koru_draw::geometry::Rect;
use koru_shell_chrome::banner;

use crate::host::{Banner, BannerSlot};

/// The rungs of `DESIGN.md`'s collapse ladder, in order.
///
/// Step 5 ("the tab lane scrolls, then reaches zero") is not a rung here: it is
/// what the strip already does when it runs out of room, and the banner has
/// nothing left to give by then.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum Rung {
    /// Avatar, full workspace name, chevron.
    #[default]
    Full,
    /// The name truncated with an ellipsis to `WORKSPACE_MIN_W`.
    Truncated,
    /// The workspace slot becomes a square carrying its own icon or colour tile.
    IconOnly,
    /// **One identity slot**: the avatar alone, opening a two-section popover.
    /// Owner-decided (2026-08-31) over dropping either selector, because a
    /// window that cannot reach either is a window somebody has to close.
    Identity,
    /// The settings slot goes to zero. It is taken *after* the identity slot
    /// because Settings is also in the overflow menu, and the four window
    /// controls never collapse.
    NoSettings,
}

impl Rung {
    const ALL: [Self; 5] = [
        Self::Full,
        Self::Truncated,
        Self::IconOnly,
        Self::Identity,
        Self::NoSettings,
    ];

    /// The rungs a **pane header band** can take. The same four, minus
    /// `NoSettings`: Settings is caption furniture and did not move with the
    /// selectors, so a column has nothing to give at that step.
    const IN_PANE: [Self; 4] = [Self::Full, Self::Truncated, Self::IconOnly, Self::Identity];

    /// Whether the leading square on this rung means *both* selectors.
    const fn merges(self) -> bool {
        matches!(self, Self::Identity | Self::NoSettings)
    }
}

/// One window's banner: what the host offered, what was measured, where the
/// ladder settled.
#[derive(Clone, Debug, Default)]
pub struct BannerState {
    /// The host's answer at the last settle. Kept so a caption change can
    /// re-walk the ladder without asking again mid-frame.
    offered: Banner,
    /// The workspace label's measured width, in the current UI font at the
    /// current density. **Measured at draw and cached here**, so the hit path
    /// never holds a `TextEngine` — the rule `koru-shell-chrome` states and its
    /// geometry tests depend on.
    label_w: f32,
    /// Where the **identity** ladder stopped, in whichever band carries it.
    rung: Rung,
    /// What the caption shows. In the pane arrangement this is Settings alone
    /// with the pane open, and the merged identity square plus Settings with it
    /// shut.
    slots: banner::Slots,
    /// What the **pane header band** shows. [`banner::Slots::NONE`] whenever the
    /// identity lives in the caption, or the pane is shut — which is what makes
    /// "is it in the band?" a question about width rather than a second flag.
    pane: banner::Slots,
    /// Whether the caption's leading square is the merged identity slot.
    caption_merged: bool,
    /// Whether the pane header's leading square is.
    pane_merged: bool,
}

impl BannerState {
    /// What the host offered at the last settle.
    pub fn offered(&self) -> &Banner {
        &self.offered
    }

    /// The **caption's** slots as the ladder left them — the one thing the
    /// caption's drawing and hit-testing both read, and what the tab lane's own
    /// arithmetic takes.
    pub fn slots(&self) -> banner::Slots {
        self.slots
    }

    /// The **pane header band's** slots, the same way.
    ///
    /// [`banner::Slots::NONE`] unless the host put the identity there *and* the
    /// pane is open, so a caller needs no second question before asking the band
    /// where a chip is.
    pub fn pane_slots(&self) -> banner::Slots {
        self.pane
    }

    /// Whether the caption's leading square is the merged identity slot — the
    /// one that opens both sections.
    pub fn caption_merged(&self) -> bool {
        self.caption_merged
    }

    /// The same question of the pane header band.
    pub fn pane_merged(&self) -> bool {
        self.pane_merged
    }

    pub fn rung(&self) -> Rung {
        self.rung
    }

    /// The measured workspace label, cached at draw.
    pub fn label_width(&self) -> f32 {
        self.label_w
    }

    /// Record a fresh measurement. Answers whether it moved, so a caller can
    /// skip the settle when it did not.
    pub fn measured(&mut self, label_w: f32) -> bool {
        let moved = (self.label_w - label_w).abs() > f32::EPSILON;
        self.label_w = label_w;
        moved
    }

    /// Walk the ladder against the **settled** caption and keep the first rung
    /// whose lane still holds its reservation.
    ///
    /// If no rung fits, the lowest one is kept: the banner has given everything
    /// it has and the lane scrolls, which is step 5 and is the strip's own
    /// behaviour rather than a sixth thing to decide here.
    ///
    /// `pane_header` is `None` for the caption arrangement and `Some(band)` for
    /// the pane one — the **settled** band, zero-width while the pane is shut,
    /// which is the same rule the caption follows and the reason there is no
    /// third code path for the slide.
    pub fn settle(
        &mut self,
        offered: Banner,
        caption: Rect,
        pane_header: Option<Rect>,
        window_buttons: bool,
        context: bool,
    ) -> bool {
        self.offered = offered;
        let before = (self.slots, self.pane, self.caption_merged);
        match pane_header {
            None => self.settle_in_caption(caption, window_buttons, context),
            Some(band) => self.settle_in_pane(band, caption, window_buttons, context),
        }
        (self.slots, self.pane, self.caption_merged) != before
    }

    /// The original arrangement: everything in the caption, collapsing against
    /// the lane's reservation.
    fn settle_in_caption(&mut self, caption: Rect, window_buttons: bool, context: bool) {
        let mut chosen = Rung::NoSettings;
        for rung in Rung::ALL {
            if banner::fits(caption, window_buttons, context, self.at(rung)) {
                chosen = rung;
                break;
            }
        }
        self.rung = chosen;
        self.slots = self.at(chosen);
        self.caption_merged = chosen.merges();
        self.pane = banner::Slots::NONE;
        self.pane_merged = false;
    }

    /// The identity in the pane header band, and the caption keeping only what
    /// never moved.
    ///
    /// Two ladders, and neither is new. The band's runs first, against its own
    /// width; the caption's is then a single question — does Settings still fit
    /// beside the lane's reservation — which is the old step 4 with nothing left
    /// above it to take.
    fn settle_in_pane(&mut self, band: Rect, caption: Rect, window_buttons: bool, context: bool) {
        let open = band.w > 0.0;
        if open {
            let mut chosen = Rung::Identity;
            for rung in Rung::IN_PANE {
                if banner::pane_fits(band, self.at(rung)) {
                    chosen = rung;
                    break;
                }
            }
            self.rung = chosen;
            // Settings is caption furniture wherever the identity is, so the
            // band never carries it.
            self.pane = banner::Slots {
                settings: false,
                ..self.at(chosen)
            };
            self.pane_merged = chosen.merges();
        } else {
            self.rung = Rung::Identity;
            self.pane = banner::Slots::NONE;
            self.pane_merged = false;
        }
        // **The caption's half.** With the pane open it keeps nothing of the
        // identity — the slots are zero-width and the lane gets the room. With
        // it shut, step 3's merged square appears so the selectors stay
        // reachable in a window with no pane on screen.
        let identity = if open {
            banner::Slots::NONE
        } else {
            self.at(Rung::Identity)
        };
        self.caption_merged = !open && identity.profile;
        self.slots = banner::Slots {
            settings: self.offered.settings,
            ..identity
        };
        if !banner::fits(caption, window_buttons, context, self.slots) {
            // The old step 4, unchanged: Settings goes before the identity does,
            // because Settings is also in the overflow menu.
            self.slots.settings = false;
        }
    }

    /// What the banner would show on one rung.
    fn at(&self, rung: Rung) -> banner::Slots {
        let has_profile = self.offered.profile.is_some();
        let has_workspace = self.offered.workspace.is_some();
        let workspace = match rung {
            _ if !has_workspace => banner::Workspace::Hidden,
            Rung::Full => banner::Workspace::Named(self.label_w),
            Rung::Truncated => banner::Workspace::truncated(),
            Rung::IconOnly => banner::Workspace::IconOnly,
            Rung::Identity | Rung::NoSettings => banner::Workspace::Hidden,
        };
        // **The identity slot is never dropped.** The merge folds the workspace
        // selector into the profile's square — so with no profile offered there
        // is nothing to merge *into*, and the workspace keeps a square of its
        // own instead. A merge is a merge, not a deletion.
        let merged = matches!(rung, Rung::Identity | Rung::NoSettings);
        banner::Slots {
            profile: has_profile,
            workspace: if merged && !has_profile && has_workspace {
                banner::Workspace::IconOnly
            } else {
                workspace
            },
            settings: self.offered.settings && !matches!(rung, Rung::NoSettings),
        }
    }

    /// Turn a chrome hit into the slot the host is told about.
    ///
    /// **This is the only place the merged identity slot exists.** Chrome
    /// answers `Hit::Profile` for the leading square whatever it is showing,
    /// because geometrically it is the same square; from rung
    /// [`Rung::Identity`] down it means *both* selectors, and the host is told
    /// so rather than being handed a profile press it would answer wrongly.
    pub fn slot_for(&self, hit: banner::Hit) -> BannerSlot {
        Self::slot_of(hit, self.caption_merged)
    }

    /// The same, for a hit in the pane header band.
    ///
    /// A band's leading square merges for its own reason — the column was
    /// dragged too narrow for a name — and the caption's merges for another, so
    /// each home answers from its own flag rather than from a shared rung.
    pub fn pane_slot_for(&self, hit: banner::Hit) -> BannerSlot {
        Self::slot_of(hit, self.pane_merged)
    }

    fn slot_of(hit: banner::Hit, merged: bool) -> BannerSlot {
        match hit {
            banner::Hit::Profile if merged => BannerSlot::Identity,
            banner::Hit::Profile => BannerSlot::Profile,
            banner::Hit::Workspace => BannerSlot::Workspace,
            banner::Hit::Settings => BannerSlot::Settings,
        }
    }
}

#[cfg(test)]
mod tests;
