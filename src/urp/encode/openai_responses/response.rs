//! Responses response objects.

use super::items::{ItemTarget, encode_envelope};
use super::media::validate_stable_responses_audio;
use super::usage::encode_usage;
use crate::urp::grouping::envelopes;
use crate::urp::{
    FinishReason, ImageSource, Node, ProviderProtocol, RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY,
    RESPONSES_REQUEST_ECHO_EXTRA_KEY, RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, ResponseOutcome,
    UrpResponse, is_internal_key,
};
use serde_json::{Map, Value, json};

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    encode_response_checked(resp, logical_model)
        .unwrap_or_else(|error| crate::urp::media::error_body(&error))
}

pub fn encode_response_checked(resp: &UrpResponse, logical_model: &str) -> Result<Value, String> {
    let mut prepared = resp.clone();
    prepare_response_nodes(&mut prepared.output)?;
    Ok(encode_response_validated(&prepared, logical_model))
}

pub(crate) fn prepare_response_nodes(nodes: &mut [Node]) -> Result<(), String> {
    validate_response_nodes(nodes)?;
    for node in nodes {
        if let Node::ToolResult { content, .. } = node {
            *content = crate::urp::media::prepare_tool_result_content(
                content,
                ProviderProtocol::Responses,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn validate_response_nodes(nodes: &[Node]) -> Result<(), String> {
    validate_stable_responses_audio(nodes)?;
    for node in nodes {
        match node {
            Node::Image {
                source: ImageSource::Base64 { media_type, .. },
                extra_body,
                ..
            } if extra_body.contains_key(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY)
                && matches!(
                    media_type.as_str(),
                    "image/png" | "image/jpeg" | "image/webp" | "image/svg+xml" | "image/gif"
                ) => {}
            Node::Image { .. } | Node::File { .. } | Node::Audio { .. } => {
                return Err("Responses output cannot represent ordinary image, file, or audio media; native image_generation_call is required for generated images".into());
            }
            Node::ProviderItem { item_type, .. }
                if matches!(
                    item_type.as_str(),
                    "input_image"
                        | "output_image"
                        | "image_url"
                        | "input_file"
                        | "output_file"
                        | "file"
                        | "input_audio"
                ) =>
            {
                return Err("Native response content cannot contain input-only media items".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn encode_response_validated(resp: &UrpResponse, logical_model: &str) -> Value {
    let resp = &crate::urp::tool_signature::project_response(resp);
    let mut output = Vec::new();
    for envelope in envelopes(&resp.output) {
        encode_envelope(envelope, ItemTarget::Output, &mut output);
    }

    let created_at = resp
        .created_at
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let mut body = if resp
        .extra_body
        .contains_key(RESPONSES_RESPONSE_SOURCE_EXTRA_KEY)
    {
        Value::Object(Map::new())
    } else {
        synthesized_response_body(resp)
    };
    if let Some(obj) = body.as_object_mut() {
        obj.retain(|key, _| !is_internal_key(key));
        obj.insert("id".to_string(), Value::String(resp.id.clone()));
        obj.insert("object".to_string(), json!("response"));
        obj.insert("created_at".to_string(), json!(created_at));
        obj.insert(
            "model".to_string(),
            Value::String(logical_model.to_string()),
        );
        obj.insert("output".to_string(), Value::Array(output));
    }

    if let Some(usage) = &resp.usage {
        body["usage"] = encode_usage(usage);
    }

    if let Some(obj) = body.as_object_mut() {
        for (key, value) in &resp.extra_body {
            if !is_internal_key(key)
                && !matches!(
                    key.as_str(),
                    "id" | "object" | "created" | "created_at" | "model" | "output" | "usage"
                )
            {
                obj.insert(key.clone(), value.clone());
            }
        }
    }
    let outcome = resp
        .outcome
        .clone()
        .unwrap_or_else(|| ResponseOutcome::from_finish(resp.finish_reason));
    outcome.write_responses(&mut body);
    finalize_output_item_statuses(&mut body);
    body
}

/// The members of a Responses object synthesized for a non-Responses source; the caller sets
/// identity, model, and output.
fn synthesized_response_body(resp: &UrpResponse) -> Value {
    let status = finish_reason_to_status(resp.finish_reason);
    let completed_at = if status == "completed" {
        json!(chrono::Utc::now().timestamp())
    } else {
        Value::Null
    };
    let mut body = json!({
        "completed_at": completed_at,
        "status": status,
        "incomplete_details": match resp.finish_reason {
            Some(FinishReason::ContentFilter) => json!({"reason": "content_filter"}),
            Some(FinishReason::Length) => json!({"reason": "max_output_tokens"}),
            _ => Value::Null,
        },
        "error": null,
    });
    if let Some(obj) = body.as_object_mut() {
        obj.extend(synthesized_response_controls(
            resp.extra_body.get(RESPONSES_REQUEST_ECHO_EXTRA_KEY),
        ));
    }
    body
}

fn finish_reason_to_status(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(
            FinishReason::Length
            | FinishReason::ContentFilter
            | FinishReason::ContextLimit
            | FinishReason::Paused
            | FinishReason::Compaction,
        ) => "incomplete",
        Some(FinishReason::Other) => "failed",
        _ => "completed",
    }
}

/// STR3p: fills the status of items that carry no native value from the response outcome.
fn finalize_output_item_statuses(body: &mut Value) {
    let last_status = match body.get("status").and_then(Value::as_str) {
        Some("incomplete" | "failed" | "cancelled") => "incomplete",
        Some("queued" | "in_progress") => "in_progress",
        _ => "completed",
    };
    let Some(output) = body.get_mut("output").and_then(Value::as_array_mut) else {
        return;
    };
    let is_status_item = |item: &Value| {
        matches!(
            item.get("type").and_then(Value::as_str),
            Some("message" | "function_call" | "custom_tool_call")
        )
    };
    let last = output.iter().rposition(is_status_item);
    for (index, item) in output.iter_mut().enumerate() {
        if is_status_item(item)
            && let Some(obj) = item.as_object_mut()
            && !obj.contains_key("status")
        {
            let status = if Some(index) == last {
                last_status
            } else {
                "completed"
            };
            obj.insert("status".to_string(), json!(status));
        }
    }
}

/// STR3t: request controls of a Responses object synthesized for a non-Responses source.
pub(crate) fn synthesized_response_controls(echo: Option<&Value>) -> Map<String, Value> {
    let Value::Object(mut controls) = json!({
        "previous_response_id": null,
        "instructions": null,
        "tools": [],
        "tool_choice": "auto",
        "truncation": "disabled",
        "parallel_tool_calls": true,
        "text": { "format": { "type": "text" } },
        "top_p": 1.0,
        "presence_penalty": 0,
        "frequency_penalty": 0,
        "top_logprobs": 0,
        "temperature": 1.0,
        "reasoning": null,
        "max_output_tokens": null,
        "max_tool_calls": null,
        "store": false,
        "background": false,
        "metadata": {},
        "safety_identifier": null,
        "prompt_cache_key": null,
        "user": null,
    }) else {
        unreachable!("object literal")
    };
    if let Some(Value::Object(echo)) = echo {
        controls.extend(echo.iter().map(|(key, value)| (key.clone(), value.clone())));
    }
    controls
}
