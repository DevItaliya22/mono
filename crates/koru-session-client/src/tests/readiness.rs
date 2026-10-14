// SPDX-License-Identifier: MPL-2.0

use super::super::*;
use koru_session_protocol::{
    CapabilityName, CapabilitySet, ExecutableAvailability, ProtocolAgreement, ProviderReadiness,
};

struct ReplyWriter {
    inner: std::sync::Weak<Inner>,
    bytes: Vec<u8>,
    requests: Arc<Mutex<Vec<Request>>>,
    readiness: bool,
}
impl Write for ReplyWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let (_, frame): (_, RequestFrame) =
            koru_ipc::frame::read_frame(&mut self.bytes.as_slice())?;
        self.bytes.clear();
        let reply = match &frame.request {
            Request::ListSessions => Response::Sessions { sessions: vec![] },
            Request::SubscribeProviders if self.readiness => Response::ProvidersSubscribed {
                subscription_id: SubscriptionId(1),
                providers: vec![ProviderReadiness {
                    kind: "fixture".parse().unwrap(),
                    availability: ExecutableAvailability::Available { version: None },
                    capabilities: Default::default(),
                    failure: None,
                }],
            },
            Request::RefreshProvider { .. } if self.readiness => Response::Ok,
            Request::Disconnect => Response::Ok,
            _ => panic!("old host received an unknown request"),
        };
        self.requests.lock().unwrap().push(frame.request);
        if let Some(inner) = self.inner.upgrade()
            && let Some(sender) = inner.pending.lock().unwrap().get(&frame.id)
        {
            let _ = sender.try_send(Ok(reply));
        }
        Ok(())
    }
}

fn client(
    version: u32,
    capability: bool,
    readiness: bool,
) -> (SessionClient, Arc<Mutex<Vec<Request>>>) {
    let inner = Arc::new(Inner {
        writer: Mutex::new(None),
        pending: Mutex::new(HashMap::new()),
        subscriptions: Mutex::new(BTreeMap::new()),
        orphaned: Mutex::new(BTreeMap::new()),
        next_request: AtomicU64::new(1),
        state: Mutex::new(ConnectionState::Connected),
        controller: Mutex::new(None),
    });
    let requests = Arc::new(Mutex::new(Vec::new()));
    *inner.writer.lock().unwrap() = Some(Box::new(ReplyWriter {
        inner: Arc::downgrade(&inner),
        bytes: Vec::new(),
        requests: requests.clone(),
        readiness,
    }));
    let capabilities = CapabilitySet::new(if capability {
        vec![
            koru_session_protocol::capability::PROVIDER_READINESS
                .parse::<CapabilityName>()
                .unwrap(),
        ]
    } else {
        vec![]
    });
    (
        SessionClient {
            inner,
            host: koru_session_wire::HostHello {
                product: "fixture".into(),
                product_version: "fixture".into(),
                instance_id: "fixture".into(),
                supervisor_launch_id: "fixture".into(),
                agreement: ProtocolAgreement {
                    version,
                    capabilities,
                },
            },
        },
        requests,
    )
}

#[test]
fn readiness_requests_never_reach_an_old_host_and_ordinary_requests_still_work() {
    for (version, capability) in [(14, false), (7, true)] {
        let (client, requests) = client(version, capability, false);
        assert!(matches!(
            client.subscribe_providers(),
            Err(ClientError::Session(SessionFailure {
                code: ErrorCode::CapabilityNotNegotiated,
                ..
            }))
        ));
        assert!(matches!(
            client.refresh_provider("fixture".parse().unwrap()),
            Err(ClientError::Session(SessionFailure {
                code: ErrorCode::CapabilityNotNegotiated,
                ..
            }))
        ));
        assert!(requests.lock().unwrap().is_empty());
        assert!(client.list().unwrap().is_empty());
        assert_eq!(client.state(), ConnectionState::Connected);
    }
}

#[test]
fn new_host_sends_no_readiness_to_an_old_client_but_supports_negotiated_snapshot_and_retry() {
    let (old_client, requests) = client(14, false, true);
    assert!(old_client.list().unwrap().is_empty());
    assert!(matches!(
        requests.lock().unwrap().as_slice(),
        [Request::ListSessions]
    ));
    let (new_client, _) = client(14, true, true);
    let (rows, _subscription) = new_client.subscribe_providers().unwrap();
    assert_eq!(rows[0].kind.as_str(), "fixture");
    new_client.refresh_provider(rows[0].kind.clone()).unwrap();
    assert!(new_client.list().unwrap().is_empty());
    assert_eq!(new_client.state(), ConnectionState::Connected);
}

#[test]
fn model_discovery_never_sends_an_unknown_request_to_old_peers() {
    for version in [8, 14, 15] {
        let (client, requests) = client(version, false, false);
        assert!(matches!(
            client
                .discover_provider_models("opencode".parse().unwrap(), PathBuf::from("C:/project")),
            Err(ClientError::Session(SessionFailure {
                code: ErrorCode::CapabilityNotNegotiated,
                ..
            }))
        ));
        assert!(requests.lock().unwrap().is_empty());
        assert!(client.list().unwrap().is_empty());
    }
}
