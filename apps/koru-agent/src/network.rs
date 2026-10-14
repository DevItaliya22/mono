// SPDX-License-Identifier: MPL-2.0

//! Agent-owned device network and the private opaque gateway bridge.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use koru_network_edge::{
    ConnectionMetadata, EdgeError, NetworkEgress, NetworkIngress, NetworkTarget, ProtocolToken,
    TransportSession,
};

use crate::Paths;

static NETWORK: std::sync::OnceLock<AgentNetwork> = std::sync::OnceLock::new();

#[derive(Clone)]
pub struct AgentNetwork {
    runtime: koru_network_iroh::RuntimeHandle,
    /// The device network's own Tokio runtime, for the one caller that
    /// must block on a network future from a plain thread: the Cloud host
    /// dialing a worker from the coordinator's worker thread.
    tokio: tokio::runtime::Handle,
    authority: Arc<Mutex<Option<koru_network_bridge::BridgeAuthority>>>,
    active: Arc<RwLock<Option<koru_network_bridge::wire::Link>>>,
    /// The machine's relay preference as this process last applied it —
    /// a cached answer, so asking never reads a disk.
    relay_enabled: Arc<AtomicBool>,
    /// Where the shared preference persists; `None` only when the machine
    /// has no local data directory at all.
    preference_path: Option<std::path::PathBuf>,
}

impl AgentNetwork {
    pub fn start(paths: &Paths, endpoint_secret: [u8; 32]) -> Result<Self, String> {
        // The startup read (decision №7): the shared machine-local value —
        // redirected under an explicit `--root` — or the default when it was
        // never written. This is where another channel's saved switch takes
        // effect for this one: on its next start.
        let preference_path = koru_relay_preference::preference_path(paths.explicit_root());
        let relay_enabled = preference_path
            .as_ref()
            .map(|path| {
                koru_relay_preference::load(&koru_relay_preference::FileStorage::new(path.clone()))
                    .relay_enabled
            })
            .unwrap_or_else(|| koru_relay_preference::RelayPreference::default().relay_enabled);
        let endpoint = koru_ipc::Endpoint::for_service_root(
            koru_ipc::AGENT_GATEWAY_BRIDGE,
            crate::segment(),
            &paths.store_dir(),
        )
        .map_err(|error| error.to_string())?;
        let listener = koru_ipc::Listener::bind(endpoint).map_err(|error| error.to_string())?;
        let authority = Arc::new(Mutex::new(None));
        let active = Arc::new(RwLock::new(None));
        let ingress = Arc::new(CurrentBridge {
            active: Arc::clone(&active),
        });
        let (ready, waiting) = std::sync::mpsc::sync_channel(1);
        let listener_authority = Arc::clone(&authority);
        let listener_active = Arc::clone(&active);
        std::thread::Builder::new()
            .name("koru-agent device network".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                let _entered = runtime.enter();
                let edge = koru_network_iroh::RuntimeHandle::spawn_with_opaque_ingress(
                    network_config(relay_enabled),
                    ingress,
                );
                edge.provision_endpoint(endpoint_secret);
                let _ = ready.send(Ok((edge.clone(), runtime.handle().clone())));
                let egress: Arc<dyn NetworkEgress> = Arc::new(IrohEgress(edge));
                runtime.block_on(async move {
                    let handle = tokio::runtime::Handle::current();
                    let _ = tokio::task::spawn_blocking(move || {
                        serve_bridge(
                            listener,
                            listener_authority,
                            listener_active,
                            egress,
                            handle,
                        )
                    })
                    .await;
                });
            })
            .map_err(|error| error.to_string())?;
        let (runtime, tokio) = waiting
            .recv_timeout(std::time::Duration::from_secs(30))
            .map_err(|_| "device network did not start within 30 seconds".to_owned())??;
        Ok(Self {
            runtime,
            tokio,
            authority,
            active,
            relay_enabled: Arc::new(AtomicBool::new(relay_enabled)),
            preference_path,
        })
    }

    pub fn install(self) -> Result<&'static Self, String> {
        NETWORK
            .set(self)
            .map_err(|_| "device network was already installed".to_owned())?;
        Ok(NETWORK.get().expect("network installed"))
    }

    pub fn runtime(&self) -> &koru_network_iroh::RuntimeHandle {
        &self.runtime
    }

    /// The network runtime's Tokio handle. Block on it only from a thread
    /// that is not itself one of its workers.
    pub fn tokio(&self) -> &tokio::runtime::Handle {
        &self.tokio
    }

    /// The relay preference as this process last applied it. A cached field,
    /// never a disk read — this is what a menu shows.
    pub fn relay_enabled(&self) -> bool {
        self.relay_enabled.load(Ordering::Relaxed)
    }

    /// Switch relays on or off, live (decisions №6/№7).
    ///
    /// This channel applies the change to its own runtime immediately — a
    /// command send, nothing the caller waits on; the endpoint rebinds with
    /// the same identity and desired peers reconnect. The shared preference
    /// file is written on its own thread, because the caller may be a thread
    /// that answers a person and such a thread never waits on a disk. Other
    /// installed channels read the saved value on their next start; there is
    /// no live cross-channel propagation in v1.
    pub fn set_relay_enabled(&self, enabled: bool) {
        self.relay_enabled.store(enabled, Ordering::Relaxed);
        self.runtime.replace_relay_provider(relay_provider(enabled));
        let Some(path) = self.preference_path.clone() else {
            tracing::warn!(
                "no local data directory: the relay switch applies to this run only and cannot persist"
            );
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("koru-agent relay preference".into())
            .spawn(move || {
                let storage = koru_relay_preference::FileStorage::new(path);
                let preference = koru_relay_preference::RelayPreference {
                    relay_enabled: enabled,
                };
                if let Err(error) = koru_relay_preference::save(&storage, preference) {
                    // Unconditional: a preference that did not persist is
                    // news — this run behaves as asked, the next start will
                    // not.
                    tracing::warn!(%error, "the relay preference could not be persisted");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "the relay preference writer thread could not start");
        }
    }

    pub fn expect_gateway(&self, bootstrap: &koru_network_bridge::LaunchBootstrap) {
        *self
            .authority
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(koru_network_bridge::BridgeAuthority::new(bootstrap));
    }

    /// True only after a gateway bridge was installed and subsequently died.
    /// `None` is the normal pre-registration state and must not trigger a
    /// supervisor restart loop while the gateway is still starting.
    pub fn bridge_lost(&self) -> bool {
        self.active
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(koru_network_bridge::wire::Link::is_closed)
    }
}

pub fn current() -> Option<&'static AgentNetwork> {
    NETWORK.get()
}

struct CurrentBridge {
    active: Arc<RwLock<Option<koru_network_bridge::wire::Link>>>,
}

impl CurrentBridge {
    fn link(&self) -> Result<koru_network_bridge::wire::Link, EdgeError> {
        self.active
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .filter(|link| !link.is_closed())
            .ok_or(EdgeError::Closed)
    }
}

impl NetworkIngress for CurrentBridge {
    fn admit(&self, session: TransportSession) -> Result<(), EdgeError> {
        self.link()?.admit(session).map_err(|_| EdgeError::Closed)
    }

    fn connected(&self, connection: ConnectionMetadata) -> Result<(), EdgeError> {
        NetworkIngress::connected(&self.link()?, connection)
    }

    fn disconnected(&self, connection_id: u64) {
        if let Ok(link) = self.link() {
            NetworkIngress::disconnected(&link, connection_id);
        }
    }
}

#[derive(Clone)]
struct IrohEgress(koru_network_iroh::RuntimeHandle);

impl NetworkEgress for IrohEgress {
    fn open(
        &self,
        target: NetworkTarget,
        protocol: ProtocolToken,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<TransportSession, EdgeError>> + Send + '_>,
    > {
        Box::pin(self.0.open_product_session(target, protocol))
    }
}

fn serve_bridge(
    listener: koru_ipc::Listener,
    authority: Arc<Mutex<Option<koru_network_bridge::BridgeAuthority>>>,
    active: Arc<RwLock<Option<koru_network_bridge::wire::Link>>>,
    egress: Arc<dyn NetworkEgress>,
    runtime: tokio::runtime::Handle,
) {
    loop {
        let Ok(mut stream) = listener.accept() else {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        };
        if !koru_ipc::same_user(&stream) {
            continue;
        }
        let Ok((version, registration)) =
            koru_ipc::frame::read_frame::<_, koru_network_bridge::wire::Registration>(&mut stream)
        else {
            continue;
        };
        let accepted = version == koru_network_bridge::wire::WIRE_VERSION
            && authority
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
                .is_some_and(|authority| {
                    authority
                        .register(&registration.identity, &registration.capability)
                        .is_ok()
                });
        if !accepted {
            let _ = koru_ipc::frame::write_frame(
                &mut stream,
                koru_network_bridge::wire::WIRE_VERSION,
                &koru_network_bridge::wire::RegistrationReply::Refused(
                    "launch capability or identity mismatch".into(),
                ),
            );
            continue;
        }
        // **Admitted and installed under one write guard**, so there is no
        // moment in which a child has been told `Accepted` and the link it
        // registered is not yet the current one. The reply has to precede the
        // attach — `Link::attach` consumes the stream — so the lock, rather
        // than the ordering, is what closes the window. It was safe before only
        // because of what the child does next (open Store, AI and a listener
        // before publishing discovery), and an ordering that is safe only
        // because of what happens next is worth making safe on its own terms.
        // It is held for one frame write and one attach, both local.
        let mut installed = active.write().unwrap_or_else(|error| error.into_inner());
        if koru_ipc::frame::write_frame(
            &mut stream,
            koru_network_bridge::wire::WIRE_VERSION,
            &koru_network_bridge::wire::RegistrationReply::Accepted,
        )
        .is_err()
        {
            continue;
        }
        let _entered = runtime.enter();
        match koru_network_bridge::wire::Link::attach(
            stream,
            koru_network_bridge::wire::Side::Agent,
            Some(Arc::clone(&egress)),
        ) {
            Ok(link) => *installed = Some(link),
            Err(error) => koru_ipc::ipc_debug!("gateway bridge: {error}"),
        }
        drop(installed);
    }
}

/// This host's startup configuration through the one shared helper
/// (Plan 5.2): isolated mode, the `KORU_IROH_RELAYS` override, the compiled
/// Koru relay list for this build's channel, and the Relay-Off precedence
/// all live in `koru-network-iroh`. The agent contributes only what is its
/// own: the relay preference it read, and which build it is.
fn network_config(relay_enabled: bool) -> koru_network_iroh::IrohNetworkConfig {
    koru_network_iroh::host_network_config(relay_enabled, channel_kind())
}

/// The same resolution for the live switch, so a mid-session change lands on
/// exactly the configuration a restart would.
fn relay_provider(relay_enabled: bool) -> koru_network_iroh::RelayProviderConfig {
    koru_network_iroh::host_relay_provider(relay_enabled, channel_kind())
}

/// Which compiled relay list this build ships (decision №5): internal builds
/// dial `dev1.relay.koru.do`, published ones `use1-1.relay.koru.do`. The
/// channel answer is this host's domain knowledge; the mapping to a hostname
/// is the helper's.
fn channel_kind() -> koru_network_iroh::RelayChannelKind {
    koru_network_iroh::RelayChannelKind::for_internal_build(
        koru_channel::ReleaseChannel::this_build_is_internal(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Row 1 of decision №8 through this host's own wiring: Off disables
    /// every relay whatever the environment says, which is also what makes
    /// this assertion immune to `KORU_IROH_RELAYS`/`KORU_IROH_ISOLATED`
    /// being set on the machine running it. The remaining rows are the
    /// resolver's table-driven suite in `koru-network-iroh`.
    #[test]
    fn relay_off_disables_the_provider_whatever_the_environment_says() {
        assert_eq!(
            network_config(false).relay_provider,
            koru_network_iroh::RelayProviderConfig::Disabled
        );
        assert_eq!(
            relay_provider(false),
            koru_network_iroh::RelayProviderConfig::Disabled
        );
    }

    /// Pure path resolution: an explicit `--root` redirects the preference
    /// with the agent's other rooted files, and nothing is created.
    #[test]
    fn an_explicit_root_redirects_the_relay_preference() {
        let paths = Paths::for_root(std::path::PathBuf::from("X:/never-created"));
        assert_eq!(
            koru_relay_preference::preference_path(paths.explicit_root()),
            Some(
                std::path::Path::new("X:/never-created")
                    .join("agent")
                    .join(koru_relay_preference::PREFERENCE_FILE)
            )
        );
    }
}
