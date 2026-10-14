// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use koru_session_protocol::{AgentEvent, ErrorCode};

use crate::stream::{PushError, Queue};
use crate::{BoundedStream, StreamReadError};

use super::{ProviderDiagnostic, ProviderFailure, ProviderStage};

#[derive(Clone)]
pub struct ProviderEventSender {
    queue: Queue<AgentEvent>,
}

pub struct ProviderEventReceiver {
    stream: BoundedStream<AgentEvent>,
}

pub fn provider_event_queue(
    capacity: usize,
) -> Result<(ProviderEventSender, ProviderEventReceiver), ProviderFailure> {
    if capacity == 0 {
        return Err(ProviderFailure::new(
            ErrorCode::InvalidRequest,
            ProviderStage::Initialize,
            ProviderDiagnostic::InvalidInput,
        ));
    }
    let (queue, stream) = Queue::new(capacity);
    Ok((
        ProviderEventSender { queue },
        ProviderEventReceiver { stream },
    ))
}

impl ProviderEventSender {
    /// Never blocks the provider reader. The owning adapter may coalesce a
    /// text delta before retrying; otherwise full capacity is explicit.
    pub fn try_send(&self, event: AgentEvent) -> Result<usize, ProviderFailure> {
        self.queue.try_push(event).map_err(|error| match error {
            PushError::Full(_) => ProviderFailure::new(
                ErrorCode::Backpressure,
                ProviderStage::Read,
                ProviderDiagnostic::Backpressure,
            ),
            PushError::Closed(_) => ProviderFailure::new(
                ErrorCode::ProviderUnavailable,
                ProviderStage::Read,
                ProviderDiagnostic::UnexpectedExit,
            ),
        })
    }

    pub fn close(&self) {
        self.queue.close();
    }
}

impl ProviderEventReceiver {
    pub fn try_recv(&self) -> Result<Option<AgentEvent>, ProviderFailure> {
        match self.stream.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(StreamReadError::Empty) => Ok(None),
            Err(StreamReadError::Closed) => Err(closed()),
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<AgentEvent>, ProviderFailure> {
        match self.stream.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(StreamReadError::Empty) => Ok(None),
            Err(StreamReadError::Closed) => Err(closed()),
        }
    }
}

fn closed() -> ProviderFailure {
    ProviderFailure::new(
        ErrorCode::ProviderUnavailable,
        ProviderStage::Read,
        ProviderDiagnostic::UnexpectedExit,
    )
}
