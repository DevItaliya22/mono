// SPDX-License-Identifier: MPL-2.0

//! The `/v1/events` API, folded from one table — the koru-store-api idiom,
//! scaled down: a route that is not a row is not served, and the OpenAPI
//! entries the gateway merges come from the same rows.
//!
//! **A handler never waits before returning its response.** The dispatcher
//! that forwards requests into this router serves them one spawn each, but a
//! handler that blocked on "more events" would still hold a connection's
//! worth of state hostage — so the pull read returns what is there, and
//! waiting is the SSE stream's job, expressed as a stream body the *caller's*
//! runtime polls.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post, put};
use axum::{Json, Router};
use futures_util::Stream;
use koru_events::wire::{
    ActivityEntry, ActivityPage, ConsentBody, DEFAULT_ACTIVITY_LIMIT, EventBatch, EventPage,
    MAX_ACTIVITY_LIMIT, MAX_PUBLISH_EVENTS, StoredEvent,
};
use serde::Deserialize;

use crate::state::{EventsState, now_ms};

/// One endpoint: documentation and wiring from one row.
pub struct Route {
    pub method: &'static str,
    pub path: &'static str,
    pub handler: fn() -> MethodRouter<Arc<EventsState>>,
    /// Internal routes exist for the tray, tests and the Milestone-2
    /// uploader; they are left out of the public OpenAPI document.
    pub public: bool,
    pub summary: &'static str,
}

/// Every route this worker serves. Adding one means adding a row — the test
/// asserts the router and this table agree.
pub const ROUTES: &[Route] = &[
    Route {
        method: "POST",
        path: "/v1/events",
        handler: || post(publish),
        public: true,
        summary: "Publish a batch of events; idempotent by event_id",
    },
    Route {
        method: "GET",
        path: "/v1/events",
        handler: || get(page),
        public: true,
        summary: "Read one page of the journal after a cursor",
    },
    Route {
        method: "GET",
        path: "/v1/events/activity",
        handler: || get(activity),
        public: true,
        summary: "One subject's activity, bounded and paged; never a journal scan",
    },
    Route {
        method: "GET",
        path: "/v1/events/stream",
        handler: || get(stream),
        public: true,
        summary: "Server-sent events from a cursor; id = seq, Last-Event-ID resumes",
    },
    Route {
        method: "GET",
        path: "/v1/events/status",
        handler: || get(status),
        public: false,
        summary: "What the events worker is doing — counters, never content",
    },
    Route {
        method: "GET",
        path: "/v1/events/telemetry",
        handler: || get(telemetry),
        public: false,
        summary: "The pending telemetry outbox, as the batch an uploader would send",
    },
    Route {
        method: "PUT",
        path: "/v1/events/telemetry/consent",
        handler: || put(consent),
        public: false,
        summary: "Grant or withdraw telemetry consent; withdrawing deletes the outbox",
    },
];

pub fn router(state: Arc<EventsState>) -> Router {
    let mut router = Router::new();
    for route in ROUTES {
        router = router.route(route.path, (route.handler)());
    }
    router.with_state(state)
}

async fn publish(State(state): State<Arc<EventsState>>, body: Json<EventBatch>) -> Response {
    if body.events.len() > MAX_PUBLISH_EVENTS {
        return failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "batch_too_large",
            "a batch carries at most 500 events",
        );
    }
    match state.append(&body.events) {
        Ok(receipt) => Json(receipt).into_response(),
        Err(error) => storage_failure(error),
    }
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_limit")]
    limit: u32,
    name: Option<String>,
    workspace: Option<String>,
}

fn default_limit() -> u32 {
    256
}

async fn page(State(state): State<Arc<EventsState>>, Query(query): Query<PageQuery>) -> Response {
    let result = state.with_journal(
        |journal| {
            journal.read_after(
                query.after,
                query.limit,
                query.name.as_deref(),
                query.workspace.as_deref(),
            )
        },
        || EventPage {
            events: Vec::new(),
            next_after: query.after,
        },
    );
    match result {
        Ok(page) => Json(page).into_response(),
        Err(error) => storage_failure(error),
    }
}

/// What `GET /v1/events/activity` takes. The subject travels as **two**
/// parameters, not one string: a document subject's id is a bare uuid while a
/// task's already begins `task:`, so a single field would have to be parsed
/// differently per kind — and guessing wrong answers one subject's question
/// with another's history.
///
/// `kind` is a `String` rather than a `SubjectKind` so an unknown value is
/// *our* refusal, with the code and the accepted set in it, instead of Axum's
/// bare rejection body. The same reason the store-api handlers parse their own
/// enums.
#[derive(Deserialize)]
struct ActivityQuery {
    kind: String,
    id: String,
    #[serde(default)]
    after: u64,
    limit: Option<u32>,
}

/// **One subject's activity, bounded — never a journal scan per pane**
/// (SPEC §9). The read is an index seek on `(subject_kind, subject_id, seq)`,
/// so the cost is the page, not the journal.
///
/// The rows are [`ActivityEntry`], a projection: what happened, when, who and
/// which gesture — never the device, the profile, or the emitting session that
/// a stored envelope also carries. See the type for why that is a different
/// shape rather than a filter applied here.
async fn activity(
    State(state): State<Arc<EventsState>>,
    Query(query): Query<ActivityQuery>,
) -> Response {
    let Some(kind) = koru_events::SubjectKind::from_token(&query.kind) else {
        let accepted: Vec<&str> = koru_events::SubjectKind::ALL
            .iter()
            .map(|kind| kind.token())
            .collect();
        return failure(
            StatusCode::BAD_REQUEST,
            "unknown_subject_kind",
            &format!("kind must be one of {}", accepted.join(", ")),
        );
    };
    if query.id.is_empty() {
        return failure(
            StatusCode::BAD_REQUEST,
            "missing_subject_id",
            "id names the subject whose activity is asked for",
        );
    }
    // Clamped, never refused: a caller asking for more than a page gets a
    // page, which is what the journal's own limit already does.
    let limit = query
        .limit
        .unwrap_or(DEFAULT_ACTIVITY_LIMIT)
        .clamp(1, MAX_ACTIVITY_LIMIT);
    let result = state.with_journal(
        |journal| journal.read_subject(kind, &query.id, query.after, limit),
        || EventPage {
            events: Vec::new(),
            next_after: query.after,
        },
    );
    match result {
        Ok(page) => {
            state.activity_queries.fetch_add(1, Ordering::Relaxed);
            state
                .activity_rows
                .fetch_add(page.events.len() as u64, Ordering::Relaxed);
            Json(ActivityPage {
                entries: page.events.iter().map(ActivityEntry::from).collect(),
                next_after: page.next_after,
                retention_days: crate::journal::RETENTION_DAYS,
            })
            .into_response()
        }
        Err(error) => storage_failure(error),
    }
}

/// The realtime tail. `Last-Event-ID` (the SSE reconnect header) wins over
/// `?after`, so a dropped stream resumes gap-free without the client doing
/// anything — the cursor is the whole protocol.
async fn stream(
    State(state): State<Arc<EventsState>>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(query.after);
    state.sse_subscribers.fetch_add(1, Ordering::Relaxed);
    let guard = SubscriberGuard(Arc::clone(&state));
    let stream = futures_util::stream::unfold(
        StreamState {
            state,
            after,
            name: query.name,
            workspace: query.workspace,
            pending: std::collections::VecDeque::new(),
            _guard: guard,
        },
        |mut tail| async move {
            loop {
                // **The host is stopping: end the tail rather than be waited
                // on.** Checked here, at the top, so it catches a tail parked
                // on `notified` (woken by `close_streams`), a tail mid-page,
                // and a tail that opened after the flag was set. Ending the
                // stream completes the response body, which is what lets a
                // graceful HTTP shutdown finish instead of waiting out the
                // client's ninety-second deadline.
                if tail.state.closing() {
                    return None;
                }
                if let Some(event) = tail.pending.pop_front() {
                    return Some((Ok(sse_event(&event)), tail));
                }
                // Arm the waiter BEFORE reading: `notify_waiters` wakes only
                // waiters registered at that moment, so an append landing
                // between the read and a later `notified().await` would be a
                // lost wakeup and a stalled stream. `enable()` registers now;
                // the read then decides whether to wait at all.
                let mut notified = std::pin::pin!(tail.state.notify.notified());
                notified.as_mut().enable();
                if tail.state.disabled() {
                    // A disabled worker holds the stream open and quiet: the
                    // keep-alive is the only traffic, and nothing ever comes.
                    notified.await;
                    continue;
                }
                let page = tail.state.with_journal(
                    |journal| {
                        journal.read_after(
                            tail.after,
                            256,
                            tail.name.as_deref(),
                            tail.workspace.as_deref(),
                        )
                    },
                    || EventPage {
                        events: Vec::new(),
                        next_after: tail.after,
                    },
                );
                match page {
                    Ok(page) if page.events.is_empty() => notified.await,
                    Ok(page) => {
                        tail.after = page.next_after;
                        tail.pending.extend(page.events);
                    }
                    Err(_) => return None, // a broken journal ends the stream; the client reconnects
                }
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(20)))
}

struct StreamState {
    state: Arc<EventsState>,
    after: u64,
    name: Option<String>,
    workspace: Option<String>,
    pending: std::collections::VecDeque<StoredEvent>,
    _guard: SubscriberGuard,
}

struct SubscriberGuard(Arc<EventsState>);

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        self.0.sse_subscribers.fetch_sub(1, Ordering::Relaxed);
    }
}

fn sse_event(stored: &StoredEvent) -> Event {
    let data = serde_json::to_string(stored).unwrap_or_else(|_| "{}".into());
    if crate::state::debug_enabled() {
        tracing::info!(
            target: "koru_events",
            name = %stored.envelope.name,
            event_id = %stored.envelope.event_id,
            seq = stored.seq,
            "delivered"
        );
    }
    Event::default().id(stored.seq.to_string()).data(data)
}

async fn status(State(state): State<Arc<EventsState>>) -> Response {
    match state.status() {
        Ok(status) => Json(status).into_response(),
        Err(error) => storage_failure(error),
    }
}

async fn telemetry(State(state): State<Arc<EventsState>>) -> Response {
    let result = state.with_journal(
        |journal| {
            let consent = journal.consent()?;
            let events = journal.telemetry_pending(5_000)?;
            // The window is meant to be *what an uploader would send*, so it
            // carries the installation id the drain would put on the batch —
            // minted here if this is the first ask, and only under consent.
            let installation = match consent.granted {
                true => Some(journal.installation_id()?),
                false => None,
            };
            Ok((consent, events, installation))
        },
        || (koru_events::telemetry::Consent::none(), Vec::new(), None),
    );
    match result {
        Ok((consent, events, installation)) => {
            let info = koru_channel::build_info();
            Json(koru_events::telemetry::TelemetryBatch {
                batch_id: uuid::Uuid::new_v4().to_string(),
                sent_at_ms: now_ms(),
                app_version: info.version.to_string(),
                build_channel: info.channel.to_string(),
                platform: std::env::consts::OS.to_string(),
                consent_version: consent.consent_version,
                anonymous_installation_id: installation,
                events,
            })
            .into_response()
        }
        Err(error) => storage_failure(error),
    }
}

async fn consent(State(state): State<Arc<EventsState>>, body: Json<ConsentBody>) -> Response {
    let result = state.with_journal(
        |journal| {
            journal.set_consent(body.granted, &body.consent_version)?;
            journal.consent()
        },
        koru_events::telemetry::Consent::none,
    );
    match result {
        Ok(consent) => Json(ConsentBody {
            granted: consent.granted,
            consent_version: consent.consent_version,
        })
        .into_response(),
        Err(error) => storage_failure(error),
    }
}

fn storage_failure(error: crate::journal::JournalError) -> Response {
    tracing::error!(target: "koru_events", %error, "events journal failed");
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "storage_failed",
        "the events journal did not answer",
    )
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    let mut response =
        Json(serde_json::json!({ "code": code, "message": message })).into_response();
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests;
