// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_session_protocol::{
    ProtocolAgreement, SequencedSessionEvent, SessionEvent, StreamSequence,
};
use koru_session_wire::{
    FRAME_VERSION, FromHost, HostHello, Request, RequestFrame, Response, ResponseFrame,
    SubscriptionFrame, SubscriptionId,
};
use std::io::{Cursor, Read, Write};

#[derive(Clone)]
struct Sink(mpsc::Sender<BackendEvent>);
impl BackendSink for Sink {
    fn deliver(&self, event: BackendEvent) -> bool {
        self.0.send(event).is_ok()
    }
}

struct Reader(mpsc::Receiver<Vec<u8>>, Cursor<Vec<u8>>);
impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.1.read(bytes)?;
            if count != 0 {
                return Ok(count);
            }
            match self.0.recv() {
                Ok(frame) => self.1 = Cursor::new(frame),
                Err(_) => return Ok(0),
            }
        }
    }
}

fn frame(sender: &mpsc::Sender<Vec<u8>>, value: FromHost) {
    let mut bytes = Vec::new();
    koru_ipc::frame::write_frame(&mut bytes, FRAME_VERSION, &value).unwrap();
    sender.send(bytes).unwrap();
}

struct Writer {
    sender: mpsc::Sender<Vec<u8>>,
    pending: Vec<u8>,
    subscribed: bool,
    available: bool,
    refuse_subscription: bool,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let (_, request): (_, RequestFrame) =
            koru_ipc::frame::read_frame(&mut self.pending.as_slice())?;
        self.pending.clear();
        let result = match request.request {
            Request::SubscribeEventsFrom { .. } if self.refuse_subscription => {
                self.refuse_subscription = false;
                Err(failure(ErrorCode::SessionNotFound))
            }
            Request::SubscribeEventsFrom { .. } => {
                assert!(!self.subscribed, "one lifecycle reader per session");
                self.subscribed = true;
                Ok(Response::Subscribed {
                    subscription_id: SubscriptionId(1),
                })
            }
            Request::ListSessions => Ok(Response::Sessions { sessions: vec![] }),
            Request::ListProjects => Ok(Response::Projects { projects: vec![] }),
            Request::RequestControl { .. } => {
                assert!(
                    self.subscribed,
                    "a newly created session must be observed before takeover"
                );
                Ok(Response::ControlRequested {
                    outcome: if self.available {
                        ControlRequestOutcome::Available
                    } else {
                        ControlRequestOutcome::Waiting
                    },
                })
            }
            Request::Disconnect => return Ok(()),
            other => panic!("unexpected request: {other:?}"),
        };
        frame(
            &self.sender,
            FromHost::Response {
                frame: ResponseFrame {
                    id: request.id,
                    result,
                },
            },
        );
        Ok(())
    }
}

fn host(available: bool, refuse_subscription: bool) -> (Arc<SessionClient>, mpsc::Sender<Vec<u8>>) {
    let (sender, frames) = mpsc::channel();
    let client = SessionClient::from_test_transport(
        HostHello {
            product: "fixture".into(),
            product_version: "fixture".into(),
            instance_id: "fixture".into(),
            supervisor_launch_id: "fixture".into(),
            agreement: ProtocolAgreement {
                version: MAX_PROTOCOL_VERSION,
                capabilities: protocol_offer().capabilities,
            },
        },
        Reader(frames, Cursor::new(vec![])),
        Writer {
            sender: sender.clone(),
            pending: vec![],
            subscribed: false,
            available,
            refuse_subscription,
        },
    )
    .unwrap();
    (Arc::new(client), sender)
}

#[test]
fn new_run_session_release_wakes_the_window_after_control_request() {
    let (client, sender) = host(false, false);
    let (events_send, events) = mpsc::channel();
    let (internal, commands) = mpsc::sync_channel(8);
    let session = SessionId::mint();
    let mut subscribed = BTreeSet::new();
    request(
        &client,
        &Sink(events_send),
        &internal,
        1,
        &mut subscribed,
        session,
        TakeoverUrgency::AtTurnBoundary,
    )
    .unwrap();
    assert!(subscribed.contains(&session));
    assert!(events.try_iter().any(|event| matches!(
        event,
        BackendEvent::Handover {
            handover: HandoverState::Requested,
            ..
        }
    )));
    // The delegate worker releases later. No tab has attached to this session.
    frame(
        &sender,
        FromHost::Subscription {
            frame: SubscriptionFrame {
                subscription_id: SubscriptionId(1),
                update: SubscriptionUpdate::Event {
                    event: SequencedSessionEvent {
                        session_id: session,
                        sequence: StreamSequence::new(1).unwrap(),
                        event: SessionEvent::HandoverChanged {
                            handover: HandoverState::Released,
                        },
                    },
                },
            },
        },
    );
    assert!(matches!(
        commands.recv_timeout(Duration::from_secs(2)).unwrap(),
        BackendCommand::Refresh
    ));
    client.disconnect();
}

#[test]
fn already_released_session_opens_and_failed_subscription_can_be_retried() {
    let (client, _sender) = host(true, true);
    let (events_send, events) = mpsc::channel();
    let sink = Sink(events_send);
    let (internal, _commands) = mpsc::sync_channel(8);
    let session = SessionId::mint();
    let mut subscribed = BTreeSet::new();
    assert!(
        request(
            &client,
            &sink,
            &internal,
            1,
            &mut subscribed,
            session,
            TakeoverUrgency::AtTurnBoundary
        )
        .is_err()
    );
    assert!(!subscribed.contains(&session));
    for _ in 0..2 {
        request(
            &client,
            &sink,
            &internal,
            1,
            &mut subscribed,
            session,
            TakeoverUrgency::AtTurnBoundary,
        )
        .unwrap();
        assert!(events.try_iter().any(|event| matches!(
            event,
            BackendEvent::Handover {
                handover: HandoverState::Released,
                ..
            }
        )));
    }
    client.disconnect();
}
