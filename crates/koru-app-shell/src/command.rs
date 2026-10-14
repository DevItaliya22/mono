// SPDX-License-Identifier: MPL-2.0

//! The one path back out of a surface.
//!
//! ADR-262: "The host talks back by recording commands — each naming its target
//! window — that the shell drains after every event, never by calling into the
//! shell mid-dispatch."
//!
//! ## The window is not a field a caller fills in
//!
//! A [`ShellCommand`] carries a [`WindowKey`], but nothing constructs one
//! directly. A surface is handed a [`Commands`] already bound to the window it
//! is being dispatched in, and `push` stamps it. So "every command names a
//! target window" is not a review rule anyone can forget: there is no way to
//! record a command without one, and no way for a surface to address a window
//! other than its own. A host — which legitimately does address other
//! windows — reaches [`crate::host::HostCx`], which takes the key explicitly.
//!
//! ## The drain, and its three properties
//!
//! **Order.** Commands are performed in the order they were recorded, and a
//! whole batch is taken before any of it runs
//! ([`ShellState::drain`](crate::ShellState::drain)).
//!
//! **Reentrancy.** A command recorded *by* a command is queued for the next
//! pass rather than recursed into, so nothing can be performed while a surface
//! or the host is borrowed. `drain` therefore loops until the queue settles.
//!
//! **Bounds.** That loop is capped at [`DRAIN_PASSES`] passes. A host that
//! answers every command with another one is a bug, and a bounded loop turns it
//! into a panic at the point of the mistake rather than a window that stops
//! responding; the cap is generous enough that no honest sequence approaches it
//! (the deepest real chain is three: a key opens a tab, the tab's activation
//! retitles it, the retitle asks for a redraw).

use koru_window::WindowRequest;

use crate::keys::{TabKey, WindowKey};
use crate::surface::Surface;

/// How many times the drain will go round before it decides the commands are
/// not settling. See the module docs.
pub const DRAIN_PASSES: u32 = 64;

/// What a surface or a host asks the shell to do.
///
/// Deliberately not `Clone`, `Debug` or `PartialEq`: it carries a
/// `Box<dyn Surface>`, and making it comparable would push an identity or a
/// kind onto `Surface` — the surface-kind policy ADR-262 forbids until a
/// consumer asks for it.
pub enum ShellAction {
    /// Something changed that no input reported. The ordinary "did anything
    /// change" bool covers the common case; this is for a surface that
    /// discovers it during a drain.
    Redraw,

    /// Something only the window itself can do — drag, minimise, maximise, pin,
    /// close. Reuses `koru-window`'s own vocabulary rather than restating it,
    /// which is what keeps the two runners honouring one list.
    ///
    /// **Except `Drag`.** `drag_window` only works inside the press that asked
    /// for it, and the drain runs after the dispatch — so the shell answers a
    /// press on the caption band from `is_caption` before dispatch, and a
    /// `Drag` recorded here would arrive too late to do anything. SP-0's event
    /// matrix records the same caveat.
    Window(WindowRequest),

    /// Open a tab in this window, showing `surface`.
    ///
    /// Hosts that hold one surface per window (Koru) never send this. Hosts
    /// whose tabs *are* surfaces (the Terminal) send it for every launch.
    ///
    /// `closable` is the host's, and it is not cosmetic: Plan 7.3's parity
    /// checkpoint hosts **one fixed, unclosable session tab**, and a ✕ that
    /// drew but could not be pressed would be a control that lies.
    OpenTab {
        title: String,
        closable: bool,
        surface: Box<dyn Surface>,
    },

    SelectTab(TabKey),

    /// Close a tab. The never-empty floor is the shell's: closing the last one
    /// mints a replacement through [`crate::host::ShellHost::floor_surface`].
    CloseTab(TabKey),

    /// The tab's label changed — a shell title, a working directory, an agent
    /// naming itself. Separate from the surface's own drawing because the strip
    /// is chrome and the surface may not be visible.
    SetTabTitle {
        tab: TabKey,
        title: String,
    },

    /// Ask for a **new chromed window**.
    ///
    /// Carries no reason and no payload. That is the whole of two-phase
    /// pop-out at this boundary: the shell asks the runner for the window, and
    /// only once the window and its render surface exist — which is
    /// `MultiWindowApp::seated`, the runner's own commit point — does it ask
    /// the host to seat it ([`ShellHost::seat_window`]). If the host refuses,
    /// it is told ([`ShellHost::window_open_failed`]) and rolls back with the
    /// source intact. The host already knows *why* it asked — it queued that
    /// itself — so the shell carrying an intent would be the shell learning a
    /// meaning.
    ///
    /// [`ShellHost::seat_window`]: crate::host::ShellHost::seat_window
    /// [`ShellHost::window_open_failed`]: crate::host::ShellHost::window_open_failed
    OpenWindow,

    /// Move `tab` into a **new** window. Two-phase, exactly as `OpenWindow`:
    /// the tab is not removed from this window until the destination exists.
    PopOutTab(TabKey),

    /// **Toggle one of the two side columns**, exactly as pressing its caption
    /// toggle does.
    ///
    /// The chord's path back in. A host resolves `Ctrl+Shift+E` at its own
    /// window layer — a shortcut is the host's vocabulary, and the shell owns no
    /// registry — but the pane it names is the shell's, and reaching in during a
    /// key dispatch is what the drain exists to forbid. So the host records this
    /// and the shell performs it after the dispatch, through the *same*
    /// `toggle_pane_with` the toggle button reaches: one path, so the chord and
    /// the button cannot drift apart about reflowing the surface or re-walking
    /// the ladder.
    TogglePane(crate::state::Side),

    /// **Set the display zoom — every window in the process, not this one.**
    ///
    /// The same shape as `TogglePane`, and for the same reason: the chord is
    /// the host's vocabulary (`Ctrl+=`, `Ctrl+-`, `Ctrl+0`, or a Settings row
    /// offering 125%) and the shell owns no registry, but what a zoom *is* — the
    /// layout viewport divided by it and the root transform multiplied by it —
    /// is the shell's, and reaching in during a key dispatch is what the drain
    /// exists to forbid.
    ///
    /// **The window this command names is ignored**, because a zoom is a
    /// preference about the person: pressing `Ctrl+=` in one window and
    /// leaving the other at 100% would be two windows disagreeing about how
    /// big text is. The key is still carried because every command carries one.
    ///
    /// Clamped to [`ZOOM_MIN`](crate::ZOOM_MIN)..=[`ZOOM_MAX`](crate::ZOOM_MAX)
    /// on arrival. A host stepping in and out snaps to the grid first with
    /// [`stepped_zoom`](crate::stepped_zoom); a host offering a chosen value
    /// sends it as it is.
    Zoom(f32),

    /// Open the popover anchored under a banner slot, or close whatever is
    /// open. The **rows are the host's** — the shell owns the panel, the
    /// anchor, the clamp and the dismiss, and renders whatever surface the host
    /// hands it into the space (DESIGN, "The banner" §8).
    Popover(Option<crate::host::BannerSlot>),

    /// **The open popover now shows a different view of itself**, arriving from
    /// the named edge.
    ///
    /// Not a second `Popover(Some(slot))`: re-opening throws the panel away and
    /// places a new one, which is right when the *list* changed under a panel
    /// that is otherwise the same, and wrong when one view is replacing another
    /// — the panel is the thing that stays while its contents travel. The shell
    /// asks [`ShellHost::popover_size`] again (the host has already switched)
    /// and slides the panel to the new height while the host draws both views.
    ///
    /// Ignored when nothing is open, because a host that swapped a view of a
    /// panel that has since been dismissed has nothing to animate.
    ///
    /// [`ShellHost::popover_size`]: crate::host::ShellHost::popover_size
    PopoverSlide(crate::popover::SlideFrom),

    /// **Something the surface wants its host to do, named by a number the host
    /// minted.** Delivered to
    /// [`ShellHost::host_request`](crate::host::ShellHost::host_request).
    ///
    /// The one thing the boundary was missing and could not do without: a
    /// surface reaches `Commands`, a host reaches `HostCx`, and until now
    /// nothing went from the first to the second. A Welcome tab pressing
    /// *launch a shell here* is the case — it is not a tab operation, not a
    /// window request, and not something the shell may understand.
    ///
    /// **A `u64`, not `Box<dyn Any>`.** Downcasting is what the minimal
    /// contract forbids and what a surface *kind* would smuggle back in; an
    /// opaque number teaches the shell nothing, and the host — which minted it —
    /// is the only thing that can read it. A host that needs a payload keeps it
    /// beside the number, which it can do because the surface is its own.
    HostRequest(u64),
}

/// An action, and the window it happens in.
pub struct ShellCommand {
    pub window: WindowKey,
    pub action: ShellAction,
}

/// A recording sink already bound to one window.
///
/// Borrowed for the length of one dispatch and drained immediately after, so a
/// command can never outlive the event that produced it.
pub struct Commands {
    window: WindowKey,
    queued: Vec<ShellCommand>,
}

impl Commands {
    /// Public so a host-side adapter can drive a surface directly in a test
    /// without a live window, which is what both conformance suites do.
    pub fn new(window: WindowKey) -> Self {
        Self {
            window,
            queued: Vec::new(),
        }
    }

    /// The window this sink is bound to. A surface may read it — to quote it
    /// back in its own bookkeeping — and cannot change it.
    pub fn window(&self) -> WindowKey {
        self.window
    }

    pub fn push(&mut self, action: ShellAction) {
        self.queued.push(ShellCommand {
            window: self.window,
            action,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    pub(crate) fn drain(&mut self) -> Vec<ShellCommand> {
        std::mem::take(&mut self.queued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Minter;

    /// The rule the type enforces: a surface holds a stamped sink, so it cannot
    /// record a command for a window other than its own.
    #[test]
    fn every_command_a_surface_records_names_its_own_window() {
        let mut minter = Minter::default();
        let window = minter.window();
        let other = minter.window();
        let mut commands = Commands::new(window);
        commands.push(ShellAction::Redraw);
        commands.push(ShellAction::OpenWindow);
        let drained = commands.drain();
        assert_eq!(drained.len(), 2);
        assert!(drained.iter().all(|command| command.window == window));
        assert!(drained.iter().all(|command| command.window != other));
    }

    #[test]
    fn a_drained_sink_is_empty_and_reusable() {
        let mut minter = Minter::default();
        let mut commands = Commands::new(minter.window());
        assert!(commands.is_empty());
        commands.push(ShellAction::Redraw);
        assert!(!commands.is_empty());
        assert_eq!(commands.drain().len(), 1);
        assert!(commands.is_empty(), "a command cannot be performed twice");
    }
}
