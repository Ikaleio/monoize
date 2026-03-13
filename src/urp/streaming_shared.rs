use crate::config::ProviderType;
use crate::error::{AppError, AppResult};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::handlers::routing::{now_ts, wrap_responses_event};
use crate::handlers::usage::{latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_chat_object, parse_usage_from_gemini_object, parse_usage_from_messages_object, parse_usage_from_responses_object, record_stream_done_sentinel, record_stream_synthetic_terminal_emitted, record_stream_terminal_event, record_stream_usage_if_present, usage_to_chat_usage_json, usage_to_messages_usage_json, usage_to_responses_usage_json};
use crate::urp::{self};
use axum::http::StatusCode;
use axum::response::sse::Event;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn emit_synthetic_responses_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let encoded = urp::encode::openai_responses::encode_response(resp, logical_model);
    let encoded_output = encoded
        .get("output")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let response_id = encoded
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("resp")
        .to_string();
    let created = encoded
        .get("created")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(now_ts);
    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": logical_model,
        "status": "in_progress",
        "output": []
    });
    send_responses_event(&tx, &mut seq, "response.created", base_response.clone()).await?;
    send_responses_event(&tx, &mut seq, "response.in_progress", base_response).await?;

    for (output_index, item) in encoded_output.iter().enumerate() {
        let item_payload = json!({
            "output_index": output_index,
            "item": item.clone()
        });
        send_responses_event(&tx, &mut seq, "response.output_item.added", item_payload).await?;

        match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "reasoning" => {
                let (text, sig) = extract_reasoning_text_and_signature(item);
                if !text.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_text.delta",
                        json!({}),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                if !sig.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_signature.delta",
                        json!({}),
                        "delta",
                        &sig,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            "function_call" => {
                let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                if !arguments.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.function_call_arguments.delta",
                        json!({
                            "output_index": output_index,
                            "call_id": call_id,
                            "name": name
                        }),
                        "delta",
                        arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            "message" => {
                let text = extract_responses_message_text(item);
                if !text.is_empty() {
                    let phase = extract_responses_message_phase(item);
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.output_text.delta",
                        responses_text_delta_payload("", phase.as_deref()),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            _ => {}
        }

        let done_item = sanitize_responses_output_item_for_frame_limit(item, sse_max_frame_length);
        send_responses_event(
            &tx,
            &mut seq,
            "response.output_item.done",
            json!({
                "output_index": output_index,
                "item": done_item
            }),
        )
        .await?;
    }
    send_responses_event(&tx, &mut seq, "response.output_text.done", json!({})).await?;
    let completed_response = sanitize_responses_completed_for_frame_limit(&encoded, sse_max_frame_length);
    send_responses_event(
        &tx,
        &mut seq,
        "response.completed",
        json!({ "response": completed_response }),
    )
    .await?;
    Ok(())
}

pub(crate) async fn emit_synthetic_chat_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut saw_tool = false;
    let mut tool_idx = 0usize;
    let merged = urp::merged_output_message(&resp.outputs);

    for part in &merged.parts {
        match part {
            urp::Part::Reasoning {
                content,
                encrypted,
                ..
            } => {
                if let Some(content) = content.as_deref().filter(|content| !content.is_empty()) {
                    send_chat_chunk_string(
                        &tx,
                        &id,
                        created,
                        logical_model,
                        chat_reasoning_delta_from_text(""),
                        content,
                        chat_delta_path_reasoning_text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                if let Some(data) = encrypted {
                    let sig = data
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| data.to_string());
                    if !sig.is_empty() {
                        send_chat_chunk_string(
                            &tx,
                            &id,
                            created,
                            logical_model,
                            chat_reasoning_delta_from_signature(""),
                            &sig,
                            chat_delta_path_reasoning_signature,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                }
            }
            urp::Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                saw_tool = true;
                let chunk = json!({
                    "id": id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": logical_model,
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": tool_idx,
                                "id": call_id,
                                "type": "function",
                                "function": { "name": name, "arguments": "" }
                            }]
                        },
                        "finish_reason": Value::Null
                    }]
                });
                tool_idx += 1;
                send_chat_chunk_string(
                    &tx,
                    &id,
                    created,
                    logical_model,
                    chunk["choices"][0]["delta"].clone(),
                    arguments,
                    chat_delta_path_tool_arguments,
                    sse_max_frame_length,
                )
                .await?;
            }
            urp::Part::Text { content, .. } | urp::Part::Refusal { content, .. } => {
                if !content.is_empty() {
                    send_chat_chunk_string(
                        &tx,
                        &id,
                        created,
                        logical_model,
                        json!({ "content": "" }),
                        content,
                        chat_delta_path_content,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            _ => {}
        }
    }

    let finish_reason = if saw_tool {
        "tool_calls"
    } else {
        finish_reason_to_chat(resp.finish_reason.unwrap_or(urp::FinishReason::Stop))
    };
    let mut done = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
    });
    if let Some(usage) = resp.usage.as_ref() {
        done["usage"] = usage_to_chat_usage_json(usage);
    }
    send_plain_sse_data(&tx, done.to_string()).await?;
    send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
    Ok(())
}

pub(super) fn finish_reason_to_chat(reason: urp::FinishReason) -> &'static str {
    match reason {
        urp::FinishReason::Stop => "stop",
        urp::FinishReason::Length => "length",
        urp::FinishReason::ToolCalls => "tool_calls",
        urp::FinishReason::ContentFilter => "content_filter",
        urp::FinishReason::Other => "stop",
    }
}

pub(crate) async fn emit_synthetic_messages_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    let mut saw_tool_use = false;
    let merged = urp::merged_output_message(&resp.outputs);
    let usage = resp.usage.clone().unwrap_or(urp::Usage {
        input_tokens: 0,
        output_tokens: 0,
        input_details: None,
        output_details: None,
        extra_body: HashMap::new(),
    });
    let start = json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": logical_model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens
            }
        }
    });
    send_plain_sse_data(&tx, start.to_string()).await?;

    let mut index = 0u32;
    for part in &merged.parts {
        match part {
            urp::Part::Reasoning {
                content,
                encrypted,
                ..
            } => {
                if let Some(content) = content.as_deref().filter(|content| !content.is_empty()) {
                    let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "thinking", "thinking": "", "signature": "" } });
                    send_plain_sse_data(&tx, s.to_string()).await?;
                    send_messages_delta_string(
                        &tx,
                        json!({ "type": "content_block_delta", "index": index, "delta": { "type": "thinking_delta", "thinking": "" } }),
                        messages_delta_path_thinking,
                        content,
                        sse_max_frame_length,
                    )
                    .await?;
                    let e = json!({ "type": "content_block_stop", "index": index });
                    send_plain_sse_data(&tx, e.to_string()).await?;
                    index += 1;
                }
                if let Some(data) = encrypted {
                    let sig = data
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| data.to_string());
                    if !sig.is_empty() {
                        let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "thinking", "thinking": "", "signature": "" } });
                        send_plain_sse_data(&tx, s.to_string()).await?;
                        send_messages_delta_string(
                            &tx,
                            json!({ "type": "content_block_delta", "index": index, "delta": { "type": "signature_delta", "signature": "" } }),
                            messages_delta_path_signature,
                            &sig,
                            sse_max_frame_length,
                        )
                        .await?;
                        let e = json!({ "type": "content_block_stop", "index": index });
                        send_plain_sse_data(&tx, e.to_string()).await?;
                        index += 1;
                    }
                }
            }
            urp::Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                saw_tool_use = true;
                let start_tool = json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": { "type": "tool_use", "id": call_id, "name": name, "input": {} }
                });
                send_plain_sse_data(&tx, start_tool.to_string()).await?;
                if !arguments.is_empty() {
                    send_messages_delta_string(
                        &tx,
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": { "type": "input_json_delta", "partial_json": "" }
                        }),
                        messages_delta_path_partial_json,
                        arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                let stop_tool = json!({ "type": "content_block_stop", "index": index });
                send_plain_sse_data(&tx, stop_tool.to_string()).await?;
                index += 1;
            }
            urp::Part::Text { content, .. } | urp::Part::Refusal { content, .. } => {
                if content.is_empty() {
                    continue;
                }
                let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "text", "text": "" } });
                send_plain_sse_data(&tx, s.to_string()).await?;
                send_messages_delta_string(
                    &tx,
                    json!({ "type": "content_block_delta", "index": index, "delta": { "type": "text_delta", "text": "" } }),
                    messages_delta_path_text,
                    content,
                    sse_max_frame_length,
                )
                .await?;
                let e = json!({ "type": "content_block_stop", "index": index });
                send_plain_sse_data(&tx, e.to_string()).await?;
                index += 1;
            }
            _ => {}
        }
    }

    let message_delta = json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": if saw_tool_use { "tool_use" } else { "end_turn" },
            "stop_sequence": Value::Null
        },
        "usage": {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens
        }
    });
    send_plain_sse_data(&tx, message_delta.to_string()).await?;
    send_plain_sse_data(&tx, json!({ "type": "message_stop" }).to_string()).await?;
    Ok(())
}

async fn send_plain_sse_data(tx: &mpsc::Sender<Event>, data: String) -> AppResult<()> {
    tx.send(Event::default().data(data))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))
}

async fn send_responses_event(
    tx: &mpsc::Sender<Event>,
    seq: &mut u64,
    name: &str,
    data: Value,
) -> AppResult<()> {
    tx.send(wrap_responses_event(seq, name, data))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))
}

async fn send_responses_delta_string(
    tx: &mpsc::Sender<Event>,
    seq: &mut u64,
    name: &str,
    template: Value,
    field: &str,
    content: &str,
    max_frame_length: Option<usize>,
) -> AppResult<()> {
    for part in split_wrapped_responses_json_string_field(*seq, template, field, content, max_frame_length) {
        send_responses_event(tx, seq, name, part).await?;
    }
    Ok(())
}

async fn send_chat_chunk_string(
    tx: &mpsc::Sender<Event>,
    id: &str,
    created: i64,
    logical_model: &str,
    delta_template: Value,
    content: &str,
    patch: fn(&mut Value, &str),
    max_frame_length: Option<usize>,
) -> AppResult<()> {
    let base = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{ "index": 0, "delta": delta_template, "finish_reason": Value::Null }]
    });
    for chunk in split_json_value_by_string_patch(base, content, patch, max_frame_length) {
        send_plain_sse_data(tx, chunk.to_string()).await?;
    }
    Ok(())
}

async fn send_messages_delta_string(
    tx: &mpsc::Sender<Event>,
    template: Value,
    patch: fn(&mut Value, &str),
    content: &str,
    max_frame_length: Option<usize>,
) -> AppResult<()> {
    for chunk in split_json_value_by_string_patch(template, content, patch, max_frame_length) {
        send_plain_sse_data(tx, chunk.to_string()).await?;
    }
    Ok(())
}

fn split_wrapped_responses_json_string_field(
    seq: u64,
    mut template: Value,
    field: &str,
    content: &str,
    max_frame_length: Option<usize>,
) -> Vec<Value> {
    if let Some(obj) = template.as_object_mut() {
        obj.insert(field.to_string(), Value::String(String::new()));
    }
    let wrapped_template_len = responses_wrapped_payload_length(seq, &template);
    split_json_by_estimated_limit(template, content, max_frame_length, wrapped_template_len, move |value, part| {
        if let Some(obj) = value.as_object_mut() {
            obj.insert(field.to_string(), Value::String(part.to_string()));
        }
    })
}

fn split_json_value_by_string_patch(
    template: Value,
    content: &str,
    patch: fn(&mut Value, &str),
    max_frame_length: Option<usize>,
) -> Vec<Value> {
    let mut empty_template = template.clone();
    patch(&mut empty_template, "");
    let template_len = empty_template.to_string().len();
    split_json_by_estimated_limit(template, content, max_frame_length, template_len, patch)
}

fn split_json_by_estimated_limit(
    template: Value,
    content: &str,
    max_frame_length: Option<usize>,
    template_len: usize,
    patch: impl Fn(&mut Value, &str),
) -> Vec<Value> {
    const ESTIMATED_ESCAPE_RESERVE_BYTES: usize = 128;

    let Some(max_len) = max_frame_length else {
        let mut value = template;
        patch(&mut value, content);
        return vec![value];
    };

    let mut empty_value = template.clone();
    patch(&mut empty_value, "");
    if template_len > max_len {
        return vec![empty_value];
    }
    if content.is_empty() {
        return vec![empty_value];
    }

    let chunk_size = max_len
        .saturating_sub(template_len)
        .saturating_sub(ESTIMATED_ESCAPE_RESERVE_BYTES)
        .max(1);

    split_string_by_bytes(content, chunk_size)
        .into_iter()
        .map(|part| {
            let mut value = template.clone();
            patch(&mut value, &part);
            value
        })
        .collect()
}

fn responses_wrapped_payload_length(seq: u64, data: &Value) -> usize {
    json!({ "sequence_number": seq, "data": data })
        .to_string()
        .len()
}

fn split_string_by_bytes(content: &str, max_bytes: usize) -> Vec<String> {
    if content.is_empty() {
        return vec![String::new()];
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_bytes = 0usize;
    for ch in content.chars() {
        let ch_len = ch.len_utf8();
        if !current.is_empty() && current_bytes + ch_len > max_bytes {
            parts.push(current);
            current = String::new();
            current_bytes = 0;
        }
        current.push(ch);
        current_bytes += ch_len;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    if parts.is_empty() {
        parts.push(String::new());
    }
    parts
}

fn chat_delta_path_content(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
    {
        delta.insert("content".to_string(), Value::String(content.to_string()));
    }
}

fn chat_delta_path_reasoning_text(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
    {
        delta.insert(
            "reasoning_details".to_string(),
            json!([{ "type": "reasoning.text", "text": content, "signature": Value::Null, "format": "unknown" }]),
        );
    }
}

fn chat_delta_path_reasoning_signature(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
    {
        delta.insert(
            "reasoning_details".to_string(),
            json!([{ "type": "reasoning.text", "text": "", "signature": content, "format": "unknown" }]),
        );
    }
}

fn chat_delta_path_tool_arguments(value: &mut Value, content: &str) {
    if let Some(function) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
        .and_then(|delta| delta.get_mut("tool_calls"))
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|tool| tool.get_mut("function"))
        .and_then(Value::as_object_mut)
    {
        function.insert("arguments".to_string(), Value::String(content.to_string()));
    }
}

fn messages_delta_path_text(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("delta")
        .and_then(Value::as_object_mut)
    {
        delta.insert("text".to_string(), Value::String(content.to_string()));
    }
}

fn messages_delta_path_thinking(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("delta")
        .and_then(Value::as_object_mut)
    {
        delta.insert("thinking".to_string(), Value::String(content.to_string()));
    }
}

fn messages_delta_path_signature(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("delta")
        .and_then(Value::as_object_mut)
    {
        delta.insert("signature".to_string(), Value::String(content.to_string()));
    }
}

fn messages_delta_path_partial_json(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("delta")
        .and_then(Value::as_object_mut)
    {
        delta.insert("partial_json".to_string(), Value::String(content.to_string()));
    }
}

fn sanitize_responses_output_item_for_frame_limit(item: &Value, max_frame_length: Option<usize>) -> Value {
    let Some(max_len) = max_frame_length else {
        return item.clone();
    };
    if item.to_string().len() <= max_len {
        return item.clone();
    }
    let mut sanitized = item.clone();
    if let Some(obj) = sanitized.as_object_mut() {
        match obj.get("type").and_then(|v| v.as_str()) {
            Some("message") => {
                if let Some(content) = obj.get_mut("content").and_then(Value::as_array_mut) {
                    for part in content {
                        if let Some(part_obj) = part.as_object_mut() {
                            if part_obj.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                                part_obj.insert("text".to_string(), Value::String(String::new()));
                            }
                        }
                    }
                }
            }
            Some("reasoning") => {
                obj.insert("text".to_string(), Value::String(String::new()));
                if let Some(summary) = obj.get_mut("summary").and_then(Value::as_array_mut) {
                    for part in summary {
                        if let Some(part_obj) = part.as_object_mut() {
                            part_obj.insert("text".to_string(), Value::String(String::new()));
                        }
                    }
                }
            }
            Some("function_call") => {
                obj.insert("arguments".to_string(), Value::String(String::new()));
            }
            _ => {}
        }
    }
    sanitized
}

fn sanitize_responses_completed_for_frame_limit(encoded: &Value, max_frame_length: Option<usize>) -> Value {
    let Some(max_len) = max_frame_length else {
        return encoded.clone();
    };
    if encoded.to_string().len() <= max_len {
        return encoded.clone();
    }
    let mut sanitized = encoded.clone();
    if let Some(output) = sanitized.get_mut("output").and_then(Value::as_array_mut) {
        for item in output.iter_mut() {
            *item = sanitize_responses_output_item_for_frame_limit(item, Some(max_len));
        }
    }
    sanitized
}

pub(super) fn extract_reasoning_text_and_signature(item: &Value) -> (String, String) {
    let mut text = item
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if text.is_empty() {
        if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
            let mut parts = Vec::new();
            for s in summary {
                if s.get("type").and_then(|v| v.as_str()) == Some("summary_text") {
                    if let Some(t) = s.get("text").and_then(|v| v.as_str()) {
                        if !t.is_empty() {
                            parts.push(t);
                        }
                    }
                }
            }
            if !parts.is_empty() {
                text = parts.join("\n");
            }
        }
    }
    let mut signature = item
        .get("signature")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if signature.is_empty() {
        signature = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }
    (text, signature)
}

pub(super) fn reasoning_text_detail_value(text: &str, signature: Option<&str>) -> Value {
    json!({
        "type": "reasoning.text",
        "text": text,
        "signature": signature,
        "format": "unknown"
    })
}

pub(super) fn reasoning_encrypted_detail_value(data: Value) -> Value {
    json!({
        "type": "reasoning.encrypted",
        "data": data,
        "format": "unknown"
    })
}

pub(super) fn extract_chat_reasoning_from_detail(
    detail: &Value,
    text_out: &mut Vec<String>,
    sig_out: &mut Vec<String>,
) {
    let Some(obj) = detail.as_object() else {
        return;
    };
    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "reasoning.text" => {
            if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
                if !t.is_empty() {
                    text_out.push(t.to_string());
                }
            }
            if let Some(sig) = obj.get("signature").and_then(|v| v.as_str()) {
                if !sig.is_empty() {
                    sig_out.push(sig.to_string());
                }
            }
        }
        "reasoning.encrypted" => {
            if let Some(data) = obj.get("data") {
                match data {
                    Value::String(s) if !s.is_empty() => sig_out.push(s.clone()),
                    Value::String(_) | Value::Null => {}
                    other => sig_out.push(other.to_string()),
                }
            }
        }
        "reasoning.summary" => {
            if let Some(summary) = obj.get("summary").and_then(|v| v.as_str()) {
                if !summary.is_empty() {
                    text_out.push(summary.to_string());
                }
            }
        }
        _ => {}
    }
}

pub(super) fn extract_chat_reasoning_deltas(delta: &Value) -> (Vec<String>, Vec<String>) {
    let mut text_parts = Vec::new();
    let mut sig_parts = Vec::new();

    if let Some(details) = delta.get("reasoning_details").and_then(|v| v.as_array()) {
        for detail in details {
            extract_chat_reasoning_from_detail(detail, &mut text_parts, &mut sig_parts);
        }
    }

    if text_parts.is_empty() {
        if let Some(reasoning) = delta.get("reasoning").and_then(|v| v.as_str()) {
            if !reasoning.is_empty() {
                text_parts.push(reasoning.to_string());
            }
        }
    }

    if let Some(reasoning) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
        if !reasoning.is_empty() {
            text_parts.push(reasoning.to_string());
        }
    }
    if let Some(sig) = delta.get("reasoning_opaque").and_then(|v| v.as_str()) {
        if !sig.is_empty() {
            sig_parts.push(sig.to_string());
        }
    }

    (text_parts, sig_parts)
}

pub(super) fn chat_reasoning_delta_from_text(text: &str) -> Value {
    json!({
        "reasoning_details": [reasoning_text_detail_value(text, None)]
    })
}

pub(super) fn chat_reasoning_delta_from_signature(signature: &str) -> Value {
    json!({
        "reasoning_details": [reasoning_encrypted_detail_value(Value::String(signature.to_string()))]
    })
}

pub(super) fn normalize_chat_reasoning_delta_object(delta: &mut Map<String, Value>) {
    if delta
        .get("reasoning_details")
        .and_then(|v| v.as_array())
        .is_none()
    {
        let mut details = Vec::new();
        if let Some(text) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
            if !text.is_empty() {
                details.push(reasoning_text_detail_value(text, None));
            }
        }
        if let Some(sig) = delta.get("reasoning_opaque").and_then(|v| v.as_str()) {
            if !sig.is_empty() {
                details.push(reasoning_encrypted_detail_value(Value::String(
                    sig.to_string(),
                )));
            }
        }
        if !details.is_empty() {
            delta.insert("reasoning_details".to_string(), Value::Array(details));
        }
    }
    delta.remove("reasoning_content");
    delta.remove("reasoning_opaque");
}

pub(super) fn extract_responses_message_text(item: &Value) -> String {
    let mut out = String::new();
    if item.get("type").and_then(|v| v.as_str()) != Some("message") {
        return out;
    }
    if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
        for part in content {
            if part.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
        }
    }
    out
}

pub(super) fn extract_responses_message_phase(item: &Value) -> Option<String> {
    if item.get("type").and_then(|v| v.as_str()) != Some("message") {
        return None;
    }
    item.get("phase")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn insert_phase_if_present(obj: &mut Map<String, Value>, phase: Option<&str>) {
    if let Some(phase) = phase {
        obj.insert("phase".to_string(), Value::String(phase.to_string()));
    }
}

fn responses_text_delta_payload(text: &str, phase: Option<&str>) -> Value {
    let mut obj = Map::new();
    obj.insert("text".to_string(), Value::String(text.to_string()));
    insert_phase_if_present(&mut obj, phase);
    Value::Object(obj)
}


pub(super) async fn ensure_anthropic_text_block(
    tx: &mpsc::Sender<Event>,
    text_index: &mut Option<u32>,
    next_index: &mut u32,
    started: &mut Vec<u32>,
) -> AppResult<u32> {
    if let Some(i) = *text_index {
        return Ok(i);
    }
    let i = *next_index;
    *next_index += 1;
    *text_index = Some(i);
    started.push(i);
    let block_start = json!({
        "type": "content_block_start",
        "index": i,
        "content_block": { "type": "text", "text": "" }
    });
    tx.send(Event::default().data(block_start.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(i)
}

pub(super) async fn ensure_anthropic_thinking_block(
    tx: &mpsc::Sender<Event>,
    thinking_index: &mut Option<u32>,
    next_index: &mut u32,
    started: &mut Vec<u32>,
) -> AppResult<u32> {
    if let Some(i) = *thinking_index {
        return Ok(i);
    }
    let i = *next_index;
    *next_index += 1;
    *thinking_index = Some(i);
    started.push(i);
    let block_start = json!({
        "type": "content_block_start",
        "index": i,
        "content_block": { "type": "thinking", "thinking": "", "signature": "" }
    });
    tx.send(Event::default().data(block_start.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(i)
}

pub(super) async fn ensure_anthropic_tool_block(
    tx: &mpsc::Sender<Event>,
    tool_indices: &mut HashMap<String, u32>,
    tool_names: &mut HashMap<String, String>,
    next_index: &mut u32,
    started: &mut Vec<u32>,
    call_id: &str,
    name: &str,
) -> AppResult<u32> {
    if let Some(i) = tool_indices.get(call_id).copied() {
        if !name.is_empty() && !tool_names.contains_key(call_id) {
            tool_names.insert(call_id.to_string(), name.to_string());
        }
        return Ok(i);
    }
    let i = *next_index;
    *next_index += 1;
    tool_indices.insert(call_id.to_string(), i);
    if !name.is_empty() {
        tool_names.insert(call_id.to_string(), name.to_string());
    }
    started.push(i);
    let block_start = json!({
        "type": "content_block_start",
        "index": i,
        "content_block": { "type": "tool_use", "id": call_id, "name": name, "input": {} }
    });
    tx.send(Event::default().data(block_start.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(i)
}

pub(crate) async fn stream_responses_sse_as_responses(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut message_phases_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, arguments)
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_text_delta = false;

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
        .await;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        // For responses upstream, we forward event names and data into our wrapper.
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::String(ev.data));
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_responses_object(&data_val),
        )
        .await;
        // Try to extract text deltas for final output.
        if ev.event == "response.output_text.delta" {
            if let Some(text) = data_val
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
            {
                output_text.push_str(text);
                saw_text_delta = true;
            }
        }
        if ev.event == "response.reasoning_text.delta" {
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                reasoning_text.push_str(delta);
            }
        }
        if ev.event == "response.reasoning_signature.delta" {
            if let Some(delta) = data_val.get("delta").and_then(|v| v.as_str()) {
                reasoning_sig.push_str(delta);
            }
        }
        if ev.event == "response.output_item.added" {
            let item = data_val.get("item").unwrap_or(&data_val);
            if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                    if !calls.contains_key(call_id) {
                        call_order.push(call_id.to_string());
                        calls.insert(
                            call_id.to_string(),
                            (
                                item.get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                item.get("arguments")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            ),
                        );
                    }
                    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                        call_ids_by_output_index.insert(idx, call_id.to_string());
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    if let Some(phase) = extract_responses_message_phase(item) {
                        message_phases_by_output_index.insert(idx, phase);
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                let (text, sig) = extract_reasoning_text_and_signature(item);
                if reasoning_text.is_empty() && !text.is_empty() {
                    reasoning_text = text;
                }
                if reasoning_sig.is_empty() && !sig.is_empty() {
                    reasoning_sig = sig;
                }
            }
        }
        if ev.event == "response.function_call_arguments.delta" {
            let call_id_opt = data_val
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    data_val
                        .get("output_index")
                        .and_then(|v| v.as_u64())
                        .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                });
            if let Some(call_id) = call_id_opt {
                let name = data_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if !calls.contains_key(call_id.as_str()) {
                    call_order.push(call_id.clone());
                    calls.insert(call_id.clone(), (name.to_string(), String::new()));
                }
                if let Some(entry) = calls.get_mut(call_id.as_str()) {
                    if entry.0.is_empty() && !name.is_empty() {
                        entry.0 = name.to_string();
                    }
                    entry.1.push_str(delta);
                }
            }
        }
        if ev.event == "response.function_call_arguments.done" {
            let call_id_opt = data_val
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    data_val
                        .get("output_index")
                        .and_then(|v| v.as_u64())
                        .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                });
            if let Some(call_id) = call_id_opt {
                let args = data_val
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(entry) = calls.get_mut(call_id.as_str()) {
                    if entry.1.is_empty() && !args.is_empty() {
                        entry.1 = args.to_string();
                    }
                }
            }
        }
        if ev.event == "response.output_item.done" {
            let item = data_val.get("item").unwrap_or(&data_val);
            if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                    if !calls.contains_key(call_id) {
                        call_order.push(call_id.to_string());
                        calls.insert(call_id.to_string(), (name.to_string(), args.to_string()));
                    } else if let Some(entry) = calls.get_mut(call_id) {
                        if entry.0.is_empty() && !name.is_empty() {
                            entry.0 = name.to_string();
                        }
                        if entry.1.is_empty() && !args.is_empty() {
                            entry.1 = args.to_string();
                        }
                    }
                    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                        call_ids_by_output_index.insert(idx, call_id.to_string());
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                let (text, sig) = extract_reasoning_text_and_signature(item);
                if reasoning_text.is_empty() && !text.is_empty() {
                    reasoning_text = text;
                }
                if reasoning_sig.is_empty() && !sig.is_empty() {
                    reasoning_sig = sig;
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("message")
                && !saw_text_delta {
                    output_text.push_str(&extract_responses_message_text(item));
                    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                        if let Some(phase) = extract_responses_message_phase(item) {
                            message_phases_by_output_index.insert(idx, phase);
                        }
                    }
                }
        }
        let data_val = if ev.event == "response.output_text.delta" {
            let mut payload = data_val;
            if let Some(idx) = payload.get("output_index").and_then(|v| v.as_u64()) {
                if let Some(phase) = message_phases_by_output_index.get(&idx) {
                    if let Some(obj) = payload.as_object_mut() {
                        obj.entry("phase".to_string())
                            .or_insert_with(|| Value::String(phase.clone()));
                    }
                }
            }
            payload
        } else {
            data_val
        };
        let name = if ev.event.is_empty() {
            "message"
        } else {
            ev.event.as_str()
        };
        let _ = tx
            .send(wrap_responses_event(&mut seq, name, data_val))
            .await;
    }

    // Minimal completion sequence.
    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            output_items.push(json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            }));
        }
    }
    let output_item = if let Some((_, phase)) = message_phases_by_output_index.iter().min_by_key(|(idx, _)| *idx) {
        json!({
            "type": "message",
            "role": "assistant",
            "phase": phase,
            "content": [{ "type": "output_text", "text": output_text }]
        })
    } else {
        json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": output_text }]
        })
    };
    output_items.push(output_item.clone());
    let mut final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    if let Some(usage) = latest_stream_usage_snapshot(&runtime_metrics).await {
        final_response["usage"] = usage_to_responses_usage_json(&usage);
    }
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.added",
            json!({ "output_index": output_items.len() - 1, "item": output_item.clone() }),
        ))
        .await;
    if !saw_text_delta {
        if let Some(text) = output_item
            .get("content")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|p| p.get("text"))
            .and_then(|v| v.as_str())
        {
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_text.delta",
                    responses_text_delta_payload(
                        text,
                        extract_responses_message_phase(&output_item).as_deref(),
                    ),
                ))
                .await;
        }
    }
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            json!({ "output_index": output_items.len() - 1, "item": output_item }),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

pub(crate) async fn stream_chat_sse_as_responses(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut assistant_message_phase: Option<String> = None;
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, arguments)
    let mut call_id_by_index: HashMap<usize, String> = HashMap::new();

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.added",
            json!({
                "output_index": 0,
                "item": {"type":"message","role":"assistant","content":[]}
            }),
        ))
        .await;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_chat_object(&data_val))
            .await;
        if let Some(reason) = data_val
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|v| v.as_str())
            .filter(|reason| !reason.is_empty())
        {
            record_stream_terminal_event(&runtime_metrics, "chat.completion.chunk", Some(reason))
                .await;
        }
        let delta = data_val
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("delta"))
            .cloned()
            .unwrap_or(Value::Null);

        if assistant_message_phase.is_none() {
            assistant_message_phase = delta
                .get("phase")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    data_val
                        .get("choices")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|choice| choice.get("delta"))
                        .and_then(|delta| delta.get("phase"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                });
        }

        if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
            if !t.is_empty() {
                output_text.push_str(t);
                let _ = tx
                    .send(wrap_responses_event(
                        &mut seq,
                        "response.output_text.delta",
                        responses_text_delta_payload(t, assistant_message_phase.as_deref()),
                    ))
                    .await;
            }
        }

        let (reasoning_text_deltas, reasoning_sig_deltas) = extract_chat_reasoning_deltas(&delta);
        for t in reasoning_text_deltas {
            reasoning_text.push_str(&t);
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.reasoning_text.delta",
                    json!({ "delta": t }),
                ))
                .await;
        }
        for sig in reasoning_sig_deltas {
            reasoning_sig.push_str(&sig);
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.reasoning_signature.delta",
                    json!({ "delta": sig }),
                ))
                .await;
        }

        // Tool call deltas (OpenAI chat format).
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for (tool_call_pos, tc) in tool_calls.iter().enumerate() {
                let tc_index = tc.get("index").and_then(|v| v.as_u64()).map(|v| v as usize);
                let mut call_id = tc
                    .get("id")
                    .or_else(|| tc.get("call_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if call_id.is_empty() {
                    if let Some(idx) = tc_index {
                        if let Some(existing) = call_id_by_index.get(&idx) {
                            call_id = existing.clone();
                        }
                    }
                }
                if call_id.is_empty() && tool_calls.len() == 1 {
                    if let Some(last) = call_order.last() {
                        call_id = last.clone();
                    }
                }
                if call_id.is_empty() {
                    if let Some(existing) = call_order.get(tool_call_pos) {
                        call_id = existing.clone();
                    }
                }
                if call_id.is_empty() {
                    continue;
                }
                if let Some(idx) = tc_index {
                    call_id_by_index.insert(idx, call_id.clone());
                }
                let name = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args_delta = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .map(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_default();

                if !calls.contains_key(&call_id) {
                    call_order.push(call_id.clone());
                    calls.insert(call_id.clone(), (name.clone(), String::new()));
                    let item = json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": ""
                    });
                    let _ = tx
                        .send(wrap_responses_event(
                            &mut seq,
                            "response.output_item.added",
                            json!({
                                "output_index": call_order.len(),
                                "item": item
                            }),
                        ))
                        .await;
                }

                let Some(entry) = calls.get_mut(&call_id) else {
                    tracing::warn!(call_id = %call_id, "unknown call_id in tool call stream delta, skipping");
                    continue;
                };

                if !name.is_empty() && entry.0.is_empty() {
                    entry.0 = name.clone();
                }
                if !args_delta.is_empty() {
                    entry.1.push_str(&args_delta);
                    let _ = tx
                        .send(wrap_responses_event(
                            &mut seq,
                            "response.function_call_arguments.delta",
                            json!({ "call_id": call_id, "name": entry.0, "delta": args_delta }),
                        ))
                        .await;
                }
            }
        }
    }

    // Finalize any function calls encountered in the chat stream.
    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            let item = json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            });
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.done",
                    json!({
                        "output_index": output_items.len(),
                        "item": item.clone()
                    }),
                ))
                .await;
            output_items.push(item);
        }
    }

    let output_item = if let Some(phase) = assistant_message_phase.as_deref() {
        json!({
            "type": "message",
            "role": "assistant",
            "phase": phase,
            "content": [{ "type": "output_text", "text": output_text }]
        })
    } else {
        json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": output_text }]
        })
    };
    output_items.push(output_item.clone());
    let mut final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    if let Some(usage) = latest_stream_usage_snapshot(&runtime_metrics).await {
        final_response["usage"] = usage_to_responses_usage_json(&usage);
    }
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            json!({
                "output_index": output_items.len() - 1,
                "item": output_item
            }),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

pub(crate) async fn stream_messages_sse_as_responses(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, arguments_json)
    let mut current_tool_call_id: Option<String> = None;

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.added",
            json!({"type":"message","role":"assistant","content":[]}),
        ))
        .await;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_messages_object(&data_val),
        )
        .await;
        let event_type = data_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match event_type {
            "content_block_start" => {
                let cb = data_val
                    .get("content_block")
                    .cloned()
                    .unwrap_or(Value::Null);
                let cb_type = cb.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if cb_type == "tool_use" {
                    let call_id = cb
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = cb
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    current_tool_call_id = if call_id.is_empty() {
                        None
                    } else {
                        Some(call_id.clone())
                    };
                    if !call_id.is_empty() && !calls.contains_key(&call_id) {
                        call_order.push(call_id.clone());
                        calls.insert(call_id.clone(), (name.clone(), String::new()));
                        let item = json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": name,
                            "arguments": ""
                        });
                        let _ = tx
                            .send(wrap_responses_event(
                                &mut seq,
                                "response.output_item.added",
                                item,
                            ))
                            .await;
                    }
                }
            }
            "content_block_delta" => {
                let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                output_text.push_str(text);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.output_text.delta",
                                        json!({ "text": text }),
                                    ))
                                    .await;
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                            if !t.is_empty() {
                                reasoning_text.push_str(t);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_text.delta",
                                        json!({ "delta": t }),
                                    ))
                                    .await;
                            }
                        }
                    }
                    "signature_delta" => {
                        if let Some(s) = delta.get("signature").and_then(|v| v.as_str()) {
                            if !s.is_empty() {
                                reasoning_sig.push_str(s);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_signature.delta",
                                        json!({ "delta": s }),
                                    ))
                                    .await;
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            if let Some(call_id) = current_tool_call_id.clone() {
                                if let Some(entry) = calls.get_mut(&call_id) {
                                    entry.1.push_str(partial);
                                    let _ = tx
                                        .send(wrap_responses_event(
                                            &mut seq,
                                            "response.function_call_arguments.delta",
                                            json!({ "call_id": call_id, "name": entry.0, "delta": partial }),
                                        ))
                                        .await;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                current_tool_call_id = None;
            }
            "message_stop" => {
                record_stream_terminal_event(&runtime_metrics, "message_stop", None).await;
                break;
            }
            _ => {}
        }
    }

    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            let item = json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            });
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.done",
                    item.clone(),
                ))
                .await;
            output_items.push(item);
        }
    }

    let output_item = json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": output_text }]
    });
    output_items.push(output_item.clone());
    let mut final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    if let Some(usage) = latest_stream_usage_snapshot(&runtime_metrics).await {
        final_response["usage"] = usage_to_responses_usage_json(&usage);
    }
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            output_item,
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

pub(crate) async fn stream_gemini_sse_as_responses(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
        .await;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_gemini_object(&data_val))
            .await;
        let Some(candidate) = data_val
            .get("candidates")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        else {
            continue;
        };
        if let Some(parts) = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                        if !text.is_empty() {
                            reasoning_text.push_str(text);
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.reasoning_text.delta",
                                    json!({ "delta": text }),
                                ))
                                .await;
                        }
                        if let Some(sig) = part.get("thoughtSignature") {
                            let sig_text = sig
                                .as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| sig.to_string());
                            if !sig_text.is_empty() {
                                reasoning_sig.push_str(&sig_text);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_signature.delta",
                                        json!({ "delta": sig_text }),
                                    ))
                                    .await;
                            }
                        }
                    } else if !text.is_empty() {
                        output_text.push_str(text);
                        let _ = tx
                            .send(wrap_responses_event(
                                &mut seq,
                                "response.output_text.delta",
                                json!({ "text": text }),
                            ))
                            .await;
                    }
                }

                if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                    let call_id = fc
                        .get("id")
                        .or_else(|| fc.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = fc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments =
                        serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                            .unwrap_or_else(|_| "{}".to_string());
                    if !name.is_empty() {
                        let key = if call_id.is_empty() {
                            format!("call_{}", call_order.len() + 1)
                        } else {
                            call_id
                        };
                        if !calls.contains_key(&key) {
                            call_order.push(key.clone());
                            calls.insert(key.clone(), (name.clone(), String::new()));
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.output_item.added",
                                    json!({
                                        "type": "function_call",
                                        "call_id": key,
                                        "name": name,
                                        "arguments": ""
                                    }),
                                ))
                                .await;
                        }
                        if !arguments.is_empty() {
                            if let Some(entry) = calls.get_mut(&key) {
                                entry.1.push_str(&arguments);
                            }
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.function_call_arguments.delta",
                                    json!({ "call_id": key, "name": name, "delta": arguments }),
                                ))
                                .await;
                        }
                    }
                }
            }
        }
    }

    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            let item = json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            });
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.done",
                    item.clone(),
                ))
                .await;
            output_items.push(item);
        }
    }

    let output_item = json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": output_text }]
    });
    output_items.push(output_item.clone());
    let mut final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    if let Some(usage) = latest_stream_usage_snapshot(&runtime_metrics).await {
        final_response["usage"] = usage_to_responses_usage_json(&usage);
    }
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            output_item,
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

pub(crate) async fn stream_gemini_sse_as_chat(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut saw_tool_call = false;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_gemini_object(&data_val))
            .await;
        let Some(candidate) = data_val
            .get("candidates")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        else {
            continue;
        };
        let Some(parts) = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(text), "finish_reason": Value::Null }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                    if let Some(sig) = part.get("thoughtSignature") {
                        let sig_text = sig
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sig.to_string());
                        if !sig_text.is_empty() {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&sig_text), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                } else if !text.is_empty() {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                }
            }

            if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                let call_id = fc
                    .get("id")
                    .or_else(|| fc.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = fc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                    .unwrap_or_else(|_| "{}".to_string());
                if !name.is_empty() {
                    saw_tool_call = true;
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": 0,
                                    "id": call_id,
                                    "type": "function",
                                    "function": { "name": name, "arguments": args }
                                }]
                            },
                            "finish_reason": Value::Null
                        }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                }
            }
        }
    }

    let finish_reason = if saw_tool_call { "tool_calls" } else { "stop" };
    let mut done = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": urp.model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
    });
    if let Some(usage) = latest_stream_usage_snapshot(&runtime_metrics).await {
        done["usage"] = usage_to_chat_usage_json(&usage);
    }
    record_stream_synthetic_terminal_emitted(&runtime_metrics).await;
    let _ = tx.send(Event::default().data(done.to_string())).await;
    record_stream_terminal_event(&runtime_metrics, "chat.completion.chunk", Some(finish_reason))
        .await;
    let _ = tx.send(Event::default().data("[DONE]")).await;
    Ok(())
}

pub(crate) async fn stream_gemini_sse_as_messages(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    let start = json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": urp.model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0
            }
        }
    });
    let _ = tx.send(Event::default().data(start.to_string())).await;

    let mut next_index: u32 = 0;
    let mut text_index: Option<u32> = None;
    let mut thinking_index: Option<u32> = None;
    let mut tool_indices: HashMap<String, u32> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut started: Vec<u32> = Vec::new();
    let mut saw_tool_use = false;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_gemini_object(&data_val))
            .await;
        let Some(candidate) = data_val
            .get("candidates")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        else {
            continue;
        };
        let Some(parts) = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                    let idx = ensure_anthropic_thinking_block(
                        &tx,
                        &mut thinking_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "thinking_delta", "thinking": text }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                    if let Some(sig) = part.get("thoughtSignature") {
                        let sig_text = sig
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sig.to_string());
                        if !sig_text.is_empty() {
                            let d = json!({
                                "type": "content_block_delta",
                                "index": idx,
                                "delta": { "type": "signature_delta", "signature": sig_text }
                            });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                } else if !text.is_empty() {
                    let idx = ensure_anthropic_text_block(
                        &tx,
                        &mut text_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "text_delta", "text": text }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }
            }

            if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                let call_id = fc
                    .get("id")
                    .or_else(|| fc.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let name = fc.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                    .unwrap_or_else(|_| "{}".to_string());
                if !name.is_empty() {
                    saw_tool_use = true;
                    let idx = ensure_anthropic_tool_block(
                        &tx,
                        &mut tool_indices,
                        &mut tool_names,
                        &mut next_index,
                        &mut started,
                        call_id,
                        name,
                    )
                    .await?;
                    if !args.is_empty() {
                        let d = json!({
                            "type": "content_block_delta",
                            "index": idx,
                            "delta": { "type": "input_json_delta", "partial_json": args }
                        });
                        let _ = tx.send(Event::default().data(d.to_string())).await;
                    }
                }
            }
        }
    }

    for idx in started.iter() {
        let stop = json!({ "type": "content_block_stop", "index": idx });
        let _ = tx.send(Event::default().data(stop.to_string())).await;
    }
    let message_usage = latest_stream_usage_snapshot(&runtime_metrics)
        .await
        .map(|usage| usage_to_messages_usage_json(&usage))
        .unwrap_or_else(|| json!({ "input_tokens": 0, "output_tokens": 0 }));
    let message_delta = json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": if saw_tool_use { "tool_use" } else { "end_turn" },
            "stop_sequence": Value::Null
        },
        "usage": message_usage
    });
    let _ = tx
        .send(Event::default().data(message_delta.to_string()))
        .await;
    let stop = json!({ "type": "message_stop" });
    let _ = tx.send(Event::default().data(stop.to_string())).await;
    Ok(())
}

pub(crate) async fn stream_any_sse_as_chat(
    urp: &HandlerUrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut out_text = String::new();
    let mut saw_tool_call = false;
    let mut saw_responses_text_delta = false;
    let mut saw_responses_tool_delta = false;
    let mut saw_responses_reasoning_delta = false;
    let mut call_order: Vec<String> = Vec::new();
    let mut call_names: HashMap<String, String> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_upstream_terminal_finish = false;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }
        match provider_type {
            ProviderType::ChatCompletion => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_chat_object(&data_val),
                )
                .await;
                let mut chunk = data_val;
                if let Some(obj) = chunk.as_object_mut() {
                    obj.insert("model".to_string(), Value::String(urp.model.clone()));
                    if !obj.contains_key("id") {
                        obj.insert("id".to_string(), Value::String(id.clone()));
                    }
                    if !obj.contains_key("object") {
                        obj.insert(
                            "object".to_string(),
                            Value::String("chat.completion.chunk".to_string()),
                        );
                    }
                    if !obj.contains_key("created") {
                        obj.insert("created".to_string(), Value::Number(created.into()));
                    }
                    if let Some(delta) = obj
                        .get_mut("choices")
                        .and_then(|v| v.as_array_mut())
                        .and_then(|arr| arr.first_mut())
                        .and_then(|v| v.get_mut("delta"))
                        .and_then(|v| v.as_object_mut())
                    {
                        normalize_chat_reasoning_delta_object(delta);
                        // Upstream may send explicit nulls (role: null, content: null, etc.)
                        // that violate the OpenAI streaming schema. Strip them — null semantically
                        // means "no update" and is equivalent to the field being absent.
                        delta.retain(|_, v| !v.is_null());
                    }
                }

                if let Some(t) = chunk
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
                {
                    out_text.push_str(t);
                }
                let choice_snapshot = chunk
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .cloned();
                if let Some(choice) = choice_snapshot {
                    let has_tool_delta = choice
                        .get("delta")
                        .and_then(|d| d.get("tool_calls"))
                        .and_then(|v| v.as_array())
                        .map(|arr| !arr.is_empty())
                        .unwrap_or(false);
                    if has_tool_delta {
                        saw_tool_call = true;
                    }

                    if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                        if !reason.is_empty() {
                            saw_upstream_terminal_finish = true;
                            record_stream_terminal_event(
                                &runtime_metrics,
                                "chat.completion.chunk",
                                Some(reason),
                            )
                            .await;
                            if reason == "tool_calls" {
                                saw_tool_call = true;
                            }
                            if reason == "stop" && saw_tool_call {
                                if let Some(choice_obj) = chunk
                                    .get_mut("choices")
                                    .and_then(|v| v.as_array_mut())
                                    .and_then(|arr| arr.first_mut())
                                    .and_then(|v| v.as_object_mut())
                                {
                                    choice_obj.insert(
                                        "finish_reason".to_string(),
                                        Value::String("tool_calls".to_string()),
                                    );
                                }
                            }
                        }
                    }
                }

                let _ = tx.send(Event::default().data(chunk.to_string())).await;
            }
            ProviderType::Responses | ProviderType::Grok => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_responses_object(&data_val),
                )
                .await;
                match ev.event.as_str() {
                    "response.output_text.delta" => {
                        let t = data_val
                            .get("text")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_text_delta = true;
                            out_text.push_str(t);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": { "content": t }, "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.reasoning_text.delta" => {
                        let t = data_val
                            .get("delta")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_reasoning_delta = true;
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(t), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.reasoning_signature.delta" => {
                        let t = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_reasoning_delta = true;
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(t), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.output_item.added" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !call_id.is_empty() {
                                saw_responses_tool_delta = true;
                                if !call_order.contains(&call_id) {
                                    call_order.push(call_id.clone());
                                }
                                if !name.is_empty() {
                                    call_names.insert(call_id.clone(), name.clone());
                                }
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index.insert(output_index, call_id.clone());
                                }
                                let idx =
                                    call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                saw_tool_call = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {
                                            "tool_calls": [{
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": "" }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let call_id = data_val
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                data_val
                                    .get("output_index")
                                    .and_then(|v| v.as_u64())
                                    .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                            })
                            .unwrap_or_default();
                        if call_id.is_empty() {
                            continue;
                        }
                        saw_responses_tool_delta = true;
                        if !call_order.contains(&call_id) {
                            call_order.push(call_id.clone());
                        }
                        let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                        let name = data_val
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| call_names.get(&call_id).cloned())
                            .unwrap_or_default();
                        if !name.is_empty() {
                            call_names.insert(call_id.clone(), name.clone());
                        }
                        let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        saw_tool_call = true;
                        let chunk = json!({
                            "id": id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": urp.model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": idx,
                                        "id": call_id,
                                        "type": "function",
                                        "function": { "name": name, "arguments": delta }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        });
                        let _ = tx.send(Event::default().data(chunk.to_string())).await;
                    }
                    "response.output_item.done" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if call_id.is_empty() {
                                continue;
                            }
                            saw_responses_tool_delta = true;
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .or_else(|| call_names.get(&call_id).cloned())
                                .unwrap_or_default();
                            if !call_order.contains(&call_id) {
                                call_order.push(call_id.clone());
                            }
                            if !name.is_empty() {
                                call_names.insert(call_id.clone(), name.clone());
                            }
                            if let Some(output_index) =
                                data_val.get("output_index").and_then(|v| v.as_u64())
                            {
                                call_ids_by_output_index.insert(output_index, call_id.clone());
                            }
                            let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                            let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                            if !args.is_empty() {
                                saw_tool_call = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {
                                            "tool_calls": [{
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": args }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                            if !saw_responses_text_delta {
                                let text = extract_responses_message_text(item);
                                if !text.is_empty() {
                                    out_text.push_str(&text);
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                            let (reasoning_text, reasoning_sig) =
                                extract_reasoning_text_and_signature(item);
                            if !reasoning_text.is_empty() {
                                saw_responses_reasoning_delta = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(&reasoning_text), "finish_reason": Value::Null }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                            if !reasoning_sig.is_empty() {
                                saw_responses_reasoning_delta = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&reasoning_sig), "finish_reason": Value::Null }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    "response.completed" => {
                        record_stream_terminal_event(&runtime_metrics, "response.completed", None)
                            .await;
                        let completed = data_val.get("response").unwrap_or(&data_val);
                        let Some(output) = completed.get("output").and_then(|v| v.as_array())
                        else {
                            continue;
                        };
                        for item in output {
                            match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                                "function_call" => {
                                    if saw_responses_tool_delta {
                                        continue;
                                    }
                                    let call_id = item
                                        .get("call_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if call_id.is_empty() {
                                        continue;
                                    }
                                    let name = item
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let args = item
                                        .get("arguments")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    if !call_order.contains(&call_id) {
                                        call_order.push(call_id.clone());
                                    }
                                    if !name.is_empty() {
                                        call_names.insert(call_id.clone(), name.clone());
                                    }
                                    let idx =
                                        call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                    saw_tool_call = true;
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{
                                            "index": 0,
                                            "delta": {
                                                "tool_calls": [{
                                                    "index": idx,
                                                    "id": call_id,
                                                    "type": "function",
                                                    "function": { "name": name, "arguments": args }
                                                }]
                                            },
                                            "finish_reason": Value::Null
                                        }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                                "message" => {
                                    if saw_responses_text_delta {
                                        continue;
                                    }
                                    let text = extract_responses_message_text(item);
                                    if !text.is_empty() {
                                        saw_responses_text_delta = true;
                                        out_text.push_str(&text);
                                        let chunk = json!({
                                            "id": id,
                                            "object": "chat.completion.chunk",
                                            "created": created,
                                            "model": urp.model,
                                            "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                                        });
                                        let _ =
                                            tx.send(Event::default().data(chunk.to_string())).await;
                                    }
                                }
                                "reasoning" => {
                                    if saw_responses_reasoning_delta {
                                        continue;
                                    }
                                    let (reasoning_text, reasoning_sig) =
                                        extract_reasoning_text_and_signature(item);
                                    if !reasoning_text.is_empty() {
                                        saw_responses_reasoning_delta = true;
                                        let chunk = json!({
                                            "id": id,
                                            "object": "chat.completion.chunk",
                                            "created": created,
                                            "model": urp.model,
                                            "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(&reasoning_text), "finish_reason": Value::Null }]
                                        });
                                        let _ =
                                            tx.send(Event::default().data(chunk.to_string())).await;
                                    }
                                    if !reasoning_sig.is_empty() {
                                        saw_responses_reasoning_delta = true;
                                        let chunk = json!({
                                            "id": id,
                                            "object": "chat.completion.chunk",
                                            "created": created,
                                            "model": urp.model,
                                            "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&reasoning_sig), "finish_reason": Value::Null }]
                                        });
                                        let _ =
                                            tx.send(Event::default().data(chunk.to_string())).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Messages => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_messages_object(&data_val),
                )
                .await;
                let t = data_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match t {
                    "content_block_delta" => {
                        let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                        let dt = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match dt {
                            "text_delta" => {
                                let txt = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                if !txt.is_empty() {
                                    out_text.push_str(txt);
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": { "content": txt }, "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "thinking_delta" => {
                                let txt =
                                    delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                                if !txt.is_empty() {
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(txt), "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "signature_delta" => {
                                let txt = delta
                                    .get("signature")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !txt.is_empty() {
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(txt), "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "input_json_delta" => {
                                let call_id = call_order.last().cloned().unwrap_or_default();
                                let partial = delta
                                    .get("partial_json")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !call_id.is_empty() && !partial.is_empty() {
                                    let idx =
                                        call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                    let name =
                                        call_names.get(&call_id).cloned().unwrap_or_default();
                                    saw_tool_call = true;
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{
                                            "index": 0,
                                            "delta": {
                                                "tool_calls": [{
                                                    "index": idx,
                                                    "id": call_id,
                                                    "type": "function",
                                                    "function": { "name": name, "arguments": partial }
                                                }]
                                            },
                                            "finish_reason": Value::Null
                                        }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_start" => {
                        let cb = data_val
                            .get("content_block")
                            .cloned()
                            .unwrap_or(Value::Null);
                        if cb.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                            let call_id = cb
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = cb
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !call_id.is_empty() {
                                if !call_order.contains(&call_id) {
                                    call_order.push(call_id.clone());
                                }
                                if !name.is_empty() {
                                    call_names.insert(call_id.clone(), name.clone());
                                }
                                let idx =
                                    call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                saw_tool_call = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {
                                            "tool_calls": [{
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": "" }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Gemini => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_gemini_object(&data_val),
                )
                .await;
                let Some(candidate) = data_val
                    .get("candidates")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                else {
                    continue;
                };
                let Some(parts) = candidate
                    .get("content")
                    .and_then(|v| v.get("parts"))
                    .and_then(|v| v.as_array())
                else {
                    continue;
                };
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(text), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            if let Some(sig) = part.get("thoughtSignature") {
                                let sig_text = sig
                                    .as_str()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| sig.to_string());
                                if !sig_text.is_empty() {
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&sig_text), "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                        } else if !text.is_empty() {
                            out_text.push_str(text);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                        let call_id = fc
                            .get("id")
                            .or_else(|| fc.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = fc
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args =
                            serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                                .unwrap_or_else(|_| "{}".to_string());
                        if !name.is_empty() {
                            if !call_order.contains(&call_id) {
                                call_order.push(call_id.clone());
                            }
                            saw_tool_call = true;
                            let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {
                                        "tool_calls": [{
                                            "index": idx,
                                            "id": call_id,
                                            "type": "function",
                                            "function": { "name": name, "arguments": args }
                                        }]
                                    },
                                    "finish_reason": Value::Null
                                }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                }
            }
            ProviderType::Group => {}
        }
    }

    let needs_synthetic_terminal =
        provider_type != ProviderType::ChatCompletion || !saw_upstream_terminal_finish;
    if needs_synthetic_terminal {
        let finish_reason = if saw_tool_call { "tool_calls" } else { "stop" };
        let mut done = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": urp.model,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
        });
        if let Some(usage) = latest_stream_usage_snapshot(&runtime_metrics).await {
            done["usage"] = usage_to_chat_usage_json(&usage);
        }
        record_stream_synthetic_terminal_emitted(&runtime_metrics).await;
        let _ = tx.send(Event::default().data(done.to_string())).await;
        record_stream_terminal_event(&runtime_metrics, "chat.completion.chunk", Some(finish_reason))
            .await;
    }
    let _ = tx.send(Event::default().data("[DONE]")).await;
    Ok(())
}

pub(crate) async fn stream_any_sse_as_messages(
    urp: &HandlerUrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    // If the upstream is already Anthropic Messages streaming, forward it as-is.
    if provider_type == ProviderType::Messages {
        let mut stream = upstream_resp.bytes_stream().eventsource();
        while let Some(ev) = stream.next().await {
            let ev = ev.map_err(|err| {
                AppError::new(
                    StatusCode::BAD_GATEWAY,
                    "upstream_stream_decode_failed",
                    err.to_string(),
                )
            })?;
            if tx.is_closed() {
                break;
            }
            mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
            if ev.data.trim() == "[DONE]" {
                record_stream_done_sentinel(&runtime_metrics).await;
                break;
            }
            let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
            record_stream_usage_if_present(
                &runtime_metrics,
                parse_usage_from_messages_object(&data_val),
            )
            .await;
            if data_val.get("type").and_then(|v| v.as_str()) == Some("message_stop") {
                record_stream_terminal_event(&runtime_metrics, "message_stop", None).await;
            }
            let _ = tx.send(Event::default().data(ev.data)).await;
        }
        return Ok(());
    }

    let start = json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": urp.model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0
            }
        }
    });
    let _ = tx.send(Event::default().data(start.to_string())).await;

    let mut next_index: u32 = 0;
    let mut text_index: Option<u32> = None;
    let mut thinking_index: Option<u32> = None;
    let mut saw_responses_text_delta = false;
    let mut saw_responses_tool_delta = false;
    let mut saw_responses_reasoning_delta = false;
    let mut tool_indices: HashMap<String, u32> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut started: Vec<u32> = Vec::new();

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }

        match provider_type {
            ProviderType::ChatCompletion => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_chat_object(&data_val),
                )
                .await;
                let delta = data_val
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("delta"))
                    .cloned()
                    .unwrap_or(Value::Null);

                let (reasoning_text_deltas, reasoning_sig_deltas) =
                    extract_chat_reasoning_deltas(&delta);
                for t in reasoning_text_deltas {
                    let idx = ensure_anthropic_thinking_block(
                        &tx,
                        &mut thinking_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "thinking_delta", "thinking": t }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }
                for s in reasoning_sig_deltas {
                    let idx = ensure_anthropic_thinking_block(
                        &tx,
                        &mut thinking_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "signature_delta", "signature": s }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }

                if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
                    if !t.is_empty() {
                        let idx = ensure_anthropic_text_block(
                            &tx,
                            &mut text_index,
                            &mut next_index,
                            &mut started,
                        )
                        .await?;
                        let d = json!({
                            "type": "content_block_delta",
                            "index": idx,
                            "delta": { "type": "text_delta", "text": t }
                        });
                        let _ = tx.send(Event::default().data(d.to_string())).await;
                    }
                }

                if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tool_calls {
                        let call_id = tc
                            .get("id")
                            .or_else(|| tc.get("call_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if call_id.is_empty() {
                            continue;
                        }
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let idx = ensure_anthropic_tool_block(
                            &tx,
                            &mut tool_indices,
                            &mut tool_names,
                            &mut next_index,
                            &mut started,
                            call_id,
                            name,
                        )
                        .await?;
                        if !args.is_empty() {
                            let d = json!({
                                "type": "content_block_delta",
                                "index": idx,
                                "delta": { "type": "input_json_delta", "partial_json": args }
                            });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                }
            }
            ProviderType::Responses | ProviderType::Grok => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_responses_object(&data_val),
                )
                .await;
                match ev.event.as_str() {
                    "response.output_text.delta" => {
                        let t = data_val
                            .get("text")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_text_delta = true;
                            let idx = ensure_anthropic_text_block(
                                &tx,
                                &mut text_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "text_delta", "text": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                    "response.reasoning_text.delta" => {
                        let t = data_val
                            .get("delta")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_reasoning_delta = true;
                            let idx = ensure_anthropic_thinking_block(
                                &tx,
                                &mut thinking_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "thinking_delta", "thinking": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                    "response.reasoning_signature.delta" => {
                        let t = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_reasoning_delta = true;
                            let idx = ensure_anthropic_thinking_block(
                                &tx,
                                &mut thinking_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "signature_delta", "signature": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                    "response.output_item.added" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id =
                                item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if !call_id.is_empty() {
                                saw_responses_tool_delta = true;
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index
                                        .insert(output_index, call_id.to_string());
                                }
                                let _ = ensure_anthropic_tool_block(
                                    &tx,
                                    &mut tool_indices,
                                    &mut tool_names,
                                    &mut next_index,
                                    &mut started,
                                    call_id,
                                    name,
                                )
                                .await?;
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let call_id = data_val
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                data_val
                                    .get("output_index")
                                    .and_then(|v| v.as_u64())
                                    .and_then(|idx| {
                                        call_ids_by_output_index.get(&idx).map(|s| s.as_str())
                                    })
                            })
                            .unwrap_or("");
                        let name = data_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !call_id.is_empty() {
                            saw_responses_tool_delta = true;
                            let idx = ensure_anthropic_tool_block(
                                &tx,
                                &mut tool_indices,
                                &mut tool_names,
                                &mut next_index,
                                &mut started,
                                call_id,
                                name,
                            )
                            .await?;
                            if !delta.is_empty() {
                                let d = json!({
                                    "type": "content_block_delta",
                                    "index": idx,
                                    "delta": { "type": "input_json_delta", "partial_json": delta }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                        }
                    }
                    "response.output_item.done" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id =
                                item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                            if !call_id.is_empty() {
                                saw_responses_tool_delta = true;
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index
                                        .insert(output_index, call_id.to_string());
                                }
                                let idx = ensure_anthropic_tool_block(
                                    &tx,
                                    &mut tool_indices,
                                    &mut tool_names,
                                    &mut next_index,
                                    &mut started,
                                    call_id,
                                    name,
                                )
                                .await?;
                                if !args.is_empty() {
                                    let d = json!({
                                        "type": "content_block_delta",
                                        "index": idx,
                                        "delta": { "type": "input_json_delta", "partial_json": args }
                                    });
                                    let _ = tx.send(Event::default().data(d.to_string())).await;
                                }
                            }
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                            if !saw_responses_text_delta {
                                let text = extract_responses_message_text(item);
                                if !text.is_empty() {
                                    let idx = ensure_anthropic_text_block(
                                        &tx,
                                        &mut text_index,
                                        &mut next_index,
                                        &mut started,
                                    )
                                    .await?;
                                    let d = json!({
                                        "type": "content_block_delta",
                                        "index": idx,
                                        "delta": { "type": "text_delta", "text": text }
                                    });
                                    let _ = tx.send(Event::default().data(d.to_string())).await;
                                }
                            }
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                            let (reasoning_text, reasoning_sig) =
                                extract_reasoning_text_and_signature(item);
                            if !reasoning_text.is_empty() {
                                saw_responses_reasoning_delta = true;
                                let idx = ensure_anthropic_thinking_block(
                                    &tx,
                                    &mut thinking_index,
                                    &mut next_index,
                                    &mut started,
                                )
                                .await?;
                                let d = json!({
                                    "type": "content_block_delta",
                                    "index": idx,
                                    "delta": { "type": "thinking_delta", "thinking": reasoning_text }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                            if !reasoning_sig.is_empty() {
                                saw_responses_reasoning_delta = true;
                                let idx = ensure_anthropic_thinking_block(
                                    &tx,
                                    &mut thinking_index,
                                    &mut next_index,
                                    &mut started,
                                )
                                .await?;
                                let d = json!({
                                    "type": "content_block_delta",
                                    "index": idx,
                                    "delta": { "type": "signature_delta", "signature": reasoning_sig }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                        }
                    }
                    "response.completed" => {
                        let completed = data_val.get("response").unwrap_or(&data_val);
                        let Some(output) = completed.get("output").and_then(|v| v.as_array())
                        else {
                            continue;
                        };
                        for item in output {
                            match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                                "function_call" => {
                                    if saw_responses_tool_delta {
                                        continue;
                                    }
                                    let call_id =
                                        item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                                    if call_id.is_empty() {
                                        continue;
                                    }
                                    let name =
                                        item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    let args = item
                                        .get("arguments")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let idx = ensure_anthropic_tool_block(
                                        &tx,
                                        &mut tool_indices,
                                        &mut tool_names,
                                        &mut next_index,
                                        &mut started,
                                        call_id,
                                        name,
                                    )
                                    .await?;
                                    if !args.is_empty() {
                                        let d = json!({
                                            "type": "content_block_delta",
                                            "index": idx,
                                            "delta": { "type": "input_json_delta", "partial_json": args }
                                        });
                                        let _ = tx.send(Event::default().data(d.to_string())).await;
                                    }
                                }
                                "message" => {
                                    if saw_responses_text_delta {
                                        continue;
                                    }
                                    let text = extract_responses_message_text(item);
                                    if !text.is_empty() {
                                        saw_responses_text_delta = true;
                                        let idx = ensure_anthropic_text_block(
                                            &tx,
                                            &mut text_index,
                                            &mut next_index,
                                            &mut started,
                                        )
                                        .await?;
                                        let d = json!({
                                            "type": "content_block_delta",
                                            "index": idx,
                                            "delta": { "type": "text_delta", "text": text }
                                        });
                                        let _ = tx.send(Event::default().data(d.to_string())).await;
                                    }
                                }
                                "reasoning" => {
                                    if saw_responses_reasoning_delta {
                                        continue;
                                    }
                                    let (reasoning_text, reasoning_sig) =
                                        extract_reasoning_text_and_signature(item);
                                    if !reasoning_text.is_empty() {
                                        saw_responses_reasoning_delta = true;
                                        let idx = ensure_anthropic_thinking_block(
                                            &tx,
                                            &mut thinking_index,
                                            &mut next_index,
                                            &mut started,
                                        )
                                        .await?;
                                        let d = json!({
                                            "type": "content_block_delta",
                                            "index": idx,
                                            "delta": { "type": "thinking_delta", "thinking": reasoning_text }
                                        });
                                        let _ = tx.send(Event::default().data(d.to_string())).await;
                                    }
                                    if !reasoning_sig.is_empty() {
                                        saw_responses_reasoning_delta = true;
                                        let idx = ensure_anthropic_thinking_block(
                                            &tx,
                                            &mut thinking_index,
                                            &mut next_index,
                                            &mut started,
                                        )
                                        .await?;
                                        let d = json!({
                                            "type": "content_block_delta",
                                            "index": idx,
                                            "delta": { "type": "signature_delta", "signature": reasoning_sig }
                                        });
                                        let _ = tx.send(Event::default().data(d.to_string())).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Gemini | ProviderType::Group | ProviderType::Messages => {}
        }
    }

    // Close all started blocks in the order they were created.
    for idx in started.iter() {
        let stop = json!({ "type": "content_block_stop", "index": idx });
        let _ = tx.send(Event::default().data(stop.to_string())).await;
    }
    let message_usage = latest_stream_usage_snapshot(&runtime_metrics)
        .await
        .map(|usage| usage_to_messages_usage_json(&usage))
        .unwrap_or_else(|| json!({ "input_tokens": 0, "output_tokens": 0 }));
    let message_delta = json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": if tool_indices.is_empty() { "end_turn" } else { "tool_use" },
            "stop_sequence": Value::Null
        },
        "usage": message_usage
    });
    let _ = tx
        .send(Event::default().data(message_delta.to_string()))
        .await;
    let stop = json!({ "type": "message_stop" });
    let _ = tx.send(Event::default().data(stop.to_string())).await;
    record_stream_terminal_event(
        &runtime_metrics,
        "message_stop",
        Some(if tool_indices.is_empty() {
            "end_turn"
        } else {
            "tool_use"
        }),
    )
    .await;
    Ok(())
}
