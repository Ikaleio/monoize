use crate::error::AppResult;
use crate::urp::stream_helpers::*;
use crate::urp::{self, Item, Part, Role};
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
    send_plain_sse_data(&tx, start.to_string()).await?;

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
                        Part::Text { content, .. } | Part::Refusal { content, .. } => {
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
    send_plain_sse_data(&tx, message_delta.to_string()).await?;
    send_plain_sse_data(&tx, json!({ "type": "message_stop" }).to_string()).await?;
    Ok(())
}
