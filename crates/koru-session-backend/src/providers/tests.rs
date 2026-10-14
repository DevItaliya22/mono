// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn late_reader_of_a_previous_host_cannot_replace_the_new_hosts_answer() {
    let mut catalog = Catalog(legacy(true, true));
    let mut late = catalog.snapshot().remove(0);
    late.availability = A::SignedOut {
        code: ErrorCode::ProviderUnavailable,
    };
    assert!(!catalog.accept(2, 1, late.clone()));
    assert!(matches!(
        catalog.snapshot()[0].availability,
        A::Available { .. }
    ));
    assert!(catalog.accept(2, 2, late));
    assert!(matches!(
        catalog.snapshot()[0].availability,
        A::SignedOut { .. }
    ));
}

#[test]
fn old_hosts_and_absent_hosts_have_finite_managed_fallbacks() {
    for rows in [legacy(true, true), legacy(false, false), unavailable()] {
        assert_eq!(
            rows.len(),
            koru_session_profiles::MANAGED_PROVIDER_REGISTRATIONS.len()
        );
        assert!(
            rows.iter()
                .all(|row| !matches!(row.availability, A::Checking))
        );
    }
}

use koru_session_wire::{
    FRAME_VERSION, FromHost, HostHello, Request, RequestFrame, Response, ResponseFrame,
    SubscriptionId,
};
use std::io::{Cursor, Read, Write};

#[derive(Clone)]
struct ReadinessSink(mpsc::Sender<BackendEvent>);
impl BackendSink for ReadinessSink {
    fn deliver(&self, event: BackendEvent) -> bool {
        self.0.send(event).is_ok()
    }
}

struct MemoryReader {
    frames: mpsc::Receiver<Vec<u8>>,
    current: Cursor<Vec<u8>>,
}
impl Read for MemoryReader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.current.read(bytes)?;
            if count != 0 {
                return Ok(count);
            }
            match self.frames.recv() {
                Ok(frame) => self.current = Cursor::new(frame),
                Err(_) => return Ok(0),
            }
        }
    }
}

fn host_frame(to_client: &mpsc::Sender<Vec<u8>>, value: FromHost) {
    let mut bytes = Vec::new();
    koru_ipc::frame::write_frame(&mut bytes, FRAME_VERSION, &value).unwrap();
    to_client.send(bytes).unwrap();
}

struct MemoryWriter {
    to_client: mpsc::Sender<Vec<u8>>,
    pending: Vec<u8>,
    snapshot: ProviderReadiness,
    negotiated: bool,
}
impl Write for MemoryWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let (_, request): (_, RequestFrame) =
            koru_ipc::frame::read_frame(&mut self.pending.as_slice())?;
        self.pending.clear();
        let result = match request.request {
            Request::SubscribeProviders if self.negotiated => Response::ProvidersSubscribed {
                subscription_id: SubscriptionId(1),
                providers: vec![self.snapshot.clone()],
            },
            Request::ListSessions => Response::Sessions { sessions: vec![] },
            Request::Disconnect => return Ok(()),
            _ => panic!("unsupported frame reached a fixture host"),
        };
        host_frame(
            &self.to_client,
            FromHost::Response {
                frame: ResponseFrame {
                    id: request.id,
                    result: Ok(result),
                },
            },
        );
        Ok(())
    }
}

fn memory_host(
    name: &str,
    snapshot: ProviderReadiness,
    negotiated: bool,
) -> (Arc<SessionClient>, mpsc::Sender<Vec<u8>>) {
    let (to_client, frames) = mpsc::channel();
    let client = SessionClient::from_test_transport(
        HostHello {
            product: "fixture".into(),
            product_version: "fixture".into(),
            instance_id: name.into(),
            supervisor_launch_id: name.into(),
            agreement: koru_session_protocol::ProtocolAgreement {
                version: 14,
                capabilities: CapabilitySet::new(if negotiated {
                    vec![capability::PROVIDER_READINESS.parse().unwrap()]
                } else {
                    vec![]
                }),
            },
        },
        MemoryReader {
            frames,
            current: Cursor::new(vec![]),
        },
        MemoryWriter {
            to_client: to_client.clone(),
            pending: vec![],
            snapshot,
            negotiated,
        },
    )
    .unwrap();
    (Arc::new(client), to_client)
}

#[test]
fn new_host_subscription_replaces_snapshot_and_rejects_actual_old_reader_update() {
    let ready = legacy(true, true).remove(0);
    let mut signed_out = ready.clone();
    signed_out.availability = A::SignedOut {
        code: ErrorCode::ProviderUnavailable,
    };
    let (old_client, old_host) = memory_host("old", signed_out.clone(), true);
    let (new_client, new_host) = memory_host("new", ready.clone(), true);
    let (event_send, events) = mpsc::channel();
    let sink = ReadinessSink(event_send);
    let (internal, commands) = mpsc::sync_channel(8);
    let _old = subscribe(&old_client, 1, &sink, &internal);
    assert!(
        matches!(events.recv_timeout(Duration::from_secs(2)).unwrap(),
        BackendEvent::Providers(rows) if rows == vec![signed_out.clone()])
    );
    let mut current = subscribe(&new_client, 2, &sink, &internal);
    assert!(
        matches!(events.recv_timeout(Duration::from_secs(2)).unwrap(),
        BackendEvent::Providers(rows) if rows == vec![ready.clone()])
    );
    // This travels through the old client's real reader and the backend's
    // old subscription reader after the new host snapshot was published.
    host_frame(
        &old_host,
        FromHost::Subscription {
            frame: koru_session_wire::SubscriptionFrame {
                subscription_id: SubscriptionId(1),
                update: SubscriptionUpdate::Providers {
                    provider: signed_out.clone(),
                },
            },
        },
    );
    let BackendCommand::ProviderChanged {
        generation,
        provider,
    } = commands.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("provider command")
    };
    assert_eq!(generation, 1);
    assert!(!current.accept(2, generation, provider));
    assert_eq!(current.snapshot(), vec![ready]);
    host_frame(
        &new_host,
        FromHost::Subscription {
            frame: koru_session_wire::SubscriptionFrame {
                subscription_id: SubscriptionId(1),
                update: SubscriptionUpdate::Providers {
                    provider: signed_out.clone(),
                },
            },
        },
    );
    let BackendCommand::ProviderChanged {
        generation,
        provider,
    } = commands.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("provider command")
    };
    assert!(current.accept(2, generation, provider));
    assert_eq!(current.snapshot(), vec![signed_out]);
    assert!(old_client.list().unwrap().is_empty());
    assert!(new_client.list().unwrap().is_empty());
    old_client.disconnect();
    new_client.disconnect();
}

#[test]
fn old_host_backend_fallback_never_sends_subscription_and_connection_stays_usable() {
    let (client, _host) = memory_host("legacy", legacy(true, true).remove(0), false);
    let (send, events) = mpsc::channel();
    let (internal, _commands) = mpsc::sync_channel(8);
    let rows = subscribe(&client, 1, &ReadinessSink(send), &internal).snapshot();
    assert!(
        rows.iter()
            .all(|row| !matches!(row.availability, A::Checking))
    );
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(2)).unwrap(),
        BackendEvent::Providers(_)
    ));
    assert!(client.list().unwrap().is_empty());
    assert_eq!(client.state(), ConnectionState::Connected);
    client.disconnect();
}
