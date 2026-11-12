//! Non-stream OpenAI Responses request and response decoding into flat URP nodes.

mod content;
mod items;
mod request_controls;
mod usage;

pub(crate) use usage::parse_usage_from_responses;

use crate::urp::decode::{parse_compatible_media_node, parse_tool_definition, split_extra};
use crate::urp::{
    FinishReason, InstructionsFormat, Node, OrdinaryRole, ProviderProtocol,
    RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, ResponseOutcome, UrpRequest, UrpResponse,
};
use content::is_content_part_type;
use items::{ItemSource, decode_input_nodes, decode_instruction_nodes, push_item_nodes};
use request_controls::{
    mark_responses_tool_origin, parse_response_format, reasoning_config, tool_choice_from_value,
};
use serde_json::{Map, Value};

/// Rejects a malformed media part (a media `type` without a decodable source).
pub(crate) fn validate_compatible_content(value: &Value) -> Result<(), String> {
    let parts = match value {
        Value::Array(parts) => parts.as_slice(),
        _ => std::slice::from_ref(value),
    };
    for obj in parts.iter().filter_map(Value::as_object) {
        parse_compatible_media_node(obj, OrdinaryRole::User)?;
    }
    Ok(())
}

/// Validates the media parts of every message, tool output, and bare content-part item.
pub(crate) fn validate_responses_items(value: &Value) -> Result<(), String> {
    let items = match value {
        Value::Array(items) => items.as_slice(),
        _ => std::slice::from_ref(value),
    };
    for item in items {
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message");
        let content = match kind {
            "message" => item.get("content"),
            "function_call_output" | "custom_tool_call_output" => item.get("output"),
            kind if is_content_part_type(kind) => Some(item),
            _ => None,
        };
        if let Some(content) = content {
            validate_compatible_content(content)?;
        }
    }
    Ok(())
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let mut request = decode_native_request(value)?;
    crate::urp::cache_breakpoint::decode_request_markers(&mut request, ProviderProtocol::Responses);
    Ok(request)
}

fn decode_native_request(value: &Value) -> Result<UrpRequest, String> {
    for key in ["input", "instructions"] {
        if let Some(items) = value.get(key) {
            validate_responses_items(items)?;
        }
    }
    let obj = value
        .as_object()
        .ok_or_else(|| "responses request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            if obj.contains_key("prompt") {
                "missing model: Monoize routes by model and cannot resolve the model stored in a prompt template; send model explicitly".to_string()
            } else {
                "missing model".to_string()
            }
        })?
        .to_string();

    let mut input = Vec::new();
    if let Some(instructions) = obj.get("instructions") {
        decode_instruction_nodes(instructions, &mut input);
    }
    if let Some(items) = obj.get("input") {
        decode_input_nodes(items, &mut input);
    }
    crate::urp::tool_signature::restore_request_call_signatures(&mut input);

    let tools = obj.get("tools").and_then(Value::as_array).map(|tools| {
        tools
            .iter()
            .filter_map(parse_tool_definition)
            .map(|mut tool| {
                mark_responses_tool_origin(&mut tool);
                tool
            })
            .collect::<Vec<_>>()
    });

    let text = obj.get("text");
    let mut extra_body = split_extra(
        obj,
        &[
            "model",
            "input",
            "instructions",
            "stream",
            "temperature",
            "top_p",
            "max_output_tokens",
            "reasoning",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "response_format",
            "user",
            "text",
        ],
    );
    if let Some(text) = text.and_then(Value::as_object) {
        let unknown = split_extra(text, &["format", "verbosity"]);
        if !unknown.is_empty() {
            extra_body.insert(
                "text".to_string(),
                Value::Object(unknown.into_iter().collect()),
            );
        }
    }
    crate::urp::logprobs::strip_request_extras(&mut extra_body);

    Ok(UrpRequest {
        image_generation: None,
        sampling: None,
        logprobs: crate::urp::logprobs::request_config(obj, ProviderProtocol::Responses),
        context: Default::default(),
        instructions_format: obj.get("instructions").map(|value| match value {
            Value::Null => InstructionsFormat::Null,
            Value::Array(_) => InstructionsFormat::Items,
            _ => InstructionsFormat::Text,
        }),
        model,
        input,
        stream: obj.get("stream").and_then(Value::as_bool),
        temperature: obj.get("temperature").and_then(Value::as_f64),
        top_p: obj.get("top_p").and_then(Value::as_f64),
        max_output_tokens: obj.get("max_output_tokens").and_then(Value::as_u64),
        reasoning: obj
            .get("reasoning")
            .and_then(Value::as_object)
            .and_then(reasoning_config),
        tools,
        tool_choice: obj.get("tool_choice").cloned().map(tool_choice_from_value),
        parallel_tool_calls: obj.get("parallel_tool_calls").and_then(Value::as_bool),
        stop: None,
        verbosity: text
            .and_then(|text| text.get("verbosity"))
            .and_then(Value::as_str)
            .map(str::to_string),
        response_format: text
            .and_then(|text| text.get("format"))
            .or_else(|| obj.get("response_format"))
            .and_then(parse_response_format),
        user: obj.get("user").and_then(Value::as_str).map(str::to_string),
        extra_body,
    })
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    if let Some(items) = value.get("output") {
        validate_responses_items(items)?;
    }
    let obj = value
        .as_object()
        .ok_or_else(|| "responses response must be object".to_string())?;

    let outcome = ResponseOutcome::from_responses(obj);
    if outcome.is_none()
        && let Some(error) = obj.get("error").filter(|error| !error.is_null())
    {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("upstream Responses error")
            .to_string());
    }
    let output_items = obj.get("output").and_then(Value::as_array);
    if outcome.is_none() && output_items.is_none() {
        return Err("Responses response requires a valid status or an output array".to_string());
    }

    let mut output = Vec::new();
    for item in output_items.into_iter().flatten() {
        if let Some(item) = item.as_object() {
            push_item_nodes(&mut output, item, ItemSource::ResponseOutput);
        }
    }

    let finish_reason = match obj.get("status").and_then(Value::as_str) {
        Some("completed") => Some(
            if output
                .iter()
                .any(|node| matches!(node, Node::ToolCall { .. }))
            {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            },
        ),
        Some("incomplete") => Some(incomplete_finish_reason(obj)),
        Some("failed" | "cancelled") => Some(FinishReason::Other),
        _ => None,
    };

    let mut extra_body = split_extra(
        obj,
        &[
            "id",
            "object",
            "created",
            "created_at",
            "model",
            "output",
            "usage",
            "status",
            "error",
            "incomplete_details",
        ],
    );
    extra_body.insert(
        RESPONSES_RESPONSE_SOURCE_EXTRA_KEY.to_string(),
        Value::Object(Map::new()),
    );

    Ok(UrpResponse {
        outcome,
        id: obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("resp")
            .to_string(),
        model: obj
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created_at: obj.get("created_at").and_then(Value::as_i64),
        output,
        finish_reason,
        usage: obj
            .get("usage")
            .and_then(Value::as_object)
            .map(parse_usage_from_responses),
        extra_body,
    })
}

pub(crate) fn incomplete_finish_reason(obj: &Map<String, Value>) -> FinishReason {
    match obj
        .get("incomplete_details")
        .and_then(|value| value.get("reason"))
        .and_then(Value::as_str)
    {
        Some("content_filter") => FinishReason::ContentFilter,
        Some("model_context_window_exceeded" | "context_length_exceeded") => {
            FinishReason::ContextLimit
        }
        Some("pause_turn") => FinishReason::Paused,
        Some("compaction") => FinishReason::Compaction,
        Some("max_output_tokens" | "max_messages") => FinishReason::Length,
        _ => FinishReason::Other,
    }
}
