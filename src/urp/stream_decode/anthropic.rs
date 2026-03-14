use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_messages_object,
    record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{
    FinishReason, Item, ItemHeader, Part, PartDelta, PartHeader, Role, UrpStreamEvent,
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

enum ActivePart {
    Text,
    Reasoning,
    ToolCall(String),
}

pub(crate) async fn stream_messages_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut response_model = urp.model.clone();
    let mut response_extra = HashMap::new();
    let mut output_text = String::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut current_tool_call_id: Option<String> = None;
    let mut active_parts: HashMap<u32, ActivePart> = HashMap::new();
    let mut finished_parts: HashMap<u32, Part> = HashMap::new();
    let mut part_order: Vec<u32> = Vec::new();
    let mut item_started = false;

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

        match data_val.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "message_start" => {
                let message = data_val.get("message").cloned().unwrap_or(Value::Null);
                if let Some(id) = message.get("id").and_then(|v| v.as_str()) {
                    response_id = id.to_string();
                }
                if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
                    response_model = model.to_string();
                }
                response_extra = object_without_keys(
                    &message,
                    &["id", "type", "role", "model", "content", "stop_reason", "stop_sequence", "usage"],
                );
                let _ = tx
                    .send(UrpStreamEvent::ResponseStart {
                        id: response_id.clone(),
                        model: response_model.clone(),
                        extra_body: response_extra.clone(),
                    })
                    .await;
                let _ = tx
                    .send(UrpStreamEvent::ItemStart {
                        item_index: 0,
                        header: ItemHeader::Message {
                            role: Role::Assistant,
                        },
                        extra_body: HashMap::new(),
                    })
                    .await;
                item_started = true;
            }
            "content_block_start" => {
                let part_index = data_val
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let cb = data_val.get("content_block").cloned().unwrap_or(Value::Null);
                let cb_type = cb.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let extra_body = object_without_keys(&cb, &["type", "id", "name", "text", "thinking"]);
                let header = match cb_type {
                    "text" => {
                        active_parts.insert(part_index, ActivePart::Text);
                        PartHeader::Text
                    }
                    "thinking" => {
                        active_parts.insert(part_index, ActivePart::Reasoning);
                        PartHeader::Reasoning
                    }
                    "tool_use" => {
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
                            calls.insert(call_id.clone(), (name.clone(), String::new()));
                        }
                        active_parts.insert(part_index, ActivePart::ToolCall(call_id.clone()));
                        PartHeader::ToolCall { call_id, name }
                    }
                    _ => continue,
                };
                if !part_order.contains(&part_index) {
                    part_order.push(part_index);
                }
                let _ = tx
                    .send(UrpStreamEvent::PartStart {
                        part_index,
                        item_index: 0,
                        header,
                        extra_body,
                    })
                    .await;
            }
            "content_block_delta" => {
                let part_index = data_val
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(|v| v.as_str())
                            && !text.is_empty()
                        {
                            output_text.push_str(text);
                            let _ = tx
                                .send(UrpStreamEvent::Delta {
                                    part_index,
                                    delta: PartDelta::Text {
                                        content: text.to_string(),
                                    },
                                    usage: None,
                                    extra_body: HashMap::new(),
                                })
                                .await;
                        }
                    }
                    "thinking_delta" => {
                        if let Some(text) = delta.get("thinking").and_then(|v| v.as_str())
                            && !text.is_empty()
                        {
                            reasoning_text.push_str(text);
                            let _ = tx
                                .send(UrpStreamEvent::Delta {
                                    part_index,
                                    delta: PartDelta::Reasoning {
                                        content: text.to_string(),
                                    },
                                    usage: None,
                                    extra_body: HashMap::new(),
                                })
                                .await;
                        }
                    }
                    "signature_delta" => {
                        if let Some(sig) = delta.get("signature").and_then(|v| v.as_str())
                            && !sig.is_empty()
                        {
                            reasoning_sig.push_str(sig);
                        }
                    }
                    "input_json_delta" => {
                        if let Some(arguments) = delta.get("partial_json").and_then(|v| v.as_str())
                            && !arguments.is_empty()
                        {
                            let tool_call_id = match active_parts.get(&part_index) {
                                Some(ActivePart::ToolCall(call_id)) if !call_id.is_empty() => {
                                    Some(call_id.clone())
                                }
                                _ => current_tool_call_id.clone(),
                            };
                            if let Some(call_id) = tool_call_id {
                                if let Some(entry) = calls.get_mut(&call_id) {
                                    entry.1.push_str(arguments);
                                }
                                let _ = tx
                                    .send(UrpStreamEvent::Delta {
                                        part_index,
                                        delta: PartDelta::ToolCallArguments {
                                            arguments: arguments.to_string(),
                                        },
                                        usage: None,
                                        extra_body: HashMap::new(),
                                    })
                                    .await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let part_index = data_val
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let part = match active_parts.get(&part_index) {
                    Some(ActivePart::Text) => Part::Text {
                        content: output_text.clone(),
                        extra_body: HashMap::new(),
                    },
                    Some(ActivePart::Reasoning) => Part::Reasoning {
                        content: (!reasoning_text.is_empty()).then(|| reasoning_text.clone()),
                        encrypted: (!reasoning_sig.is_empty())
                            .then(|| Value::String(reasoning_sig.clone())),
                        summary: None,
                        source: None,
                        extra_body: HashMap::new(),
                    },
                    Some(ActivePart::ToolCall(call_id)) => {
                        let (name, arguments) = calls
                            .get(call_id)
                            .cloned()
                            .unwrap_or_else(|| (String::new(), String::new()));
                        Part::ToolCall {
                            call_id: call_id.clone(),
                            name,
                            arguments,
                            extra_body: HashMap::new(),
                        }
                    }
                    None => continue,
                };
                current_tool_call_id = None;
                finished_parts.insert(part_index, part.clone());
                let _ = tx
                    .send(UrpStreamEvent::PartDone {
                        part_index,
                        part,
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
            "message_delta" => {
                let finish_reason = data_val
                    .get("delta")
                    .and_then(|v| v.get("stop_reason"))
                    .and_then(|v| v.as_str())
                    .and_then(map_finish_reason);
                let usage = latest_stream_usage_snapshot(&runtime_metrics).await;
                let parts = part_order
                    .iter()
                    .filter_map(|index| finished_parts.get(index).cloned())
                    .collect::<Vec<_>>();
                let item = Item::Message {
                    role: Role::Assistant,
                    parts,
                    extra_body: HashMap::new(),
                };
                if item_started {
                    let _ = tx
                        .send(UrpStreamEvent::ItemDone {
                            item_index: 0,
                            item: item.clone(),
                            usage: usage.clone(),
                            extra_body: HashMap::new(),
                        })
                        .await;
                }
                let _ = tx
                    .send(UrpStreamEvent::ResponseDone {
                        finish_reason,
                        usage,
                        outputs: vec![item],
                        extra_body: response_extra.clone(),
                    })
                    .await;
                record_stream_terminal_event(
                    &runtime_metrics,
                    "response_done",
                    finish_reason.as_ref().map(finish_reason_name),
                )
                .await;
            }
            "message_stop" => {
                record_stream_terminal_event(&runtime_metrics, "message_stop", None).await;
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

fn object_without_keys(value: &Value, ignored: &[&str]) -> HashMap<String, Value> {
    let Some(obj) = value.as_object() else {
        return HashMap::new();
    };
    obj.iter()
        .filter(|(key, _)| !ignored.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn map_finish_reason(reason: &str) -> Option<FinishReason> {
    match reason {
        "end_turn" => Some(FinishReason::Stop),
        "max_tokens" => Some(FinishReason::Length),
        "tool_use" => Some(FinishReason::ToolCalls),
        "refusal" => Some(FinishReason::ContentFilter),
        "stop_sequence" => Some(FinishReason::Stop),
        "" => None,
        _ => Some(FinishReason::Other),
    }
}

fn finish_reason_name(reason: &FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other => "other",
    }
}
