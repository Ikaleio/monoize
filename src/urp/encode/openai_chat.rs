//! URP to OpenAI Chat Completions: upstream requests and downstream non-stream responses.

mod messages;
mod reasoning;
mod request;
mod response;
mod tools;
mod usage;

pub use request::{encode_request, encode_request_checked};
pub(crate) use response::{
    encode_assistant_chat_message_from_nodes, finish_reason_to_chat, validate_response_nodes,
};
pub use response::{encode_response, encode_response_checked};
pub(crate) use tools::is_web_search_builtin;
pub(crate) use usage::encode_chat_usage;

use crate::urp::encode::sanitize_provider_item_wire_body;
use crate::urp::{
    CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY, ProviderProtocol, ToolCallType, is_internal_key,
    tool_call_arguments_for_wire,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// PC0: the Chat Completions wire dialect of one upstream attempt, set on its Channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatDialect {
    OpenAi,
    DeepSeek,
    OpenRouter,
    Vercel,
    Generic,
}

impl ChatDialect {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "openai" => Some(Self::OpenAi),
            "deepseek" => Some(Self::DeepSeek),
            "openrouter" => Some(Self::OpenRouter),
            "vercel" => Some(Self::Vercel),
            "generic" => Some(Self::Generic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::OpenRouter => "openrouter",
            Self::Vercel => "vercel",
            Self::Generic => "generic",
        }
    }

    fn reasoning_details(self) -> bool {
        matches!(self, Self::OpenRouter | Self::Vercel | Self::Generic)
    }
}

/// A request the Chat encoder refuses before dispatch; `code` is the HTTP 400 error code.
#[derive(Debug, Clone)]
pub struct ChatEncodeError {
    pub code: &'static str,
    pub message: String,
}

/// DC7b: the Chat completion id for both modes; generated only when the URP id is empty.
pub(crate) fn chat_completion_id(id: &str) -> String {
    if id.is_empty() {
        format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())
    } else {
        id.to_string()
    }
}

fn flag_set(extra_body: &HashMap<String, Value>, key: &str) -> bool {
    extra_body.get(key).and_then(Value::as_bool) == Some(true)
}

/// Copies non-internal extras onto `wire_object`; generated members win.
fn merge_chat_wire_extra(
    wire_object: &mut Map<String, Value>,
    extra_body: &HashMap<String, Value>,
) {
    for (key, value) in extra_body {
        if !is_internal_key(key) && !wire_object.contains_key(key) {
            wire_object.insert(key.clone(), value.clone());
        }
    }
}

fn is_legacy_function_call(tool_type: ToolCallType, extra_body: &HashMap<String, Value>) -> bool {
    tool_type == ToolCallType::Function && flag_set(extra_body, CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
}

/// The deprecated assistant `function_call` member (PC2.9).
fn encode_legacy_function_call(
    name: &str,
    arguments: &str,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut function_call =
        json!({ "name": name, "arguments": tool_call_arguments_for_wire(arguments) });
    if let Some(obj) = function_call.as_object_mut() {
        merge_chat_wire_extra(obj, extra_body);
    }
    function_call
}

fn encode_chat_tool_call(
    tool_type: ToolCallType,
    call_id: &str,
    name: &str,
    arguments: &str,
) -> Value {
    match tool_type {
        ToolCallType::Function => json!({
            "id": call_id,
            "type": "function",
            "function": { "name": name, "arguments": tool_call_arguments_for_wire(arguments) }
        }),
        ToolCallType::Custom => json!({
            "id": call_id,
            "type": "custom",
            "custom": { "name": name, "input": arguments }
        }),
    }
}

/// A Chat-origin provider item as message content; other origins have no Chat form.
fn encode_chat_provider_part(
    origin_protocol: ProviderProtocol,
    id: Option<&str>,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    (origin_protocol == ProviderProtocol::ChatCompletion)
        .then(|| chat_provider_item_wire_body(id, item_type, body, extra_body))
}

fn chat_provider_item_wire_body(
    id: Option<&str>,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut part = sanitize_provider_item_wire_body(body);
    if let Some(obj) = part.as_object_mut() {
        merge_chat_wire_extra(obj, extra_body);
        obj.remove("id");
        obj.remove("type");
        // Synthetic identities must not add fields absent from the native envelope.
        if body.get("id").is_some()
            && let Some(id) = id
        {
            obj.insert("id".into(), json!(id));
        }
        if body.get("type").is_some() && !item_type.is_empty() {
            obj.insert("type".into(), json!(item_type));
        }
    }
    part
}
