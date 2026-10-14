// SPDX-License-Identifier: MPL-2.0

//! The client of the hosted metrics service — and the only client code that
//! names `m.koru.do`.
//!
//! **A shipped binary holds no credential.** The ingest route is anonymous by
//! design — a shipped binary cannot hold a secret (ADR-39) — so uploading is a
//! POST of the `TelemetryBatch` wire shape to `{base}/v1/metrics`, and where
//! the events go from there, which provider and which project, is the
//! service's configuration and never this crate's. The service, and the
//! reasoning: `docs/architecture/eventbus/README.md`.
//!
//! ## What this crate is for
//!
//! The events worker's drain keeps its mechanics — consent gate, outbox read
//! under no lock, backoff ladder, split-on-permanent — and delivers through
//! [`TelemetryClient`] instead of straight to a vendor. What changed hands is
//! the destination only: the vendor credential that used to sit in the binary
//! is gone, and the service is now the only PostHog client.
//!
//! ## Two pure functions
//!
//! [`body`] and [`classify`] carry all of the behaviour, which is why the
//! tests need no HTTP mock: what is left in [`TelemetryClient::deliver`] is
//! the request itself, plus the two answers only the wire can give — the
//! `TelemetryAck` a `200` carries, and a status.
//!
//! ## A `200` is parsed, not just acked
//!
//! The service validates every event against the registry and names the ones
//! it refuses permanently in `TelemetryAck.rejected`. [`Sink::deliver`] logs
//! one warning per rejection — the `event_id` and the `reason`, the two things
//! needed to find the offending registry row — and still answers
//! [`Delivery::Ok`]: a rejected event will never be accepted, so the drain
//! acks the whole batch rather than retrying a refusal forever. The warn is
//! what keeps "dropped by the server" from being silent.

use std::time::Duration;

use koru_events::telemetry::{MAX_BATCH_EVENTS, TelemetryAck, TelemetryBatch, TelemetryEvent};
use koru_metrics_sink::{Delivery, Sink, SinkEvent, SinkId, code};

pub const KORU_METRICS_SERVER: SinkId = SinkId("koru-metrics-server");

/// Where an installation uploads when nothing says otherwise: the hosted
/// metrics service, TLS terminated at Cloudflare. `KORU_TELEMETRY_BASE_URL`
/// overrides it — a local service during development.
pub const DEFAULT_BASE_URL: &str = "https://m.koru.do";

/// A refusal between us and the service is not a reason to throw a queue away,
/// so 401/403 is retryable — but retrying it every five seconds is a log full
/// of nothing. The ingest route is anonymous, so this status is reachable only
/// through a misconfigured proxy; fifteen minutes is long enough that fixing
/// the config and restarting is the faster path, which is the behaviour we
/// want.
const UNAUTHORIZED_FLOOR: Duration = Duration::from_secs(15 * 60);

/// Where and how to reach the metrics service.
///
/// **The base URL comes from the caller, never from this crate.** Reading the
/// environment here would put the destination policy inside the adapter, where
/// the host cannot see it.
#[derive(Clone, Debug)]
pub struct Config {
    /// e.g. `https://m.koru.do`. A trailing slash is tolerated.
    pub base_url: String,
    /// Events per outbound request.
    pub batch: usize,
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct BuildError(String);

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the telemetry client could not be built: {}", self.0)
    }
}

impl std::error::Error for BuildError {}

pub struct TelemetryClient {
    client: reqwest::blocking::Client,
    endpoint: String,
    batch: usize,
}

impl TelemetryClient {
    pub fn new(config: Config) -> Result<Self, BuildError> {
        if config.base_url.trim().is_empty() {
            return Err(BuildError("a base URL is required".into()));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| BuildError(error.to_string()))?;
        Ok(Self {
            client,
            endpoint: endpoint(&config.base_url),
            batch: config.batch.clamp(1, MAX_BATCH_EVENTS),
        })
    }

    /// Where this client posts. Public so a boot line can name it.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Sink for TelemetryClient {
    fn id(&self) -> SinkId {
        KORU_METRICS_SERVER
    }

    fn max_batch(&self) -> usize {
        self.batch
    }

    fn deliver(&self, events: &[SinkEvent]) -> Delivery {
        // A string id that does not parse came from OUR drain, not from the
        // network — that is a bug, and it is said at `error`. Permanent, so
        // the drain's split isolates the offender and the rest of the batch
        // still goes; the event's name is what a person needs, never a
        // property.
        let mut bug = false;
        for event in events {
            if pack(event).is_none() {
                bug = true;
                tracing::error!(
                    target: "koru_metrics",
                    sink = %KORU_METRICS_SERVER,
                    name = %event.name,
                    version = event.version,
                    "the drain handed an id that does not parse; the event is isolated and dropped — this is a bug"
                );
            }
        }
        if bug {
            return Delivery::Permanent {
                code: code::BAD_REQUEST,
            };
        }
        if events.is_empty() {
            // The port promises a batch is never empty, but an empty one has
            // no request to make and Ok is the only honest answer.
            return Delivery::Ok;
        }
        let response = self.client.post(&self.endpoint).json(&body(events)).send();
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                // Deliberately not `%error`: a reqwest error's Display carries
                // the URL, and the URL is one config mistake away from
                // carrying a query string. The kind is what a person needs.
                tracing::warn!(
                    target: "koru_metrics",
                    sink = %KORU_METRICS_SERVER,
                    kind = if error.is_timeout() { "timeout" } else { "transport" },
                    "the metrics request did not complete"
                );
                return Delivery::Retry {
                    after: None,
                    code: code::TIMEOUT,
                };
            }
        };
        let status = response.status().as_u16();
        if status == 200 {
            return match response.json::<TelemetryAck>() {
                // See the module note: a rejection is a permanent refusal,
                // named so the registry row can be found, and the batch is
                // acked whole — a refusal retried is a refusal forever.
                Ok(ack) => {
                    for rejection in &ack.rejected {
                        tracing::warn!(
                            target: "koru_metrics",
                            sink = %KORU_METRICS_SERVER,
                            event_id = %rejection.event_id,
                            reason = %rejection.reason,
                            "the metrics service refused an event permanently; it is acked away, not retried"
                        );
                    }
                    Delivery::Ok
                }
                // A 200 with no readable ack is a proxy answering, not the
                // service. Retried rather than acked: the bytes may never have
                // arrived, and a duplicate delivery is deduped on the event
                // id, so retrying is the cheap mistake.
                Err(_) => {
                    tracing::warn!(
                        target: "koru_metrics",
                        sink = %KORU_METRICS_SERVER,
                        "the metrics answer was a 200 without a readable ack; the batch is retried"
                    );
                    Delivery::Retry {
                        after: None,
                        code: code::UNEXPECTED,
                    }
                }
            };
        }
        let retry_after = retry_after(response.headers());
        let delivery = classify(status, retry_after);
        if matches!(delivery, Delivery::Retry { code: c, .. } if c == code::UNAUTHORIZED) {
            // The one outcome nobody discovers from a counter: it retries
            // forever and looks exactly like an outage until someone reads a
            // log. Said at `error`, and said once per attempt rather than once
            // per event.
            tracing::error!(
                target: "koru_metrics",
                sink = %KORU_METRICS_SERVER,
                status,
                "a proxy between the app and the metrics service refused the upload; the ingest route is anonymous, so the proxy is misconfigured — telemetry is queued, not lost"
            );
        }
        delivery
    }
}

/// `{base}/v1/metrics`, with however many slashes the base was written with.
fn endpoint(base_url: &str) -> String {
    format!("{}/v1/metrics", base_url.trim().trim_end_matches('/'))
}

/// One event as the wire carries it, or none: the drain hands ids as strings,
/// and one that does not parse cannot travel.
fn pack(event: &SinkEvent) -> Option<TelemetryEvent> {
    Some(TelemetryEvent {
        event_id: event.event_id.parse().ok()?,
        name: event.name.clone(),
        version: event.version,
        occurred_at_ms: event.occurred_at_ms,
        session_id: event.session_id.parse().ok()?,
        properties: event.properties.clone(),
    })
}

/// The `/v1/metrics` body.
///
/// Pure, so the whole mapping is testable without a server — which is the
/// reason the tests need no HTTP mock and no new dev-dependency.
///
/// **The batch header comes from the first event.** App version, channel,
/// platform, consent version and the anonymous installation id are identical
/// across a batch by construction — the drain stamps every event in one from
/// the same build and the same journal read — so carrying them on the batch
/// rather than on each event is a restatement, not a choice. `batch_id` is a
/// fresh v4 per call and `sent_at_ms` is now: both describe the upload
/// attempt, not the events.
pub fn body(events: &[SinkEvent]) -> serde_json::Value {
    let first = events.first();
    let batch = TelemetryBatch {
        batch_id: uuid::Uuid::new_v4().to_string(),
        sent_at_ms: now_ms(),
        app_version: first
            .map(|event| event.app_version.clone())
            .unwrap_or_default(),
        build_channel: first
            .map(|event| event.build_channel.clone())
            .unwrap_or_default(),
        platform: first
            .map(|event| event.platform.clone())
            .unwrap_or_default(),
        consent_version: first
            .map(|event| event.consent_version.clone())
            .unwrap_or_default(),
        anonymous_installation_id: first.and_then(|event| event.installation_id.clone()),
        // `deliver` refuses the batch on the first unparseable id before this
        // is ever called; skipping here is the pure function staying total,
        // not a silent drop.
        events: events.iter().filter_map(pack).collect(),
    };
    serde_json::to_value(&batch).expect("the telemetry wire shape serializes")
}

/// What an HTTP status means for a batch.
///
/// **The question is only ever whether time fixes it.** A `Retry` deletes
/// nothing; a `Permanent` tells the forwarder to find the offending event
/// rather than to discard the batch. The table mirrors the PostHog adapter's
/// because the problem is the same — and the `429` row is why this service
/// exists with a WAF in front of it: a rate-limit trip answers with a
/// `Retry-After`, so honouring it is the ordinary case, not an alarm.
pub fn classify(status: u16, retry_after: Option<Duration>) -> Delivery {
    match status {
        200..=299 => Delivery::Ok,
        // Not "the credential is dead" — the ingest route is anonymous, so a
        // refusal is a misconfigured proxy, fixed by a config change; a queue
        // discarded over it is unrecoverable.
        401 | 403 => Delivery::Retry {
            after: Some(
                retry_after
                    .unwrap_or(UNAUTHORIZED_FLOOR)
                    .max(UNAUTHORIZED_FLOOR),
            ),
            code: code::UNAUTHORIZED,
        },
        408 => Delivery::Retry {
            after: retry_after,
            code: code::TIMEOUT,
        },
        429 => Delivery::Retry {
            after: retry_after,
            code: code::RATE_LIMITED,
        },
        // The two the batch itself is wrong for: a malformed event, or a body
        // over the limit. Both are answered by splitting, never by resending.
        400 | 413 => Delivery::Permanent {
            code: code::BAD_REQUEST,
        },
        500..=599 => Delivery::Retry {
            after: retry_after,
            code: code::SERVER_ERROR,
        },
        // Anything else is retried rather than dropped. An unknown status is
        // not evidence that the bytes are bad, and losing data is the more
        // expensive of the two mistakes available here.
        _ => Delivery::Retry {
            after: retry_after,
            code: code::UNEXPECTED,
        },
    }
}

/// `Retry-After`, seconds form only — the form the WAF's rate-limit answer
/// carries. The HTTP-date form is legal and nothing on this path sends it;
/// parsing a format nothing sends is a second thing to get wrong, so an
/// unparseable header simply yields `None` and the caller's own backoff
/// applies.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Epoch milliseconds UTC — when the upload left, not when anything happened.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
