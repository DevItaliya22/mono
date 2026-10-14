// SPDX-License-Identifier: MPL-2.0

use super::*;

fn event() -> SinkEvent {
    let mut properties = serde_json::Map::new();
    properties.insert("blocks".into(), serde_json::json!(12));
    properties.insert("kind".into(), serde_json::json!("document"));
    SinkEvent {
        event_id: "6f1d0e2a-0000-4000-8000-000000000001".into(),
        name: "document.created".into(),
        version: 1,
        occurred_at_ms: 1_756_312_221_123,
        session_id: "11111111-1111-4111-8111-111111111111".into(),
        installation_id: Some("22222222-2222-4222-8222-222222222222".into()),
        app_version: "0.0.3".into(),
        build_channel: "internal".into(),
        platform: "windows".into(),
        consent_version: "1".into(),
        properties,
    }
}

fn client() -> TelemetryClient {
    TelemetryClient::new(Config {
        base_url: "https://m.koru.do".into(),
        batch: 250,
        timeout: Duration::from_secs(15),
    })
    .expect("built")
}

/// The golden shape. `batch_id` and `sent_at_ms` describe the upload attempt
/// and are minted per call, so they are asserted for form rather than value;
/// everything else is compared against the whole body because a field that
/// quietly stops being sent is exactly the failure a per-field assertion
/// misses.
#[test]
fn the_body_is_what_the_service_receives() {
    let body = body(&[event()]);
    let object = body.as_object().expect("a JSON object");
    assert_eq!(
        object.len(),
        8,
        "the wire shape is these eight fields and no more"
    );
    assert_eq!(body["app_version"], "0.0.3");
    assert_eq!(body["build_channel"], "internal");
    assert_eq!(body["platform"], "windows");
    assert_eq!(body["consent_version"], "1");
    assert_eq!(
        body["anonymous_installation_id"],
        "22222222-2222-4222-8222-222222222222"
    );
    assert_eq!(
        body["events"],
        serde_json::json!([{
            "event_id": "6f1d0e2a-0000-4000-8000-000000000001",
            "name": "document.created",
            "version": 1,
            "occurred_at_ms": 1_756_312_221_123i64,
            "session_id": "11111111-1111-4111-8111-111111111111",
            "properties": {
                "blocks": 12,
                "kind": "document"
            }
        }])
    );
    uuid::Uuid::parse_str(body["batch_id"].as_str().expect("a string batch id"))
        .expect("the batch id is a uuid");
    assert!(
        body["sent_at_ms"].as_i64().expect("epoch milliseconds") > 0,
        "the upload left at a real time"
    );
}

/// Build dimensions ride the batch, taken from the first event — they are
/// identical across a batch by construction, so the second event's copy is
/// never consulted and never sent.
#[test]
fn the_header_comes_from_the_first_event() {
    let mut second = event();
    second.event_id = "6f1d0e2a-0000-4000-8000-000000000002".into();
    second.app_version = "9.9.9".into();
    let body = body(&[event(), second]);
    assert_eq!(body["app_version"], "0.0.3");
    assert_eq!(
        body["events"].as_array().expect("an array").len(),
        2,
        "both events travel"
    );
}

/// Without an installation id the field is absent from the wire, not null —
/// the server reads its absence as "AnonymousUsage events only".
#[test]
fn without_an_installation_id_the_field_is_not_sent() {
    let mut anonymous = event();
    anonymous.installation_id = None;
    let body = body(&[anonymous]);
    assert!(
        body.as_object()
            .expect("a JSON object")
            .get("anonymous_installation_id")
            .is_none(),
        "the key is skipped, not null"
    );
}

/// `body` stays total: an id that does not parse is skipped rather than
/// panicking. In `deliver` the same event fails the whole batch *before* this
/// is called — the skip exists so the pure function has no partial path, not
/// to drop anything quietly.
#[test]
fn an_event_whose_id_does_not_parse_cannot_travel() {
    let mut bad = event();
    bad.event_id = "not-a-uuid".into();
    let with_bad = body(&[event(), bad]);
    assert_eq!(with_bad["events"].as_array().expect("an array").len(), 1);
    let mut bad_session = event();
    bad_session.session_id = "not-a-uuid".into();
    let only_bad = body(&[bad_session]);
    assert!(only_bad["events"].as_array().expect("an array").is_empty());
}

/// A bad id is refused before any byte leaves — the assertion is on the
/// answer, and the proof no request happened is that this test has no server
/// to answer one.
#[test]
fn a_bad_id_is_permanent_so_the_drain_splits_it_out() {
    let mut bad = event();
    bad.event_id = "not-a-uuid".into();
    assert_eq!(
        client().deliver(&[bad]),
        Delivery::Permanent {
            code: code::BAD_REQUEST
        }
    );
}

#[test]
fn an_empty_batch_is_ok_without_a_request() {
    assert_eq!(client().deliver(&[]), Delivery::Ok);
}

/// The classification table, row by row — no HTTP, no mock, no dev-dependency.
#[test]
fn every_status_is_classified() {
    let none = None;
    assert_eq!(classify(200, none), Delivery::Ok);
    assert_eq!(classify(204, none), Delivery::Ok);

    for status in [408, 429, 500, 502, 503, 504] {
        assert!(
            matches!(classify(status, none), Delivery::Retry { .. }),
            "{status} must be retryable"
        );
    }
    for status in [400, 413] {
        assert_eq!(
            classify(status, none),
            Delivery::Permanent {
                code: code::BAD_REQUEST
            }
        );
    }
    // An unknown status is retried, because it is not evidence the bytes are
    // bad, and losing data is the more expensive mistake.
    assert!(
        matches!(classify(418, none), Delivery::Retry { code, .. } if code == code::UNEXPECTED)
    );
}

/// The WAF answers a rate-limit trip with a 429 and a `Retry-After`; the wait
/// it asked for is the wait it gets.
#[test]
fn a_rate_limit_honours_the_wafs_own_instruction() {
    assert_eq!(
        classify(429, Some(Duration::from_secs(90))),
        Delivery::Retry {
            after: Some(Duration::from_secs(90)),
            code: code::RATE_LIMITED
        }
    );
}

/// A refusal on an anonymous route is a misconfigured proxy: retryable, so no
/// queue is ever dead-lettered over it, and slow, so the log does not fill
/// while nobody has fixed it.
#[test]
fn a_refusal_is_retryable_and_slow() {
    let Delivery::Retry { after, code } = classify(401, None) else {
        panic!("401 must be retryable");
    };
    assert_eq!(code, code::UNAUTHORIZED);
    assert_eq!(after, Some(UNAUTHORIZED_FLOOR));
    // Even when the answer asks for a short wait, the floor holds: a proxy is
    // fixed by a person, not by waiting five seconds.
    let Delivery::Retry { after, .. } = classify(403, Some(Duration::from_secs(5))) else {
        panic!("403 must be retryable");
    };
    assert_eq!(after, Some(UNAUTHORIZED_FLOOR));
}

#[test]
fn the_endpoint_tolerates_however_the_base_was_written() {
    assert_eq!(
        endpoint("https://m.koru.do"),
        "https://m.koru.do/v1/metrics"
    );
    assert_eq!(
        endpoint("https://m.koru.do/"),
        "https://m.koru.do/v1/metrics"
    );
    assert_eq!(
        endpoint("  http://127.0.0.1:8787//  "),
        "http://127.0.0.1:8787/v1/metrics"
    );
}

#[test]
fn a_client_without_a_base_url_refuses_to_exist() {
    let config = |base_url: &str| Config {
        base_url: base_url.into(),
        batch: 250,
        timeout: Duration::from_secs(15),
    };
    assert!(TelemetryClient::new(config("")).is_err());
    assert!(TelemetryClient::new(config("   ")).is_err());
    assert!(TelemetryClient::new(config("https://m.koru.do")).is_ok());
}

#[test]
fn a_batch_size_is_clamped_to_something_a_request_can_carry() {
    let batch = |batch| {
        TelemetryClient::new(Config {
            base_url: "https://m.koru.do".into(),
            batch,
            timeout: Duration::from_secs(1),
        })
        .expect("built")
        .max_batch()
    };
    assert_eq!(batch(0), 1);
    assert_eq!(batch(250), 250);
    assert_eq!(batch(10_000), MAX_BATCH_EVENTS);
}
