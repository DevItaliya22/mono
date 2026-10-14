// SPDX-License-Identifier: MPL-2.0

//! **The reusable session surface** — what one hosted session presents, with
//! no authority over anything else.
//!
//! [`SessionSurface`] owns everything one session tab shows: the terminal
//! view, the managed transcript, the composer, the raw-switch state machine
//! and its sized-snapshot gate, the epochs, the blink phase and the surface's
//! own presentation metrics (docs/features/terminal/ARCHITECTURE.md, ADR-263). It moved
//! here from the standalone Terminal app (TERMINATAB W3) so a second host —
//! koru.exe — could host a session view without linking it; that app is gone
//! now (W5) and koru.exe is the host.
//!
//! **Surface-neutral by contract.** The crate owns no connection, no
//! discovery, no root, no process and no launch authority, and it names no
//! window runner: a host hands the surface an implementation of
//! [`SurfaceInteraction`] — its own path to the session backend — and adapts
//! the surface onto its own shell (koru.exe's `session_tab` is that adapter;
//! the deleted Terminal app carried the first). `src/tests.rs` pins the
//! dependency direction.

mod interaction;
mod surface;
pub mod watching;

pub use interaction::SurfaceInteraction;
pub use surface::{
    CURSOR_BLINK_INTERVAL, PaneFrame, PaneStatus, RenderOutcome, SessionSurface, SurfaceFocus,
};
pub use watching::Watching;

#[cfg(test)]
mod tests;
