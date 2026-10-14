// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::state::EventsHandle;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt as _;
use koru_events::wire::{
    ActivityPage, AppendReceipt, ConsentBody, EventBatch, EventPage, EventsStatus,
};
use koru_events::{
    Draft, EmitContext, Envelope, EventId, ImportCompleted, ImportOutcome, ImportSource, bucket,
};

/// A delegated run's lifecycle through the real activity route (P1.8).
mod delegate;

fn handle() -> (tempfile::TempDir, EventsHandle) {
    let dir = tempfile::tempdir().unwrap();
    // `quiet`: these tests count sequences from their own first publish.
    let handle =
        EventsHandle::open(crate::state::WorkerConfig::quiet(dir.path().to_path_buf())).unwrap();
    (dir, handle)
}

/// The installation announces itself exactly once per journal — the first
/// shipping-shaped open emits `app.installed`, and a reopen does not.
#[tokio::test]
async fn a_new_journal_announces_the_installation_once() {
    let dir = tempfile::tempdir().unwrap();
    let handle =
        EventsHandle::open(crate::state::WorkerConfig::new(dir.path().to_path_buf())).unwrap();
    let page: EventPage = body_json(handle.request(get("/v1/events?after=0")).await).await;
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].envelope.name.as_str(), "app.installed");
    drop(handle);

    let handle =
        EventsHandle::open(crate::state::WorkerConfig::new(dir.path().to_path_buf())).unwrap();
    let page: EventPage = body_json(handle.request(get("/v1/events?after=0")).await).await;
    assert_eq!(
        page.events.len(),
        1,
        "a reopen is not a second installation"
    );
}

fn imported(id: u128) -> Envelope {
    Draft::from(ImportCompleted {
        source: ImportSource::Files,
        document_count_bucket: bucket(2),
        outcome: ImportOutcome::Ok,
    })
    .build(
        &EmitContext::user_session(),
        EventId(uuid::Uuid::from_u128(id)),
        1_756_000_000_000,
    )
}

async fn body_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn post_json(path: &str, body: &impl serde::Serialize) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn put_json(path: &str, body: &impl serde::Serialize) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn publish_then_read_round_trips() {
    let (_dir, handle) = handle();
    let batch = EventBatch {
        events: vec![imported(1)],
    };
    let receipt: AppendReceipt =
        body_json(handle.request(post_json("/v1/events", &batch)).await).await;
    assert_eq!(receipt.accepted, 1);
    assert_eq!(receipt.last_seq, 1);

    // The same batch again is a duplicate, not an error — the retry story.
    let receipt: AppendReceipt =
        body_json(handle.request(post_json("/v1/events", &batch)).await).await;
    assert_eq!((receipt.accepted, receipt.duplicates), (0, 1));

    let page: EventPage = body_json(handle.request(get("/v1/events?after=0")).await).await;
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].envelope, batch.events[0]);
    assert_eq!(page.next_after, 1);
}

#[tokio::test]
async fn an_unregistered_event_is_refused_with_a_code() {
    let (_dir, handle) = handle();
    let bad = Draft::new(koru_events::EventName::new("kitchen.sink")).build(
        &EmitContext::user_session(),
        EventId(uuid::Uuid::from_u128(2)),
        1_756_000_000_000,
    );
    let receipt: AppendReceipt = body_json(
        handle
            .request(post_json("/v1/events", &EventBatch { events: vec![bad] }))
            .await,
    )
    .await;
    assert_eq!(receipt.accepted, 0);
    assert_eq!(receipt.rejected.len(), 1);
    assert_eq!(receipt.rejected[0].code, "malformed_name");
}

#[tokio::test]
async fn consent_gates_the_telemetry_window() {
    let (_dir, handle) = handle();
    let status: koru_events::wire::EventsStatus =
        body_json(handle.request(get("/v1/events/status")).await).await;
    assert!(
        !status.telemetry_consent_answered,
        "a fresh device is asked"
    );
    assert!(!status.telemetry_consent);

    // Opt-in: an installation nobody has asked spools nothing, and the event
    // recorded now is never uploaded, even after the yes below.
    handle
        .request(post_json(
            "/v1/events",
            &EventBatch {
                events: vec![imported(3)],
            },
        ))
        .await;
    let batch: koru_events::telemetry::TelemetryBatch =
        body_json(handle.request(get("/v1/events/telemetry")).await).await;
    assert!(batch.events.is_empty());
    assert!(
        batch.anonymous_installation_id.is_none(),
        "no id is minted for a device that has not said yes"
    );

    let consent = ConsentBody {
        granted: true,
        consent_version: "v1".into(),
    };
    let echoed: ConsentBody = body_json(
        handle
            .request(put_json("/v1/events/telemetry/consent", &consent))
            .await,
    )
    .await;
    assert!(echoed.granted);
    let status: koru_events::wire::EventsStatus =
        body_json(handle.request(get("/v1/events/status")).await).await;
    assert!(status.telemetry_consent_answered && status.telemetry_consent);

    handle
        .request(post_json(
            "/v1/events",
            &EventBatch {
                events: vec![imported(4)],
            },
        ))
        .await;
    let batch: koru_events::telemetry::TelemetryBatch =
        body_json(handle.request(get("/v1/events/telemetry")).await).await;
    assert_eq!(
        batch.events.len(),
        1,
        "only the event recorded after the yes is queued"
    );
    let installation = batch
        .anonymous_installation_id
        .clone()
        .expect("the window shows the id an uploader would send");

    // A repeated yes keeps what the first yes queued.
    handle
        .request(put_json("/v1/events/telemetry/consent", &consent))
        .await;
    handle
        .request(post_json(
            "/v1/events",
            &EventBatch {
                events: vec![imported(5)],
            },
        ))
        .await;
    let batch: koru_events::telemetry::TelemetryBatch =
        body_json(handle.request(get("/v1/events/telemetry")).await).await;
    assert_eq!(
        batch.events.len(),
        2,
        "re-granting consent does not clear it"
    );
    assert_eq!(
        batch.anonymous_installation_id.as_deref(),
        Some(installation.as_str()),
        "the id is stable while consent stands"
    );

    let withdrawn = ConsentBody {
        granted: false,
        consent_version: "v1".into(),
    };
    handle
        .request(put_json("/v1/events/telemetry/consent", &withdrawn))
        .await;
    let batch: koru_events::telemetry::TelemetryBatch =
        body_json(handle.request(get("/v1/events/telemetry")).await).await;
    assert!(
        batch.events.is_empty(),
        "withdrawing consent deletes the spool"
    );
    assert!(
        batch.anonymous_installation_id.is_none(),
        "and the id with it, so opting back in is a rotation"
    );
}

#[tokio::test]
async fn status_answers_what_the_worker_is_doing() {
    let (_dir, handle) = handle();
    handle
        .request(post_json(
            "/v1/events",
            &EventBatch {
                events: vec![imported(5)],
            },
        ))
        .await;
    let status: EventsStatus = body_json(handle.request(get("/v1/events/status")).await).await;
    assert!(!status.disabled);
    assert_eq!(status.total_events, 1);
    assert_eq!(status.last_seq, 1);
    assert_eq!(status.sse_subscribers, 0);
}

#[tokio::test]
async fn the_stream_delivers_and_resumes_from_the_cursor() {
    let (_dir, handle) = handle();
    handle
        .request(post_json(
            "/v1/events",
            &EventBatch {
                events: vec![imported(6), imported(7)],
            },
        ))
        .await;

    // From the top: the first frame is seq 1.
    let response = handle.request(get("/v1/events/stream?after=0")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), body.next())
        .await
        .expect("the stream answers immediately when events exist")
        .unwrap()
        .unwrap();
    let first = String::from_utf8_lossy(&first).to_string();
    assert!(first.contains("id: 1"), "got: {first}");

    // The SSE reconnect header resumes past what was already seen.
    let resume = Request::builder()
        .uri("/v1/events/stream")
        .header("last-event-id", "1")
        .body(Body::empty())
        .unwrap();
    let response = handle.request(resume).await;
    let mut body = response.into_body().into_data_stream();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), body.next())
        .await
        .expect("resume answers immediately")
        .unwrap()
        .unwrap();
    let frame = String::from_utf8_lossy(&frame).to_string();
    assert!(frame.contains("id: 2"), "got: {frame}");
    assert!(
        !frame.contains("id: 1"),
        "seq 1 was already delivered: {frame}"
    );
}

#[tokio::test]
async fn a_new_append_wakes_a_waiting_stream() {
    let (_dir, handle) = handle();
    let response = handle.request(get("/v1/events/stream?after=0")).await;
    let mut body = response.into_body().into_data_stream();
    let waiting = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
            .await
            .expect("the append must wake the stream")
            .unwrap()
            .unwrap()
    });
    // Give the stream a moment to arm its waiter, then publish.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    handle
        .request(post_json(
            "/v1/events",
            &EventBatch {
                events: vec![imported(8)],
            },
        ))
        .await;
    let frame = String::from_utf8_lossy(&waiting.await.unwrap()).to_string();
    assert!(frame.contains("id: 1"), "got: {frame}");
}

/// **A host that is shutting down closes the tails rather than being waited
/// on.**
///
/// A tail never ends on its own — it is held until the client's own
/// ninety-second deadline cuts it — and a graceful HTTP shutdown waits for
/// in-flight responses, so one attached window made a gateway asked to stop
/// take 90.1 s to exit. `close_streams` is what the host calls first: a parked
/// tail is woken and the stream *ends*, which completes the response body.
#[tokio::test]
async fn closing_the_worker_ends_a_parked_stream_rather_than_holding_it() {
    let (_dir, handle) = handle();
    let response = handle.request(get("/v1/events/stream?after=0")).await;
    let mut body = response.into_body().into_data_stream();
    let ending = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
            .await
            .expect("closing the worker must end the parked stream")
    });
    // The tail is parked on its waiter with nothing to deliver — the state a
    // quiet window sits in for ninety seconds at a time.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    handle.close_streams();
    assert!(
        ending.await.unwrap().is_none(),
        "the stream produced a frame instead of ending"
    );
}

/// And a tail opened *after* the host started shutting down ends at once
/// rather than parking, so a client reconnecting into the drain cannot hold
/// the door open behind it.
#[tokio::test]
async fn a_stream_opened_after_the_close_ends_at_once() {
    let (_dir, handle) = handle();
    handle
        .request(post_json(
            "/v1/events",
            &EventBatch {
                events: vec![imported(9)],
            },
        ))
        .await;
    handle.close_streams();
    let response = handle.request(get("/v1/events/stream?after=0")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
        .await
        .expect("a stream opened during a shutdown must not park");
    assert!(
        frame.is_none(),
        "a closing worker delivered a frame instead of ending the stream"
    );
}

#[tokio::test]
async fn a_batch_over_the_cap_is_refused_whole() {
    let (_dir, handle) = handle();
    let events: Vec<Envelope> = (0..=koru_events::wire::MAX_PUBLISH_EVENTS as u128)
        .map(|id| imported(id + 100))
        .collect();
    let response = handle
        .request(post_json("/v1/events", &EventBatch { events }))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

// ------------------------------------------------------- the activity read

/// A task, by the triple behind the `task:<workspace>:<document>:<node>`
/// spelling — the typed fixture, so a test cannot disagree with the wrapper
/// about how a subject is written.
fn task(node: u128) -> koru_events::TaskSubject {
    koru_events::TaskSubject {
        workspace: uuid::Uuid::from_u128(0x1111),
        document: uuid::Uuid::from_u128(0x2222),
        node: uuid::Uuid::from_u128(0x3000 + node),
    }
}

/// One `task.completed`, through the typed payload rather than a hand-built
/// draft: the reason token is then the lifecycle enum's spelling and nothing
/// else (roadmap §3.10). `correlation: None` is an event that started its own
/// chain, which is what a first gesture looks like.
fn task_completed(
    id: u128,
    task: koru_events::TaskSubject,
    reason: koru_events::TaskCompletedReason,
    correlation: Option<EventId>,
) -> Envelope {
    let mut draft = Draft::from(koru_events::TaskCompleted { task, reason });
    if let Some(root) = correlation {
        draft = draft.in_chain(root);
    }
    draft.build(
        &EmitContext::user_session(),
        EventId(uuid::Uuid::from_u128(id)),
        1_756_000_000_000,
    )
}

async fn publish(handle: &EventsHandle, events: Vec<Envelope>) -> AppendReceipt {
    body_json(
        handle
            .request(post_json("/v1/events", &EventBatch { events }))
            .await,
    )
    .await
}

fn activity_path(task: koru_events::TaskSubject, after: u64, limit: u32) -> String {
    let subject = task.subject();
    format!(
        "/v1/events/activity?kind={}&id={}&after={after}&limit={limit}",
        subject.kind.token(),
        subject.id.replace(':', "%3A"),
    )
}

/// The bound is the whole point (SPEC §9): a page is at most `limit`, the
/// cursor advances by the journal's `seq`, and paging with it walks the
/// subject's history once, in order, with nothing repeated or skipped.
#[tokio::test]
async fn activity_pages_are_bounded_and_the_cursor_advances() {
    let (_dir, handle) = handle();
    let subject = task(1);
    let noise = task(2);
    // Interleaved, so a page that ignored the subject would be caught by the
    // count as well as by the ids.
    let mut events = Vec::new();
    for step in 0..5u128 {
        events.push(task_completed(
            100 + step,
            subject,
            koru_events::TaskCompletedReason::ExplicitComplete,
            None,
        ));
        events.push(task_completed(
            200 + step,
            noise,
            koru_events::TaskCompletedReason::ExplicitComplete,
            None,
        ));
    }
    assert_eq!(publish(&handle, events).await.accepted, 10);

    let first: ActivityPage =
        body_json(handle.request(get(&activity_path(subject, 0, 2))).await).await;
    assert_eq!(first.entries.len(), 2, "the limit bounds the page");
    assert_eq!(first.retention_days, crate::journal::RETENTION_DAYS);
    assert!(
        first
            .entries
            .windows(2)
            .all(|pair| pair[0].seq < pair[1].seq),
        "oldest first, by seq"
    );
    assert_eq!(first.next_after, first.entries[1].seq);

    let mut seen: Vec<u64> = first.entries.iter().map(|entry| entry.seq).collect();
    let mut cursor = first.next_after;
    loop {
        let page: ActivityPage = body_json(
            handle
                .request(get(&activity_path(subject, cursor, 2)))
                .await,
        )
        .await;
        assert!(page.entries.len() <= 2);
        if page.entries.is_empty() {
            assert_eq!(page.next_after, cursor, "an empty page does not move");
            break;
        }
        seen.extend(page.entries.iter().map(|entry| entry.seq));
        cursor = page.next_after;
    }
    assert_eq!(seen.len(), 5, "every event of this subject, exactly once");
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));

    // The other task's history is not in it — the match is exact on both
    // halves, so one task's activity can never answer another's.
    let other: ActivityPage =
        body_json(handle.request(get(&activity_path(noise, 0, 200))).await).await;
    assert_eq!(other.entries.len(), 5);
    assert!(other.entries.iter().all(|entry| !seen.contains(&entry.seq)));
}

/// A limit above the ceiling is clamped rather than refused, and a limit of
/// zero is a page rather than an error — the same contract the journal's own
/// read has, so a caller cannot ask its way into an unbounded answer.
#[tokio::test]
async fn an_oversized_limit_is_clamped_not_refused() {
    let (_dir, handle) = handle();
    let subject = task(1);
    let events: Vec<Envelope> = (0..3u128)
        .map(|step| {
            task_completed(
                300 + step,
                subject,
                koru_events::TaskCompletedReason::ExplicitComplete,
                None,
            )
        })
        .collect();
    publish(&handle, events).await;
    for limit in [0, 1, koru_events::wire::MAX_ACTIVITY_LIMIT + 5_000] {
        let response = handle.request(get(&activity_path(subject, 0, limit))).await;
        assert_eq!(response.status(), StatusCode::OK, "limit={limit}");
        let page: ActivityPage = body_json(response).await;
        assert!(
            page.entries.len() <= koru_events::wire::MAX_ACTIVITY_LIMIT as usize,
            "limit={limit}"
        );
    }
}

/// **A cascade is a set of events sharing one correlation, not one event with
/// a count** — so the row carries the group key, and an event that started its
/// own chain reports its own id rather than an absence the reader has to know
/// the rule for.
#[tokio::test]
async fn a_cascade_shares_one_correlation_and_the_page_carries_it() {
    let (_dir, handle) = handle();
    let parent = task(1);
    let child = task(2);
    let root = EventId(uuid::Uuid::from_u128(400));
    publish(
        &handle,
        vec![
            // The gesture: the parent, its own chain root.
            task_completed(
                400,
                parent,
                koru_events::TaskCompletedReason::ExplicitComplete,
                None,
            ),
            // What it cascaded into, under the same correlation.
            task_completed(
                401,
                child,
                koru_events::TaskCompletedReason::CascadeFromAncestor,
                Some(root),
            ),
        ],
    )
    .await;

    let parent_page: ActivityPage =
        body_json(handle.request(get(&activity_path(parent, 0, 50))).await).await;
    let child_page: ActivityPage =
        body_json(handle.request(get(&activity_path(child, 0, 50))).await).await;
    assert_eq!(parent_page.entries.len(), 1);
    assert_eq!(child_page.entries.len(), 1);
    assert_eq!(
        parent_page.entries[0].correlation_id, child_page.entries[0].correlation_id,
        "the two halves of one gesture group together"
    );
    assert_eq!(parent_page.entries[0].correlation_id, root);
    assert_eq!(
        parent_page.entries[0].reason(),
        Some("explicit_complete"),
        "the reason token travels"
    );
    assert_eq!(
        child_page.entries[0].reason(),
        Some("cascade_from_ancestor")
    );
    assert_eq!(parent_page.entries[0].actor, koru_events::ActorKind::User);
    assert_eq!(parent_page.entries[0].actor_token, None);
    assert_eq!(
        parent_page.entries[0].name.as_str(),
        "task.completed",
        "display-only occurred_at travels beside the name"
    );
    assert_eq!(parent_page.entries[0].occurred_at_ms, 1_756_000_000_000);
}

/// Activity is device-local with a 90-day domain retention (SPEC §9). An event
/// past that window is **absent**, not stale — and the page still says how far
/// back it could have reached, which is the only way an empty answer can be
/// read honestly.
///
/// Retention is amortized onto the write path rather than run on a timer, so
/// the fixture drives `prune` directly with a clock 90 days on instead of
/// publishing a quarter of a million events to trip the counter.
#[tokio::test]
async fn an_event_past_the_retention_window_is_absent_and_the_page_says_so() {
    let (_dir, handle) = handle();
    let subject = task(1);
    publish(
        &handle,
        vec![task_completed(
            500,
            subject,
            koru_events::TaskCompletedReason::ExplicitComplete,
            None,
        )],
    )
    .await;
    let before: ActivityPage =
        body_json(handle.request(get(&activity_path(subject, 0, 50))).await).await;
    assert_eq!(before.entries.len(), 1);

    // Ninety days and a day later, the write path's retention sweep runs.
    let later = crate::state::now_ms() + (crate::journal::RETENTION_DAYS as i64 + 1) * 86_400_000;
    handle
        .state
        .with_journal(|journal| journal.prune_now(later), || ())
        .unwrap();

    let after: ActivityPage =
        body_json(handle.request(get(&activity_path(subject, 0, 50))).await).await;
    assert!(after.entries.is_empty(), "nothing outlives the window");
    assert_eq!(after.next_after, 0, "an empty page leaves the cursor alone");
    assert_eq!(
        after.retention_days, 90,
        "and the answer says how far back it could have reached"
    );
}

/// A subject nobody has ever filed an event under is an empty page, not a 404:
/// a task with no history is a normal task.
#[tokio::test]
async fn an_unknown_subject_is_an_empty_page() {
    let (_dir, handle) = handle();
    let response = handle.request(get(&activity_path(task(9), 0, 50))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ActivityPage = body_json(response).await;
    assert!(page.entries.is_empty());
    assert_eq!(page.next_after, 0);
    assert_eq!(page.retention_days, crate::journal::RETENTION_DAYS);
}

/// The kind is half the identity, so a kind outside the closed set is refused
/// with a code — never defaulted, which would answer a different subject.
#[tokio::test]
async fn an_unknown_subject_kind_is_refused_with_a_code() {
    let (_dir, handle) = handle();
    let response = handle
        .request(get("/v1/events/activity?kind=sandwich&id=x"))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = body_json(response).await;
    assert_eq!(body["code"], "unknown_subject_kind");

    let response = handle
        .request(get("/v1/events/activity?kind=task&id="))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = body_json(response).await;
    assert_eq!(body["code"], "missing_subject_id");
}

/// The counters answer "is a pane scanning?" and carry no reference, no name
/// and no id — `/v1/events/status` is a debug surface and publishes counters
/// only (roadmap §3.4).
#[tokio::test]
async fn the_activity_counters_move_and_carry_no_content() {
    let (_dir, handle) = handle();
    let subject = task(1);
    publish(
        &handle,
        vec![
            task_completed(
                600,
                subject,
                koru_events::TaskCompletedReason::ExplicitComplete,
                None,
            ),
            task_completed(
                601,
                subject,
                koru_events::TaskCompletedReason::CheckboxCompleted,
                None,
            ),
        ],
    )
    .await;
    let before: EventsStatus = body_json(handle.request(get("/v1/events/status")).await).await;
    assert_eq!((before.activity_queries, before.activity_rows), (0, 0));

    handle.request(get(&activity_path(subject, 0, 50))).await;
    handle.request(get(&activity_path(task(9), 0, 50))).await;

    let after: EventsStatus = body_json(handle.request(get("/v1/events/status")).await).await;
    assert_eq!(
        after.activity_queries, 2,
        "an empty answer is still a query"
    );
    assert_eq!(after.activity_rows, 2);
    let rendered = serde_json::to_string(&after).unwrap();
    assert!(
        !rendered.contains("task:") && !rendered.contains("task."),
        "the status carries counters, never a reference or an event name: {rendered}"
    );
}

#[tokio::test]
async fn every_route_row_is_served() {
    let (_dir, handle) = handle();
    for route in ROUTES {
        let request = match route.method {
            // The activity read needs a subject; without one it is a 400,
            // which this test's assertion already accepts as "served".
            "GET" if route.path == "/v1/events/activity" => {
                get("/v1/events/activity?kind=task&id=task:a:b:c")
            }
            "GET" => get(route.path),
            "POST" => post_json(route.path, &EventBatch { events: Vec::new() }),
            "PUT" => put_json(
                route.path,
                &ConsentBody {
                    granted: false,
                    consent_version: String::new(),
                },
            ),
            other => panic!("teach this test about {other}"),
        };
        let response = handle.request(request).await;
        assert!(
            response.status() != StatusCode::NOT_FOUND
                && response.status() != StatusCode::METHOD_NOT_ALLOWED,
            "{} {} answered {}",
            route.method,
            route.path,
            response.status()
        );
    }
}
