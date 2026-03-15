use crate::error::AppResult;
use crate::urp::stream_helpers::*;
use crate::urp::{self, FinishReason, Item, Part, PartHeader, Role, UrpStreamEvent, Usage};
use axum::response::sse::Event;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

pub(crate) async fn emit_synthetic_messages_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    let mut saw_tool_use = false;
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
    send_named_messages_event(&tx, start).await?;

    let mut index = 0u32;
    for item in &resp.outputs {
        match item {
            Item::Message {
                role: Role::Assistant,
                parts,
                ..
            } => {
                for part in parts {
                    match part {
                        Part::Reasoning {
                            content,
                            encrypted,
                            ..
                        } => {
                            if let Some(content) =
                                content.as_deref().filter(|content| !content.is_empty())
                            {
                                let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "thinking", "thinking": "", "signature": "" } });
                                send_named_messages_event(&tx, s).await?;
                                send_messages_delta_string(
                                    &tx,
                                    json!({ "type": "content_block_delta", "index": index, "delta": { "type": "thinking_delta", "thinking": "" } }),
                                    messages_delta_path_thinking,
                                    content,
                                    sse_max_frame_length,
                                )
                                .await?;
                                let e = json!({ "type": "content_block_stop", "index": index });
                                send_named_messages_event(&tx, e).await?;
                                index += 1;
                            }
                            if let Some(data) = encrypted {
                                let sig = data
                                    .as_str()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| data.to_string());
                                if !sig.is_empty() {
                                    let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "thinking", "thinking": "", "signature": "" } });
                                    send_named_messages_event(&tx, s).await?;
                                    send_messages_delta_string(
                                        &tx,
                                        json!({ "type": "content_block_delta", "index": index, "delta": { "type": "signature_delta", "signature": "" } }),
                                        messages_delta_path_signature,
                                        &sig,
                                        sse_max_frame_length,
                                    )
                                    .await?;
                                    let e = json!({ "type": "content_block_stop", "index": index });
                                    send_named_messages_event(&tx, e).await?;
                                    index += 1;
                                }
                            }
                        }
                        Part::ToolCall {
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
                            send_named_messages_event(&tx, start_tool).await?;
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
                            send_named_messages_event(&tx, stop_tool).await?;
                            index += 1;
                        }
                        Part::Text { content, .. } | Part::Refusal { content, .. } => {
                            if content.is_empty() {
                                continue;
                            }
                            let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "text", "text": "" } });
                            send_named_messages_event(&tx, s).await?;
                            send_messages_delta_string(
                                &tx,
                                json!({ "type": "content_block_delta", "index": index, "delta": { "type": "text_delta", "text": "" } }),
                                messages_delta_path_text,
                                content,
                                sse_max_frame_length,
                            )
                            .await?;
                            let e = json!({ "type": "content_block_stop", "index": index });
                            send_named_messages_event(&tx, e).await?;
                            index += 1;
                        }
                        _ => {}
                    }
                }
            }
            Item::ToolResult { .. } => continue,
            Item::Message { .. } => continue,
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
    send_named_messages_event(&tx, message_delta).await?;
    send_named_messages_event(&tx, json!({ "type": "message_stop" })).await?;
    Ok(())
}

pub(crate) async fn encode_urp_stream_as_messages(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    logical_model: &str,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let mut next_content_block_index = 0u32;
    let mut saw_tool_use = false;
    let mut saw_stream_parts = false;
    let mut response_usage: Option<Usage> = None;

    while let Some(event) = rx.recv().await {
        match event {
            UrpStreamEvent::ResponseStart { id, .. } => {
                let start = json!({
                    "type": "message_start",
                    "message": {
                        "id": id,
                        "type": "message",
                        "role": "assistant",
                        "model": logical_model,
                        "content": [],
                        "stop_reason": Value::Null,
                        "stop_sequence": Value::Null,
                        "usage": {
                            "input_tokens": 0,
                            "output_tokens": 0
                        }
                    }
                });
                send_named_messages_event(&tx, start).await?;
            }
            UrpStreamEvent::ItemStart { .. } | UrpStreamEvent::ItemDone { .. } => {}
            UrpStreamEvent::PartStart {
                header, ..
            } => {
                saw_stream_parts = true;
                if matches!(header, PartHeader::ToolCall { .. }) {
                    saw_tool_use = true;
                }
            }
            UrpStreamEvent::Delta {
                delta: _,
                usage,
                ..
            } => {
                if let Some(usage) = usage {
                    response_usage = Some(usage);
                }
            }
            UrpStreamEvent::PartDone { part, .. } => {
                saw_stream_parts = true;
                let block_index = next_content_block_index;
                next_content_block_index += 1;
                let content_block = content_block_from_part(&part, &mut saw_tool_use)?;
                let start = json!({
                    "type": "content_block_start",
                    "index": block_index,
                    "content_block": content_block
                });
                send_named_messages_event(&tx, start).await?;

                emit_messages_part_done_payload(&tx, block_index, &part, sse_max_frame_length).await?;

                let stop = json!({ "type": "content_block_stop", "index": block_index });
                send_named_messages_event(&tx, stop).await?;
            }
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                outputs,
                ..
            } => {
                if !saw_stream_parts {
                    emit_messages_outputs_from_response_done(
                        &tx,
                        &mut next_content_block_index,
                        &mut saw_tool_use,
                        &outputs,
                        sse_max_frame_length,
                    )
                    .await?;
                }

                let usage = usage.or_else(|| response_usage.clone()).unwrap_or(Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    input_details: None,
                    output_details: None,
                    extra_body: HashMap::new(),
                });
                let stop_reason = match finish_reason {
                    Some(FinishReason::Stop) => "end_turn",
                    Some(FinishReason::ToolCalls) => "tool_use",
                    Some(FinishReason::Length) => "max_tokens",
                    Some(FinishReason::Other | FinishReason::ContentFilter) => "end_turn",
                    None if saw_tool_use => "tool_use",
                    None => "end_turn",
                };
                let message_delta = json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": stop_reason,
                        "stop_sequence": Value::Null
                    },
                    "usage": {
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens
                    }
                });
                send_named_messages_event(&tx, message_delta).await?;
                send_named_messages_event(&tx, json!({ "type": "message_stop" })).await?;
            }
            UrpStreamEvent::Error { code, message, .. } => {
                let error = json!({
                    "type": "error",
                    "error": {
                        "type": code.unwrap_or_else(|| "server_error".to_string()),
                        "message": message
                    }
                });
                send_named_messages_event(&tx, error).await?;
            }
        }
    }

    Ok(())
}

fn content_block_from_part(
    part: &Part,
    saw_tool_use: &mut bool,
) -> AppResult<Value> {
    let content_block = match part {
        Part::Text { .. } | Part::Refusal { .. } => json!({ "type": "text", "text": "" }),
        Part::Reasoning { .. } => json!({ "type": "thinking", "thinking": "", "signature": "" }),
        Part::ToolCall { call_id, name, .. } => {
            *saw_tool_use = true;
            json!({ "type": "tool_use", "id": call_id, "name": name, "input": {} })
        }
        _ => return Err(crate::error::AppError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "stream_encode_failed",
            "unsupported URP part for Anthropic messages stream",
        )),
    };
    Ok(content_block)
}

async fn emit_messages_part_done_payload(
    tx: &mpsc::Sender<Event>,
    block_index: u32,
    part: &Part,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    match part {
        Part::Text { content, .. } | Part::Refusal { content, .. } => {
            if !content.is_empty() {
                send_messages_delta_string(
                    tx,
                    json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": { "type": "text_delta", "text": "" }
                    }),
                    messages_delta_path_text,
                    content,
                    sse_max_frame_length,
                )
                .await?;
            }
        }
        Part::Reasoning {
            content,
            encrypted,
            extra_body,
            ..
        } => {
            if let Some(content) = content.as_deref().filter(|content| !content.is_empty()) {
                send_messages_delta_string(
                    tx,
                    json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": { "type": "thinking_delta", "thinking": "" }
                    }),
                    messages_delta_path_thinking,
                    content,
                    sse_max_frame_length,
                )
                .await?;
            }

            let signature = encrypted
                .as_ref()
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    extra_body
                        .get("signature")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            if let Some(signature) = signature.filter(|signature| !signature.is_empty()) {
                send_messages_delta_string(
                    tx,
                    json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": { "type": "signature_delta", "signature": "" }
                    }),
                    messages_delta_path_signature,
                    &signature,
                    sse_max_frame_length,
                )
                .await?;
            }
        }
        Part::ToolCall { arguments, .. } => {
            if !arguments.is_empty() {
                send_messages_delta_string(
                    tx,
                    json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": { "type": "input_json_delta", "partial_json": "" }
                    }),
                    messages_delta_path_partial_json,
                    arguments,
                    sse_max_frame_length,
                )
                .await?;
            }
        }
        _ => {}
    }

    Ok(())
}

async fn emit_messages_outputs_from_response_done(
    tx: &mpsc::Sender<Event>,
    next_content_block_index: &mut u32,
    saw_tool_use: &mut bool,
    outputs: &[Item],
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    for item in outputs {
        let Item::Message { parts, .. } = item else {
            continue;
        };

        for part in parts {
            let block_index = *next_content_block_index;
            *next_content_block_index += 1;

            let content_block = match part {
                Part::Text { .. } | Part::Refusal { .. } => json!({ "type": "text", "text": "" }),
                Part::Reasoning { .. } => json!({ "type": "thinking", "thinking": "", "signature": "" }),
                Part::ToolCall { call_id, name, .. } => {
                    *saw_tool_use = true;
                    json!({ "type": "tool_use", "id": call_id, "name": name, "input": {} })
                }
                _ => continue,
            };

            let start = json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": content_block
            });
            send_named_messages_event(tx, start).await?;
            emit_messages_part_done_payload(tx, block_index, part, sse_max_frame_length).await?;
            let stop = json!({ "type": "content_block_stop", "index": block_index });
            send_named_messages_event(tx, stop).await?;
        }
    }

    Ok(())
}

async fn send_named_messages_event(tx: &mpsc::Sender<Event>, payload: Value) -> AppResult<()> {
    let event_name = payload
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| crate::error::AppError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "stream_encode_failed",
            "messages stream payload missing type field",
        ))?;
    send_named_sse_json(tx, &event_name, payload).await
}
