use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_gemini_object,
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
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_gemini_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut output_text = String::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut call_part_indices: HashMap<String, u32> = HashMap::new();
    let mut next_tool_part_index: u32 = 2;
    let mut started_response = false;
    let mut started_text_part = false;
    let mut started_reasoning_part = false;
    let mut finish_reason: Option<FinishReason> = None;

    let idle_timeout = std::time::Duration::from_secs(120);
    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                "upstream stream idle for 120s without data",
            )
        })?
    {
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

        if !started_response {
            let extra_body = HashMap::new();
            let _ = tx
                .send(UrpStreamEvent::ResponseStart {
                    id: response_id.clone(),
                    model: urp.model.clone(),
                    extra_body: extra_body.clone(),
                })
                .await;
            let _ = tx
                .send(UrpStreamEvent::ItemStart {
                    item_index: 0,
                    header: ItemHeader::Message {
                        role: Role::Assistant,
                    },
                    extra_body,
                })
                .await;
            started_response = true;
        }

        let mut current_output_text = String::new();
        let mut current_reasoning_text = String::new();
        let mut current_reasoning_sig = String::new();
        let mut current_calls: HashMap<String, (String, String)> = HashMap::new();
        let mut current_call_order: Vec<String> = Vec::new();

        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                    current_reasoning_text.push_str(text);
                    if let Some(sig) = part.get("thoughtSignature") {
                        let sig_text = sig
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sig.to_string());
                        current_reasoning_sig.push_str(&sig_text);
                    }
                } else {
                    current_output_text.push_str(text);
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
                        format!("call_{}", current_call_order.len() + 1)
                    } else {
                        call_id
                    };
                    if !current_calls.contains_key(&key) {
                        current_call_order.push(key.clone());
                    }
                    current_calls.insert(key, (name, arguments));
                }
            }
        }

        if current_output_text.len() > output_text.len() {
            if !started_text_part {
                let _ = tx
                    .send(UrpStreamEvent::PartStart {
                        part_index: 0,
                        item_index: 0,
                        header: PartHeader::Text,
                        extra_body: HashMap::new(),
                    })
                    .await;
                started_text_part = true;
            }

            let delta = current_output_text[output_text.len()..].to_string();
            output_text = current_output_text;
            if !delta.is_empty() {
                let _ = tx
                    .send(UrpStreamEvent::Delta {
                        part_index: 0,
                        delta: PartDelta::Text { content: delta },
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
        }

        if current_reasoning_text.len() > reasoning_text.len() {
            if !started_reasoning_part {
                let _ = tx
                    .send(UrpStreamEvent::PartStart {
                        part_index: 1,
                        item_index: 0,
                        header: PartHeader::Reasoning,
                        extra_body: HashMap::new(),
                    })
                    .await;
                started_reasoning_part = true;
            }

            let delta = current_reasoning_text[reasoning_text.len()..].to_string();
            reasoning_text = current_reasoning_text;
            if !delta.is_empty() {
                let _ = tx
                    .send(UrpStreamEvent::Delta {
                        part_index: 1,
                        delta: PartDelta::Reasoning {
                            content: Some(delta),
                            encrypted: None,
                            summary: None,
                            source: None,
                        },
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
        } else {
            reasoning_text = current_reasoning_text;
        }

        if current_reasoning_sig.len() > reasoning_sig.len() && !started_reasoning_part {
            let _ = tx
                .send(UrpStreamEvent::PartStart {
                    part_index: 1,
                    item_index: 0,
                    header: PartHeader::Reasoning,
                    extra_body: HashMap::new(),
                })
                .await;
            started_reasoning_part = true;
        }
        reasoning_sig = current_reasoning_sig;

        for call_id in &current_call_order {
            let Some((name, arguments)) = current_calls.get(call_id).cloned() else {
                continue;
            };

            if !calls.contains_key(call_id) {
                call_order.push(call_id.clone());
                calls.insert(call_id.clone(), (name.clone(), String::new()));

                let part_index = next_tool_part_index;
                next_tool_part_index += 1;
                call_part_indices.insert(call_id.clone(), part_index);

                let _ = tx
                    .send(UrpStreamEvent::PartStart {
                        part_index,
                        item_index: 0,
                        header: PartHeader::ToolCall {
                            call_id: call_id.clone(),
                            name: name.clone(),
                        },
                        extra_body: HashMap::new(),
                    })
                    .await;
            }

            let Some(part_index) = call_part_indices.get(call_id).copied() else {
                continue;
            };

            let previous_arguments_len =
                calls.get(call_id).map(|(_, args)| args.len()).unwrap_or(0);
            if let Some(entry) = calls.get_mut(call_id) {
                entry.0 = name.clone();
                if arguments.len() > previous_arguments_len {
                    let delta = arguments[previous_arguments_len..].to_string();
                    entry.1 = arguments.clone();
                    if !delta.is_empty() {
                        let _ = tx
                            .send(UrpStreamEvent::Delta {
                                part_index,
                                delta: PartDelta::ToolCallArguments { arguments: delta },
                                usage: None,
                                extra_body: HashMap::new(),
                            })
                            .await;
                    }
                } else {
                    entry.1 = arguments.clone();
                }
            }
        }

        if let Some(reason) = candidate.get("finishReason").and_then(|v| v.as_str()) {
            finish_reason = Some(parse_finish_reason(reason));
            break;
        }
    }

    if started_reasoning_part {
        let _ = tx
            .send(UrpStreamEvent::PartDone {
                part_index: 1,
                part: Part::Reasoning {
                    content: (!reasoning_text.is_empty()).then(|| reasoning_text.clone()),
                    encrypted: (!reasoning_sig.is_empty())
                        .then(|| Value::String(reasoning_sig.clone())),
                    summary: None,
                    source: None,
                    extra_body: HashMap::new(),
                },
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
    }

    if started_text_part {
        let _ = tx
            .send(UrpStreamEvent::PartDone {
                part_index: 0,
                part: Part::Text {
                    content: output_text.clone(),
                    extra_body: HashMap::new(),
                },
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
    }

    let mut item_parts = Vec::new();
    if started_text_part || !output_text.is_empty() {
        item_parts.push(Part::Text {
            content: output_text.clone(),
            extra_body: HashMap::new(),
        });
    }
    if started_reasoning_part || !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        item_parts.push(Part::Reasoning {
            content: (!reasoning_text.is_empty()).then(|| reasoning_text.clone()),
            encrypted: (!reasoning_sig.is_empty()).then(|| Value::String(reasoning_sig.clone())),
            summary: None,
            source: None,
            extra_body: HashMap::new(),
        });
    }

    for call_id in &call_order {
        if let Some((name, arguments)) = calls.get(call_id) {
            if let Some(part_index) = call_part_indices.get(call_id).copied() {
                let _ = tx
                    .send(UrpStreamEvent::PartDone {
                        part_index,
                        part: Part::ToolCall {
                            call_id: call_id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                            extra_body: HashMap::new(),
                        },
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .await;
            }

            item_parts.push(Part::ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                extra_body: HashMap::new(),
            });
        }
    }

    let output_item = Item::Message {
        role: Role::Assistant,
        parts: item_parts,
        extra_body: HashMap::new(),
    };
    let outputs = vec![output_item.clone()];
    let usage = latest_stream_usage_snapshot(&runtime_metrics).await;

    if started_response {
        let _ = tx
            .send(UrpStreamEvent::ItemDone {
                item_index: 0,
                item: output_item,
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
        let _ = tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                outputs,
                extra_body: HashMap::new(),
            })
            .await;
    }

    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

fn parse_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY" => FinishReason::ContentFilter,
        "STOP" => FinishReason::Stop,
        _ => FinishReason::Other,
    }
}
