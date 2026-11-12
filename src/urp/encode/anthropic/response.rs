use super::blocks::{encode_assistant_response_block, validate_complete_tool_inputs};
use super::media::{messages_media_error_body, validate_response_nodes};
use super::reasoning::with_merged_chat_reasoning;
use super::usage::anthropic_native_usage_json;
use crate::urp::encode::merge_extra;
use crate::urp::{FinishReason, Node, UrpResponse, Usage};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    encode_response_checked(resp, logical_model)
        .unwrap_or_else(|message| messages_media_error_body(&message))
}

/// Encodes a Messages response only when all output nodes have a legal response representation.
pub fn encode_response_checked(resp: &UrpResponse, logical_model: &str) -> Result<Value, String> {
    if let Some(body) = resp
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.failure_body(true))
    {
        return Ok(body);
    }
    validate_response_nodes(&resp.output)?;
    validate_complete_tool_inputs(&resp.output)?;
    Ok(encode_supported_response(
        &crate::urp::tool_signature::project_response(resp),
        logical_model,
    ))
}

fn encode_supported_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut content = Vec::new();
    // Control-node members before the first block belong to the message envelope.
    let mut envelope_extra = HashMap::new();
    for node in with_merged_chat_reasoning(&resp.output) {
        match node.as_ref() {
            Node::NextDownstreamEnvelopeExtra { extra_body } => {
                if content.is_empty() {
                    envelope_extra.extend(
                        extra_body
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone())),
                    );
                }
            }
            node => content.extend(encode_assistant_response_block(node)),
        }
    }

    let stop_reason = resp
        .extra_body
        .get("stop_reason")
        .and_then(Value::as_str)
        .filter(|reason| messages_finish_reason(reason) == resp.finish_reason)
        .unwrap_or_else(|| messages_stop_reason_for_output(resp.finish_reason, &resp.output));
    let usage = match &resp.usage {
        Some(usage) => anthropic_native_usage_json(usage),
        None => anthropic_native_usage_json(&Usage::default()),
    };
    let mut body = Map::new();
    body.insert(
        "id".to_string(),
        Value::String(messages_response_id(&resp.id)),
    );
    body.insert("type".to_string(), json!("message"));
    body.insert("role".to_string(), json!("assistant"));
    body.insert("model".to_string(), json!(logical_model));
    body.insert("content".to_string(), Value::Array(content));
    body.insert("stop_reason".to_string(), json!(stop_reason));
    body.insert("usage".to_string(), usage);
    merge_extra(&mut body, &envelope_extra);
    merge_extra(&mut body, &resp.extra_body);
    if stop_reason != "stop_sequence" {
        body.insert("stop_sequence".into(), Value::Null);
    }
    Value::Object(body)
}

/// Maps the canonical finish reason, reporting `refusal` when an otherwise ordinary stop
/// carries a refusal node from another protocol family (DM4a).
pub(crate) fn messages_stop_reason_for_output(
    finish_reason: Option<FinishReason>,
    output: &[Node],
) -> &'static str {
    if matches!(
        finish_reason,
        None | Some(FinishReason::Stop | FinishReason::Other)
    ) && output
        .iter()
        .any(|node| matches!(node, Node::Refusal { .. }))
    {
        return "refusal";
    }
    match finish_reason {
        Some(FinishReason::Length) => "max_tokens",
        Some(FinishReason::ContextLimit) => "model_context_window_exceeded",
        Some(FinishReason::Paused) => "pause_turn",
        Some(FinishReason::Compaction) => "compaction",
        Some(FinishReason::ToolCalls) => "tool_use",
        Some(FinishReason::ContentFilter) => "refusal",
        _ => "end_turn",
    }
}

/// Keeps native `msg_` ids and replaces foreign or missing ids with a fresh `msg_` id (DM4b).
pub(crate) fn messages_response_id(id: &str) -> String {
    if id.starts_with("msg_") {
        id.to_string()
    } else {
        format!("msg_{}", uuid::Uuid::new_v4().simple())
    }
}

pub(crate) fn messages_finish_reason(reason: &str) -> Option<FinishReason> {
    match reason {
        "end_turn" | "stop_sequence" => Some(FinishReason::Stop),
        "max_tokens" => Some(FinishReason::Length),
        "model_context_window_exceeded" => Some(FinishReason::ContextLimit),
        "pause_turn" => Some(FinishReason::Paused),
        "compaction" => Some(FinishReason::Compaction),
        "tool_use" => Some(FinishReason::ToolCalls),
        "refusal" => Some(FinishReason::ContentFilter),
        "" => None,
        _ => Some(FinishReason::Other),
    }
}
