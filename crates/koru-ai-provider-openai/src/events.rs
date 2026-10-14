// SPDX-License-Identifier: MPL-2.0

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum OpenAiEvent {
    #[serde(rename = "response.created")]
    Created { response: Response },
    #[serde(rename = "response.output_text.delta")]
    TextDelta { delta: String },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { output_index: usize, item: Value },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone { output_index: usize, item: Value },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionArgumentsDelta { output_index: usize, delta: String },
    #[serde(rename = "response.output_text.annotation.added")]
    AnnotationAdded { annotation: Annotation },
    #[serde(rename = "response.web_search_call.in_progress")]
    WebSearchInProgress { item_id: String },
    #[serde(rename = "response.web_search_call.searching")]
    WebSearchSearching { item_id: String },
    #[serde(rename = "response.web_search_call.completed")]
    WebSearchCompleted { item_id: String },
    #[serde(rename = "response.completed")]
    Completed { response: Response },
    #[serde(rename = "response.failed")]
    Failed { response: Response },
    #[serde(rename = "response.incomplete")]
    Incomplete { response: Response },
    #[serde(rename = "error")]
    Error,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Response {
    pub id: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub output: Vec<Value>,
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Usage {
    pub input_tokens: Option<u64>,
    pub input_tokens_details: Option<InputTokenDetails>,
    pub output_tokens: Option<u64>,
    pub output_tokens_details: Option<OutputTokenDetails>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct InputTokenDetails {
    pub cached_tokens: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct OutputTokenDetails {
    pub reasoning_tokens: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum Annotation {
    #[serde(rename = "url_citation")]
    UrlCitation { url: String, title: String },
    #[serde(other)]
    Unknown,
}
