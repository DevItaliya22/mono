// SPDX-License-Identifier: MPL-2.0

use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use koru_ai_core::{
    AiError, AiErrorClass, AiEvent, AiToolCall, AiUsage, Cancellation, InferenceAttemptId,
    ReasoningChoice, SourceLink, Timestamp, UsageSource, WebSearchPhase,
};
use koru_ai_transport::SseDecoder;
use serde_json::Value;

use crate::events::{Annotation, OpenAiEvent, Response, Usage};

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const STREAM_POLL: Duration = Duration::from_millis(50);

type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

#[derive(Default)]
struct PendingCall {
    call_id: String,
    name: String,
    arguments: String,
}

pub(crate) struct OpenAiStream {
    pub runtime: tokio::runtime::Runtime,
    pub bytes: ByteStream,
    pub attempt_id: InferenceAttemptId,
    pub resolved_model: String,
    pub effective_reasoning: Option<ReasoningChoice>,
    pub request_payload_bytes: u64,
    pub cancellation: Cancellation,
    decoder: SseDecoder,
    pending: VecDeque<AiEvent>,
    calls: HashMap<usize, PendingCall>,
    searches_started: HashSet<String>,
    activity_deadline: Instant,
    terminal: bool,
}

impl OpenAiStream {
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
            calls: HashMap::new(),
            searches_started: HashSet::new(),
            activity_deadline: Instant::now() + STREAM_IDLE_TIMEOUT,
            terminal: false,
        }
    }

    fn handle(&mut self, event: OpenAiEvent) {
        if self.terminal {
            return;
        }
        match event {
            OpenAiEvent::Created { response } => {
                self.pending.push_back(AiEvent::Started {
                    attempt_id: self.attempt_id,
                    provider_response_id: response.id,
                    request_payload_bytes: Some(self.request_payload_bytes),
                    resolved_model: Some(self.resolved_model.clone()),
                    effective_reasoning: self.effective_reasoning,
                    at: Timestamp::now(),
                });
            }
            OpenAiEvent::TextDelta { delta } if !delta.is_empty() => {
                self.pending.push_back(AiEvent::TextDelta {
                    attempt_id: self.attempt_id,
                    text: delta,
                    at: Timestamp::now(),
                });
            }
            OpenAiEvent::TextDelta { .. } => {}
            OpenAiEvent::OutputItemAdded { output_index, item } => {
                self.capture_call(output_index, &item, false);
            }
            OpenAiEvent::OutputItemDone { output_index, item } => {
                self.capture_call(output_index, &item, true);
            }
            OpenAiEvent::FunctionArgumentsDelta {
                output_index,
                delta,
            } => {
                self.calls
                    .entry(output_index)
                    .or_default()
                    .arguments
                    .push_str(&delta);
            }
            OpenAiEvent::AnnotationAdded { annotation } => {
                if let Annotation::UrlCitation { url, title } = annotation {
                    self.pending.push_back(AiEvent::Citation {
                        attempt_id: self.attempt_id,
                        source: SourceLink { url, title },
                        at: Timestamp::now(),
                    });
                }
            }
            OpenAiEvent::WebSearchInProgress { item_id }
            | OpenAiEvent::WebSearchSearching { item_id } => {
                if self.searches_started.insert(item_id.clone()) {
                    self.pending.push_back(AiEvent::WebSearch {
                        attempt_id: self.attempt_id,
                        call_id: item_id,
                        phase: WebSearchPhase::Started,
                        at: Timestamp::now(),
                    });
                }
            }
            OpenAiEvent::WebSearchCompleted { item_id } => {
                self.searches_started.insert(item_id.clone());
                self.pending.push_back(AiEvent::WebSearch {
                    attempt_id: self.attempt_id,
                    call_id: item_id,
                    phase: WebSearchPhase::Completed,
                    at: Timestamp::now(),
                });
            }
            OpenAiEvent::Completed { response } => self.complete(response),
            OpenAiEvent::Failed { response } | OpenAiEvent::Incomplete { response } => {
                let _provider_status = response.status;
                self.fail(AiError::new(
                    AiErrorClass::Provider,
                    "OpenAI could not complete the request.",
                    true,
                ));
            }
            OpenAiEvent::Error => {
                self.fail(AiError::new(
                    AiErrorClass::Provider,
                    "OpenAI could not complete the request.",
                    true,
                ));
            }
            OpenAiEvent::Unknown => {}
        }
    }

    fn capture_call(&mut self, output_index: usize, item: &Value, done: bool) {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let call = self.calls.entry(output_index).or_default();
        if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
            call.call_id = call_id.to_owned();
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            call.name = name.to_owned();
        }
        if done && let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
            call.arguments = arguments.to_owned();
        }
    }

    fn complete(&mut self, response: Response) {
        for (index, item) in response.output.iter().enumerate() {
            self.capture_call(index, item, true);
        }
        if let Some(usage) = response.usage {
            self.pending.push_back(AiEvent::Usage {
                attempt_id: self.attempt_id,
                usage: usage.into_core(),
            });
        }
        let mut calls = self
            .calls
            .drain()
            .filter_map(|(index, call)| {
                if call.call_id.is_empty() || call.name.is_empty() {
                    return None;
                }
                Some((
                    index,
                    AiToolCall {
                        call_id: call.call_id,
                        name: call.name,
                        arguments: serde_json::from_str(&call.arguments)
                            .unwrap_or_else(|_| serde_json::json!({})),
                        thought_signatures: Vec::new(),
                    },
                ))
            })
            .collect::<Vec<_>>();
        calls.sort_by_key(|(index, _)| *index);
        let calls = calls.into_iter().map(|(_, call)| call).collect::<Vec<_>>();
        let stop_reason = response.status.or_else(|| Some("completed".into()));
        self.pending.push_back(if calls.is_empty() {
            AiEvent::Completed {
                attempt_id: self.attempt_id,
                stop_reason,
                at: Timestamp::now(),
            }
        } else {
            AiEvent::ToolCalls {
                attempt_id: self.attempt_id,
                calls,
                stop_reason,
                at: Timestamp::now(),
            }
        });
        self.terminal = true;
    }

    fn fail(&mut self, error: AiError) {
        if !self.terminal {
            self.pending.push_back(AiEvent::Failed {
                attempt_id: self.attempt_id,
                error,
                at: Timestamp::now(),
            });
            self.terminal = true;
        }
    }

    fn decode(&mut self, bytes: &[u8]) {
        match self.decoder.push(bytes) {
            Ok(frames) => {
                for frame in frames {
                    if frame.data == "[DONE]" {
                        continue;
                    }
                    match serde_json::from_str::<OpenAiEvent>(&frame.data) {
                        Ok(event) => self.handle(event),
                        Err(_) => self.fail(AiError::new(
                            AiErrorClass::Provider,
                            "OpenAI returned an invalid streaming response.",
                            false,
                        )),
                    }
                }
            }
            Err(_) => self.fail(AiError::new(
                AiErrorClass::Provider,
                "OpenAI returned an invalid streaming response.",
                false,
            )),
        }
    }
}

impl Iterator for OpenAiStream {
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
            if Instant::now() >= self.activity_deadline {
                self.fail(AiError::new(
                    AiErrorClass::Network,
                    "OpenAI stopped responding before completing the response.",
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
                    self.decode(&bytes);
                }
                Ok(Some(Err(_))) => self.fail(AiError::new(
                    AiErrorClass::Network,
                    "OpenAI is temporarily unavailable.",
                    true,
                )),
                Ok(None) => {
                    if let Ok(frames) = self.decoder.finish() {
                        for frame in frames {
                            if let Ok(event) = serde_json::from_str::<OpenAiEvent>(&frame.data) {
                                self.handle(event);
                            }
                        }
                    }
                    if !self.terminal {
                        self.fail(AiError::new(
                            AiErrorClass::Network,
                            "The OpenAI response ended unexpectedly.",
                            true,
                        ));
                    }
                }
            }
        }
    }
}

impl Usage {
    fn into_core(self) -> AiUsage {
        let cached_prompt_tokens = self.input_tokens_details.and_then(|d| d.cached_tokens);
        let reasoning_tokens = self.output_tokens_details.and_then(|d| d.reasoning_tokens);
        let available = self.input_tokens.is_some()
            || cached_prompt_tokens.is_some()
            || self.output_tokens.is_some()
            || reasoning_tokens.is_some()
            || self.total_tokens.is_some();
        AiUsage {
            prompt_tokens: self.input_tokens,
            cached_prompt_tokens,
            completion_tokens: self.output_tokens,
            reasoning_tokens,
            total_tokens: self.total_tokens,
            source: if available {
                UsageSource::ProviderReported
            } else {
                UsageSource::Unavailable
            },
        }
    }
}
