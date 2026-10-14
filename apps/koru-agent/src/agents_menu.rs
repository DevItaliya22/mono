// SPDX-License-Identifier: MPL-2.0

//! **Where the tray's Agents menu gets its rows: the session host itself.**
//!
//! The menu used to be a projection *forwarded* by Koru Terminal windows —
//! each window pushed its session list over the Agent IPC and the tray showed
//! the union. Sessions live in Koru Workspace as ordinary tabs now (Q-E8 P9),
//! and the Agent already supervises the host that owns them, so the menu reads
//! the catalog at its source instead of trusting whichever windows happen to
//! be open: the sessions (each with its profile and placement) and the project
//! registry they are grouped by.
//!
//! ## A watcher thread, because the tray cannot ask at the right moment
//!
//! `tray-icon` offers no "menu is about to open" hook — the same constraint
//! every other tray row lives under — and the winit loop runs under
//! `ControlFlow::Wait` with no clock, so neither lazy reads nor polling are
//! available. What is left is what the rest of the tray does: hold the current
//! answer and push it on change. This module owns the holding half — a thread
//! that keeps one client connection to the host and re-reads the catalog when
//! something says it moved:
//!
//! - the host's **project catalog subscription** (`ProjectsChanged`), which
//!   fires on every project create/rename/delete/move *and* on a session being
//!   assigned to one;
//! - a **per-session event subscription** for every session it knows, which
//!   fires on lifecycle changes — a session ending is what most often changes
//!   this menu;
//! - a **kick from the Agent's event loop** (`Waker`-adjacent, but inbound):
//!   raised on the window events the Agent can observe — a Workspace window
//!   arriving, focusing, or leaving, and the tray's own Agents clicks.
//!
//! **The gap, stated plainly:** the host publishes no session-catalog
//! invalidation, so a session *created* while nothing else moves is not seen
//! until the next kick (the person focusing a window, or any project or
//! session event). Adding a sessions-changed arm to the host's catalog stream
//! would close it and is a `koru-session-wire` change of its own.
//!
//! Every read happens on this thread; nothing here ever runs on the winit
//! thread. The fresh catalog is delivered as [`AgentEvent::AgentsCatalog`] and
//! the loop only stores it. Connection loss is ordinary — the host is a
//! supervised child that restarts under updates and crashes — so the thread
//! reconnects quietly and the menu keeps its last answer until the host
//! answers again.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use koru_session_client::{ClientError, SessionClient, Subscription};
use koru_session_protocol::{ProjectSummary, SessionId, SessionSummary};
use koru_session_wire::SubscriptionUpdate;

use crate::event::{AgentEvent, Waker};

/// Everything the Agents menu is drawn from, read together off one
/// connection so the groups and the rows cannot come from two moments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentsCatalog {
    pub sessions: Vec<SessionSummary>,
    pub projects: Vec<ProjectSummary>,
}

/// How often to retry a connect while the host is away. The host is this
/// Agent's own supervised child, so "away" is a restart window measured in
/// milliseconds, not a service that may never come back.
const RECONNECT_EVERY: Duration = Duration::from_millis(500);

/// Start the watcher. The returned sender is the Agent's way to say "the
/// catalog may have moved — read it again"; `try_send` semantics, one pending
/// kick at most, because two kicks owe the same single re-read.
pub fn watch(endpoint: koru_ipc::Endpoint, waker: Waker) -> SyncSender<()> {
    let (kick, kicks) = std::sync::mpsc::sync_channel::<()>(1);
    let readers = kick.clone();
    let _ = std::thread::Builder::new()
        .name("koru-agent agents menu".into())
        .stack_size(192 * 1024)
        .spawn(move || run(&endpoint, &waker, &readers, &kicks));
    kick
}

/// Nudge the watcher. A full queue is a refresh already owed, and a
/// disconnected one is a watcher that is gone — neither is an error.
pub fn nudge(kick: &SyncSender<()>) {
    let _ = kick.try_send(());
}

fn run(endpoint: &koru_ipc::Endpoint, waker: &Waker, kick: &SyncSender<()>, kicks: &Receiver<()>) {
    loop {
        // Connect, retrying quietly — see `RECONNECT_EVERY`. Kicks that
        // arrive while there is nothing to read are consumed here: they asked
        // for a fresh catalog, and the connect's first publish is one.
        let client = loop {
            match SessionClient::connect(endpoint, "koru-agent-menu", offer()) {
                Ok(client) => break Arc::new(client),
                Err(_) => match kicks.recv_timeout(RECONNECT_EVERY) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    // Every sender is gone: the Agent dropped its kick and no
                    // reader threads survive a client that never connected.
                    Err(RecvTimeoutError::Disconnected) => return,
                },
            }
        };
        // The catalog subscription, taken *before* the first read so a
        // project mutation between the two is not the one this menu misses —
        // the order `koru-session-backend` keeps for the same reason.
        if let Ok(subscription) = client.subscribe_projects() {
            spawn_projects_reader(subscription, kick.clone());
        }
        let mut subscribed = BTreeSet::new();
        if publish(&client, waker, kick, &mut subscribed).is_err() {
            continue;
        }
        loop {
            match kicks.recv() {
                // A re-read that fails is a dead connection, not a catalog
                // answer: reconnect. The menu keeps its last rows meanwhile.
                Ok(()) => {
                    if publish(&client, waker, kick, &mut subscribed).is_err() {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
    }
}

/// Read the whole catalog, subscribe to any session not yet watched, and hand
/// the answer to the loop.
fn publish(
    client: &Arc<SessionClient>,
    waker: &Waker,
    kick: &SyncSender<()>,
    subscribed: &mut BTreeSet<SessionId>,
) -> Result<(), ClientError> {
    let sessions = client.list()?;
    // A host that has not negotiated `projects` refuses the list, which is
    // not a failure: it has no projects, so every session is Unassigned. A
    // transport error is a different thing and propagates.
    let projects = match client.list_projects() {
        Ok(projects) => projects,
        Err(ClientError::Session(_)) => Vec::new(),
        Err(error) => return Err(error),
    };
    for session in &sessions {
        if subscribed.insert(session.id)
            && let Ok(subscription) = client.subscribe_events_from(session.id, None)
        {
            spawn_session_reader(Arc::clone(client), session.id, subscription, kick.clone());
        }
    }
    waker.wake(AgentEvent::AgentsCatalog {
        catalog: Box::new(AgentsCatalog { sessions, projects }),
    });
    Ok(())
}

/// Forward every `ProjectsChanged` as a kick. The event is invalidation and
/// carries nothing (`CatalogEvent` has one arm, deliberately), so the only
/// thing a holder can do with one is ask again — which is what a kick is.
fn spawn_projects_reader(subscription: Subscription, kick: SyncSender<()>) {
    let _ = std::thread::Builder::new()
        .name("koru-agent agents menu projects".into())
        .stack_size(96 * 1024)
        .spawn(move || {
            loop {
                match subscription.recv() {
                    Ok(SubscriptionUpdate::Catalog { .. }) => nudge(&kick),
                    Ok(SubscriptionUpdate::Closed { .. }) | Err(_) => {
                        // The stream or the transport ended. The kick makes
                        // the watcher's next read fail visibly, which is what
                        // sends it to reconnect.
                        nudge(&kick);
                        return;
                    }
                    Ok(_) => {}
                }
            }
        });
}

/// Forward one session's lifecycle events as kicks. Output is filtered — a
/// menu does not change because a shell printed a line — and a gapped stream
/// is resubscribed from the sequence the gap names, the recovery
/// `koru-session-backend` uses for the same host behaviour: a gapped
/// subscriber is never un-marked, so continuing to listen hears nothing.
fn spawn_session_reader(
    client: Arc<SessionClient>,
    session: SessionId,
    subscription: Subscription,
    kick: SyncSender<()>,
) {
    let _ = std::thread::Builder::new()
        .name("koru-agent agents menu session".into())
        .stack_size(96 * 1024)
        .spawn(move || {
            let mut subscription = subscription;
            loop {
                match subscription.recv() {
                    Ok(SubscriptionUpdate::Event { event }) => {
                        if !matches!(
                            event.event,
                            koru_session_protocol::SessionEvent::Output { .. }
                        ) {
                            nudge(&kick);
                        }
                    }
                    Ok(SubscriptionUpdate::EventGap { actual, .. }) => {
                        nudge(&kick);
                        match client.subscribe_events_from(session, Some(actual)) {
                            Ok(next) => subscription = next,
                            Err(_) => return,
                        }
                    }
                    // `Closed` is the host ending this stream — the session is
                    // gone, which is exactly a menu change. A dead transport
                    // reads the same here; the re-read sorts the two out.
                    Ok(SubscriptionUpdate::Closed { .. }) | Err(_) => {
                        nudge(&kick);
                        return;
                    }
                    Ok(_) => {}
                }
            }
        });
}

/// What this client can do: read and subscribe, nothing else. No controller
/// lease, no writes, no terminal bytes — the menu is a reader.
fn offer() -> koru_session_protocol::ProtocolOffer {
    use koru_session_protocol::{
        CapabilityName, CapabilitySet, MAX_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION, ProtocolRange,
        capability,
    };
    use std::str::FromStr;

    let capabilities = [
        // The project registry: `ListProjects` and the catalog subscription.
        capability::PROJECTS,
        // The cursor form of the event subscription, so a gapped status
        // stream can be resumed instead of going permanently silent.
        capability::OBSERVATION_CURSORS,
    ]
    .into_iter()
    .map(|name| CapabilityName::from_str(name).expect("built-in capability"));
    koru_session_protocol::ProtocolOffer {
        versions: ProtocolRange {
            min: MIN_PROTOCOL_VERSION,
            max: MAX_PROTOCOL_VERSION,
        },
        capabilities: CapabilitySet::new(capabilities),
    }
}
