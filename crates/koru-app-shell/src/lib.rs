// SPDX-License-Identifier: MPL-2.0

//! **The app shell: host-provided surfaces behind identical chrome.**
//!
//! It can open nothing itself. What a tab, a pane, a profile or a workspace
//! *means* is always the host's ([ADR-262](../../../docs/decisions/README.md)),
//! and the one sentence this crate's charter has to exclude is that one: **the
//! shell may not name a document, a workspace, a profile, a session or a store**
//! — check `Cargo.toml` and `src/tests.rs` rather than believing this sentence,
//! because the compile graph is what keeps a workspace switch from ever being
//! the shell's business.
//!
//! It sits on [`koru_window::MultiWindowApp`] and owns policy only. The window
//! *behaviour* — key quarantine, resize borders, drag timing, click counts,
//! capture, IME, focus and occlusion, theme and scale ordering, present
//! acknowledgement, renderer warm-up, settle and reschedule — lives once in
//! `koru-window`'s private dispatch core, which both runners call.
//!
//! ## What is here
//!
//! | | |
//! |---|---|
//! | [`Surface`] | what the shell hosts: one event path in, one command path out |
//! | [`ShellHost`] | what the application supplies: every decision the shell may not make |
//! | [`ShellState`] | the container — windows, tabs, chrome, the drain, and the `MultiWindowApp` impl |
//! | [`TabSet`] / [`TabStrip`] / [`TabSlides`] | the tab manager, generic over its element |
//! | [`Panel`] | a pane slot that slides open and shut |
//! | [`BannerState`] | the banner's slots and DESIGN's collapse ladder |
//! | [`Popover`] | one anchored floating panel; the rows in it are the host's |
//! | [`chrome`] | painting the caption band, the chips and the lane |
//! | [`SharedSurface`] | a surface the host keeps a handle on while the shell owns the tab |
//!
//! ## What is deliberately absent
//!
//! No `restore`/`capture`, no payload persistence, no surface *kind*, no reuse
//! policy, no `Any` downcast, no second dispatcher, and no `SidePaneSurface` —
//! that last deferred to SP-11 by owner decision, because a pane trait with no
//! consumer is an inner platform nobody compiled against. Each absence waits
//! for the consumer that would have to appear before it is added — ADR-262's
//! charter; the closed UIREFACTOR program (git history) carries the list.
//!
//! ## Strings
//!
//! There are none. Every word a person reads comes in from the host, already
//! localized — the same rule `koru-shell-chrome` follows, and the reason
//! neither crate depends on `koru-i18n`.

pub mod banner_state;
pub mod chrome;
pub mod command;
pub mod element;
pub mod host;
pub mod keys;
pub mod panel;
pub mod popover;
pub mod state;
pub mod surface;
pub mod tabset;
pub mod tabslide;
pub mod tabstrip;
pub mod zoom_label;

pub use banner_state::{BannerState, Rung};
pub use command::{Commands, ShellAction, ShellCommand};
pub use element::TabElement;
pub use host::{
    Banner, BannerChip, BannerKey, BannerSlot, HostCx, PopoverFrame, SeatRefused, SeatedTab,
    ShellHost,
};
pub use keys::{ProfileKey, TabKey, WindowKey, WorkspaceKey};
pub use panel::Panel;
pub use popover::{Popover, SlideFrom};
pub use state::{
    MIN_WINDOW_SIZE, ShellState, ShellTab, ShellWindow, Side, ZOOM_MAX, ZOOM_MIN, ZOOM_STEP,
    stepped_zoom,
};
pub use surface::{CloseResponse, PointerEvent, SharedSurface, Surface, SurfaceCx};
pub use tabset::TabSet;
pub use tabslide::TabSlides;
pub use tabstrip::{TabGroup, TabLoc, TabStrip};
pub use zoom_label::ZoomLabel;

#[cfg(test)]
mod tests;
