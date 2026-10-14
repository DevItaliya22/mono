// SPDX-License-Identifier: MPL-2.0

//! `client.runs()` — delegated runs through the gateway.
//!
//! Blocking calls over the gateway session this crate already holds, the
//! way the artifact namespace does it: the bearer, its renewal and the
//! loopback endpoint are the `Client`'s, and a call here adds only the
//! path and the body. [`RunsClient::stream`] follows a run: a thread of
//! its own reads the SSE body and hands each item to the caller's handler,
//! resuming from the last index across the gateway's routine reconnects.

use std::io::{BufRead as _, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use koru_delegate_api::wire::{
    CancelRunResponse, CreateRunRequest, CreateRunResponse, Failure, ListRunsResponse,
    PurgeRunResponse, RateRunRequest, RateRunResponse, RestoreRunResponse, ResumeRunRequest,
    ResumeRunResponse, RunStreamItem, RunView, TrashRunRequest, TrashRunResponse,
};
use koru_delegate_core::{ArtifactDisposition, RunId, WorkspaceId};

use crate::{Client, Error};

pub type RunsResult<T> = std::result::Result<T, Failure>;

pub struct RunsClient<'a> {
    pub(crate) client: &'a Client,
}

impl RunsClient<'_> {
    /// Whether the gateway published the `delegate` capability — whether
    /// the host composed a delegate transport. When false every call below
    /// answers `delegate_unavailable`.
    pub fn available(&self) -> bool {
        self.client
            .capabilities
            .iter()
            .any(|value| value == koru_delegate_api::CAPABILITY)
    }

    /// `POST /v1/runs`: durable before it answers.
    pub fn create(&self, request: &CreateRunRequest) -> RunsResult<CreateRunResponse> {
        self.json(
            reqwest::Method::POST,
            create_path(request.requested_model.is_some()).into(),
            Some(json_body(request)?),
        )
    }

    pub fn list(&self, workspace: WorkspaceId) -> RunsResult<ListRunsResponse> {
        self.json(
            reqwest::Method::GET,
            format!("/v1/runs?workspace={}", workspace.to_storage()),
            None,
        )
    }

    /// The header and a window of the history, oldest first from `from`.
    ///
    /// `workspace` is for a run this device did not create — one that
    /// reached it by sync. Without it the worker has nothing to find such
    /// a run by, and answers `unknown_run`.
    pub fn run(
        &self,
        run: RunId,
        workspace: Option<WorkspaceId>,
        from: u64,
        len: u32,
    ) -> RunsResult<RunView> {
        let mut path = format!("/v1/runs/{}?from={from}&len={len}", run.to_storage());
        if let Some(workspace) = workspace {
            path.push_str(&format!("&workspace={}", workspace.to_storage()));
        }
        self.json(reqwest::Method::GET, path, None)
    }

    pub fn cancel(&self, run: RunId) -> RunsResult<CancelRunResponse> {
        self.json(
            reqwest::Method::POST,
            format!("/v1/runs/{}/cancel", run.to_storage()),
            None,
        )
    }

    /// Continue an interrupted run as a fresh attempt. The ordinary case
    /// carries no body.
    pub fn resume(&self, run: RunId) -> RunsResult<ResumeRunResponse> {
        self.resume_with(run, ResumeRunRequest::default())
    }

    /// A resume with the person's answer. §7.6's last row — nothing proved
    /// the earlier attempt stopped — is never resumed without
    /// `confirm: true`, and the person is told earlier work may exist.
    pub fn resume_with(
        &self,
        run: RunId,
        request: ResumeRunRequest,
    ) -> RunsResult<ResumeRunResponse> {
        self.json(
            reqwest::Method::POST,
            resume_path(run),
            Some(json_body(&request)?),
        )
    }

    /// One gesture on a finished run (D44). The note travels here because
    /// it lives in the run's own content; nothing else ever carries it.
    pub fn rate(&self, run: RunId, request: &RateRunRequest) -> RunsResult<RateRunResponse> {
        self.json(
            reqwest::Method::POST,
            format!("/v1/runs/{}/rate", run.to_storage()),
            Some(json_body(request)?),
        )
    }

    /// **The worker's half of putting a run in the trash**: refuse one that
    /// is still working, and remember what to do with its results. The
    /// caller moves the Run document into the workspace's bin afterwards —
    /// that is a manifest edit, and the manifest is the window's.
    pub fn trash(
        &self,
        run: RunId,
        artifacts: ArtifactDisposition,
    ) -> RunsResult<TrashRunResponse> {
        let request = TrashRunRequest { artifacts };
        self.json(
            reqwest::Method::POST,
            format!("/v1/runs/{}/trash", run.to_storage()),
            Some(json_body(&request)?),
        )
    }

    /// Back out of the trash, whole.
    pub fn restore(&self, run: RunId) -> RunsResult<RestoreRunResponse> {
        self.json(
            reqwest::Method::POST,
            format!("/v1/runs/{}/restore", run.to_storage()),
            None,
        )
    }

    /// **The trash was emptied.** Everything this device holds about the
    /// run, and its working folder when Koru made that folder — never when
    /// the run worked inside the person's own project.
    pub fn purge(&self, run: RunId) -> RunsResult<PurgeRunResponse> {
        self.json(
            reqwest::Method::POST,
            format!("/v1/runs/{}/purge", run.to_storage()),
            None,
        )
    }

    /// Follow a run. `handler` runs on the subscription's own thread with
    /// every item in order; the subscription ends on `Closed`, on an error
    /// the gateway named, or when the handle is dropped.
    pub fn stream(
        &self,
        run: RunId,
        workspace: Option<WorkspaceId>,
        after: Option<u64>,
        mut handler: impl FnMut(RunsResult<RunStreamItem>) + Send + 'static,
    ) -> RunsResult<RunSubscription> {
        let client = self.client.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("koru-runs".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut cursor = after;
                let mut backoff = Duration::from_millis(250);
                while !thread_stop.load(Ordering::Relaxed) {
                    match tail_once(
                        &client,
                        run,
                        workspace,
                        &mut cursor,
                        &thread_stop,
                        &mut handler,
                    ) {
                        Ok(true) => break,
                        Ok(false) => backoff = Duration::from_millis(250),
                        Err(failure) if !failure.retryable => {
                            handler(Err(failure));
                            break;
                        }
                        Err(failure) => {
                            if thread_stop.load(Ordering::Relaxed) {
                                break;
                            }
                            handler(Err(failure));
                            std::thread::sleep(backoff);
                            backoff = (backoff * 2).min(Duration::from_secs(5));
                        }
                    }
                }
            })
            .map_err(|error| protocol_failure(&format!("could not start the reader: {error}")))?;
        Ok(RunSubscription {
            stop,
            join: Some(join),
        })
    }

    fn json<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: String,
        body: Option<Bytes>,
    ) -> RunsResult<T> {
        let protected_model_semantics = path.starts_with("/v2/runs");
        let response = self.execute(method, path, body, None)?;
        let status = response.status();
        let response = checked(response).map_err(|failure| {
            if protected_model_semantics
                && status == reqwest::StatusCode::NOT_FOUND
                && failure.code == "gateway_failure"
            {
                Failure::new(
                    "delegate_incompatible",
                    "Restart Koru to update the gateway before starting work with model selection.",
                    false,
                )
            } else {
                failure
            }
        })?;
        response
            .json()
            .map_err(|_| protocol_failure("The run response body was invalid."))
    }

    fn execute(
        &self,
        method: reqwest::Method,
        path: String,
        body: Option<Bytes>,
        accept: Option<&'static str>,
    ) -> RunsResult<reqwest::blocking::Response> {
        execute(self.client, method, path, body, accept)
    }
}

/// A live subscription; dropping it stops the reader.
pub struct RunSubscription {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl RunSubscription {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for RunSubscription {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// One connection's worth of the stream. `Ok(true)` is the final end.
fn tail_once(
    client: &Client,
    run: RunId,
    workspace: Option<WorkspaceId>,
    cursor: &mut Option<u64>,
    stop: &AtomicBool,
    handler: &mut (impl FnMut(RunsResult<RunStreamItem>) + Send),
) -> RunsResult<bool> {
    let mut path = format!("/v1/runs/{}/stream", run.to_storage());
    let mut separator = '?';
    if let Some(after) = cursor {
        path.push_str(&format!("{separator}after={after}"));
        separator = '&';
    }
    if let Some(workspace) = workspace {
        path.push_str(&format!("{separator}workspace={}", workspace.to_storage()));
    }
    let response = checked(execute(
        client,
        reqwest::Method::GET,
        path,
        None,
        Some("text/event-stream"),
    )?)?;
    let mut reader = BufReader::new(response);
    let mut line = String::new();
    let mut data = String::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(true);
        }
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|_| transport_failure())?;
        if read == 0 {
            return Ok(false);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        } else if line.is_empty() && !data.is_empty() {
            let item: RunStreamItem = serde_json::from_str(&data)
                .map_err(|_| protocol_failure("invalid run stream frame"))?;
            data.clear();
            if let RunStreamItem::Entry { index, .. } = &item {
                *cursor = Some(*index);
            }
            let closed = matches!(item, RunStreamItem::Closed);
            handler(Ok(item));
            if closed {
                return Ok(true);
            }
        }
    }
}

pub(crate) fn execute(
    client: &Client,
    method: reqwest::Method,
    path: String,
    body: Option<Bytes>,
    accept: Option<&'static str>,
) -> RunsResult<reqwest::blocking::Response> {
    let session = client.gateway.as_ref().ok_or_else(|| {
        Failure::new(
            "delegate_unavailable",
            "This client was not created from a Koru API gateway session.",
            true,
        )
    })?;
    let current = session.current().map_err(gateway_failure)?;
    let http = if accept.is_some() {
        client.stream_http.as_ref()
    } else {
        client.http.as_ref()
    }
    .expect("a gateway session owns its HTTP adapters");
    let send = |current: &koru_api_discovery::GatewayHello| {
        let mut request = http
            .request(method.clone(), format!("{}{}", current.api_endpoint, path))
            .bearer_auth(current.session.expose());
        if let Some(accept) = accept {
            request = request.header(reqwest::header::ACCEPT, accept);
        }
        if let Some(body) = &body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone());
        }
        request.send()
    };
    match send(&current) {
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
            let renewed = session
                .refresh_after_rejection(current.session.expose())
                .map_err(gateway_failure)?;
            if renewed.session.expose() == current.session.expose() {
                Ok(response)
            } else {
                send(&renewed).map_err(|_| transport_failure())
            }
        }
        Ok(response) => Ok(response),
        Err(_) => {
            let renewed = session.refresh("unreachable").map_err(gateway_failure)?;
            send(&renewed).map_err(|_| transport_failure())
        }
    }
}

pub(crate) fn json_body<T: serde::Serialize>(value: &T) -> RunsResult<Bytes> {
    serde_json::to_vec(value)
        .map(Bytes::from)
        .map_err(|_| protocol_failure("The run request could not be encoded."))
}

pub(crate) fn checked(
    response: reqwest::blocking::Response,
) -> RunsResult<reqwest::blocking::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    match response.json::<Failure>() {
        Ok(failure) => Err(failure),
        Err(_) => Err(Failure::new(
            if status == reqwest::StatusCode::UNAUTHORIZED {
                "unauthenticated"
            } else {
                "gateway_failure"
            },
            "The Koru API gateway returned an invalid error response.",
            status.is_server_error(),
        )),
    }
}

pub(crate) fn protocol_failure(message: &str) -> Failure {
    Failure::new("invalid_response", message, true)
}

fn gateway_failure(error: Error) -> Failure {
    Failure::new("gateway_unavailable", error.to_string(), true)
}

fn transport_failure() -> Failure {
    Failure::new(
        "gateway_unavailable",
        "The Koru API gateway connection ended.",
        true,
    )
}

/// Versioned mutation routes are a semantic fence, not a readiness hint.
/// An older gateway must refuse before it can discard a selected model.
fn create_path(explicit_model: bool) -> &'static str {
    if explicit_model {
        "/v2/runs"
    } else {
        "/v1/runs"
    }
}

fn resume_path(run: RunId) -> String {
    // Always guard resume: an older reader could already have omitted the
    // stored choice, so checking a client-side header is insufficient.
    format!("/v2/runs/{}/resume", run.to_storage())
}

#[cfg(test)]
mod tests;
