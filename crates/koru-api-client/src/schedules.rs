// SPDX-License-Identifier: MPL-2.0

//! `client.schedules()` — scheduled delegated work through the gateway.

use std::time::Duration;

use koru_delegate_api::wire::{
    CreateScheduleRequest, CreateScheduleResponse, EditScheduleRequest, EditScheduleResponse,
    Failure, ListSchedulesResponse, PauseScheduleRequest, PauseScheduleResponse,
    RenameScheduleRequest, RenameScheduleResponse, ResumeScheduleRequest, ResumeScheduleResponse,
    RetireScheduleRequest, RetireScheduleResponse, ScheduleChange,
};
use koru_delegate_core::WorkspaceId;

use crate::Client;
use crate::runs::{checked, execute, json_body, protocol_failure};

pub type SchedulesResult<T> = std::result::Result<T, Failure>;
const STREAM_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SchedulesClient<'a> {
    pub(crate) client: &'a Client,
}

impl SchedulesClient<'_> {
    pub fn available(&self) -> bool {
        self.client
            .capabilities
            .iter()
            .any(|value| value == koru_delegate_api::CAPABILITY)
    }

    /// Create one durable schedule. The worker, not this caller, stamps its
    /// authority fields.
    pub fn create(
        &self,
        request: &CreateScheduleRequest,
    ) -> SchedulesResult<CreateScheduleResponse> {
        self.json(
            reqwest::Method::POST,
            "/v1/schedules".to_owned(),
            Some(json_body(request)?),
        )
    }

    pub fn list(&self, workspace: WorkspaceId) -> SchedulesResult<ListSchedulesResponse> {
        self.json(
            reqwest::Method::GET,
            format!("/v1/schedules?workspace={}", workspace.to_storage()),
            None,
        )
    }

    pub fn rename(
        &self,
        request: &RenameScheduleRequest,
    ) -> SchedulesResult<RenameScheduleResponse> {
        self.json(
            reqwest::Method::POST,
            "/v1/schedules/rename".to_owned(),
            Some(json_body(request)?),
        )
    }

    pub fn edit(&self, request: &EditScheduleRequest) -> SchedulesResult<EditScheduleResponse> {
        self.json(
            reqwest::Method::POST,
            "/v1/schedules/edit".to_owned(),
            Some(json_body(request)?),
        )
    }

    pub fn pause(&self, request: &PauseScheduleRequest) -> SchedulesResult<PauseScheduleResponse> {
        self.json(
            reqwest::Method::POST,
            "/v1/schedules/pause".to_owned(),
            Some(json_body(request)?),
        )
    }

    pub fn resume(
        &self,
        request: &ResumeScheduleRequest,
    ) -> SchedulesResult<ResumeScheduleResponse> {
        self.json(
            reqwest::Method::POST,
            "/v1/schedules/resume".to_owned(),
            Some(json_body(request)?),
        )
    }

    pub fn retire(
        &self,
        request: &RetireScheduleRequest,
    ) -> SchedulesResult<RetireScheduleResponse> {
        self.json(
            reqwest::Method::POST,
            "/v1/schedules/retire".to_owned(),
            Some(json_body(request)?),
        )
    }

    /// Follow content-free invalidations for one workspace's list. Routine
    /// gateway reconnects carry the last opaque generation. After a retryable
    /// error, a successful reconnect repeats the current cursor even when it is
    /// unchanged, giving callers a refresh opportunity that clears transient
    /// failures.
    pub fn watch(
        &self,
        workspace: WorkspaceId,
        after: Option<ScheduleChange>,
        mut handler: impl FnMut(SchedulesResult<ScheduleChange>) + Send + 'static,
    ) -> SchedulesResult<ScheduleSubscription> {
        let client = self.client.clone();
        let (cancel, cancellation) = tokio::sync::oneshot::channel();
        let join = std::thread::Builder::new()
            .name("koru-schedules".into())
            .stack_size(192 * 1024)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(watch_loop(
                        client,
                        workspace,
                        after,
                        cancellation,
                        &mut handler,
                    )),
                    Err(error) => handler(Err(protocol_failure(&format!(
                        "could not start schedule runtime: {error}"
                    )))),
                }
            })
            .map_err(|error| {
                protocol_failure(&format!("could not start schedule reader: {error}"))
            })?;
        Ok(ScheduleSubscription {
            cancel: Some(cancel),
            join: Some(join),
        })
    }

    fn json<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: String,
        body: Option<bytes::Bytes>,
    ) -> SchedulesResult<T> {
        checked(execute(self.client, method, path, body, None)?)?
            .json()
            .map_err(|_| protocol_failure("The schedule response body was invalid."))
    }
}

pub struct ScheduleSubscription {
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl ScheduleSubscription {
    pub fn stop(mut self) {
        self.cancel_and_join();
    }

    fn cancel_and_join(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(join) = self.join.take()
            && join.thread().id() != std::thread::current().id()
        {
            let _ = join.join();
        }
    }
}

impl Drop for ScheduleSubscription {
    fn drop(&mut self) {
        self.cancel_and_join();
    }
}

async fn watch_loop(
    client: Client,
    workspace: WorkspaceId,
    mut cursor: Option<ScheduleChange>,
    mut cancellation: tokio::sync::oneshot::Receiver<()>,
    handler: &mut (impl FnMut(SchedulesResult<ScheduleChange>) + Send),
) {
    let http = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
    {
        Ok(http) => http,
        Err(error) => {
            handler(Err(protocol_failure(&format!(
                "could not build schedule transport: {error}"
            ))));
            return;
        }
    };
    let mut backoff = Duration::from_millis(250);
    let mut reconnecting = false;
    loop {
        let outcome = tokio::select! {
            _ = &mut cancellation => break,
            outcome = tail_once(
                &client,
                &http,
                workspace,
                &mut cursor,
                reconnecting,
                handler,
            ) => outcome,
        };
        match outcome {
            Ok(()) => {
                backoff = Duration::from_millis(250);
                reconnecting = false;
            }
            Err(failure) if !failure.retryable => {
                handler(Err(failure));
                break;
            }
            Err(failure) => {
                handler(Err(failure));
                reconnecting = true;
                tokio::select! {
                    _ = &mut cancellation => break,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        }
    }
}

async fn tail_once(
    client: &Client,
    http: &reqwest::Client,
    workspace: WorkspaceId,
    cursor: &mut Option<ScheduleChange>,
    reconnecting: bool,
    handler: &mut (impl FnMut(SchedulesResult<ScheduleChange>) + Send),
) -> SchedulesResult<()> {
    let path = stream_path(workspace, cursor.as_ref());
    let mut response = checked_stream(send_stream(client, http, &path).await?).await?;
    // Establishing a stream is itself a useful edge after an error. The host
    // quite correctly sends no event when `cursor` is already its current
    // generation; repeat that cursor once so invalidate-and-refetch callers
    // can clear a transient error without a timer or a new schedule mutation.
    if reconnecting && let Some(current) = cursor.as_ref() {
        handler(Ok(current.clone()));
    }
    let mut pending = Vec::new();
    let mut data = String::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| stream_ended())? {
        pending.extend_from_slice(&chunk);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line = pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line)
                .map_err(|_| protocol_failure("invalid schedule stream text"))?;
            if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.trim_start());
                if data.len() > koru_delegate_api::REQUEST_BODY_LIMIT {
                    return Err(protocol_failure("schedule stream frame is too large"));
                }
            } else if line.is_empty() && !data.is_empty() {
                let change: ScheduleChange = serde_json::from_str(&data)
                    .map_err(|_| protocol_failure("invalid schedule stream frame"))?;
                data.clear();
                *cursor = Some(change.clone());
                handler(Ok(change));
            }
        }
        if pending.len() > koru_delegate_api::REQUEST_BODY_LIMIT {
            return Err(protocol_failure("schedule stream frame is too large"));
        }
    }
    Err(stream_ended())
}

async fn send_stream(
    client: &Client,
    http: &reqwest::Client,
    path: &str,
) -> SchedulesResult<reqwest::Response> {
    let session = client.gateway.as_ref().ok_or_else(|| {
        Failure::new(
            "delegate_unavailable",
            "This client was not created from a Koru API gateway session.",
            true,
        )
    })?;
    let current = session.current().map_err(gateway_failure)?;
    match send_stream_request(http, &current, path, STREAM_HANDSHAKE_TIMEOUT).await {
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
            let renewed = session
                .refresh_after_rejection(current.session.expose())
                .map_err(gateway_failure)?;
            if renewed.session.expose() == current.session.expose() {
                Ok(response)
            } else {
                send_stream_request(http, &renewed, path, STREAM_HANDSHAKE_TIMEOUT).await
            }
        }
        Ok(response) => Ok(response),
        Err(_) => {
            let renewed = session.refresh("unreachable").map_err(gateway_failure)?;
            send_stream_request(http, &renewed, path, STREAM_HANDSHAKE_TIMEOUT).await
        }
    }
}

async fn send_stream_request(
    http: &reqwest::Client,
    current: &koru_api_discovery::GatewayHello,
    path: &str,
    timeout: Duration,
) -> SchedulesResult<reqwest::Response> {
    let request = http
        .get(format!("{}{}", current.api_endpoint, path))
        .bearer_auth(current.session.expose())
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send();
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| stream_ended())?
        .map_err(|_| stream_ended())
}

async fn checked_stream(response: reqwest::Response) -> SchedulesResult<reqwest::Response> {
    if response.status().is_success() {
        let event_stream = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"));
        return if event_stream {
            Ok(response)
        } else {
            Err(protocol_failure(
                "The schedule stream response was not text/event-stream.",
            ))
        };
    }
    let status = response.status();
    match response.json::<Failure>().await {
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

fn gateway_failure(error: crate::Error) -> Failure {
    Failure::new("gateway_unavailable", error.to_string(), true)
}

fn stream_ended() -> Failure {
    Failure::new("gateway_unavailable", "The schedule stream ended.", true)
}

fn stream_path(workspace: WorkspaceId, cursor: Option<&ScheduleChange>) -> String {
    let mut path = format!("/v1/schedules/stream?workspace={}", workspace.to_storage());
    if let Some(cursor) = cursor {
        path.push_str(&format!(
            "&after_instance={}&after_generation={}",
            cursor.instance, cursor.generation
        ));
    }
    path
}

#[cfg(test)]
mod tests;
