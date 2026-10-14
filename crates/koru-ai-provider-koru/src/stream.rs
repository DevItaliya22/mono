// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use koru_ai_core::{
    AiError, AiErrorClass, AiEvent, AiToolCall, AiUsage, Cancellation, InferenceAttemptId,
    ReasoningChoice, Timestamp, UsageSource,
};
use koru_ai_transport::SseDecoder;
use serde_json::Value;

use crate::think::{Piece, ThinkSplitter};

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const STREAM_POLL: Duration = Duration::from_millis(50);

type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

#[derive(Default)]
struct PendingCall {
    call_id: String,
    name: String,
    arguments: String,
}

pub(crate) struct KoruStream {
    pub runtime: tokio::runtime::Runtime,
    pub bytes: ByteStream,
    pub attempt_id: InferenceAttemptId,
    pub resolved_model: String,
    pub effective_reasoning: Option<ReasoningChoice>,
    pub request_payload_bytes: u64,
    pub cancellation: Cancellation,
    decoder: SseDecoder,
    pending: VecDeque<AiEvent>,
    calls: BTreeMap<usize, PendingCall>,
    think: ThinkSplitter,
    activity_deadline: Instant,
    started: bool,
    terminal: bool,
}

impl KoruStream {
    pub fn new(
        runtime: tokio::runtime::Runtime,
        response: reqwest::Response,
        attempt_id: InferenceAttemptId,
        resolved_model: String,
        effective_reasoning: Option<ReasoningChoice>,
        request_payload_bytes: u64,
        cancellation: Cancellation,
    ) -> Self {
        Self {
            runtime,
            bytes: Box::pin(response.bytes_stream()),
            attempt_id,
            resolved_model,
            effective_reasoning,
            request_payload_bytes,
            cancellation,
            decoder: SseDecoder::default(),
            pending: VecDeque::new(),
            calls: BTreeMap::new(),
            think: ThinkSplitter::default(),
            activity_deadline: Instant::now() + STREAM_IDLE_TIMEOUT,
            started: false,
            terminal: false,
        }
    }

    fn ensure_started(&mut self, response_id: Option<String>) {
        if self.started {
            return;
        }
        self.started = true;
        self.pending.push_back(AiEvent::Started {
            attempt_id: self.attempt_id,
            provider_response_id: response_id,
            request_payload_bytes: Some(self.request_payload_bytes),
            resolved_model: Some(self.resolved_model.clone()),
            effective_reasoning: self.effective_reasoning,
            at: Timestamp::now(),
        });
    }

    fn handle_data(&mut self, data: &str) {
        if self.terminal {
            return;
        }
        if data.trim() == "[DONE]" {
            self.finish_stream();
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            self.fail(provider_error());
            return;
        };
        if value.get("error").is_some() {
            self.fail(provider_error());
            return;
        }
        let id = value.get("id").and_then(Value::as_str).map(str::to_owned);
        self.ensure_started(id);
        if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
            self.pending.push_back(AiEvent::Usage {
                attempt_id: self.attempt_id,
                usage: map_usage(usage),
            });
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        if let Some(delta) = choice.get("delta") {
            let reasoning = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty());
            if let Some(reasoning) = reasoning {
                self.pending.push_back(AiEvent::ReasoningDelta {
                    attempt_id: self.attempt_id,
                    text: reasoning.to_owned(),
                    at: Timestamp::now(),
                });
            }
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                for piece in self.think.push(content) {
                    self.emit_piece(piece);
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    self.accumulate_call(call);
                }
            }
        }
        match choice.get("finish_reason").and_then(Value::as_str) {
            Some("tool_calls") => self.emit_tool_calls(Some("tool_calls".into())),
            Some(reason) if !reason.is_empty() => {
                self.finish_with(Some(reason.to_owned()));
            }
            _ => {}
        }
    }

    fn emit_piece(&mut self, piece: Piece) {
        match piece {
            Piece::Text(text) => self.pending.push_back(AiEvent::TextDelta {
                attempt_id: self.attempt_id,
                text,
                at: Timestamp::now(),
            }),
            Piece::Reasoning(text) => self.pending.push_back(AiEvent::ReasoningDelta {
                attempt_id: self.attempt_id,
                text,
                at: Timestamp::now(),
            }),
        }
    }

    fn accumulate_call(&mut self, call: &Value) {
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let pending = self.calls.entry(index).or_default();
        if let Some(id) = call.get("id").and_then(Value::as_str) {
            pending.call_id = id.to_owned();
        }
        if let Some(function) = call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                pending.name.push_str(name);
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                pending.arguments.push_str(arguments);
            }
        }
    }

    fn emit_tool_calls(&mut self, stop_reason: Option<String>) {
        for piece in self.think.finish() {
            self.emit_piece(piece);
        }
        let mut calls = Vec::new();
        for (_, pending) in std::mem::take(&mut self.calls) {
            if pending.name.is_empty() {
                continue;
            }
            let arguments =
                serde_json::from_str(&pending.arguments).unwrap_or_else(|_| serde_json::json!({}));
            let call_id = if pending.call_id.is_empty() {
                format!("koru-{}-{}", self.attempt_id, calls.len())
            } else {
                pending.call_id
            };
            calls.push(AiToolCall {
                call_id,
                name: pending.name,
                arguments,
                thought_signatures: Vec::new(),
            });
        }
        self.terminal = true;
        self.pending.push_back(AiEvent::ToolCalls {
            attempt_id: self.attempt_id,
            calls,
            stop_reason,
            at: Timestamp::now(),
        });
    }

    fn finish_stream(&mut self) {
        if self.terminal {
            return;
        }
        if !self.calls.is_empty() {
            self.emit_tool_calls(Some("tool_calls".into()));
            return;
        }
        self.finish_with(None);
    }

    fn finish_with(&mut self, stop_reason: Option<String>) {
        if self.terminal {
            return;
        }
        for piece in self.think.finish() {
            self.emit_piece(piece);
        }
        self.terminal = true;
        self.pending.push_back(AiEvent::Completed {
            attempt_id: self.attempt_id,
            stop_reason,
            at: Timestamp::now(),
        });
    }

    fn fail(&mut self, error: AiError) {
        if self.terminal {
            return;
        }
        self.terminal = true;
        self.pending.push_back(AiEvent::Failed {
            attempt_id: self.attempt_id,
            error,
            at: Timestamp::now(),
        });
    }
}

impl Iterator for KoruStream {
    type Item = AiEvent;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
            if self.terminal {
                return None;
            }
            if self.cancellation.is_cancelled() {
                self.terminal = true;
                return Some(AiEvent::Cancelled {
                    attempt_id: self.attempt_id,
                    at: Timestamp::now(),
                });
            }
            if Instant::now() > self.activity_deadline {
                self.fail(AiError::new(
                    AiErrorClass::Network,
                    "The hosted Koru AI response timed out.",
                    true,
                ));
                continue;
            }
            match self
                .runtime
                .block_on(async { tokio::time::timeout(STREAM_POLL, self.bytes.next()).await })
            {
                Err(_) => continue,
                Ok(Some(Ok(bytes))) => {
                    self.activity_deadline = Instant::now() + STREAM_IDLE_TIMEOUT;
                    match self.decoder.push(&bytes) {
                        Ok(frames) => {
                            for frame in frames {
                                self.handle_data(&frame.data);
                            }
                        }
                        Err(_) => self.fail(provider_error()),
                    }
                }
                Ok(Some(Err(_))) => self.fail(AiError::new(
                    AiErrorClass::Network,
                    "Koru could not reach the hosted AI service.",
                    true,
                )),
                Ok(None) => match self.decoder.finish() {
                    Ok(frames) => {
                        for frame in frames {
                            self.handle_data(&frame.data);
                        }
                        self.finish_stream();
                    }
                    Err(_) => self.fail(provider_error()),
                },
            }
        }
    }
}

fn map_usage(usage: &Value) -> AiUsage {
    AiUsage {
        prompt_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
        cached_prompt_tokens: None,
        completion_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
        reasoning_tokens: usage
            .get("completion_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64),
        total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
        source: UsageSource::ProviderReported,
    }
}

fn provider_error() -> AiError {
    AiError::new(
        AiErrorClass::Provider,
        "The hosted Koru AI service returned an invalid response.",
        true,
    )
}
