// SPDX-License-Identifier: MPL-2.0

use super::*;

fn event(id: &str) -> SinkEvent {
    SinkEvent {
        event_id: id.to_string(),
        name: "document.created".to_string(),
        version: 1,
        occurred_at_ms: 1_756_000_000_000,
        session_id: "11111111-1111-4111-8111-111111111111".to_string(),
        installation_id: None,
        app_version: "0.0.3".to_string(),
        build_channel: "internal".to_string(),
        platform: "windows".to_string(),
        consent_version: "1".to_string(),
        properties: serde_json::Map::new(),
    }
}

#[test]
fn the_blackhole_takes_everything_and_counts_it() {
    let sink = Blackhole::new();
    assert_eq!(sink.deliver(&[event("a"), event("b")]), Delivery::Ok);
    assert_eq!(sink.deliver(&[event("c")]), Delivery::Ok);
    assert_eq!(sink.delivered(), 3);
    assert_eq!(sink.batches(), 2);
}

#[test]
fn every_outcome_carries_a_stable_token() {
    assert_eq!(Delivery::Ok.code(), "ok");
    assert_eq!(
        Delivery::Retry {
            after: None,
            code: code::SERVER_ERROR
        }
        .code(),
        "server_error"
    );
    assert_eq!(
        Delivery::Permanent {
            code: code::BAD_REQUEST
        }
        .code(),
        "bad_request"
    );
}

/// The whole port, exercised as a caller writes one. If this stops compiling in
/// five lines, the trait has grown something a sink should not have to know.
#[test]
fn a_sink_is_five_lines() {
    struct Counting(std::sync::atomic::AtomicUsize);
    impl Sink for Counting {
        fn id(&self) -> SinkId {
            SinkId("counting")
        }
        fn max_batch(&self) -> usize {
            2
        }
        fn deliver(&self, events: &[SinkEvent]) -> Delivery {
            self.0
                .fetch_add(events.len(), std::sync::atomic::Ordering::Relaxed);
            Delivery::Ok
        }
    }

    let sink: Box<dyn Sink> = Box::new(Counting(std::sync::atomic::AtomicUsize::new(0)));
    assert_eq!(sink.max_batch(), 2);
    assert!(sink.deliver(&[event("a")]).is_ok());
    assert_eq!(sink.id().as_str(), "counting");
}

/// A `SinkEvent` is what leaves the machine, so its field list is a privacy
/// claim as much as a data shape. Serializing one and reading the key set back
/// is what makes an added field a failing test rather than a quiet export.
#[test]
fn a_sink_event_carries_no_identity_beyond_the_two_anonymous_ids() {
    let json = serde_json::to_value(event("a")).expect("serializable");
    let mut keys: Vec<&str> = json
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "app_version",
            "build_channel",
            "consent_version",
            "event_id",
            "installation_id",
            "name",
            "occurred_at_ms",
            "platform",
            "properties",
            "session_id",
            "version",
        ]
    );
}
