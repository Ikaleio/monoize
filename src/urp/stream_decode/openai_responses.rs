use crate::error::{AppError, AppResult};
use crate::handlers::routing::now_ts;
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_responses_object,
    record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::stream_helpers::{
    extract_reasoning_text_and_signature, extract_responses_message_phase,
    extract_responses_message_text,
};
use crate::urp::{
    FinishReason, Item, ItemHeader, Part, PartDelta, PartHeader, Role, UrpStreamEvent,
    greedy::{Action, GreedyMerger},
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_responses_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut message_phases_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_text_delta = false;
    let mut response_done_sent = false;

    let _ = tx
        .send(UrpStreamEvent::ResponseStart {
            id: response_id.clone(),
            model: urp.model.clone(),
            extra_body: HashMap::from([
                ("object".to_string(), json!("response")),
                ("created".to_string(), json!(created)),
                ("status".to_string(), json!("in_progress")),
                ("output".to_string(), json!([])),
            ]),
        })
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
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::String(ev.data));
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_responses_object(&data_val),
        )
        .await;

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
                && !saw_text_delta
            {
                output_text.push_str(&extract_responses_message_text(item));
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    if let Some(phase) = extract_responses_message_phase(item) {
                        message_phases_by_output_index.insert(idx, phase);
                    }
                }
            }
        }

        if let Some(stream_event) =
            map_responses_event_to_urp_event(&ev.event, data_val, &message_phases_by_output_index)
        {
            response_done_sent |= matches!(stream_event, UrpStreamEvent::ResponseDone { .. });
            let _ = tx.send(stream_event).await;
        }
    }

    if !response_done_sent {
        let message_phase = message_phases_by_output_index
            .iter()
            .min_by_key(|(idx, _)| *idx)
            .map(|(_, phase)| phase.as_str());
        let output_items = build_accumulated_output_items(
            &reasoning_text,
            &reasoning_sig,
            &output_text,
            message_phase,
            &call_order,
            &calls,
        );
        let final_usage = latest_stream_usage_snapshot(&runtime_metrics).await;

        if !saw_text_delta {
            if let Some((final_item_index, output_item)) = output_items
                .iter()
                .enumerate()
                .find(|(_, item)| output_item_text(item).is_some())
                .map(|(idx, item)| (idx as u32, item))
            {
                let synthetic_text_item = Item::Message {
                    role: Role::Assistant,
                    parts: vec![Part::Text {
                        content: output_item_text(output_item)
                            .expect("text item index must contain text")
                            .to_string(),
                        extra_body: HashMap::new(),
                    }],
                    extra_body: item_extra_body_from_item(output_item),
                };
                let _ = tx
                    .send(UrpStreamEvent::ItemStart {
                        item_index: final_item_index,
                        header: ItemHeader::Message {
                            role: Role::Assistant,
                        },
                        extra_body: item_extra_body_from_item(&synthetic_text_item),
                    })
                    .await;
                if let Some(text) = output_item_text(&synthetic_text_item) {
                    let _ = tx
                        .send(UrpStreamEvent::PartStart {
                            item_index: final_item_index,
                            part_index: final_item_index,
                            header: PartHeader::Text,
                            extra_body: HashMap::new(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::Delta {
                            part_index: final_item_index,
                            delta: PartDelta::Text {
                                content: text.to_string(),
                            },
                            usage: final_usage.clone(),
                            extra_body: HashMap::new(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::PartDone {
                            part_index: final_item_index,
                            part: Part::Text {
                                content: text.to_string(),
                                extra_body: HashMap::new(),
                            },
                            usage: final_usage.clone(),
                            extra_body: HashMap::new(),
                        })
                        .await;
                }
                let _ = tx
                    .send(UrpStreamEvent::ItemDone {
                        item_index: final_item_index,
                        item: synthetic_text_item,
                        usage: final_usage.clone(),
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
        }

        let _ = tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(if outputs_have_tool_calls(&output_items) {
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                }),
                usage: final_usage,
                outputs: output_items,
                extra_body: HashMap::from([
                    ("id".to_string(), json!(response_id)),
                    ("object".to_string(), json!("response")),
                    ("created".to_string(), json!(created)),
                    ("model".to_string(), json!(urp.model.clone())),
                    ("status".to_string(), json!("completed")),
                ]),
            })
            .await;
    }
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

fn map_responses_event_to_urp_event(
    event_name: &str,
    data_val: Value,
    message_phases_by_output_index: &HashMap<u64, String>,
) -> Option<UrpStreamEvent> {
    match event_name {
        "response.created" | "response.in_progress" => None,
        "response.output_item.added" => map_output_item_added(data_val),
        "response.content_part.added" => map_content_part_added(data_val),
        "response.output_text.delta" => Some(UrpStreamEvent::Delta {
            part_index: data_val
                .get("content_index")
                .or_else(|| data_val.get("part_index"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            delta: PartDelta::Text {
                content: output_text_delta_content(&data_val).to_string(),
            },
            usage: None,
            extra_body: delta_extra_body_with_phase(data_val, message_phases_by_output_index),
        }),
        "response.reasoning_text.delta" => Some(UrpStreamEvent::Delta {
            part_index: data_val
                .get("content_index")
                .or_else(|| data_val.get("part_index"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            delta: PartDelta::Reasoning {
                content: data_val
                    .get("delta")
                    .or_else(|| data_val.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            usage: None,
            extra_body: split_known_fields(
                data_val,
                &[
                    "delta",
                    "text",
                    "output_index",
                    "content_index",
                    "part_index",
                ],
            ),
        }),
        "response.function_call_arguments.delta" => Some(UrpStreamEvent::Delta {
            part_index: data_val
                .get("content_index")
                .or_else(|| data_val.get("part_index"))
                .or_else(|| data_val.get("output_index"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            delta: PartDelta::ToolCallArguments {
                arguments: data_val
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            usage: None,
            extra_body: split_known_fields(
                data_val,
                &[
                    "delta",
                    "call_id",
                    "name",
                    "output_index",
                    "content_index",
                    "part_index",
                ],
            ),
        }),
        "response.content_part.done" => map_content_part_done(data_val),
        "response.output_item.done" => map_output_item_done(data_val),
        "response.completed" => map_response_completed(data_val),
        "error" => Some(UrpStreamEvent::Error {
            code: data_val
                .get("code")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            message: data_val
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| data_val.as_str().unwrap_or("upstream error"))
                .to_string(),
            extra_body: split_known_fields(data_val, &["code", "message"]),
        }),
        _ => None,
    }
}

fn map_output_item_added(data_val: Value) -> Option<UrpStreamEvent> {
    let item = data_val.get("item")?;
    let item_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "message" => Some(UrpStreamEvent::ItemStart {
            item_index,
            header: ItemHeader::Message {
                role: role_from_item(item),
            },
            extra_body: item_extra_body_from_value(item),
        }),
        "function_call_output" => Some(UrpStreamEvent::ItemStart {
            item_index,
            header: ItemHeader::ToolResult {
                call_id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            extra_body: item_extra_body_from_value(item),
        }),
        _ => None,
    }
}

fn map_content_part_added(data_val: Value) -> Option<UrpStreamEvent> {
    let part = data_val.get("part")?;
    let part_index = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let item_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let header = match part.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "output_text" | "text" => PartHeader::Text,
        "reasoning" => PartHeader::Reasoning,
        "refusal" => PartHeader::Refusal,
        "tool_call" | "function_call" => PartHeader::ToolCall {
            call_id: part
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: part
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        other => PartHeader::ProviderItem {
            item_type: other.to_string(),
            body: part.clone(),
        },
    };
    Some(UrpStreamEvent::PartStart {
        part_index,
        item_index,
        header,
        extra_body: part_extra_body_from_value(part),
    })
}

fn map_content_part_done(data_val: Value) -> Option<UrpStreamEvent> {
    let part = data_val.get("part")?;
    let part_index = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Some(UrpStreamEvent::PartDone {
        part_index,
        part: decode_part_from_value(part),
        usage: None,
        extra_body: part_extra_body_from_value(part),
    })
}

fn map_output_item_done(data_val: Value) -> Option<UrpStreamEvent> {
    let item = data_val.get("item")?;
    let item_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Some(UrpStreamEvent::ItemDone {
        item_index,
        item: decode_item_from_value(item),
        usage: None,
        extra_body: item_extra_body_from_value(item),
    })
}

fn map_response_completed(data_val: Value) -> Option<UrpStreamEvent> {
    let response_obj = data_val
        .get("response")
        .and_then(|v| v.as_object())
        .cloned()
        .or_else(|| data_val.as_object().cloned())?;
    let response_value = Value::Object(response_obj.clone());
    let decoded = crate::urp::decode::openai_responses::decode_response(&response_value).ok();
    let outputs = decoded
        .as_ref()
        .map(|resp| resp.outputs.clone())
        .unwrap_or_else(|| {
            response_obj
                .get("output")
                .and_then(|v| v.as_array())
                .map(|items| items.iter().map(decode_item_from_value).collect())
                .unwrap_or_default()
        });
    let finish_reason = decoded
        .as_ref()
        .and_then(|resp| resp.finish_reason)
        .or_else(
            || match response_obj.get("status").and_then(|v| v.as_str()) {
                Some("completed") => Some(if outputs_have_tool_calls(&outputs) {
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                }),
                Some("incomplete") => Some(FinishReason::Length),
                Some("failed") => Some(FinishReason::Other),
                _ => None,
            },
        );
    Some(UrpStreamEvent::ResponseDone {
        finish_reason,
        usage: decoded
            .and_then(|resp| resp.usage)
            .or_else(|| parse_usage_from_responses_object(&response_value)),
        outputs,
        extra_body: split_known_fields(
            response_value,
            &[
                "id", "object", "created", "model", "status", "output", "usage", "error",
            ],
        ),
    })
}

fn output_text_delta_content<'a>(data_val: &'a Value) -> &'a str {
    data_val
        .get("text")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
        .unwrap_or_default()
}

fn delta_extra_body_with_phase(
    data_val: Value,
    message_phases_by_output_index: &HashMap<u64, String>,
) -> HashMap<String, Value> {
    let mut extra = split_known_fields(
        data_val.clone(),
        &[
            "text",
            "delta",
            "output_index",
            "content_index",
            "part_index",
            "phase",
        ],
    );
    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
        if let Some(phase) = message_phases_by_output_index.get(&idx) {
            extra
                .entry("phase".to_string())
                .or_insert_with(|| json!(phase));
        }
    }
    extra
}

fn role_from_item(item: &Value) -> Role {
    match item
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("assistant")
    {
        "system" => Role::System,
        "developer" => Role::Developer,
        "user" => Role::User,
        "tool" => Role::Tool,
        _ => Role::Assistant,
    }
}

fn decode_item_from_value(item: &Value) -> Item {
    match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "message" => {
            let parts = item
                .get("content")
                .and_then(|v| v.as_array())
                .map(|parts| parts.iter().map(decode_part_from_value).collect())
                .unwrap_or_default();
            Item::Message {
                role: role_from_item(item),
                parts,
                extra_body: item_extra_body_from_value(item),
            }
        }
        "function_call_output" => Item::ToolResult {
            call_id: item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            is_error: false,
            content: Vec::new(),
            extra_body: item_extra_body_from_value(item),
        },
        "reasoning" => Item::Message {
            role: Role::Assistant,
            parts: vec![decode_part_from_value(item)],
            extra_body: HashMap::new(),
        },
        "function_call" => Item::Message {
            role: Role::Assistant,
            parts: vec![decode_part_from_value(item)],
            extra_body: HashMap::new(),
        },
        other => Item::Message {
            role: Role::Assistant,
            parts: vec![Part::ProviderItem {
                item_type: other.to_string(),
                body: item.clone(),
                extra_body: HashMap::new(),
            }],
            extra_body: HashMap::new(),
        },
    }
}

fn decode_part_from_value(part: &Value) -> Part {
    match part.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "output_text" | "text" => Part::Text {
            content: part
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra_body: part_extra_body_from_value(part),
        },
        "reasoning" => Part::Reasoning {
            content: part
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            encrypted: part.get("encrypted_content").cloned(),
            summary: None,
            source: part
                .get("source")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            extra_body: part_extra_body_from_value(part),
        },
        "refusal" => Part::Refusal {
            content: part
                .get("refusal")
                .or_else(|| part.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra_body: part_extra_body_from_value(part),
        },
        "function_call" | "tool_call" => Part::ToolCall {
            call_id: part
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: part
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            arguments: part
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra_body: part_extra_body_from_value(part),
        },
        other => Part::ProviderItem {
            item_type: other.to_string(),
            body: part.clone(),
            extra_body: HashMap::new(),
        },
    }
}

fn item_extra_body_from_item(item: &Item) -> HashMap<String, Value> {
    match item {
        Item::Message { extra_body, .. } | Item::ToolResult { extra_body, .. } => {
            extra_body.clone()
        }
    }
}

fn output_item_text(item: &Item) -> Option<&str> {
    match item {
        Item::Message { parts, .. } => parts.iter().find_map(|part| match part {
            Part::Text { content, .. } => Some(content.as_str()),
            _ => None,
        }),
        _ => None,
    }
}

fn outputs_have_tool_calls(items: &[Item]) -> bool {
    items.iter().any(|item| {
        matches!(item, Item::Message { parts, .. } if parts.iter().any(|part| matches!(part, Part::ToolCall { .. })))
    })
}

fn build_message_item(role: Role, parts: Vec<Part>, extra_body: HashMap<String, Value>) -> Item {
    Item::Message {
        role,
        parts,
        extra_body,
    }
}

fn merge_extra_body(dst: &mut HashMap<String, Value>, src: HashMap<String, Value>) {
    for (key, value) in src {
        dst.entry(key).or_insert(value);
    }
}

fn flush_assistant_merger(
    merger: &mut GreedyMerger,
    pending_extra_body: &mut HashMap<String, Value>,
    outputs: &mut Vec<Item>,
) {
    if let Some(parts) = merger.finish() {
        outputs.push(build_message_item(
            Role::Assistant,
            parts,
            std::mem::take(pending_extra_body),
        ));
    }
}

fn feed_assistant_part(
    merger: &mut GreedyMerger,
    pending_extra_body: &mut HashMap<String, Value>,
    outputs: &mut Vec<Item>,
    part: Part,
    item_extra_body: &HashMap<String, Value>,
) {
    match merger.feed(part, Role::Assistant) {
        Action::Append => {
            if pending_extra_body.is_empty() {
                *pending_extra_body = item_extra_body.clone();
            } else {
                merge_extra_body(pending_extra_body, item_extra_body.clone());
            }
        }
        Action::FlushAndNew(flushed_parts) => {
            outputs.push(build_message_item(
                Role::Assistant,
                flushed_parts,
                std::mem::take(pending_extra_body),
            ));
            *pending_extra_body = item_extra_body.clone();
        }
    }
}

fn build_accumulated_output_items(
    reasoning_text: &str,
    reasoning_sig: &str,
    output_text: &str,
    message_phase: Option<&str>,
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
) -> Vec<Item> {
    let mut outputs = Vec::new();
    let mut merger = GreedyMerger::new();
    let mut pending_assistant_extra_body = HashMap::new();

    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        feed_assistant_part(
            &mut merger,
            &mut pending_assistant_extra_body,
            &mut outputs,
            Part::Reasoning {
                content: (!reasoning_text.is_empty()).then(|| reasoning_text.to_string()),
                encrypted: (!reasoning_sig.is_empty())
                    .then(|| Value::String(reasoning_sig.to_string())),
                summary: None,
                source: None,
                extra_body: HashMap::new(),
            },
            &HashMap::new(),
        );
    }

    if !output_text.is_empty() {
        let mut text_extra_body = HashMap::new();
        if let Some(phase) = message_phase {
            text_extra_body.insert("phase".to_string(), json!(phase));
        }
        feed_assistant_part(
            &mut merger,
            &mut pending_assistant_extra_body,
            &mut outputs,
            Part::Text {
                content: output_text.to_string(),
                extra_body: text_extra_body,
            },
            &HashMap::new(),
        );
    }

    for call_id in call_order {
        if let Some((name, arguments)) = calls.get(call_id) {
            feed_assistant_part(
                &mut merger,
                &mut pending_assistant_extra_body,
                &mut outputs,
                Part::ToolCall {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    extra_body: HashMap::new(),
                },
                &HashMap::new(),
            );
        }
    }

    flush_assistant_merger(&mut merger, &mut pending_assistant_extra_body, &mut outputs);
    outputs
}

fn item_extra_body_from_value(item: &Value) -> HashMap<String, Value> {
    split_known_fields(
        item.clone(),
        &[
            "type",
            "role",
            "content",
            "call_id",
            "id",
            "output",
            "name",
            "arguments",
        ],
    )
}

fn part_extra_body_from_value(part: &Value) -> HashMap<String, Value> {
    split_known_fields(
        part.clone(),
        &[
            "type",
            "text",
            "refusal",
            "call_id",
            "name",
            "arguments",
            "source",
            "encrypted_content",
        ],
    )
}

fn split_known_fields(value: Value, known_fields: &[&str]) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            if !known_fields.iter().any(|known| known == key) {
                out.insert(key.clone(), val.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_accumulated_output_items_greedily_merges_reasoning_text_and_tool_calls() {
        let calls = HashMap::from([(
            "call_1".to_string(),
            ("lookup".to_string(), "{}".to_string()),
        )]);

        let outputs = build_accumulated_output_items(
            "think",
            "sig_1",
            "answer",
            Some("analysis"),
            &["call_1".to_string()],
            &calls,
        );

        assert_eq!(outputs.len(), 1);
        let Item::Message {
            role,
            parts,
            extra_body,
        } = &outputs[0]
        else {
            panic!("expected assistant message output");
        };
        assert_eq!(*role, Role::Assistant);
        assert!(extra_body.is_empty());
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                encrypted: Some(Value::String(sig)),
                ..
            } if content == "think" && sig == "sig_1"
        ));
        assert!(matches!(
            &parts[1],
            Part::Text { content, extra_body } if content == "answer" && extra_body.get("phase") == Some(&json!("analysis"))
        ));
        assert!(matches!(
            &parts[2],
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } if call_id == "call_1" && name == "lookup" && arguments == "{}"
        ));
    }

    #[test]
    fn build_accumulated_output_items_omits_empty_text_message() {
        let outputs = build_accumulated_output_items(
            "",
            "sig_only",
            "",
            Some("analysis"),
            &[],
            &HashMap::new(),
        );

        assert_eq!(outputs.len(), 1);
        let Item::Message {
            parts, extra_body, ..
        } = &outputs[0]
        else {
            panic!("expected assistant message output");
        };
        assert!(extra_body.is_empty());
        assert_eq!(parts.len(), 1);
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: None,
                encrypted: Some(Value::String(sig)),
                ..
            } if sig == "sig_only"
        ));
    }

    #[test]
    fn map_response_completed_uses_greedy_nonstream_decoder_shape() {
        let event = json!({
            "response": {
                "id": "resp_test",
                "object": "response",
                "created": 1,
                "model": "gpt-5.4",
                "status": "completed",
                "output": [
                    {
                        "type": "reasoning",
                        "text": "think",
                        "encrypted_content": "sig_1"
                    },
                    {
                        "type": "message",
                        "role": "assistant",
                        "phase": "analysis",
                        "content": [
                            { "type": "output_text", "text": "answer" }
                        ]
                    },
                    {
                        "type": "function_call",
                        "call_id": "call_1",
                        "name": "lookup",
                        "arguments": "{}"
                    }
                ]
            }
        });

        let Some(UrpStreamEvent::ResponseDone {
            finish_reason,
            outputs,
            ..
        }) = map_response_completed(event)
        else {
            panic!("expected response done event");
        };

        assert_eq!(finish_reason, Some(FinishReason::ToolCalls));
        assert_eq!(outputs.len(), 1);
        let Item::Message {
            parts, extra_body, ..
        } = &outputs[0]
        else {
            panic!("expected assistant message output");
        };
        assert!(extra_body.is_empty());
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                encrypted: Some(Value::String(sig)),
                ..
            } if content == "think" && sig == "sig_1"
        ));
        assert!(matches!(
            &parts[1],
            Part::Text { content, extra_body } if content == "answer" && extra_body.get("phase") == Some(&json!("analysis"))
        ));
        assert!(matches!(&parts[2], Part::ToolCall { call_id, .. } if call_id == "call_1"));
    }
}
