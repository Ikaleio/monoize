use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::handlers::usage::usage_to_chat_usage_json;
use crate::urp::stream_helpers::*;
use crate::urp::{self, FinishReason, Item, Part, PartDelta, PartHeader, Role, UrpStreamEvent};
use axum::response::sse::Event;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

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
                        Part::ToolCall {
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
                        Part::Text { content, .. } | Part::Refusal { content, .. } => {
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
            }
            Item::ToolResult { .. } => continue,
            Item::Message { .. } => continue,
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

fn finish_reason_to_chat(reason: urp::FinishReason) -> &'static str {
    match reason {
        urp::FinishReason::Stop => "stop",
        urp::FinishReason::Length => "length",
        urp::FinishReason::ToolCalls => "tool_calls",
        urp::FinishReason::ContentFilter => "content_filter",
        urp::FinishReason::Other => "stop",
    }
}

pub(crate) async fn encode_urp_stream_as_chat(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    logical_model: &str,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let mut chat_id = String::new();
    let mut created = 0i64;
    let mut tool_idx = 0usize;
    let mut saw_tool = false;
    let mut tool_info: HashMap<u32, (String, String, usize, bool)> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            UrpStreamEvent::ResponseStart { .. } => {
                chat_id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
                created = now_ts();
                let chunk = json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": logical_model,
                    "choices": [{
                        "index": 0,
                        "delta": { "role": "assistant" },
                        "finish_reason": Value::Null
                    }]
                });
                send_plain_sse_data(&tx, chunk.to_string()).await?;
            }
            UrpStreamEvent::PartStart {
                part_index,
                header: PartHeader::ToolCall { call_id, name },
                ..
            } => {
                tool_info.insert(part_index, (call_id, name, tool_idx, false));
                tool_idx += 1;
            }
            UrpStreamEvent::PartStart { .. }
            | UrpStreamEvent::ItemStart { .. }
            | UrpStreamEvent::PartDone { .. }
            | UrpStreamEvent::ItemDone { .. } => {}
            UrpStreamEvent::Delta {
                delta: PartDelta::Text { content },
                ..
            }
            | UrpStreamEvent::Delta {
                delta: PartDelta::Refusal { content },
                ..
            } => {
                send_chat_chunk_string(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    json!({ "content": "" }),
                    &content,
                    chat_delta_path_content,
                    sse_max_frame_length,
                )
                .await?;
            }
            UrpStreamEvent::Delta {
                delta: PartDelta::Reasoning { content },
                ..
            } => {
                send_chat_chunk_string(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    chat_reasoning_delta_from_text(""),
                    &content,
                    chat_delta_path_reasoning_text,
                    sse_max_frame_length,
                )
                .await?;
            }
            UrpStreamEvent::Delta {
                part_index,
                delta: PartDelta::ToolCallArguments { arguments },
                ..
            } => {
                let Some((call_id, name, idx, header_sent)) = tool_info.get_mut(&part_index) else {
                    continue;
                };

                saw_tool = true;

                if !*header_sent {
                    let chunk = json!({
                        "id": chat_id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": logical_model,
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": *idx,
                                    "id": call_id,
                                    "type": "function",
                                    "function": { "name": name, "arguments": "" }
                                }]
                            },
                            "finish_reason": Value::Null
                        }]
                    });
                    send_plain_sse_data(&tx, chunk.to_string()).await?;
                    *header_sent = true;
                }

                let delta = json!({
                    "tool_calls": [{
                        "index": *idx,
                        "function": { "arguments": "" }
                    }]
                });
                send_chat_chunk_string(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    delta,
                    &arguments,
                    chat_delta_path_tool_arguments,
                    sse_max_frame_length,
                )
                .await?;
            }
            UrpStreamEvent::Delta { .. } => {}
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                ..
            } => {
                let finish_reason = if saw_tool {
                    "tool_calls"
                } else {
                    finish_reason_to_chat(finish_reason.unwrap_or(FinishReason::Stop))
                };
                let mut done = json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": logical_model,
                    "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
                });
                if let Some(usage) = usage.as_ref() {
                    done["usage"] = usage_to_chat_usage_json(usage);
                }
                send_plain_sse_data(&tx, done.to_string()).await?;
                send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
            }
            UrpStreamEvent::Error { code, message, .. } => {
                let error = json!({
                    "error": {
                        "message": message,
                        "type": "server_error",
                        "code": code
                    }
                });
                send_plain_sse_data(&tx, error.to_string()).await?;
            }
        }
    }

    Ok(())
}
