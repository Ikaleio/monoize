use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::handlers::usage::usage_to_chat_usage_json;
use crate::urp::stream_helpers::*;
use crate::urp::{self, Item, Part, Role};
use axum::response::sse::Event;
use serde_json::{json, Value};
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
