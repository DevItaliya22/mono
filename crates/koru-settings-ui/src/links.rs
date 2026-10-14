// SPDX-License-Identifier: MPL-2.0

//! **The surface's own use of the typed settings client.**
//!
//! SP-6b put the whole loop — discover the Gateway, take the bounded startup
//! wait, apply the registry's defaults meanwhile, refetch when a change fact
//! says the section moved, send per-field patches for what this window changed
//! — behind `koru_api_client::SectionLink`, on its own worker. **None of that
//! is reimplemented here.** What this module adds is the two things a surface
//! needs and a link does not offer:
//!
//! - **What is outstanding.** A link's `save` applies a patch locally and sends
//!   it; whether the authority took it is not something the link says, because
//!   it publishes the authoritative section either way. Remembering what was
//!   asked for is what lets the surface tell *the authority kept it* from
//!   *the authority put it back*, which is the difference between silence and
//!   S23's visible reconciliation.
//! - **Whether anybody is home.** `SectionLink::is_connected` answers whether a
//!   worker thread started, not whether the Gateway is reachable — and there is
//!   no `is_reachable`. Since a reachable authority publishes a section on
//!   every outcome, and an unreachable one publishes nothing, "nothing has
//!   published inside the deadline" is the inference, and it is the only signal
//!   the client gives. The honest consequence is recorded on
//!   [`SettingsSurface::poll`](crate::SettingsSurface::poll).

use std::time::Instant;

use koru_api_client::{GlobalLink, TerminalLink};
use koru_settings::{GlobalPatch, GlobalSettings, TerminalPatch, TerminalSettings};

#[cfg(test)]
mod tests;

use crate::state::{FIRST_SECTION_DEADLINE, PATCH_DEADLINE};

/// One section's link, and what this surface is still waiting to hear about it.
struct Watched<L, P> {
    link: L,
    /// The patch this surface sent and has not seen answered. Only fields this
    /// surface itself patches ever appear here.
    outstanding: Option<P>,
    /// When the answer stops being late and starts being absence.
    due: Instant,
    /// Whether anything has ever arrived.
    heard: bool,
}

impl<L, P> Watched<L, P> {
    fn new(link: L) -> Self {
        Self {
            link,
            outstanding: None,
            due: Instant::now() + FIRST_SECTION_DEADLINE,
            heard: false,
        }
    }

    fn arrived(&mut self) {
        self.outstanding = None;
        self.heard = true;
        // Nothing is pending, so nothing is due. Pushed out of the way rather
        // than made optional, because "no deadline" and "a deadline far away"
        // are the same answer to every question asked of it.
        self.due = Instant::now() + FIRST_SECTION_DEADLINE;
    }

    fn sent(&mut self, patch: P) {
        self.outstanding = Some(patch);
        self.due = Instant::now() + PATCH_DEADLINE;
    }

    fn is_overdue(&self, now: Instant) -> bool {
        (!self.heard || self.outstanding.is_some()) && now >= self.due
    }
}

/// **The links a Settings surface holds**, or none at all.
///
/// A host that already owns a link for a section does not hand one here — it
/// feeds the section with `adopt_global` / `adopt_terminal` instead, and the
/// two never both write the same field. That is the arrangement in Koru: the
/// shell has held the Global link since SP-6b and goes on owning every
/// preference it already patches, while this surface holds the Terminal link
/// because nothing else in that process holds one.
pub struct SettingsLinks {
    global: Option<Watched<GlobalLink, GlobalPatch>>,
    terminal: Option<Watched<TerminalLink, TerminalPatch>>,
}

impl SettingsLinks {
    /// No links. Every section is fed by the host, and nothing dials anything —
    /// which is what every test uses, and what a host under `cfg(test)` must
    /// pass so a suite never reaches a Gateway somebody has running.
    pub fn disabled() -> Self {
        Self {
            global: None,
            terminal: None,
        }
    }

    /// Open the links this surface wants, and return at once.
    ///
    /// `segment` is the release channel's segment; `root` is a test or
    /// alternate store root. Both are the link's own arguments — nothing here
    /// resolves a path.
    pub fn open(
        segment: &str,
        root: Option<std::path::PathBuf>,
        global: bool,
        terminal: bool,
    ) -> Self {
        Self {
            global: global
                .then(|| Watched::new(GlobalLink::global(segment.to_owned(), root.clone()))),
            terminal: terminal
                .then(|| Watched::new(TerminalLink::terminal(segment.to_owned(), root))),
        }
    }

    /// Only the Terminal section — Koru's arrangement, where the shell keeps
    /// the Global link it has held since SP-6b.
    pub fn terminal_only(segment: &str, root: Option<std::path::PathBuf>) -> Self {
        Self::open(segment, root, false, true)
    }

    /// **Links that own a section and never connect.**
    ///
    /// The surface behaves exactly as it does with real ones — a patch is
    /// applied locally and recorded as outstanding, a fed section reconciles
    /// against it, and the deadline expires — but nothing is discovered and no
    /// socket is opened. This is what a host constructs under `cfg(test)`:
    /// a guard inside this crate cannot cover `apps/koru`, because a
    /// dependency is never compiled with `cfg(test)`.
    pub fn inert(global: bool, terminal: bool) -> Self {
        Self {
            global: global.then(|| Watched::new(GlobalLink::disabled())),
            terminal: terminal.then(|| Watched::new(TerminalLink::disabled())),
        }
    }

    pub fn global_authoritative(&self) -> GlobalSettings {
        self.global
            .as_ref()
            .map_or_else(GlobalSettings::default, |watched| {
                watched.link.authoritative().clone()
            })
    }

    pub fn terminal_authoritative(&self) -> TerminalSettings {
        self.terminal
            .as_ref()
            .map_or_else(TerminalSettings::default, |watched| {
                watched.link.authoritative().clone()
            })
    }

    pub(crate) fn poll_global(&mut self) -> Option<GlobalSettings> {
        let watched = self.global.as_mut()?;
        let section = watched.link.poll()?;
        watched.arrived();
        Some(section)
    }

    /// A read-only view of the Terminal section for a host that only needs
    /// to *apply* it — Koru's terminal tabs. Same as the surface's own poll.
    pub fn poll_terminal_section(&mut self) -> Option<TerminalSettings> {
        self.poll_terminal()
    }

    pub(crate) fn poll_terminal(&mut self) -> Option<TerminalSettings> {
        let watched = self.terminal.as_mut()?;
        let section = watched.link.poll()?;
        watched.arrived();
        Some(section)
    }

    pub(crate) fn save_global(&mut self, patch: GlobalPatch) {
        let Some(watched) = self.global.as_mut() else {
            return;
        };
        watched.sent(patch.clone());
        watched
            .link
            .save(patch, |patch, section| patch.clone().apply(section));
    }

    pub(crate) fn save_terminal(&mut self, patch: TerminalPatch) {
        let Some(watched) = self.terminal.as_mut() else {
            return;
        };
        watched.sent(patch.clone());
        watched
            .link
            .save(patch, |patch, section| patch.clone().apply(section));
    }

    pub(crate) fn take_outstanding_global(&mut self) -> Option<GlobalPatch> {
        self.global.as_mut()?.outstanding.take()
    }

    pub(crate) fn take_outstanding_terminal(&mut self) -> Option<TerminalPatch> {
        self.terminal.as_mut()?.outstanding.take()
    }

    /// Whether either link has gone quiet past its deadline.
    pub(crate) fn is_overdue(&self) -> bool {
        let now = Instant::now();
        self.global
            .as_ref()
            .is_some_and(|watched| watched.is_overdue(now))
            || self
                .terminal
                .as_ref()
                .is_some_and(|watched| watched.is_overdue(now))
    }

    /// The soonest moment a caller should look again even if nothing happens.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let mut soonest: Option<Instant> = None;
        for due in [
            self.global.as_ref().map(|watched| watched.due),
            self.terminal.as_ref().map(|watched| watched.due),
        ]
        .into_iter()
        .flatten()
        {
            soonest = Some(soonest.map_or(due, |current: Instant| current.min(due)));
        }
        soonest
    }

    /// Hand the wake proxy down to whichever links exist.
    pub(crate) fn on_update(&self, wake: impl Fn() + Send + Clone + 'static) {
        if let Some(watched) = self.global.as_ref() {
            watched.link.on_update(wake.clone());
        }
        if let Some(watched) = self.terminal.as_ref() {
            watched.link.on_update(wake);
        }
    }

    /// Whether this surface holds a link for a section at all — a host that
    /// feeds one must not also be handed one.
    pub fn holds_global(&self) -> bool {
        self.global.is_some()
    }

    pub fn holds_terminal(&self) -> bool {
        self.terminal.is_some()
    }
}

impl std::fmt::Debug for SettingsLinks {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SettingsLinks")
            .field("global", &self.global.is_some())
            .field("terminal", &self.terminal.is_some())
            .finish()
    }
}
