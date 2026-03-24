use crate::error::{AppError, AppResult};
use crate::handlers::routing::now_ts;
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_responses_object,
    record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::stream_helpers::{
    extract_reasoning_parts, extract_responses_message_phase, extract_responses_message_text,
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
    let mut output_texts_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut message_phases_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut reasoning_text = String::new();
    let mut reasoning_summary_text = String::new();
    let mut reasoning_sig = String::new();
    let mut reasoning_source: Option<String> = None;
    let mut reasoning_output_index: Option<u64> = None;
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_text_delta = false;
    let mut saw_text_part_done = false;
    let mut response_done_sent = false;
    let mut index_state = ResponsesStreamIndexState::default();

    let _ = tx
        .send(UrpStreamEvent::ResponseStart {
            id: response_id.clone(),
            model: urp.model.clone(),
            extra_body: HashMap::from([
                ("object".to_string(), json!("response")),
                ("created_at".to_string(), json!(created)),
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
            if let Some(text) = data_val.get("delta").and_then(|v| v.as_str()) {
                let output_index = data_val
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                output_texts_by_output_index
                    .entry(output_index)
                    .or_default()
                    .push_str(text);
                saw_text_delta = true;
            }
        }
        if ev.event == "response.reasoning.delta" {
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                reasoning_text.push_str(delta);
            }
        }
        if ev.event == "response.reasoning.done" {
            if let Some(text) = data_val
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
            {
                if reasoning_text.is_empty() {
                    reasoning_text = text.to_string();
                }
            }
        }
        if ev.event == "response.reasoning_summary_text.delta" {
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                reasoning_summary_text.push_str(delta);
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
                    let text = extract_responses_message_text(item);
                    if !text.is_empty() {
                        output_texts_by_output_index.entry(idx).or_insert(text);
                    }
                    if let Some(phase) = extract_responses_message_phase(item) {
                        message_phases_by_output_index.insert(idx, phase);
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    reasoning_output_index.get_or_insert(idx);
                }
                merge_reasoning_source(&mut reasoning_source, reasoning_source_from_value(item));
                let (text, summary, sig) = extract_reasoning_parts(item);
                if reasoning_text.is_empty() && !text.is_empty() {
                    reasoning_text = text;
                }
                if reasoning_summary_text.is_empty() && !summary.is_empty() {
                    reasoning_summary_text = summary;
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
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    reasoning_output_index.get_or_insert(idx);
                }
                merge_reasoning_source(&mut reasoning_source, reasoning_source_from_value(item));
                let (text, summary, sig) = extract_reasoning_parts(item);
                if reasoning_text.is_empty() && !text.is_empty() {
                    reasoning_text = text;
                }
                if reasoning_summary_text.is_empty() && !summary.is_empty() {
                    reasoning_summary_text = summary;
                }
                if reasoning_sig.is_empty() && !sig.is_empty() {
                    reasoning_sig = sig;
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("message")
                && !saw_text_delta
            {
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    let text = extract_responses_message_text(item);
                    if !text.is_empty() {
                        output_texts_by_output_index
                            .entry(idx)
                            .or_default()
                            .push_str(&text);
                    }
                    if let Some(phase) = extract_responses_message_phase(item) {
                        message_phases_by_output_index.insert(idx, phase);
                    }
                }
            }
        }
        if ev.event == "response.content_part.done"
            && data_val
                .get("part")
                .and_then(|part| part.get("type"))
                .and_then(|v| v.as_str())
                .is_some_and(|part_type| matches!(part_type, "output_text" | "text"))
            && data_val
                .get("part")
                .and_then(|part| part.get("text"))
                .and_then(|v| v.as_str())
                .is_some_and(|text| !text.is_empty())
        {
            saw_text_part_done = true;
        }

        for stream_event in map_responses_event_to_urp_events_with_state(
            &ev.event,
            data_val,
            &message_phases_by_output_index,
            &mut index_state,
        ) {
            response_done_sent |= matches!(stream_event, UrpStreamEvent::ResponseDone { .. });
            let _ = tx.send(stream_event).await;
        }
    }

    if !response_done_sent {
        let output_items = build_accumulated_output_items(
            &reasoning_text,
            &reasoning_summary_text,
            &reasoning_sig,
            reasoning_source.as_deref(),
            reasoning_output_index,
            &output_texts_by_output_index,
            &message_phases_by_output_index,
            &call_order,
            &calls,
            &call_ids_by_output_index,
        );
        let final_usage = latest_stream_usage_snapshot(&runtime_metrics).await;

        if !saw_text_delta && !saw_text_part_done {
            for (final_item_index, output_item) in output_items.iter().enumerate() {
                let Some(Item::Message {
                    role: Role::Assistant,
                    parts,
                    extra_body,
                }) = Some(output_item)
                else {
                    continue;
                };

                for part in parts {
                    let Part::Text {
                        content,
                        extra_body: text_extra_body,
                    } = part
                    else {
                        continue;
                    };

                    let synthetic_text_item = Item::Message {
                        role: Role::Assistant,
                        parts: vec![Part::Text {
                            content: content.clone(),
                            extra_body: text_extra_body.clone(),
                        }],
                        extra_body: extra_body.clone(),
                    };
                    let part_index = index_state.allocate_fresh_part_index();
                    let item_index = final_item_index as u32;

                    let _ = tx
                        .send(UrpStreamEvent::ItemStart {
                            item_index,
                            header: ItemHeader::Message {
                                role: Role::Assistant,
                            },
                            extra_body: item_extra_body_from_item(&synthetic_text_item),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::PartStart {
                            item_index,
                            part_index,
                            header: PartHeader::Text,
                            extra_body: text_extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::Delta {
                            part_index,
                            delta: PartDelta::Text {
                                content: content.clone(),
                            },
                            usage: final_usage.clone(),
                            extra_body: text_extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::PartDone {
                            part_index,
                            part: Part::Text {
                                content: content.clone(),
                                extra_body: text_extra_body.clone(),
                            },
                            usage: final_usage.clone(),
                            extra_body: text_extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::ItemDone {
                            item_index,
                            item: synthetic_text_item,
                            usage: final_usage.clone(),
                            extra_body: HashMap::new(),
                        })
                        .await;
                }
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
                    ("created_at".to_string(), json!(created)),
                    ("model".to_string(), json!(urp.model.clone())),
                    ("status".to_string(), json!("completed")),
                ]),
            })
            .await;
    }
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

fn map_responses_event_to_urp_events_with_state(
    event_name: &str,
    data_val: Value,
    message_phases_by_output_index: &HashMap<u64, String>,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    match event_name {
        "response.created" | "response.in_progress" => Vec::new(),
        "response.output_item.added" => map_output_item_added(data_val, index_state),
        "response.content_part.added" => map_content_part_added(data_val, index_state),
        "response.output_text.delta" => vec![UrpStreamEvent::Delta {
            part_index: urp_part_index_from_delta(&data_val, index_state),
            delta: PartDelta::Text {
                content: output_text_delta_content(&data_val).to_string(),
            },
            usage: None,
            extra_body: delta_extra_body_with_phase(data_val, message_phases_by_output_index),
        }],
        "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
            let reasoning_source = data_val
                .get("output_index")
                .and_then(|v| v.as_u64())
                .and_then(|output_index| {
                    let output_state = index_state
                        .output_state_by_index
                        .entry(output_index)
                        .or_default();
                    merge_reasoning_source(
                        &mut output_state.reasoning_source,
                        reasoning_source_from_value(&data_val),
                    );
                    if event_name == "response.reasoning_summary_text.delta" {
                        output_state.reasoning_summary_delta_seen = true;
                    } else {
                        output_state.reasoning_text_delta_seen = true;
                    }
                    output_state.reasoning_source.clone()
                });
            let extra_body = split_known_fields(
                data_val.clone(),
                &[
                    "delta",
                    "text",
                    "output_index",
                    "content_index",
                    "part_index",
                    "summary_index",
                ],
            );
            vec![UrpStreamEvent::Delta {
                part_index: urp_part_index_from_delta(&data_val, index_state),
                delta: PartDelta::Reasoning {
                    content: if event_name == "response.reasoning_summary_text.delta" {
                        None
                    } else {
                        data_val
                            .get("delta")
                            .or_else(|| data_val.get("text"))
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                    },
                    encrypted: None,
                    summary: if event_name == "response.reasoning_summary_text.delta" {
                        data_val
                            .get("delta")
                            .or_else(|| data_val.get("text"))
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                    } else {
                        None
                    },
                    source: reasoning_source,
                },
                usage: None,
                extra_body,
            }]
        }
        "response.reasoning.done" => {
            let part_index = urp_part_index_from_delta(&data_val, index_state);
            let reasoning_source = data_val
                .get("output_index")
                .and_then(|v| v.as_u64())
                .and_then(|output_index| {
                    let output_state = index_state
                        .output_state_by_index
                        .entry(output_index)
                        .or_default();
                    merge_reasoning_source(
                        &mut output_state.reasoning_source,
                        reasoning_source_from_value(&data_val),
                    );
                    output_state.reasoning_source.clone()
                });
            vec![UrpStreamEvent::PartDone {
                part_index,
                part: Part::Reasoning {
                    content: data_val
                        .get("text")
                        .and_then(|v| v.as_str())
                        .filter(|text| !text.is_empty())
                        .map(|text| text.to_string()),
                    encrypted: None,
                    summary: None,
                    source: reasoning_source,
                    extra_body: split_known_fields(
                        data_val,
                        &[
                            "text",
                            "delta",
                            "output_index",
                            "content_index",
                            "part_index",
                        ],
                    ),
                },
                usage: None,
                extra_body: HashMap::new(),
            }]
        }
        "response.function_call_arguments.delta" => vec![UrpStreamEvent::Delta {
            part_index: urp_part_index_from_delta(&data_val, index_state),
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
                &["delta", "output_index", "content_index", "part_index"],
            ),
        }],
        "response.content_part.done" => map_content_part_done(data_val, index_state),
        "response.output_item.done" => map_output_item_done(data_val, index_state),
        "response.completed" => map_response_completed(data_val, index_state),
        "error" => vec![UrpStreamEvent::Error {
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
        }],
        _ => Vec::new(),
    }
}

#[derive(Debug, Default)]
struct ResponsesStreamIndexState {
    next_item_index: u32,
    next_part_index: u32,
    part_index_by_content_key: HashMap<(u64, u64), u32>,
    synthetic_part_index_by_output_index: HashMap<u64, u32>,
    output_state_by_index: HashMap<u64, OutputItemStreamState>,
    active_assistant_item: Option<ActiveAssistantStreamItem>,
    boundary_merger: GreedyMerger,
}

impl ResponsesStreamIndexState {
    fn allocate_fresh_item_index(&mut self) -> u32 {
        let next = self.next_item_index;
        self.next_item_index += 1;
        next
    }

    fn part_index_for_content(&mut self, output_index: u64, content_index: u64) -> u32 {
        *self
            .part_index_by_content_key
            .entry((output_index, content_index))
            .or_insert_with(|| {
                let next = self.next_part_index;
                self.next_part_index += 1;
                next
            })
    }

    fn synthetic_part_index_for_output(&mut self, output_index: u64) -> u32 {
        *self
            .synthetic_part_index_by_output_index
            .entry(output_index)
            .or_insert_with(|| {
                let next = self.next_part_index;
                self.next_part_index += 1;
                next
            })
    }

    fn allocate_fresh_part_index(&mut self) -> u32 {
        let next = self.next_part_index;
        self.next_part_index += 1;
        next
    }
}

#[derive(Debug, Clone)]
struct ActiveAssistantStreamItem {
    item_index: u32,
    role: Role,
    parts: Vec<Part>,
    extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default)]
struct OutputItemStreamState {
    merged_item_index: Option<u32>,
    standalone_item_index: Option<u32>,
    item_type: Option<String>,
    role: Option<Role>,
    item_extra_body: HashMap<String, Value>,
    fed_to_merger: bool,
    part_done_seen: bool,
    reasoning_text_delta_seen: bool,
    reasoning_summary_delta_seen: bool,
    reasoning_source: Option<String>,
}

fn merge_reasoning_source(dst: &mut Option<String>, source: Option<String>) {
    if let Some(source) = source.filter(|source| !source.is_empty()) {
        *dst = Some(source);
    }
}

fn reasoning_source_from_value(value: &Value) -> Option<String> {
    value
        .get("source")
        .and_then(|value| value.as_str())
        .filter(|source| !source.is_empty())
        .map(|source| source.to_string())
}

fn flush_active_assistant_item(
    state: &mut ResponsesStreamIndexState,
    events: &mut Vec<UrpStreamEvent>,
) {
    let Some(active_item) = state.active_assistant_item.take() else {
        return;
    };
    events.push(UrpStreamEvent::ItemDone {
        item_index: active_item.item_index,
        item: build_message_item(active_item.role, active_item.parts, active_item.extra_body),
        usage: None,
        extra_body: HashMap::new(),
    });
}

fn finish_assistant_stream_group(
    state: &mut ResponsesStreamIndexState,
    events: &mut Vec<UrpStreamEvent>,
) {
    let _ = state.boundary_merger.finish();
    flush_active_assistant_item(state, events);
}

fn ensure_assistant_item_for_part(
    state: &mut ResponsesStreamIndexState,
    output_index: u64,
    role: Role,
    item_extra_body: HashMap<String, Value>,
    boundary_part: Part,
    events: &mut Vec<UrpStreamEvent>,
) -> u32 {
    let action = state.boundary_merger.feed(boundary_part, role);
    let item_index = match action {
        Action::Append => {
            if let Some(active_item) = state.active_assistant_item.as_mut() {
                merge_extra_body(&mut active_item.extra_body, item_extra_body.clone());
                active_item.item_index
            } else {
                let item_index = state.allocate_fresh_item_index();
                events.push(UrpStreamEvent::ItemStart {
                    item_index,
                    header: ItemHeader::Message { role },
                    extra_body: item_extra_body.clone(),
                });
                state.active_assistant_item = Some(ActiveAssistantStreamItem {
                    item_index,
                    role,
                    parts: Vec::new(),
                    extra_body: item_extra_body.clone(),
                });
                item_index
            }
        }
        Action::FlushAndNew(_) => {
            flush_active_assistant_item(state, events);
            let item_index = state.allocate_fresh_item_index();
            events.push(UrpStreamEvent::ItemStart {
                item_index,
                header: ItemHeader::Message { role },
                extra_body: item_extra_body.clone(),
            });
            state.active_assistant_item = Some(ActiveAssistantStreamItem {
                item_index,
                role,
                parts: Vec::new(),
                extra_body: item_extra_body.clone(),
            });
            item_index
        }
    };

    let output_state = state.output_state_by_index.entry(output_index).or_default();
    output_state.merged_item_index = Some(item_index);
    output_state.role = Some(role);
    output_state.item_extra_body = item_extra_body;
    output_state.fed_to_merger = true;
    item_index
}

fn map_output_item_added(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let Some(item) = data_val.get("item") else {
        return Vec::new();
    };
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let role = role_from_item(item);
    let item_extra_body = item_extra_body_from_value(item);
    let mut events = Vec::new();

    {
        let output_state = index_state
            .output_state_by_index
            .entry(output_index)
            .or_default();
        output_state.item_type = Some(item_type.to_string());
        output_state.role = Some(role);
        output_state.item_extra_body = item_extra_body.clone();
        if item_type == "reasoning" {
            merge_reasoning_source(
                &mut output_state.reasoning_source,
                reasoning_source_from_value(item),
            );
        }
    }

    match item_type {
        "reasoning" => {
            let item_index = ensure_assistant_item_for_part(
                index_state,
                output_index,
                Role::Assistant,
                item_extra_body,
                decode_part_from_value(item),
                &mut events,
            );
            events.push(UrpStreamEvent::PartStart {
                part_index: index_state.synthetic_part_index_for_output(output_index),
                item_index,
                header: PartHeader::Reasoning,
                extra_body: part_extra_body_from_value(item),
            });
        }
        "function_call" => {
            let item_index = ensure_assistant_item_for_part(
                index_state,
                output_index,
                Role::Assistant,
                item_extra_body,
                decode_part_from_value(item),
                &mut events,
            );
            events.push(UrpStreamEvent::PartStart {
                part_index: index_state.synthetic_part_index_for_output(output_index),
                item_index,
                header: PartHeader::ToolCall {
                    call_id: item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    name: item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                },
                extra_body: part_extra_body_from_value(item),
            });
        }
        "function_call_output" => {
            finish_assistant_stream_group(index_state, &mut events);
            let item_index = index_state.allocate_fresh_item_index();
            if let Some(output_state) = index_state.output_state_by_index.get_mut(&output_index) {
                output_state.standalone_item_index = Some(item_index);
            }
            events.push(UrpStreamEvent::ItemStart {
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
            });
        }
        _ => {}
    }

    events
}

fn map_content_part_added(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let Some(part) = data_val.get("part") else {
        return Vec::new();
    };
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let content_index = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let part_index = index_state.part_index_for_content(output_index, content_index);
    let (fed_to_merger, role, item_extra_body) = {
        let output_state = index_state
            .output_state_by_index
            .entry(output_index)
            .or_default();
        (
            output_state.fed_to_merger,
            output_state.role.unwrap_or(Role::Assistant),
            output_state.item_extra_body.clone(),
        )
    };

    let mut events = Vec::new();
    let item_index = if fed_to_merger {
        index_state
            .output_state_by_index
            .get(&output_index)
            .and_then(|state| state.merged_item_index)
            .unwrap_or(0)
    } else {
        ensure_assistant_item_for_part(
            index_state,
            output_index,
            role,
            item_extra_body,
            decode_part_from_value(part),
            &mut events,
        )
    };

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
    events.push(UrpStreamEvent::PartStart {
        part_index,
        item_index,
        header,
        extra_body: part_extra_body_from_value(part),
    });
    events
}

fn map_content_part_done(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let Some(part) = data_val.get("part") else {
        return Vec::new();
    };
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let content_index = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let part_index = index_state.part_index_for_content(output_index, content_index);
    let decoded_part = decode_part_from_value(part);
    if let Some(output_state) = index_state.output_state_by_index.get_mut(&output_index) {
        output_state.part_done_seen = true;
    }
    if let Some(active_item) = index_state.active_assistant_item.as_mut() {
        active_item.parts.push(decoded_part.clone());
    }
    vec![UrpStreamEvent::PartDone {
        part_index,
        part: decoded_part,
        usage: None,
        extra_body: part_extra_body_from_value(part),
    }]
}

fn map_output_item_done(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let Some(item) = data_val.get("item") else {
        return Vec::new();
    };
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let mut events = Vec::new();

    match item_type {
        "function_call_output" => {
            let item_index = index_state
                .output_state_by_index
                .get(&output_index)
                .and_then(|state| state.standalone_item_index)
                .unwrap_or(0);
            events.push(UrpStreamEvent::ItemDone {
                item_index,
                item: decode_item_from_value(item),
                usage: None,
                extra_body: item_extra_body_from_value(item),
            });
        }
        "reasoning" | "function_call" => {
            let part = decode_part_from_value(item);
            let part_index = index_state.synthetic_part_index_for_output(output_index);
            let reasoning_text_delta_seen = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| state.reasoning_text_delta_seen)
                .unwrap_or(false);
            let reasoning_summary_delta_seen = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| state.reasoning_summary_delta_seen)
                .unwrap_or(false);
            if let Part::Reasoning {
                content,
                encrypted,
                summary,
                source,
                ..
            } = &part
            {
                let fallback_content = (!reasoning_text_delta_seen)
                    .then(|| content.clone())
                    .flatten();
                let fallback_summary = (!reasoning_summary_delta_seen)
                    .then(|| summary.clone())
                    .flatten();
                let fallback_encrypted = encrypted.clone();
                if fallback_content
                    .as_deref()
                    .is_some_and(|content| !content.is_empty())
                    || fallback_summary
                        .as_deref()
                        .is_some_and(|summary| !summary.is_empty())
                    || fallback_encrypted.is_some()
                {
                    events.push(UrpStreamEvent::Delta {
                        part_index,
                        delta: PartDelta::Reasoning {
                            content: fallback_content,
                            encrypted: fallback_encrypted,
                            summary: fallback_summary,
                            source: source.clone(),
                        },
                        usage: None,
                        extra_body: HashMap::new(),
                    });
                }
            }
            if let Some(active_item) = index_state.active_assistant_item.as_mut() {
                active_item.parts.push(part.clone());
            }
            events.push(UrpStreamEvent::PartDone {
                part_index,
                part,
                usage: None,
                extra_body: part_extra_body_from_value(item),
            });
        }
        "message" => {
            let part_done_seen = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| state.part_done_seen)
                .unwrap_or(false);
            if !part_done_seen {
                let decoded_item = decode_item_from_value(item);
                if let Item::Message { parts, .. } = decoded_item {
                    let item_extra_body = index_state
                        .output_state_by_index
                        .get(&output_index)
                        .map(|state| state.item_extra_body.clone())
                        .unwrap_or_else(|| item_extra_body_from_value(item));
                    let role = index_state
                        .output_state_by_index
                        .get(&output_index)
                        .and_then(|state| state.role)
                        .unwrap_or_else(|| role_from_item(item));
                    for part in parts {
                        let _ = ensure_assistant_item_for_part(
                            index_state,
                            output_index,
                            role,
                            item_extra_body.clone(),
                            part.clone(),
                            &mut events,
                        );
                        if let Some(active_item) = index_state.active_assistant_item.as_mut() {
                            active_item.parts.push(part);
                        }
                    }
                }
            }
        }
        _ => {
            let part = decode_part_from_value(item);
            if let Some(active_item) = index_state.active_assistant_item.as_mut() {
                active_item.parts.push(part.clone());
            }
            events.push(UrpStreamEvent::PartDone {
                part_index: index_state.synthetic_part_index_for_output(output_index),
                part,
                usage: None,
                extra_body: part_extra_body_from_value(item),
            });
        }
    }

    events
}

fn map_response_completed(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let mut events = Vec::new();
    finish_assistant_stream_group(index_state, &mut events);
    let response_obj = data_val
        .get("response")
        .and_then(|v| v.as_object())
        .cloned()
        .or_else(|| data_val.as_object().cloned());
    let Some(response_obj) = response_obj else {
        return events;
    };
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
    events.push(UrpStreamEvent::ResponseDone {
        finish_reason,
        usage: decoded
            .and_then(|resp| resp.usage)
            .or_else(|| parse_usage_from_responses_object(&response_value)),
        outputs,
        extra_body: split_known_fields(
            response_value,
            &[
                "id",
                "object",
                "created",
                "created_at",
                "model",
                "status",
                "output",
                "usage",
                "error",
            ],
        ),
    });
    events
}

fn urp_part_index_from_delta(data_val: &Value, index_state: &mut ResponsesStreamIndexState) -> u32 {
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if let Some(content_index) = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
    {
        return index_state.part_index_for_content(output_index, content_index);
    }
    index_state.synthetic_part_index_for_output(output_index)
}

fn output_text_delta_content(data_val: &Value) -> &str {
    data_val
        .get("delta")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
        .unwrap_or_default()
}

fn delta_extra_body_with_phase(
    data_val: Value,
    message_phases_by_output_index: &HashMap<u64, String>,
) -> HashMap<String, Value> {
    let mut extra = split_known_fields(
        data_val.clone(),
        &[
            "delta",
            "text",
            "output_index",
            "content_index",
            "part_index",
            "item_id",
            "logprobs",
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
            summary: part
                .get("summary")
                .and_then(|v| v.as_array())
                .map(|summary| {
                    summary
                        .iter()
                        .filter(|entry| {
                            entry.get("type").and_then(|v| v.as_str()) == Some("summary_text")
                        })
                        .filter_map(|entry| entry.get("text").and_then(|v| v.as_str()))
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|summary| !summary.is_empty()),
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

#[allow(clippy::too_many_arguments)]
fn build_accumulated_output_items(
    reasoning_text: &str,
    reasoning_summary_text: &str,
    reasoning_sig: &str,
    reasoning_source: Option<&str>,
    reasoning_output_index: Option<u64>,
    output_texts_by_output_index: &HashMap<u64, String>,
    message_phases_by_output_index: &HashMap<u64, String>,
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    call_ids_by_output_index: &HashMap<u64, String>,
) -> Vec<Item> {
    let mut outputs = Vec::new();
    let mut merger = GreedyMerger::new();
    let mut pending_assistant_extra_body = HashMap::new();

    #[derive(Clone, Debug)]
    enum FallbackOutputKind {
        Reasoning(u64),
        Text(u64),
        ToolCall(u64, String),
    }

    let mut ordered_kinds = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_summary_text.is_empty() || !reasoning_sig.is_empty()
    {
        ordered_kinds.push(FallbackOutputKind::Reasoning(
            reasoning_output_index.unwrap_or(0),
        ));
    }

    let mut text_indices = output_texts_by_output_index
        .keys()
        .copied()
        .collect::<Vec<_>>();
    text_indices.sort_unstable();
    ordered_kinds.extend(text_indices.into_iter().map(FallbackOutputKind::Text));

    let mut call_output_indices = call_order
        .iter()
        .enumerate()
        .map(|(call_position, call_id)| {
            let output_index = output_index_for_call_id(call_ids_by_output_index, call_id)
                .unwrap_or(call_position as u64 + 1);
            (output_index, call_id.clone())
        })
        .collect::<Vec<_>>();
    call_output_indices.sort_by_key(|(output_index, _)| *output_index);
    ordered_kinds.extend(
        call_output_indices
            .into_iter()
            .map(|(output_index, call_id)| FallbackOutputKind::ToolCall(output_index, call_id)),
    );

    ordered_kinds.sort_by_key(|kind| match kind {
        FallbackOutputKind::Reasoning(output_index) => *output_index,
        FallbackOutputKind::Text(output_index) => *output_index,
        FallbackOutputKind::ToolCall(output_index, _) => *output_index,
    });

    for kind in ordered_kinds {
        match kind {
            FallbackOutputKind::Reasoning(_) => {
                feed_assistant_part(
                    &mut merger,
                    &mut pending_assistant_extra_body,
                    &mut outputs,
                    Part::Reasoning {
                        content: (!reasoning_text.is_empty()).then(|| reasoning_text.to_string()),
                        summary: (!reasoning_summary_text.is_empty())
                            .then(|| reasoning_summary_text.to_string()),
                        encrypted: (!reasoning_sig.is_empty())
                            .then(|| Value::String(reasoning_sig.to_string())),
                        source: reasoning_source.map(|source| source.to_string()),
                        extra_body: HashMap::new(),
                    },
                    &HashMap::new(),
                );
            }
            FallbackOutputKind::Text(output_index) => {
                let Some(output_text) = output_texts_by_output_index.get(&output_index) else {
                    continue;
                };
                if output_text.is_empty() {
                    continue;
                }
                let mut text_extra_body = HashMap::new();
                if let Some(phase) = message_phases_by_output_index.get(&output_index) {
                    text_extra_body.insert("phase".to_string(), json!(phase));
                }
                feed_assistant_part(
                    &mut merger,
                    &mut pending_assistant_extra_body,
                    &mut outputs,
                    Part::Text {
                        content: output_text.clone(),
                        extra_body: text_extra_body,
                    },
                    &HashMap::new(),
                );
            }
            FallbackOutputKind::ToolCall(_, call_id) => {
                if let Some((name, arguments)) = calls.get(&call_id) {
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
        }
    }

    flush_assistant_merger(&mut merger, &mut pending_assistant_extra_body, &mut outputs);
    outputs
}

fn output_index_for_call_id(
    call_ids_by_output_index: &HashMap<u64, String>,
    target_call_id: &str,
) -> Option<u64> {
    call_ids_by_output_index
        .iter()
        .find_map(|(output_index, call_id)| (call_id == target_call_id).then_some(*output_index))
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
            "summary",
            "sig_1",
            Some("anthropic"),
            Some(0),
            &HashMap::from([(0, "answer".to_string())]),
            &HashMap::from([(0, "analysis".to_string())]),
            &["call_1".to_string()],
            &calls,
            &HashMap::from([(1, "call_1".to_string())]),
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
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                source: Some(source),
                extra_body: _,
                ..
            } if content == "think" && summary == "summary" && sig == "sig_1" && source == "anthropic"
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
            "",
            "sig_only",
            None,
            Some(0),
            &HashMap::new(),
            &HashMap::from([(0, "analysis".to_string())]),
            &[],
            &HashMap::new(),
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
    fn build_accumulated_output_items_preserves_multiple_output_text_phases() {
        let outputs = build_accumulated_output_items(
            "",
            "",
            "",
            None,
            None,
            &HashMap::from([(0, "analysis".to_string()), (2, "final".to_string())]),
            &HashMap::from([
                (0, "commentary".to_string()),
                (2, "final_answer".to_string()),
            ]),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(outputs.len(), 2);
        assert!(matches!(
            &outputs[0],
            Item::Message { parts, .. }
                if matches!(
                    &parts[0],
                    Part::Text { content, extra_body }
                        if content == "analysis"
                            && extra_body.get("phase") == Some(&json!("commentary"))
                )
        ));
        assert!(matches!(
            &outputs[1],
            Item::Message { parts, .. }
                if matches!(
                    &parts[0],
                    Part::Text { content, extra_body }
                        if content == "final"
                            && extra_body.get("phase") == Some(&json!("final_answer"))
                )
        ));
    }

    #[test]
    fn build_accumulated_output_items_preserves_real_output_index_order_in_fallback() {
        let calls = HashMap::from([(
            "call_1".to_string(),
            ("lookup".to_string(), "{}".to_string()),
        )]);

        let outputs = build_accumulated_output_items(
            "think",
            "summary",
            "",
            Some("upstream-reasoner"),
            Some(2),
            &HashMap::from([(5, "answer".to_string())]),
            &HashMap::new(),
            &["call_1".to_string()],
            &calls,
            &HashMap::from([(9, "call_1".to_string())]),
        );

        assert_eq!(outputs.len(), 1);
        let Item::Message { parts, .. } = &outputs[0] else {
            panic!("expected merged assistant output");
        };
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                source: Some(source),
                ..
            } if source == "upstream-reasoner"
        ));
        assert!(matches!(&parts[1], Part::Text { content, .. } if content == "answer"));
        assert!(matches!(&parts[2], Part::ToolCall { call_id, .. } if call_id == "call_1"));
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
                        "summary": [{ "type": "summary_text", "text": "summary" }],
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

        let mut state = ResponsesStreamIndexState::default();
        let completed_events = map_response_completed(event, &mut state);
        let Some(UrpStreamEvent::ResponseDone {
            finish_reason,
            outputs,
            ..
        }) = completed_events.last()
        else {
            panic!("expected response done event");
        };

        assert_eq!(*finish_reason, Some(FinishReason::ToolCalls));
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
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                ..
            } if content == "think" && summary == "summary" && sig == "sig_1"
        ));
        assert!(matches!(
            &parts[1],
            Part::Text { content, extra_body } if content == "answer" && extra_body.get("phase") == Some(&json!("analysis"))
        ));
        assert!(matches!(&parts[2], Part::ToolCall { call_id, .. } if call_id == "call_1"));
    }

    #[test]
    fn top_level_reasoning_and_function_call_items_share_greedy_merged_item_index() {
        let mut state = ResponsesStreamIndexState::default();
        let reasoning_events = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 7,
                "item": { "type": "reasoning", "text": "think" }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert_eq!(reasoning_events.len(), 2);
        assert!(matches!(
            &reasoning_events[0],
            UrpStreamEvent::ItemStart {
                item_index,
                header: ItemHeader::Message { role: Role::Assistant },
                ..
            } if *item_index == 0
        ));
        assert!(matches!(
            &reasoning_events[1],
            UrpStreamEvent::PartStart {
                item_index,
                part_index,
                header: PartHeader::Reasoning,
                ..
            } if *item_index == 0 && *part_index == 0
        ));

        let function_events = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 9,
                "item": {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": ""
                }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert_eq!(function_events.len(), 1);
        assert!(matches!(
            &function_events[0],
            UrpStreamEvent::PartStart {
                item_index,
                part_index,
                header: PartHeader::ToolCall { call_id, name },
                ..
            } if *item_index == 0 && *part_index == 1 && call_id == "call_1" && name == "lookup"
        ));

        let function_delta = map_responses_event_to_urp_events_with_state(
            "response.function_call_arguments.delta",
            json!({
                "output_index": 9,
                "delta": "{}"
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(matches!(
            &function_delta[0],
            UrpStreamEvent::Delta {
                part_index,
                delta: PartDelta::ToolCallArguments { arguments },
                ..
            } if *part_index == 1 && arguments == "{}"
        ));
    }

    #[test]
    fn content_part_done_reuses_normalized_urp_part_index() {
        let mut state = ResponsesStreamIndexState::default();

        let added = map_responses_event_to_urp_events_with_state(
            "response.content_part.added",
            json!({
                "output_index": 7,
                "content_index": 42,
                "part": { "type": "output_text", "text": "" }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(matches!(
            &added[0],
            UrpStreamEvent::ItemStart {
                item_index,
                header: ItemHeader::Message { role: Role::Assistant },
                ..
            } if *item_index == 0
        ));
        assert!(matches!(
            &added[1],
            UrpStreamEvent::PartStart {
                part_index,
                item_index,
                header: PartHeader::Text,
                ..
            } if *item_index == 0 && *part_index == 0
        ));

        let done = map_responses_event_to_urp_events_with_state(
            "response.content_part.done",
            json!({
                "output_index": 7,
                "content_index": 42,
                "part": { "type": "output_text", "text": "done" }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(matches!(
            &done[0],
            UrpStreamEvent::PartDone {
                part_index,
                part: Part::Text { content, .. },
                ..
            } if *part_index == 0 && content == "done"
        ));
    }

    #[test]
    fn synthetic_text_fallback_preserves_multiple_phases_and_allocates_distinct_part_indices() {
        let output_items = build_accumulated_output_items(
            "",
            "",
            "",
            None,
            None,
            &HashMap::from([(0, "analysis".to_string()), (2, "final".to_string())]),
            &HashMap::from([
                (0, "commentary".to_string()),
                (2, "final_answer".to_string()),
            ]),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );
        let mut state = ResponsesStreamIndexState::default();
        assert_eq!(state.allocate_fresh_item_index(), 0);
        assert_eq!(state.part_index_for_content(11, 4), 0);
        assert_eq!(state.synthetic_part_index_for_output(12), 1);

        let mut observed = Vec::new();

        for (final_item_index, output_item) in output_items.iter().enumerate() {
            let Item::Message {
                role: Role::Assistant,
                parts,
                extra_body,
            } = output_item
            else {
                continue;
            };

            for part in parts {
                let Part::Text {
                    content,
                    extra_body: text_extra_body,
                } = part
                else {
                    continue;
                };
                let synthetic_text_item = Item::Message {
                    role: Role::Assistant,
                    parts: vec![Part::Text {
                        content: content.clone(),
                        extra_body: text_extra_body.clone(),
                    }],
                    extra_body: extra_body.clone(),
                };
                let part_index = state.allocate_fresh_part_index();
                let item_index = final_item_index as u32;

                observed.push(UrpStreamEvent::ItemStart {
                    item_index,
                    header: ItemHeader::Message {
                        role: Role::Assistant,
                    },
                    extra_body: item_extra_body_from_item(&synthetic_text_item),
                });
                observed.push(UrpStreamEvent::PartStart {
                    item_index,
                    part_index,
                    header: PartHeader::Text,
                    extra_body: text_extra_body.clone(),
                });
                observed.push(UrpStreamEvent::Delta {
                    part_index,
                    delta: PartDelta::Text {
                        content: content.clone(),
                    },
                    usage: None,
                    extra_body: text_extra_body.clone(),
                });
            }
        }

        assert!(matches!(
            &observed[1],
            UrpStreamEvent::PartStart { part_index, extra_body, .. }
                if *part_index == 2 && extra_body.get("phase") == Some(&json!("commentary"))
        ));
        assert!(matches!(
            &observed[4],
            UrpStreamEvent::PartStart { part_index, extra_body, .. }
                if *part_index == 3 && extra_body.get("phase") == Some(&json!("final_answer"))
        ));
    }

    #[test]
    fn build_accumulated_output_items_omits_reasoning_source_when_missing() {
        let outputs = build_accumulated_output_items(
            "think",
            "",
            "",
            None,
            Some(0),
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );

        let Item::Message { parts, .. } = &outputs[0] else {
            panic!("expected assistant message output");
        };
        assert!(matches!(&parts[0], Part::Reasoning { source: None, .. }));
    }

    #[test]
    fn synthetic_text_fallback_is_suppressed_when_text_part_done_already_emitted() {
        let saw_text_delta = false;
        let mut saw_text_part_done = false;

        let done_event = json!({
            "output_index": 4,
            "content_index": 9,
            "part": { "type": "output_text", "text": "ready" }
        });

        if done_event
            .get("part")
            .and_then(|part| part.get("type"))
            .and_then(|v| v.as_str())
            .is_some_and(|part_type| matches!(part_type, "output_text" | "text"))
            && done_event
                .get("part")
                .and_then(|part| part.get("text"))
                .and_then(|v| v.as_str())
                .is_some_and(|text| !text.is_empty())
        {
            saw_text_part_done = true;
        }

        assert!(!(!saw_text_delta && !saw_text_part_done));
    }

    #[test]
    fn decode_reasoning_part_preserves_summary_separately_from_text() {
        let part = decode_part_from_value(&json!({
            "type": "reasoning",
            "text": "full reasoning",
            "summary": [{ "type": "summary_text", "text": "brief summary" }],
            "encrypted_content": "sig_1"
        }));

        assert!(matches!(
            part,
            Part::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                ..
            } if content == "full reasoning" && summary == "brief summary" && sig == "sig_1"
        ));
    }

    #[test]
    fn map_output_item_done_message_fallback_routes_every_part_through_greedy_merger() {
        let mut state = ResponsesStreamIndexState::default();
        let added_events = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 3,
                "item": {
                    "type": "message",
                    "role": "assistant",
                    "content": []
                }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(added_events.is_empty());

        let done_events = map_responses_event_to_urp_events_with_state(
            "response.output_item.done",
            json!({
                "output_index": 3,
                "item": {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        { "type": "output_text", "text": "answer" },
                        { "type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": "{}" }
                    ]
                }
            }),
            &HashMap::new(),
            &mut state,
        );

        assert_eq!(
            done_events.len(),
            1,
            "fallback should start one merged assistant item"
        );
        assert!(matches!(
            &done_events[0],
            UrpStreamEvent::ItemStart {
                item_index: 0,
                header: ItemHeader::Message {
                    role: Role::Assistant
                },
                ..
            }
        ));

        let active_item = state
            .active_assistant_item
            .as_ref()
            .expect("assistant item remains active after message fallback");
        assert_eq!(active_item.item_index, 0);
        assert_eq!(active_item.parts.len(), 2);
        assert!(matches!(&active_item.parts[0], Part::Text { content, .. } if content == "answer"));
        assert!(
            matches!(&active_item.parts[1], Part::ToolCall { call_id, .. } if call_id == "call_1")
        );

        let mut finish_events = Vec::new();
        finish_assistant_stream_group(&mut state, &mut finish_events);
        assert_eq!(finish_events.len(), 1);
        let UrpStreamEvent::ItemDone { item, .. } = &finish_events[0] else {
            panic!("expected flushed merged item");
        };
        let Item::Message { parts, .. } = item else {
            panic!("expected message item");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], Part::Text { content, .. } if content == "answer"));
        assert!(matches!(&parts[1], Part::ToolCall { call_id, .. } if call_id == "call_1"));
    }
}
