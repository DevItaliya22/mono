// SPDX-License-Identifier: MPL-2.0

use koru_ai_core::{AiError, AiErrorClass, AiRequest, ChatRole, ReasoningChoice, conversion};
use serde_json::{Value, json};

pub(crate) fn encode(request: &AiRequest) -> Result<Vec<u8>, AiError> {
    encode_with_tools(request, true)
}

pub(crate) fn encode_without_tools(request: &AiRequest) -> Result<Vec<u8>, AiError> {
    encode_with_tools(request, false)
}

fn encode_with_tools(request: &AiRequest, include_tools: bool) -> Result<Vec<u8>, AiError> {
    let converted = conversion::convert(request)?;
    let mut messages = Vec::with_capacity(converted.messages.len() + 1);
    messages.push(json!({ "role": "system", "content": converted.system }));
    messages.extend(converted.messages.into_iter().map(|message| {
        let role = match message.role {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        };
        json!({ "role": role, "content": message.markdown })
    }));
    append_tool_history(&mut messages, request)?;
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
        "chat_template_kwargs": {
            "enable_thinking": enable_thinking(request.reasoning)
        }
    });
    if include_tools && !request.tools.is_empty() {
        body["tools"] = json!(
            request
                .tools
                .iter()
                .map(|tool| json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    }
                }))
                .collect::<Vec<_>>()
        );
    }
    serde_json::to_vec(&body).map_err(|_| encode_error())
}

pub(crate) fn enable_thinking(choice: ReasoningChoice) -> bool {
    !matches!(choice, ReasoningChoice::Off)
}

fn append_tool_history(messages: &mut Vec<Value>, request: &AiRequest) -> Result<(), AiError> {
    for turn in &request.tool_history {
        let tool_calls = turn
            .calls
            .iter()
            .map(|call| {
                Ok(json!({
                    "id": call.call_id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": serde_json::to_string(&call.arguments)
                            .map_err(|_| encode_error())?,
                    }
                }))
            })
            .collect::<Result<Vec<_>, AiError>>()?;
        messages.push(json!({
            "role": "assistant",
            "content": "",
            "tool_calls": tool_calls,
        }));
        for result in &turn.results {
            let content = serde_json::to_string(&json!({
                "ok": !result.is_error,
                "result": result.content,
            }))
            .map_err(|_| encode_error())?;
            messages.push(json!({
                "role": "tool",
                "tool_call_id": result.call_id,
                "content": content,
            }));
        }
    }
    Ok(())
}

fn encode_error() -> AiError {
    AiError::new(
        AiErrorClass::Provider,
        "The Koru AI request could not be encoded.",
        false,
    )
}
