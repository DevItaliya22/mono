// SPDX-License-Identifier: MPL-2.0

//! **When a window has a native accessibility provider, and for how long.**
//!
//! One sentence, and it excludes almost everything: *this module owns the
//! lifetime of the AccessKit adapter against one winit window — created before
//! the window is first shown, told about every window event before the
//! application handles it, drained on the loop thread, and dropped when the
//! window goes.* It names no node, no document, no tab, no role and no label,
//! it never decides what an action means, and it never builds a tree. Those are
//! knowledge about what a window *contains*, and they live with their own
//! subject in `koru_workspace_host::accessibility`.
//!
//! It is here for the same reason the rest of this crate is here: the ordering
//! rules below are platform traps, each one is a real bug if it is got wrong,
//! and a second copy is a place the next fix would not be applied.
//!
//! ## The three traps, verified against `accesskit_winit` 0.32.2
//!
//! - **The adapter must exist before the first reveal.**
//!   `Adapter::with_direct_handlers` *panics* when the window is already
//!   visible. Koru creates every window hidden and reveals it after the first
//!   frame, so the install site is the moment after `create_window`; a reveal
//!   path that ran first would take the process down.
//!   [`may_install`] is that check, kept separate so it can be asserted
//!   without a window.
//! - **Window events go to the adapter before the application sees them.**
//!   `Adapter::process_event` is documented that way. On Windows it is
//!   currently a no-op — the provider is a `WM_GETOBJECT` subclass on the
//!   HWND, so it never needs winit's view of an event — but macOS and the
//!   Unix adapter do use it, and the contract is the adapter's, not ours.
//! - **Activation is late and asynchronous.** A screen reader attaching sends
//!   `WM_GETOBJECT` from the window procedure, which can happen at any moment,
//!   including while the loop is idle. Answering it synchronously would mean
//!   building a semantic tree on whatever thread the platform chose, from
//!   state the loop owns. So the handlers here only *record* the request and
//!   wake the loop; the loop drains [`take_pending`](WindowBridge::take_pending)
//!   and answers with a coherent full snapshot. The Windows adapter serves a
//!   placeholder tree in the meantime and swaps it for the first full update —
//!   that is its documented behaviour for a handler that returns `None`, not a
//!   workaround.
//!
//! ## Nothing happens while no screen reader is attached
//!
//! [`Provider::publish`] takes a closure and hands it to `update_if_active`,
//! which does not call it unless a client has actually asked for the tree — and
//! it returns whether the closure ran, so the caller's accounting is the
//! adapter's answer rather than a guess. The cost of this module in an ordinary
//! session is one `Option` and one branch;
//! `koru_workspace_host::accessibility::Counters::rebuilds` is the number that
//! says so rather than an impression.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

/// How many action requests may be waiting for the loop before the oldest are
/// dropped.
///
/// A bound rather than a `Vec`, because the producer is an out-of-process
/// automation client and the consumer is the thread that answers a person. A
/// client that queues faster than the loop drains must not be able to grow this
/// process's memory; [`Pending::dropped_actions`] says when that happened.
const MAX_QUEUED_ACTIONS: usize = 256;

/// Whether an adapter may be installed on a window in this state.
///
/// `Some(true)` is the one refusal: `accesskit_winit` panics on an already
/// visible window. `None` is a platform that will not answer, and is treated as
/// permission — the adapter's own check runs a moment later and is the
/// authority.
pub(crate) fn may_install(visible: Option<bool>) -> bool {
    visible != Some(true)
}

/// What a screen reader asked of one window, waiting for the loop thread.
#[derive(Debug, Default)]
pub struct Pending {
    /// A client attached and wants the whole tree. Coalesced: a hundred
    /// attaches between two drains are still one full snapshot.
    pub initial_tree: bool,
    /// The platform said no client needs the tree any more.
    pub deactivated: bool,
    /// Actions to resolve, oldest first.
    pub actions: Vec<ActionRequest>,
    /// Actions dropped because the queue was full. News, not routine.
    pub dropped_actions: u64,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        !self.initial_tree && !self.deactivated && self.actions.is_empty()
    }
}

/// The mailbox the platform's handlers write into and the loop drains.
///
/// A `Mutex` around a small queue: every handler holds it for the length of a
/// push, and the loop for the length of a drain. Nothing here waits on the
/// world.
#[derive(Default)]
struct Inbox {
    initial_tree: AtomicBool,
    deactivated: AtomicBool,
    dropped: AtomicU64,
    actions: Mutex<VecDeque<ActionRequest>>,
}

impl Inbox {
    fn push_action(&self, request: ActionRequest) {
        let Ok(mut queue) = self.actions.lock() else {
            return;
        };
        if queue.len() >= MAX_QUEUED_ACTIONS {
            queue.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        queue.push_back(request);
    }

    fn drain(&self) -> Pending {
        let actions = self
            .actions
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default();
        Pending {
            initial_tree: self.initial_tree.swap(false, Ordering::AcqRel),
            deactivated: self.deactivated.swap(false, Ordering::AcqRel),
            actions,
            dropped_actions: self.dropped.swap(0, Ordering::Relaxed),
        }
    }
}

/// Wake the event loop. `Arc` rather than a proxy, because this crate must not
/// know what a host's user event is called.
type Wake = Arc<dyn Fn() + Send + Sync>;

struct Handlers {
    inbox: Arc<Inbox>,
    wake: Wake,
}

impl Handlers {
    fn note(&self, flag: &AtomicBool) {
        flag.store(true, Ordering::Release);
        (self.wake)();
    }
}

impl ActivationHandler for Handlers {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        // **Deliberately `None`.** The tree is the loop's to build, from state
        // the loop owns, and this runs on whichever thread the platform chose.
        // The adapter serves its own placeholder until the loop answers.
        self.note(&self.inbox.initial_tree);
        None
    }
}

impl ActionHandler for Handlers {
    fn do_action(&mut self, request: ActionRequest) {
        self.inbox.push_action(request);
        (self.wake)();
    }
}

impl DeactivationHandler for Handlers {
    fn deactivate_accessibility(&mut self) {
        self.note(&self.inbox.deactivated);
    }
}

/// What the discipline asks of a native provider.
///
/// The seam exists for the reason [`WindowHandle`](crate::dispatch) does: the
/// real implementation needs a live event loop, a real window and, on Windows,
/// a screen reader attached, so without it the rules above the provider — when
/// a tree is worth building, which requests are still about something that
/// exists — could only be exercised by hand on a desktop.
pub trait Provider {
    /// The adapter sees a window event before the application does.
    fn process_event(&mut self, window: &Window, event: &WindowEvent);
    /// Everything a client has asked for since the last drain.
    fn take_pending(&mut self) -> Pending;
    /// Build and publish a tree, but only if a client is listening. Returns
    /// whether `build` was called.
    fn publish(&mut self, build: &mut dyn FnMut() -> TreeUpdate) -> bool;
}

/// One window's native accessibility provider.
///
/// Dropping it is how a window's accessibility state is retired: the platform
/// adapter goes with it, and any request that was still queued for it dies
/// with the window rather than being handed to whichever window happens to
/// still be open.
pub struct WindowBridge {
    platform: platform::Adapter,
    inbox: Arc<Inbox>,
}

impl WindowBridge {
    /// Install a provider on a window that has not been shown yet.
    ///
    /// `None` when the window is already visible — see [`may_install`]. The
    /// caller has lost nothing it can recover: the provider for that window
    /// cannot be installed later, so this is reported once and loudly rather
    /// than retried.
    pub fn before_first_reveal(
        events: &ActiveEventLoop,
        window: &Window,
        wake: Wake,
    ) -> Option<Self> {
        if !may_install(window.is_visible()) {
            tracing::error!(
                target: "koru::accessibility",
                "no accessibility provider for this window: it was already visible when the \
                 bridge was asked for, and the adapter can only be installed on a hidden window"
            );
            return None;
        }
        let inbox = Arc::new(Inbox::default());
        let platform = platform::Adapter::install(
            events,
            window,
            Handlers {
                inbox: Arc::clone(&inbox),
                wake: Arc::clone(&wake),
            },
            Handlers {
                inbox: Arc::clone(&inbox),
                wake: Arc::clone(&wake),
            },
            Handlers {
                inbox: Arc::clone(&inbox),
                wake,
            },
        );
        Some(Self { platform, inbox })
    }
}

impl Provider for WindowBridge {
    fn process_event(&mut self, window: &Window, event: &WindowEvent) {
        self.platform.process_event(window, event);
    }

    fn take_pending(&mut self) -> Pending {
        self.inbox.drain()
    }

    /// `build` is not called when no screen reader has attached, which is what
    /// makes an ordinary session cost nothing.
    fn publish(&mut self, build: &mut dyn FnMut() -> TreeUpdate) -> bool {
        let mut built = false;
        self.platform.update_if_active(|| {
            built = true;
            build()
        });
        built
    }
}

/// The platform adapter, or an inert stand-in.
///
/// **Windows only, deliberately** (ADR-336). The maintainers scoped this roadmap to
/// Windows; `accesskit_winit` would also give macOS a real adapter for free,
/// and the one-line change that does so is written down in
/// `docs/features/macos/CATCHUP.md` rather than taken here, because nobody on this
/// branch can build or verify it. Everything above this line is
/// platform-neutral, so catching up is filling in an implementation rather
/// than redesigning.
#[cfg(windows)]
mod platform {
    use accesskit::{ActionHandler, ActivationHandler, DeactivationHandler, TreeUpdate};
    use winit::event::WindowEvent;
    use winit::event_loop::ActiveEventLoop;
    use winit::window::Window;

    pub(super) struct Adapter(accesskit_winit::Adapter);

    impl Adapter {
        pub(super) fn install(
            events: &ActiveEventLoop,
            window: &Window,
            activation: impl 'static + ActivationHandler + Send,
            action: impl 'static + ActionHandler + Send,
            deactivation: impl 'static + DeactivationHandler + Send,
        ) -> Self {
            Self(accesskit_winit::Adapter::with_direct_handlers(
                events,
                window,
                activation,
                action,
                deactivation,
            ))
        }

        pub(super) fn process_event(&mut self, window: &Window, event: &WindowEvent) {
            self.0.process_event(window, event);
        }

        pub(super) fn update_if_active(&mut self, build: impl FnOnce() -> TreeUpdate) {
            self.0.update_if_active(build);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use accesskit::{ActionHandler, ActivationHandler, DeactivationHandler, TreeUpdate};
    use winit::event::WindowEvent;
    use winit::event_loop::ActiveEventLoop;
    use winit::window::Window;

    /// No provider, and therefore no client: `update_if_active` never calls its
    /// closure, exactly as the real adapter does not before a screen reader
    /// attaches.
    pub(super) struct Adapter;

    impl Adapter {
        pub(super) fn install(
            _events: &ActiveEventLoop,
            _window: &Window,
            _activation: impl 'static + ActivationHandler + Send,
            _action: impl 'static + ActionHandler + Send,
            _deactivation: impl 'static + DeactivationHandler + Send,
        ) -> Self {
            Self
        }

        pub(super) fn process_event(&mut self, _window: &Window, _event: &WindowEvent) {}

        pub(super) fn update_if_active(&mut self, _build: impl FnOnce() -> TreeUpdate) {}
    }
}

#[cfg(test)]
mod tests;
