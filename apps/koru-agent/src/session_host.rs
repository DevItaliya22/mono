// SPDX-License-Identifier: MPL-2.0

//! Supervision of the private terminal-session host.
//!
//! The host is a worker child of this same executable —
//! `koru-agent.exe --worker sessions` — spawned and watched through
//! `koru-agent-worker`. The Agent owns launch identity and final shutdown
//! authority. It never reads, writes, proxies, or logs terminal traffic;
//! clients dial the host endpoint directly through `koru-session-client`.
//!
//! Three secrets, three lifetimes:
//!
//! - **`launch_id`, per child** — identity; readiness proves the endpoint
//!   answers as this launch before anything trusts it.
//! - **The stop authority, per child** — stops exactly that child. Nobody
//!   captures it at startup: every lifecycle operation resolves the current
//!   proven child at the moment of use, and a multi-step flow (an update's
//!   prepare … cancel) is fenced by the child generation it began against,
//!   so an operation aimed at a crashed child is refused visibly rather
//!   than routed to its replacement through their shared endpoint.
//! - **The workspace-lifecycle (cleanup) authority, per Agent run** — the
//!   narrow secret the Gateway holds so a workspace's removal can close its
//!   terminals. Every restarted child accepts the same one, so the
//!   Gateway's copy never goes stale and no restart notification exists.
//!   It must not be the authority that stops the host, because the Gateway
//!   would then be able to end every session on the machine.
//!
//! A crash destroys the child's live PTYs. The worker machinery restarts
//! the host within its budget; nothing here pretends the sessions
//! survived — clients reconnect to an empty catalog and present the truth.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use koru_agent_worker::machine::State;
use koru_agent_worker::supervisor::{
    self as worker, Generation, LaunchPlan, StopOutcome, WorkerEventKind, WorkerHandle, WorkerSpec,
};

use crate::Paths;

/// The per-child handles the role's closures need to reach and stop their
/// own child, and nothing else's.
struct SessionsChild {
    launch_id: String,
    stop_authority: String,
}

struct Shared {
    handle: WorkerHandle<SessionsChild>,
    endpoint: koru_ipc::Endpoint,
    /// The child generation the current update prepare was minted against —
    /// the fence a later cancel checks before speaking to the host.
    held: Mutex<Option<Generation>>,
}

pub struct Supervisor {
    shared: Arc<Shared>,
}

#[derive(Clone)]
pub struct Lifecycle {
    shared: Arc<Shared>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogFact {
    pub generation: u64,
    pub active_count: u32,
}

#[derive(Clone, Debug)]
pub enum PrepareResult {
    Prepared(CatalogFact),
    Refused {
        message: String,
        current: Option<CatalogFact>,
    },
}

impl Lifecycle {
    fn current(&self) -> Result<worker::CurrentChild<SessionsChild>, String> {
        self.shared
            .handle
            .current()
            .ok_or_else(|| "the session host is not serving".to_owned())
    }

    pub fn catalog(&self) -> Result<CatalogFact, String> {
        let catalog = connect(&self.shared.endpoint, "koru-agent-supervisor")?
            .catalog()
            .map_err(|error| error.to_string())?;
        Ok(CatalogFact {
            generation: catalog.generation.get(),
            active_count: catalog.sessions.len().try_into().unwrap_or(u32::MAX),
        })
    }

    /// Compare consent and freeze new session creation in one host
    /// operation, then hold the supervisor: a child that dies inside the
    /// update window is reported, not replaced.
    pub fn prepare(&self, observed_generation: u64) -> Result<CatalogFact, String> {
        let child = self.current()?;
        let generation = koru_session_protocol::SessionCatalogGeneration::new(observed_generation)
            .map_err(|code| code.as_str().to_owned())?;
        let catalog = connect(&self.shared.endpoint, "koru-agent-supervisor")?
            .prepare_shutdown(child.payload.stop_authority.clone(), generation)
            .map_err(|error| error.to_string())?;
        *self.shared.held.lock().expect("held generation") = Some(child.generation);
        self.shared.handle.hold();
        Ok(CatalogFact {
            generation: catalog.generation.get(),
            active_count: catalog.sessions.len().try_into().unwrap_or(u32::MAX),
        })
    }

    pub fn prepare_and_refresh(&self, observed_generation: u64) -> PrepareResult {
        match self.prepare(observed_generation) {
            Ok(fact) => PrepareResult::Prepared(fact),
            Err(message) => PrepareResult::Refused {
                message,
                current: self.catalog().ok(),
            },
        }
    }

    /// The aborted update's way back: reopen the surviving child's session
    /// admission (the authenticated `CancelShutdown`), then resume
    /// supervision — a child that died inside the window is replaced now,
    /// and a stopped service is revived. A cancellation failure is
    /// returned, never swallowed into a false "ready".
    pub fn cancel(&self) -> Result<(), String> {
        let held = self.shared.held.lock().expect("held generation").take();
        let result = match held {
            Some(generation) => match self.shared.handle.issue(generation) {
                Ok(child) => {
                    connect(&self.shared.endpoint, "koru-agent-supervisor").and_then(|client| {
                        client
                            .cancel_shutdown(child.stop_authority.clone())
                            .map_err(|error| error.to_string())
                    })
                }
                // The prepared child is gone; there is no preparation left
                // to cancel, and resume below replaces it.
                Err(stale) => {
                    tracing::info!(
                        target: "koru::session_host",
                        %stale,
                        "the prepared session host is gone; resuming with a fresh one"
                    );
                    Ok(())
                }
            },
            None => Ok(()),
        };
        self.shared.handle.resume();
        result
    }

    /// Gracefully stop every session tree and wait until the child is
    /// provably gone. The supervision stays parked and revivable — the
    /// install that asked for this can still abort, and `cancel` must be
    /// able to bring the service back. This is called only on a lifecycle
    /// worker, never winit.
    ///
    /// **The answer names what happened**, rather than being a `Result` whose
    /// `Ok` covered a graceful exit, a deadline kill and a child that had
    /// already died alike. A caller deciding whether an image may be replaced
    /// reads [`StopOutcome::child_is_gone`]; one reporting what a person
    /// should be told reads the whole outcome.
    pub fn stop(&self) -> StopOutcome {
        self.shared.handle.stop_serving()
    }
}

impl Supervisor {
    /// Start and prove the sessions worker. Blocks until the first child
    /// answers as this launch, so callers keep today's contract: a
    /// returned supervisor is a serving host.
    pub fn start(paths: &Paths) -> Result<Self, String> {
        let endpoint = crate::session_endpoint(paths).map_err(|error| error.to_string())?;
        stop_stale(&endpoint);

        // **Per Agent run, not per child.** The Agent hands this one on to
        // the supervised Gateway so a workspace's removal can close its
        // terminals across host restarts; see the module doc for why it is
        // never the stop authority.
        let cleanup_authority = uuid::Uuid::new_v4().to_string();

        let spec = sessions_spec(paths, &endpoint, cleanup_authority);
        let handle = worker::start(spec);

        // The worker machinery retries and backs off on its own; startup
        // keeps the fail-fast contract by giving the first child one
        // readiness window and refusing to start the Agent without it.
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let status = handle.status();
            match status.state {
                State::Ready => break,
                State::Exhausted => {
                    return Err(format!(
                        "the session worker could not start: {}",
                        status.last_error.unwrap_or_else(|| "unknown".into())
                    ));
                }
                _ if Instant::now() >= deadline => {
                    return Err(format!(
                        "the session worker was not ready in time: {}",
                        status.last_error.unwrap_or_else(|| "unknown".into())
                    ));
                }
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }

        Ok(Self {
            shared: Arc::new(Shared {
                handle,
                endpoint,
                held: Mutex::new(None),
            }),
        })
    }

    pub fn health(&self) -> Result<koru_session_wire::HostHealth, String> {
        connect(&self.shared.endpoint, "koru-agent-supervisor")?
            .health()
            .map_err(|error| error.to_string())
    }

    pub fn lifecycle(&self) -> Lifecycle {
        Lifecycle {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// The role's whole knowledge, spelled as the worker crate's closures.
fn sessions_spec(
    paths: &Paths,
    endpoint: &koru_ipc::Endpoint,
    cleanup_authority: String,
) -> WorkerSpec<SessionsChild> {
    let segment: String = crate::segment().into();
    let root = paths.root().to_path_buf();
    let sessions_dir = paths.session_paths().directory().to_path_buf();
    let store_dir = paths.store_dir();
    let explicit_root = paths.explicit_root().map(std::path::Path::to_path_buf);
    let launch_endpoint = endpoint.clone();
    let publish_authority = cleanup_authority.clone();

    let mut spec = WorkerSpec::new(
        "sessions",
        move |_generation: Generation| {
            // A fresh UUID per child — the host validates it parses as one,
            // and readiness proves the endpoint answers with this exact
            // value. The generation fence is the supervisor's own counter,
            // not something smuggled into the launch id.
            let launch_id = uuid::Uuid::new_v4().to_string();
            let stop_authority = uuid::Uuid::new_v4().to_string();
            let bootstrap = koru_session_wire::LaunchBootstrap::new(
                launch_id.clone(),
                segment.clone(),
                root.clone(),
                sessions_dir.clone(),
                store_dir.clone(),
                explicit_root.is_some(),
                stop_authority.clone(),
            )
            .with_lifecycle_authority(cleanup_authority.clone());
            let bootstrap = serde_json::to_vec(&bootstrap)
                .map_err(|error| format!("could not encode session bootstrap: {error}"))?;
            let mut args: Vec<std::ffi::OsString> = Vec::new();
            if let Some(root) = &explicit_root {
                args.push("--root".into());
                args.push(root.clone().into());
            }
            Ok(LaunchPlan {
                args,
                bootstrap,
                payload: SessionsChild {
                    launch_id,
                    stop_authority,
                },
            })
        },
        {
            let endpoint = launch_endpoint.clone();
            move |child: &SessionsChild| {
                let health = connect(&endpoint, "koru-agent-supervisor")?
                    .health()
                    .map_err(|error| error.to_string())?;
                if health.supervisor_launch_id != child.launch_id {
                    return Err("the session endpoint belongs to another launch".into());
                }
                Ok(())
            }
        },
    );
    spec.graceful_stop = Some(Box::new({
        let endpoint = launch_endpoint;
        move |child: &SessionsChild| {
            connect(&endpoint, "koru-agent-supervisor")?
                .stop_host(child.stop_authority.clone())
                .map_err(|error| error.to_string())
        }
    }));
    spec.stop_deadline = Duration::from_secs(10);
    spec.on_event = Some(Box::new(move |event| {
        if matches!(event.kind, WorkerEventKind::Ready) {
            // Published only once the host proved it is this Agent's
            // launch, so the Gateway can never be handed a secret for a
            // host that turned out to belong to somebody else. Restarts
            // re-prove and re-publish the same value harmlessly.
            publish_lifecycle_authority(publish_authority.clone());
        }
    }));
    spec
}

/// The run's narrow session-lifecycle secret, for the one other supervised
/// child that needs it.
///
/// **A process global rather than a field**, because the two supervisions do
/// not meet anywhere else: `Supervisor` is owned by the winit host and
/// `ensure_gateway` is a free function reached from a worker thread. It is
/// written once, after the first child proved its launch, and read when the
/// Gateway is spawned — which happens later in `run.rs` by construction.
static LIFECYCLE_AUTHORITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn publish_lifecycle_authority(authority: String) {
    let _ = LIFECYCLE_AUTHORITY.set(authority);
}

/// `None` until the sessions worker has proved its first child. The Gateway
/// then opens no lifecycle bridge and the management operation reports
/// itself unavailable, which is the truth.
pub fn lifecycle_authority() -> Option<String> {
    LIFECYCLE_AUTHORITY.get().cloned()
}

fn connect(
    endpoint: &koru_ipc::Endpoint,
    product: &str,
) -> Result<koru_session_client::SessionClient, String> {
    koru_session_client::SessionClient::connect(endpoint, product, crate_offer())
        .map_err(|error| error.to_string())
}

fn stop_stale(endpoint: &koru_ipc::Endpoint) {
    if let Ok(client) = connect(endpoint, "koru-session-host-cli") {
        let _ = client.stop_host(String::new());
        let deadline = Instant::now() + Duration::from_secs(5);
        while koru_ipc::probe(endpoint) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn crate_offer() -> koru_session_protocol::ProtocolOffer {
    use koru_session_protocol::{
        CapabilityName, CapabilitySet, MAX_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION, ProtocolRange,
        capability,
    };
    use std::str::FromStr;

    let capabilities = [
        capability::CONTROLLER_LEASES,
        capability::RENDER_DELTAS,
        capability::SCROLLBACK_PAGES,
        capability::RAW_WRITE,
        capability::TERMINAL_KEYS,
        capability::RESIZE,
        capability::INTERRUPT,
        capability::TERMINATE,
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

#[cfg(test)]
mod tests {
    #[test]
    fn the_worker_role_is_dispatched_by_this_executable_not_a_sibling() {
        // The old sibling-binary lookup is gone: the sessions worker is a
        // role of the Agent's own image. What remains checkable here is
        // that the role the supervisor names is the role main dispatches.
        assert_eq!(crate::session_worker::role().name, "sessions");
    }
}
