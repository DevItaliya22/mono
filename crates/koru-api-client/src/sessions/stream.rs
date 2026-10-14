// SPDX-License-Identifier: MPL-2.0

use std::io::{BufRead as _, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use koru_session_api::{RouteId, SessionStreamItem, SubscribeRequest};
use koru_session_protocol::SessionId;
use koru_store_core::errors::{Code, Error};

use super::{SessionsError, SessionsResult, map_public, route_path};

pub(super) fn subscribe(
    store: koru_store_client::Client,
    session_id: SessionId,
    from: SubscribeRequest,
    mut handler: impl FnMut(SessionsResult<SessionStreamItem>) + Send + 'static,
) -> SessionsResult<SessionSubscription> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let join = std::thread::Builder::new()
        .name("koru-sessions".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let mut cursor = from;
            let mut backoff = Duration::from_millis(250);
            while !thread_stop.load(Ordering::Relaxed) {
                match tail_once(&store, session_id, &mut cursor, &thread_stop, &mut handler) {
                    Ok(StreamEnd::Reconnect) => {
                        backoff = Duration::from_millis(250);
                    }
                    Ok(StreamEnd::Final) => break,
                    Err(error @ SessionsError::Api(_)) => {
                        handler(Err(error));
                        break;
                    }
                    Err(error) if thread_stop.load(Ordering::Relaxed) => {
                        drop(error);
                        break;
                    }
                    Err(error) => {
                        handler(Err(error));
                        sleep_until_stopped(&thread_stop, backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(5));
                    }
                }
            }
        })
        .map_err(|error| SessionsError::Transport(Error::new(Code::Internal, error.to_string())))?;
    Ok(SessionSubscription {
        stop,
        join: Some(join),
    })
}

enum StreamEnd {
    Reconnect,
    Final,
}

fn tail_once(
    store: &koru_store_client::Client,
    session_id: SessionId,
    cursor: &mut SubscribeRequest,
    stop: &AtomicBool,
    handler: &mut (impl FnMut(SessionsResult<SessionStreamItem>) + Send),
) -> SessionsResult<StreamEnd> {
    const STREAM_DEADLINE: Duration = Duration::from_secs(90);
    let mut path = route_path(RouteId::Subscribe, Some(session_id), None);
    let mut query = Vec::with_capacity(2);
    if let Some(after) = cursor.after_event {
        query.push(format!("after_event={after}"));
    }
    if let Some(after) = cursor.after_output {
        query.push(format!("after_output={after}"));
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    let body = store
        .get_public_streaming(&path, "text/event-stream", STREAM_DEADLINE)
        .map_err(map_public)?;
    let mut reader = BufReader::new(body);
    let mut line = String::new();
    let mut data = String::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(StreamEnd::Final);
        }
        line.clear();
        let read = reader.read_line(&mut line).map_err(|error| {
            SessionsError::Transport(Error::new(Code::StorageFailed, error.to_string()))
        })?;
        if read == 0 {
            return Ok(StreamEnd::Reconnect);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        } else if line.is_empty() && !data.is_empty() {
            let item = decode_stream_item(&data)?;
            data.clear();
            match &item {
                SessionStreamItem::Event { event } => cursor.after_event = Some(event.sequence),
                SessionStreamItem::Output { chunk } => cursor.after_output = Some(chunk.sequence),
                SessionStreamItem::Gap { .. } | SessionStreamItem::Closed { .. } => {
                    handler(Ok(item));
                    return Ok(StreamEnd::Final);
                }
            }
            handler(Ok(item));
        }
    }
}

fn decode_stream_item(data: &str) -> SessionsResult<SessionStreamItem> {
    serde_json::from_str(data).map_err(|error| {
        SessionsError::Transport(Error::new(
            Code::StorageFailed,
            format!("invalid session stream frame: {error}"),
        ))
    })
}

fn sleep_until_stopped(stop: &AtomicBool, duration: Duration) {
    let deadline = std::time::Instant::now() + duration;
    while !stop.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

/// One live session reader. Drop signals and detaches; `close` additionally
/// waits for the named thread, bounded by the stream deadline.
pub struct SessionSubscription {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl SessionSubscription {
    pub fn close(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for SessionSubscription {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
