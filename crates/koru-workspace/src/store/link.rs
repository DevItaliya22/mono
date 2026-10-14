// SPDX-License-Identifier: MPL-2.0

//! The transport under a [`StoreSession`], behind a trait.
//!
//! [`StoreSession`]: super::StoreSession
//!
//! A session owns the *bookkeeping* — what this replica has handed over, what
//! the store has confirmed, what is still owed. A link owns the *socket*, and
//! knows nothing about version vectors, documents or durability. It moves bytes
//! and reports what the wire said.
//!
//! ## Why a trait, when there is one implementation
//!
//! Two reasons, and the first is the one that pays immediately.
//!
//! **It makes the bookkeeping testable without a store process.** Everything
//! interesting about a sync session is what it does when the answer is *not*
//! "fine": an acknowledgement that refuses, a connection that dies with edits in
//! flight, a room that asks to be rejoined. None of those can be provoked
//! reliably against a real store, which is why none of them was tested and why
//! [three of them were wrong](../../../../docs/architecture/WORKSPACE.md). A fake link
//! injects any of them in a line.
//!
//! **And it let the socket move off the event loop without a call site
//! moving.** [`ThreadLink`] replaced the synchronous one underneath
//! `StoreSession` rather than beside it, and nothing above changed.

use koru_api_client::Event;
use koru_store_core::errors::Result;
use loro_protocol::BatchId;

/// Severity of a transport transition reported to an executable-owned logger.
///
/// The workspace crate deliberately owns no logging framework. An executable
/// may install a sink after it initializes its logger; tests and smaller tools
/// keep the stdout fallback instead.
#[derive(Clone, Copy, Debug)]
pub enum DiagnosticLevel {
    Info,
    Warn,
}

type DiagnosticSink = fn(DiagnosticLevel, &str);

static DIAGNOSTIC_SINK: std::sync::OnceLock<DiagnosticSink> = std::sync::OnceLock::new();

/// Send Store-link transitions through the embedding executable's logger.
///
/// The first executable-level sink wins. A process has one logger, so replacing
/// it later would make a background socket write through a lifetime it cannot
/// know anything about.
pub fn set_diagnostic_sink(sink: DiagnosticSink) -> bool {
    DIAGNOSTIC_SINK.set(sink).is_ok()
}

pub(super) fn diagnostic(level: DiagnosticLevel, message: &str) {
    if let Some(sink) = DIAGNOSTIC_SINK.get() {
        sink(level, message);
        return;
    }

    println!("{message}");
    // A supervised GUI process may keep stdout behind a pipe. A transition is
    // useful only if it appears before the process exits or the next retry.
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

/// Something the store said, or something that happened to the connection
/// carrying it.
///
/// An enum rather than a bare [`Event`] because the transport has things to
/// report that the *protocol* does not, and a session has to answer them
/// differently: an [`Event`] is news about a document, while [`Lost`] and
/// [`Ready`] are news about whether anything at all is getting through.
///
/// [`Lost`]: Inbound::Lost
/// [`Ready`]: Inbound::Ready
pub enum Inbound {
    Event(Event),
    /// The socket died. **Everything sent and not acknowledged may or may not
    /// have landed**, which is why a session answers this by rewinding to what
    /// it knows was committed rather than by assuming either way.
    Lost {
        why: String,
    },
    /// A new socket is up. It has joined nothing — a fresh connection is a fresh
    /// membership, so the rooms have to be asked for again.
    Ready,
}

/// Whether a link is getting through.
#[derive(Clone, Debug)]
pub enum LinkState {
    Live,
    /// The socket died and a new one is being sought. Ordinary — a store
    /// restarting looks exactly like this — so the *duration* is what decides
    /// whether it is worth telling anybody about.
    Reconnecting {
        since: std::time::Instant,
        why: String,
    },
    /// It will not come back on its own.
    Stopped {
        why: String,
    },
}

impl LinkState {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }
}

/// How a socket thread tells the event loop there is something to collect.
///
/// **A closure, so `koru-workspace` never learns about winit.** The shell
/// builds one over an `EventLoopProxy`; a test builds one over an
/// `AtomicBool`. Nothing here can tell the difference, which is the point.
///
/// **Edge-triggered.** A thousand frames arriving between two turns must post
/// one wake-up, not a thousand — the loop is going to drain all of them anyway,
/// and a queue of redundant events is its own kind of busy loop. `wake` fires
/// only on the transition from *settled* to *pending*, and [`rearm`](Self::rearm)
/// is what the UI thread calls to settle it again.
///
/// **`rearm` before the drain, never after.** A frame that arrives *during* a
/// drain must still wake the loop; rearming afterwards would swallow it and the
/// window would sit on a change until something else happened to wake it.
#[derive(Clone)]
pub struct Waker(std::sync::Arc<WakerInner>);

struct WakerInner {
    pending: std::sync::atomic::AtomicBool,
    fire: Box<dyn Fn() + Send + Sync>,
}

impl Waker {
    pub fn new(fire: impl Fn() + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(WakerInner {
            pending: std::sync::atomic::AtomicBool::new(false),
            fire: Box::new(fire),
        }))
    }

    /// There is something to collect. Costs one atomic swap when a wake-up is
    /// already outstanding.
    pub fn wake(&self) {
        use std::sync::atomic::Ordering;
        if !self.0.pending.swap(true, Ordering::AcqRel) {
            (self.0.fire)();
        }
    }

    /// The loop is about to look. Anything that arrives from here on has to
    /// wake it again.
    pub fn rearm(&self) {
        self.0
            .pending
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

/// A joined room, and the bytes going both ways over it.
///
/// **One link, one room, today.** The protocol does not require that —
/// `Connection`'s own documentation says one connection carries any number of
/// rooms, and every message names its own — but `StoreSession` opens one per
/// document, so nothing here needs to say which room it means. When that is
/// collapsed, the room becomes a parameter and this comment becomes wrong.
pub trait Link: Send + Sync {
    fn join(&self, room: &str, version: &loro::VersionVector) -> Result<()>;

    /// Hand over one update. The `BatchId` is what an acknowledgement will name.
    fn send_update(&self, room: &str, bytes: Vec<u8>) -> Result<BatchId>;

    fn leave(&self, room: &str) -> Result<()>;

    /// Take up to `max` events.
    ///
    /// **Bounded by the caller, because the caller is the one with a frame to
    /// keep.** A busy room must not be able to hold the event loop; whatever is
    /// left is still there next time.
    ///
    /// **An implementation must [`rearm`](Waker::rearm) its waker here, before
    /// it looks.** Anything arriving from this moment has to wake the loop
    /// again; rearming afterwards swallows whatever landed during the drain, and
    /// the window sits on it until something unrelated stirs it.
    fn take(&self, max: usize) -> Result<Vec<Inbound>>;

    /// Whether anything is getting through.
    ///
    /// Separate from what [`take`](Link::take) reports because the two answer
    /// different questions: `take` says what has *happened* since last time, and
    /// this says where things stand now. A window that has been reconnecting for
    /// a minute has nothing new to report and a great deal to say.
    fn state(&self) -> LinkState;

    /// Tell this link how to wake whoever will call [`take`](Link::take).
    ///
    /// Installed after construction rather than passed in, because a session is
    /// built by `StoreSession::join` — which knows nothing about event loops —
    /// and the waker belongs to the window. Calling it twice replaces the first;
    /// a link with none simply never wakes anybody, which is what a test wants.
    fn set_waker(&self, waker: Waker);
}

// **A `DirectLink` was here** — the socket read by whoever called, which is what
// the code did before the trait existed. It went with `POLL_READ`, because
// keeping a second implementation whose only property is *blocks the event
// loop* is keeping the bug next to the fix. `FakeLink` covers what tests need,
// and git has the rest.

mod thread;
pub use thread::ThreadLink;

#[cfg(test)]
pub(crate) mod fake;
