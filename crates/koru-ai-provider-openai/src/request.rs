// SPDX-License-Identifier: MPL-2.0

use koru_ai_core::{AiError, AiErrorClass, AiRequest, ChatRole, ReasoningChoice, ReasoningLevel};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    instructions: &'a str,
    input: Vec<InputItem<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolSpec<'a>>,
    stream: bool,
    store: bool,
}

#[derive(Serialize)]
#[serde(untagged)]
enum InputItem<'a> {
    Message {
        role: &'static str,
        content: &'a str,
    },
    FunctionCall {
        r#type: &'static str,
        call_id: &'a str,
        name: &'a str,
        arguments: String,
    },
    FunctionCallOutput {
        r#type: &'static str,
        call_id: &'a str,
        output: String,
    },
}

#[derive(Serialize)]
struct Reasoning {
    effort: &'static str,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToolSpec<'a> {
    Function {
        name: &'a str,
        description: &'a str,
        parameters: &'a Value,
    },
    WebSearch,
}

pub(crate) fn encode(request: &AiRequest) -> Result<Vec<u8>, AiError> {
    let converted = koru_ai_core::conversion::convert(request)?;
    let mut input = converted
        .messages
        .iter()
        .map(|message| InputItem::Message {
            role: match message.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
            },
            content: &message.markdown,
        })
        .collect::<Vec<_>>();
    for turn in &request.tool_history {
        for call in &turn.calls {
            input.push(InputItem::FunctionCall {
                r#type: "function_call",
                call_id: &call.call_id,
                name: &call.name,
                arguments: serde_json::to_string(&call.arguments).map_err(encode_error)?,
            });
        }
        for result in &turn.results {
            input.push(InputItem::FunctionCallOutput {
                r#type: "function_call_output",
                call_id: &result.call_id,
                output: serde_json::to_string(&serde_json::json!({
                    "ok": !result.is_error,
                    "result": result.content,
                }))
                .map_err(encode_error)?,
            });
        }
    }
    let mut tools = request
        .tools
        .iter()
        .map(|tool| ToolSpec::Function {
            name: &tool.name,
            description: &tool.description,
            parameters: &tool.input_schema,
        })
        .collect::<Vec<_>>();
    if request.options.web_search {
        tools.push(ToolSpec::WebSearch);
    }
    serde_json::to_vec(&ResponsesRequest {
        model: &request.model,
        instructions: &converted.system,
        input,
        reasoning: reasoning(request.reasoning),
        tools,
        stream: true,
        store: false,
    })
    .map_err(encode_error)
}

fn reasoning(choice: ReasoningChoice) -> Option<Reasoning> {
    let effort = match choice {
        ReasoningChoice::Default | ReasoningChoice::On => return None,
        ReasoningChoice::Off => "none",
        ReasoningChoice::Level(ReasoningLevel::Minimal) => "minimal",
        ReasoningChoice::Level(ReasoningLevel::Low) => "low",
        ReasoningChoice::Level(ReasoningLevel::Medium) => "medium",
        ReasoningChoice::Level(ReasoningLevel::High) => "high",
        ReasoningChoice::Level(ReasoningLevel::XHigh) => "xhigh",
        ReasoningChoice::Level(ReasoningLevel::Max) => "max",
    };
    Some(Reasoning { effort })
}

fn encode_error(_: serde_json::Error) -> AiError {
    AiError::new(
        AiErrorClass::Provider,
        "The OpenAI request could not be encoded.",
        false,
    )
}
