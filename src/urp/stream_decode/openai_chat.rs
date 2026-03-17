use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_chat_object,
    record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::stream_helpers::extract_chat_reasoning_deltas;
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

pub(crate) async fn stream_chat_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut output_text = String::new();
    let mut assistant_message_phase: Option<String> = None;
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut call_id_by_index: HashMap<usize, String> = HashMap::new();

    let mut response_started = false;
    let mut item_started = false;
    let mut next_part_index = 0u32;
    let mut text_part_index = None;
    let mut reasoning_part_index = None;
    let mut tool_part_index_by_call_id: HashMap<String, u32> = HashMap::new();
    let mut finish_reason = None;

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
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_chat_object(&data_val)).await;

        if let Some(reason) = data_val
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|v| v.as_str())
            .filter(|reason| !reason.is_empty())
        {
            finish_reason = Some(parse_finish_reason(reason));
            record_stream_terminal_event(&runtime_metrics, "chat.completion.chunk", Some(reason)).await;
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

        if delta.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            ensure_response_and_item_started(
                &tx,
                &response_id,
                &urp.model,
                &mut response_started,
                &mut item_started,
            )
            .await?;
        }

        if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
            if !t.is_empty() {
                ensure_response_and_item_started(
                    &tx,
                    &response_id,
                    &urp.model,
                    &mut response_started,
                    &mut item_started,
                )
                .await?;
                let part_index = ensure_part_started(
                    &tx,
                    0,
                    &mut text_part_index,
                    &mut next_part_index,
                    PartHeader::Text,
                )
                .await?;
                output_text.push_str(t);
                send_event(
                    &tx,
                    UrpStreamEvent::Delta {
                        part_index,
                        delta: PartDelta::Text {
                            content: t.to_string(),
                        },
                        usage: None,
                        extra_body: HashMap::new(),
                    },
                )
                .await?;
            }
        }

        let (reasoning_text_deltas, reasoning_summary_deltas, reasoning_sig_deltas) =
            extract_chat_reasoning_deltas(&delta);
        for summary in reasoning_summary_deltas {
            if summary.is_empty() {
                continue;
            }
            ensure_response_and_item_started(
                &tx,
                &response_id,
                &urp.model,
                &mut response_started,
                &mut item_started,
            )
            .await?;
            let part_index = ensure_part_started(
                &tx,
                0,
                &mut reasoning_part_index,
                &mut next_part_index,
                PartHeader::Reasoning,
            )
            .await?;
            send_event(
                &tx,
                UrpStreamEvent::Delta {
                    part_index,
                    delta: PartDelta::Reasoning {
                        content: None,
                        encrypted: None,
                        summary: Some(summary),
                        source: Some("openrouter".to_string()),
                    },
                    usage: None,
                    extra_body: HashMap::new(),
                },
            )
            .await?;
        }
        for t in reasoning_text_deltas {
            if t.is_empty() {
                continue;
            }
            ensure_response_and_item_started(
                &tx,
                &response_id,
                &urp.model,
                &mut response_started,
                &mut item_started,
            )
            .await?;
            let part_index = ensure_part_started(
                &tx,
                0,
                &mut reasoning_part_index,
                &mut next_part_index,
                PartHeader::Reasoning,
            )
            .await?;
            reasoning_text.push_str(&t);
            send_event(
                &tx,
                UrpStreamEvent::Delta {
                    part_index,
                    delta: PartDelta::Reasoning {
                        content: Some(t),
                        encrypted: None,
                        summary: None,
                        source: Some("openrouter".to_string()),
                    },
                    usage: None,
                    extra_body: HashMap::new(),
                },
            )
            .await?;
        }
        for sig in reasoning_sig_deltas {
            if !sig.is_empty() {
                ensure_response_and_item_started(
                    &tx,
                    &response_id,
                    &urp.model,
                    &mut response_started,
                    &mut item_started,
                )
                .await?;
                let _ = ensure_part_started(
                    &tx,
                    0,
                    &mut reasoning_part_index,
                    &mut next_part_index,
                    PartHeader::Reasoning,
                )
                .await?;
                reasoning_sig.push_str(&sig);
                send_event(
                    &tx,
                    UrpStreamEvent::Delta {
                        part_index: reasoning_part_index.expect("reasoning part index must exist"),
                        delta: PartDelta::Reasoning {
                            content: None,
                            encrypted: Some(Value::String(sig.clone())),
                            summary: None,
                            source: Some("openrouter".to_string()),
                        },
                        usage: None,
                        extra_body: HashMap::new(),
                    },
                )
                .await?;
            }
        }

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
                }

                ensure_response_and_item_started(
                    &tx,
                    &response_id,
                    &urp.model,
                    &mut response_started,
                    &mut item_started,
                )
                .await?;

                let part_index = if let Some(part_index) = tool_part_index_by_call_id.get(&call_id) {
                    *part_index
                } else {
                    let part_index = next_part_index;
                    next_part_index += 1;
                    tool_part_index_by_call_id.insert(call_id.clone(), part_index);
                    send_event(
                        &tx,
                        UrpStreamEvent::PartStart {
                            part_index,
                            item_index: 0,
                            header: PartHeader::ToolCall {
                                call_id: call_id.clone(),
                                name: name.clone(),
                            },
                            extra_body: HashMap::new(),
                        },
                    )
                    .await?;
                    part_index
                };

                let Some(entry) = calls.get_mut(&call_id) else {
                    tracing::warn!(call_id = %call_id, "unknown call_id in tool call stream delta, skipping");
                    continue;
                };

                if !name.is_empty() && entry.0.is_empty() {
                    entry.0 = name.clone();
                }
                if !args_delta.is_empty() {
                    entry.1.push_str(&args_delta);
                    send_event(
                        &tx,
                        UrpStreamEvent::Delta {
                            part_index,
                            delta: PartDelta::ToolCallArguments {
                                arguments: args_delta,
                            },
                            usage: None,
                            extra_body: HashMap::new(),
                        },
                    )
                    .await?;
                }
            }
        }
    }

    if response_started || item_started || !output_text.is_empty() || !reasoning_text.is_empty() || !call_order.is_empty() {
        ensure_response_and_item_started(
            &tx,
            &response_id,
            &urp.model,
            &mut response_started,
            &mut item_started,
        )
        .await?;
    }

    let usage = latest_stream_usage_snapshot(&runtime_metrics).await;
    let item = build_assistant_item(
        assistant_message_phase,
        text_part_index,
        &output_text,
        reasoning_part_index,
        &reasoning_text,
        &reasoning_sig,
        &call_order,
        &calls,
        &tool_part_index_by_call_id,
    );

    if matches!(item, Item::Message { .. }) {
        for (part_index, part) in sorted_parts(
            text_part_index,
            &output_text,
            reasoning_part_index,
            &reasoning_text,
            &reasoning_sig,
            &call_order,
            &calls,
            &tool_part_index_by_call_id,
        ) {
            send_event(
                &tx,
                UrpStreamEvent::PartDone {
                    part_index,
                    part,
                    usage: None,
                    extra_body: HashMap::new(),
                },
            )
            .await?;
        }

        send_event(
            &tx,
            UrpStreamEvent::ItemDone {
                item_index: 0,
                item: item.clone(),
                usage: usage.clone(),
                extra_body: HashMap::new(),
            },
        )
        .await?;

        send_event(
            &tx,
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                outputs: vec![item.clone()],
                extra_body: HashMap::new(),
            },
        )
        .await?;
    }

    Ok(())
}

async fn ensure_response_and_item_started(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    response_started: &mut bool,
    item_started: &mut bool,
) -> AppResult<()> {
    if !*response_started {
        send_event(
            tx,
            UrpStreamEvent::ResponseStart {
                id: response_id.to_string(),
                model: model.to_string(),
                extra_body: HashMap::new(),
            },
        )
        .await?;
        *response_started = true;
    }
    if !*item_started {
        send_event(
            tx,
            UrpStreamEvent::ItemStart {
                item_index: 0,
                header: ItemHeader::Message {
                    role: Role::Assistant,
                },
                extra_body: HashMap::new(),
            },
        )
        .await?;
        *item_started = true;
    }
    Ok(())
}

async fn ensure_part_started(
    tx: &mpsc::Sender<UrpStreamEvent>,
    item_index: u32,
    slot: &mut Option<u32>,
    next_part_index: &mut u32,
    header: PartHeader,
) -> AppResult<u32> {
    if let Some(part_index) = *slot {
        return Ok(part_index);
    }
    let part_index = *next_part_index;
    *next_part_index += 1;
    *slot = Some(part_index);
    send_event(
        tx,
        UrpStreamEvent::PartStart {
            part_index,
            item_index,
            header,
            extra_body: HashMap::new(),
        },
    )
    .await?;
    Ok(part_index)
}

async fn send_event(tx: &mpsc::Sender<UrpStreamEvent>, event: UrpStreamEvent) -> AppResult<()> {
    tx.send(event)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))
}

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

fn build_assistant_item(
    assistant_message_phase: Option<String>,
    text_part_index: Option<u32>,
    output_text: &str,
    reasoning_part_index: Option<u32>,
    reasoning_text: &str,
    reasoning_sig: &str,
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    tool_part_index_by_call_id: &HashMap<String, u32>,
) -> Item {
    Item::Message {
        role: Role::Assistant,
        parts: sorted_parts(
            text_part_index,
            output_text,
            reasoning_part_index,
            reasoning_text,
            reasoning_sig,
            call_order,
            calls,
            tool_part_index_by_call_id,
        )
        .into_iter()
        .map(|(_, part)| part)
        .collect(),
        extra_body: item_extra_body(&assistant_message_phase),
    }
}

fn sorted_parts(
    text_part_index: Option<u32>,
    output_text: &str,
    reasoning_part_index: Option<u32>,
    reasoning_text: &str,
    reasoning_sig: &str,
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    tool_part_index_by_call_id: &HashMap<String, u32>,
) -> Vec<(u32, Part)> {
    let mut parts = Vec::new();

    if let Some(part_index) = text_part_index {
        parts.push((
            part_index,
            Part::Text {
                content: output_text.to_string(),
                extra_body: HashMap::new(),
            },
        ));
    }

    if let Some(part_index) = reasoning_part_index {
        parts.push((
            part_index,
            Part::Reasoning {
                content: (!reasoning_text.is_empty()).then(|| reasoning_text.to_string()),
                encrypted: (!reasoning_sig.is_empty()).then(|| Value::String(reasoning_sig.to_string())),
                summary: None,
                source: None,
                extra_body: HashMap::new(),
            },
        ));
    }

    for call_id in call_order {
        let Some(part_index) = tool_part_index_by_call_id.get(call_id).copied() else {
            continue;
        };
        let Some((name, arguments)) = calls.get(call_id) else {
            continue;
        };
        parts.push((
            part_index,
            Part::ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                extra_body: HashMap::new(),
            },
        ));
    }

    parts.sort_by_key(|(part_index, _)| *part_index);
    parts
}

fn item_extra_body(assistant_message_phase: &Option<String>) -> HashMap<String, Value> {
    let mut extra_body = HashMap::new();
    if let Some(phase) = assistant_message_phase {
        extra_body.insert("phase".to_string(), Value::String(phase.clone()));
    }
    extra_body
}
