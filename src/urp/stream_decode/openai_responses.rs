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
    FinishReason, Item, ItemHeader, Node, NodeDelta, NodeHeader, OrdinaryRole, Part, PartDelta,
    PartHeader, Role, UrpStreamEvent, nodes_to_items,
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
    mut pending_request_envelope_extra: Option<HashMap<String, Value>>,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_texts_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut message_phases_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut message_item_extra_by_output_index: HashMap<u64, HashMap<String, Value>> =
        HashMap::new();
    let mut item_ids_by_output_index: HashMap<u64, String> = HashMap::new();
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
                if !message_item_extra_by_output_index.contains_key(&output_index) {
                    if let Some(extra_body) = pending_request_envelope_extra.take() {
                        output_state_for(&mut index_state, output_index).item_extra_body = extra_body.clone();
                        message_item_extra_by_output_index.insert(output_index, extra_body);
                    }
                }
                saw_text_delta = true;
            }
        }
        if ev.event == "response.reasoning.delta" {
            tracing::info!(
                target: "monoize::urp::reasoning_trace",
                event = %ev.event,
                output_index = data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0),
                item_id = data_val.get("item_id").and_then(|v| v.as_str()).unwrap_or(""),
                has_text = data_val.get("delta").and_then(|v| v.as_str()).is_some_and(|v| !v.is_empty()),
                "responses reasoning delta observed"
            );
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
            tracing::info!(
                target: "monoize::urp::reasoning_trace",
                event = %ev.event,
                output_index = data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0),
                item_id = data_val.get("item_id").and_then(|v| v.as_str()).unwrap_or(""),
                has_text = data_val.get("delta").and_then(|v| v.as_str()).is_some_and(|v| !v.is_empty()),
                "responses reasoning summary delta observed"
            );
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                reasoning_summary_text.push_str(delta);
            }
            if let (Some(idx), Some(id)) = (
                data_val.get("output_index").and_then(|v| v.as_u64()),
                data_val.get("item_id").and_then(|v| v.as_str()),
            ) {
                if !id.is_empty() {
                    item_ids_by_output_index.insert(idx, id.to_string());
                }
            }
        }
        if ev.event == "response.output_item.added" {
            let item = data_val.get("item").unwrap_or(&data_val);
            if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                tracing::info!(
                    target: "monoize::urp::reasoning_trace",
                    event = %ev.event,
                    output_index = data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0),
                    item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    encrypted_len = item
                        .get("encrypted_content")
                        .and_then(|v| v.as_str())
                        .map(|v| v.len())
                        .unwrap_or(0),
                    "responses reasoning output item added"
                );
            }
            if let (Some(idx), Some(id)) = (
                data_val.get("output_index").and_then(|v| v.as_u64()),
                item.get("id").and_then(|v| v.as_str()),
            ) {
                if !id.is_empty() {
                    item_ids_by_output_index
                        .entry(idx)
                        .or_insert_with(|| id.to_string());
                }
            }
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
                    if !message_item_extra_by_output_index.contains_key(&idx) {
                        if let Some(extra_body) = pending_request_envelope_extra.take() {
                            output_state_for(&mut index_state, idx).item_extra_body = extra_body.clone();
                            message_item_extra_by_output_index.insert(idx, extra_body);
                        }
                    }
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
            if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                tracing::info!(
                    target: "monoize::urp::reasoning_trace",
                    event = %ev.event,
                    output_index = data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0),
                    item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    encrypted_len = item
                        .get("encrypted_content")
                        .and_then(|v| v.as_str())
                        .map(|v| v.len())
                        .unwrap_or(0),
                    text_len = item.get("text").and_then(|v| v.as_str()).map(|v| v.len()).unwrap_or(0),
                    "responses reasoning output item done"
                );
            }
            if let (Some(idx), Some(id)) = (
                data_val.get("output_index").and_then(|v| v.as_u64()),
                item.get("id").and_then(|v| v.as_str()),
            ) {
                if !id.is_empty() {
                    item_ids_by_output_index
                        .entry(idx)
                        .or_insert_with(|| id.to_string());
                }
            }
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
                if !text.is_empty() {
                    reasoning_text = text;
                }
                if !summary.is_empty() {
                    reasoning_summary_text = summary;
                }
                if !sig.is_empty() {
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

        let stream_events = if ev.event == "response.completed" {
            let accumulated_output_nodes = build_accumulated_output_nodes(
                &reasoning_text,
                &reasoning_summary_text,
                &reasoning_sig,
                reasoning_source.as_deref(),
                reasoning_output_index,
                &output_texts_by_output_index,
                &message_phases_by_output_index,
                &message_item_extra_by_output_index,
                &item_ids_by_output_index,
                &call_order,
                &calls,
                &call_ids_by_output_index,
            );
            map_response_completed_with_accumulated(
                data_val,
                &mut index_state,
                &accumulated_output_nodes,
            )
        } else {
            map_responses_event_to_urp_events_with_state(
                &ev.event,
                data_val,
                &message_phases_by_output_index,
                &mut index_state,
            )
        };
        for stream_event in stream_events {
            response_done_sent |= matches!(stream_event, UrpStreamEvent::ResponseDone { .. });
            let _ = tx.send(stream_event).await;
        }
    }

    if !response_done_sent {
        let output_nodes = build_accumulated_output_nodes(
            &reasoning_text,
            &reasoning_summary_text,
            &reasoning_sig,
            reasoning_source.as_deref(),
            reasoning_output_index,
            &output_texts_by_output_index,
            &message_phases_by_output_index,
            &message_item_extra_by_output_index,
            &item_ids_by_output_index,
            &call_order,
            &calls,
            &call_ids_by_output_index,
        );
        let output_items = nodes_to_items(&output_nodes);
        for (output_index, output_item) in output_items.iter().enumerate() {
            let output_state = output_state_for(&mut index_state, output_index as u64);
            if output_state.item_extra_body.is_empty() {
                output_state.item_extra_body = item_extra_body_from_item(output_item);
            }
            if output_state.role.is_none() {
                output_state.role = Some(match output_item {
                    Item::Message { role, .. } => *role,
                    Item::ToolResult { .. } => Role::Tool,
                });
            }
        }
        let final_usage = latest_stream_usage_snapshot(&runtime_metrics).await;

        if !saw_text_delta && !saw_text_part_done {
            let mut grouped_output_nodes = Vec::new();
            for output_item in &output_items {
                grouped_output_nodes.push(match output_item {
                    Item::Message { id, role, parts, extra_body } => {
                        let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
                        let mut item_nodes = Vec::new();
                        for (index, part) in parts.iter().cloned().enumerate() {
                            let mut node = part.into_node(ordinary_role);
                            if index == 0 && !extra_body.is_empty() {
                                node.extra_body_mut().extend(extra_body.clone());
                            }
                            if index == 0 {
                                node.set_id(id.clone());
                            }
                            item_nodes.push(node);
                        }
                        item_nodes
                    }
                    Item::ToolResult {
                        id,
                        call_id,
                        is_error,
                        content,
                        extra_body,
                    } => vec![Node::ToolResult {
                        id: id.clone(),
                        call_id: call_id.clone(),
                        is_error: *is_error,
                        content: content.clone(),
                        extra_body: extra_body.clone(),
                    }],
                });
            }

            for (output_item, item_nodes) in output_items.iter().zip(grouped_output_nodes.iter()) {
                let Item::Message { role: Role::Assistant, .. } = output_item
                else {
                    continue;
                };

                for node in item_nodes.iter().filter(|node| {
                    matches!(
                        node,
                        Node::Text {
                            role: OrdinaryRole::Assistant,
                            ..
                        }
                    )
                }) {
                    let Node::Text {
                        id,
                        role: OrdinaryRole::Assistant,
                        content,
                        phase,
                        extra_body,
                    } = node.clone()
                    else {
                        continue;
                    };
                    let node_index = index_state.allocate_fresh_node_index();
                    let synthetic_node = Node::Text {
                        id,
                        role: OrdinaryRole::Assistant,
                        content: content.clone(),
                        phase: phase.clone(),
                        extra_body: extra_body.clone(),
                    };
                    let item_extra = item_extra_body_from_item(output_item);
                    if !item_extra.is_empty() {
                        let _ = tx
                            .send(UrpStreamEvent::NodeStart {
                                node_index: index_state.allocate_fresh_node_index(),
                                header: NodeHeader::NextDownstreamEnvelopeExtra,
                                extra_body: item_extra.clone(),
                            })
                            .await;
                        let _ = tx
                            .send(UrpStreamEvent::NodeDone {
                                node_index: index_state.allocate_fresh_node_index() - 1,
                                node: Node::NextDownstreamEnvelopeExtra {
                                    extra_body: item_extra.clone(),
                                },
                                usage: final_usage.clone(),
                                extra_body: item_extra.clone(),
                            })
                            .await;
                    }

                    let _ = tx
                        .send(UrpStreamEvent::NodeStart {
                            node_index,
                            header: NodeHeader::Text {
                                id: synthetic_node.id().cloned(),
                                role: OrdinaryRole::Assistant,
                                phase,
                            },
                            extra_body: extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::NodeDelta {
                            node_index,
                            delta: NodeDelta::Text {
                                content: content.clone(),
                            },
                            usage: final_usage.clone(),
                            extra_body: extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::NodeDone {
                            node_index,
                            node: synthetic_node,
                            usage: final_usage.clone(),
                            extra_body: extra_body.clone(),
                        })
                        .await;
                    let bridge_part = Part::Text {
                        content: content.clone(),
                        extra_body: extra_body.clone(),
                    };
                    let item_index = index_state.allocate_fresh_item_index();
                    let _ = tx
                        .send(UrpStreamEvent::ItemStart {
                            item_index,
                            header: ItemHeader::Message {
                                id: output_item_message_id(output_item),
                                role: Role::Assistant,
                            },
                            extra_body: item_extra_body_from_item(output_item),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::PartStart {
                            item_index,
                            part_index: node_index,
                            header: PartHeader::Text,
                            extra_body: extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::Delta {
                            part_index: node_index,
                            delta: PartDelta::Text {
                                content: content.clone(),
                            },
                            usage: final_usage.clone(),
                            extra_body: extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::PartDone {
                            part_index: node_index,
                            part: bridge_part.clone(),
                            usage: final_usage.clone(),
                            extra_body: extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::ItemDone {
                            item_index,
                            item: Item::Message {
                                id: output_item_message_id(output_item),
                                role: Role::Assistant,
                                parts: vec![bridge_part],
                                extra_body: item_extra_body_from_item(output_item),
                            },
                            usage: final_usage.clone(),
                            extra_body: HashMap::new(),
                        })
                        .await;
                }
            }
        }

        let _ = tx
            .send(UrpStreamEvent::ResponseDone { finish_reason: Some(if outputs_have_tool_calls(&output_nodes) {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            }), usage: final_usage, output: output_nodes, extra_body: HashMap::from([
                ("id".to_string(), json!(response_id)),
                ("object".to_string(), json!("response")),
                ("created_at".to_string(), json!(created)),
                ("model".to_string(), json!(urp.model.clone())),
                ("status".to_string(), json!("completed")),
            ]) })
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
        "response.output_text.delta" => {
            let mut extra = delta_extra_body_with_phase(data_val.clone(), message_phases_by_output_index);
            let mut events = Vec::new();
            let output_index = data_val
                .get("output_index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if let Some(item_extra_body) = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| state.item_extra_body.clone())
                .filter(|extra_body| !extra_body.is_empty())
            {
                for (key, value) in item_extra_body {
                    extra.entry(key).or_insert(value);
                }
            }
            let content_index = data_val
                .get("content_index")
                .or_else(|| data_val.get("part_index"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let node_index = index_state.node_index_for_content(output_index, content_index);
            let should_emit_start = {
                let output_state = output_state_for(index_state, output_index);
                !output_state.item_start_sent && !output_state.emitted_any_node
            };
            if should_emit_start {
                let output_state = output_state_for(index_state, output_index);
                if output_state.item_extra_body.is_empty() {
                    output_state.item_extra_body = extra.clone();
                }
                emit_node_start_with_bridge(
                    output_index,
                    node_index,
                    &Node::Text {
                        id: output_state_for(index_state, output_index)
                            .item_id
                            .clone()
                            .or_else(|| Some(crate::urp::synthetic_message_id())),
                        role: output_state_for(index_state, output_index)
                            .role
                            .unwrap_or(Role::Assistant)
                            .to_ordinary()
                            .unwrap_or(OrdinaryRole::Assistant),
                        content: String::new(),
                        phase: extra
                            .get("phase")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        extra_body: extra.clone(),
                    },
                    extra.clone(),
                    index_state,
                    &mut events,
                );
            }
            emit_node_delta_with_bridge(
                node_index,
                NodeDelta::Text {
                    content: output_text_delta_content(&data_val).to_string(),
                },
                extra,
                &mut events,
            );
            events
        }
        "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
            let (reasoning_source, reasoning_item_id) = data_val
                .get("output_index")
                .and_then(|v| v.as_u64())
                .map(|output_index| {
                    let output_state = index_state
                        .output_state_by_index
                        .entry(output_index)
                        .or_default();
                    if let Some(item_id) = data_val.get("item_id").and_then(|v| v.as_str())
                        && !item_id.is_empty()
                    {
                        output_state.item_id = Some(item_id.to_string());
                    }
                    merge_reasoning_source(
                        &mut output_state.reasoning_source,
                        reasoning_source_from_value(&data_val),
                    );
                    if event_name == "response.reasoning_summary_text.delta" {
                        output_state.reasoning_summary_delta_seen = true;
                    } else {
                        output_state.reasoning_text_delta_seen = true;
                    }
                    (
                        output_state.reasoning_source.clone(),
                        output_state.item_id.clone(),
                    )
                })
                .unwrap_or_default();
            let mut extra_body = split_known_fields(
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
            if let Some(id) = reasoning_item_id {
                extra_body.insert(
                    "reasoning_item_id".to_string(),
                    Value::String(id),
                );
            }
            let mut events = Vec::new();
            emit_node_delta_with_bridge(
                urp_node_index_from_delta(&data_val, index_state),
                node_delta_from_reasoning_event(event_name, &data_val, reasoning_source),
                extra_body,
                &mut events,
            );
            events
        }
        "response.reasoning.done" => {
            let node_index = urp_node_index_from_delta(&data_val, index_state);
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
            let mut events = Vec::new();
            emit_node_done_with_bridge(
                data_val
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                node_index,
                Node::Reasoning {
                    id: output_state_for(index_state, data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0))
                        .item_id
                        .clone()
                        .or_else(|| Some(crate::urp::synthetic_reasoning_id())),
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
                HashMap::new(),
                index_state,
                &mut events,
            );
            events
        }
        "response.function_call_arguments.delta" => {
            let mut events = Vec::new();
            emit_node_delta_with_bridge(
                urp_node_index_from_delta(&data_val, index_state),
                NodeDelta::ToolCallArguments {
                    arguments: data_val
                        .get("delta")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                },
                split_known_fields(
                    data_val,
                    &["delta", "output_index", "content_index", "part_index"],
                ),
                &mut events,
            );
            events
        }
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
    next_node_index: u32,
    node_index_by_content_key: HashMap<(u64, u64), u32>,
    synthetic_node_index_by_output_index: HashMap<u64, u32>,
    output_state_by_index: HashMap<u64, OutputItemStreamState>,
}

impl ResponsesStreamIndexState {
    fn allocate_fresh_item_index(&mut self) -> u32 {
        let next = self.next_item_index;
        self.next_item_index += 1;
        next
    }

    fn node_index_for_content(&mut self, output_index: u64, content_index: u64) -> u32 {
        *self
            .node_index_by_content_key
            .entry((output_index, content_index))
            .or_insert_with(|| {
                let next = self.next_node_index;
                self.next_node_index += 1;
                next
            })
    }

    fn synthetic_node_index_for_output(&mut self, output_index: u64) -> u32 {
        *self
            .synthetic_node_index_by_output_index
            .entry(output_index)
            .or_insert_with(|| {
                let next = self.next_node_index;
                self.next_node_index += 1;
                next
            })
    }

    fn allocate_fresh_node_index(&mut self) -> u32 {
        let next = self.next_node_index;
        self.next_node_index += 1;
        next
    }
}

#[derive(Debug, Clone, Default)]
struct OutputItemStreamState {
    bridge_item_index: Option<u32>,
    item_type: Option<String>,
    item_id: Option<String>,
    role: Option<Role>,
    item_extra_body: HashMap<String, Value>,
    item_start_sent: bool,
    emitted_any_node: bool,
    control_emitted: bool,
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

fn output_state_for<'a>(
    index_state: &'a mut ResponsesStreamIndexState,
    output_index: u64,
) -> &'a mut OutputItemStreamState {
    index_state.output_state_by_index.entry(output_index).or_default()
}

fn output_role_to_ordinary(role: Role) -> OrdinaryRole {
    role.to_ordinary().unwrap_or(OrdinaryRole::Assistant)
}

fn first_node_from_item_value(item: &Value) -> Option<Node> {
    nodes_from_item_value(item).into_iter().next()
}

fn nodes_from_item_value(item: &Value) -> Vec<Node> {
    match decode_item_from_value(item) {
        Item::Message {
            id,
            role,
            parts,
            extra_body,
        } => {
            let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
            let mut nodes = Vec::new();
            for (index, part) in parts.into_iter().enumerate() {
                let mut node = part.into_node(ordinary_role);
                if index == 0 && !extra_body.is_empty() {
                    node.extra_body_mut().extend(extra_body.clone());
                }
                if index == 0 {
                    node.set_id(id.clone());
                }
                nodes.push(node);
            }
            nodes
        }
        Item::ToolResult {
            id,
            call_id,
            is_error,
            content,
            extra_body,
        } => vec![Node::ToolResult {
            id,
            call_id,
            is_error,
            content,
            extra_body,
        }],
    }
}

fn node_from_part_value(part: &Value, role: Role, item_id: Option<String>) -> Node {
    let mut node = decode_part_from_value(part).into_node(output_role_to_ordinary(role));
    node.set_id(item_id);
    node
}

fn node_header_from_node(node: &Node) -> NodeHeader {
    match node {
        Node::Text { id, role, phase, .. } => NodeHeader::Text {
            id: id.clone(),
            role: *role,
            phase: phase.clone(),
        },
        Node::Image { id, role, .. } => NodeHeader::Image { id: id.clone(), role: *role },
        Node::Audio { id, role, .. } => NodeHeader::Audio { id: id.clone(), role: *role },
        Node::File { id, role, .. } => NodeHeader::File { id: id.clone(), role: *role },
        Node::Refusal { id, .. } => NodeHeader::Refusal { id: id.clone() },
        Node::Reasoning { id, .. } => NodeHeader::Reasoning { id: id.clone() },
        Node::ToolCall { id, call_id, name, .. } => NodeHeader::ToolCall {
            id: id.clone(),
            call_id: call_id.clone(),
            name: name.clone(),
        },
        Node::ProviderItem { id, role, item_type, .. } => NodeHeader::ProviderItem {
            id: id.clone(),
            role: *role,
            item_type: item_type.clone(),
        },
        Node::ToolResult { id, call_id, .. } => NodeHeader::ToolResult {
            id: id.clone(),
            call_id: call_id.clone(),
        },
        Node::NextDownstreamEnvelopeExtra { .. } => NodeHeader::NextDownstreamEnvelopeExtra,
    }
}

fn part_header_from_node(node: &Node) -> Option<PartHeader> {
    match node {
        Node::Text { .. } => Some(PartHeader::Text),
        Node::Image { extra_body, .. } => Some(PartHeader::Image {
            extra_body: extra_body.clone(),
        }),
        Node::Audio { extra_body, .. } => Some(PartHeader::Audio {
            extra_body: extra_body.clone(),
        }),
        Node::File { extra_body, .. } => Some(PartHeader::File {
            extra_body: extra_body.clone(),
        }),
        Node::Refusal { .. } => Some(PartHeader::Refusal),
        Node::Reasoning { id, .. } => Some(PartHeader::Reasoning { id: id.clone() }),
        Node::ToolCall { id, call_id, name, .. } => Some(PartHeader::ToolCall {
            id: id.clone(),
            call_id: call_id.clone(),
            name: name.clone(),
        }),
        Node::ProviderItem {
            id, item_type, body, ..
        } => Some(PartHeader::ProviderItem {
            id: id.clone(),
            item_type: item_type.clone(),
            body: body.clone(),
        }),
        Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

fn bridge_part_from_node(node: &Node) -> Option<Part> {
    match node {
        Node::Text { content, extra_body, .. } => Some(Part::Text {
            content: content.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::Image { source, extra_body, .. } => Some(Part::Image {
            source: source.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::Audio { source, extra_body, .. } => Some(Part::Audio {
            source: source.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::File { source, extra_body, .. } => Some(Part::File {
            source: source.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::Refusal { content, extra_body, .. } => Some(Part::Refusal {
            content: content.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::Reasoning {
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => Some(Part::Reasoning {
            id: id.clone().or_else(|| Some(crate::urp::synthetic_reasoning_id())),
            content: content.clone(),
            encrypted: encrypted.clone(),
            summary: summary.clone(),
            source: source.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::ToolCall {
            id,
            call_id,
            name,
            arguments,
            extra_body,
        } => Some(Part::ToolCall {
            id: id.clone().or_else(|| Some(crate::urp::synthetic_tool_call_id())),
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::ProviderItem {
            item_type,
            body,
            extra_body,
            ..
        } => Some(Part::ProviderItem {
            id: node.id().cloned().or_else(|| Some(crate::urp::synthetic_provider_item_id())),
            item_type: item_type.clone(),
            body: body.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

fn node_delta_from_reasoning_event(event_name: &str, data_val: &Value, source: Option<String>) -> NodeDelta {
    NodeDelta::Reasoning {
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
        source,
    }
}

fn part_delta_from_node_delta(delta: &NodeDelta) -> Option<PartDelta> {
    match delta {
        NodeDelta::Text { content } => Some(PartDelta::Text {
            content: content.clone(),
        }),
        NodeDelta::Reasoning {
            content,
            encrypted,
            summary,
            source,
        } => Some(PartDelta::Reasoning {
            content: content.clone(),
            encrypted: encrypted.clone(),
            summary: summary.clone(),
            source: source.clone(),
        }),
        NodeDelta::Refusal { content } => Some(PartDelta::Refusal {
            content: content.clone(),
        }),
        NodeDelta::ToolCallArguments { arguments } => Some(PartDelta::ToolCallArguments {
            arguments: arguments.clone(),
        }),
        NodeDelta::Image { source } => Some(PartDelta::Image {
            source: source.clone(),
        }),
        NodeDelta::Audio { source } => Some(PartDelta::Audio {
            source: source.clone(),
        }),
        NodeDelta::File { source } => Some(PartDelta::File {
            source: source.clone(),
        }),
        NodeDelta::ProviderItem { data } => Some(PartDelta::ProviderItem { data: data.clone() }),
    }
}

fn ensure_bridge_item_start(
    output_index: u64,
    index_state: &mut ResponsesStreamIndexState,
    events: &mut Vec<UrpStreamEvent>,
) -> u32 {
    let (existing_index, already_sent, item_type, role, item_extra_body, item_id) = {
        let output_state = output_state_for(index_state, output_index);
        (
            output_state.bridge_item_index,
            output_state.item_start_sent,
            output_state.item_type.clone(),
            output_state.role.unwrap_or(Role::Assistant),
            output_state.item_extra_body.clone(),
            output_state.item_id.clone(),
        )
    };

    let item_index = existing_index.unwrap_or_else(|| index_state.allocate_fresh_item_index());

    if !already_sent {
        let header = if item_type.as_deref() == Some("function_call_output") {
            ItemHeader::ToolResult {
                id: item_id.clone(),
                call_id: item_id.unwrap_or_default(),
            }
        } else {
            ItemHeader::Message {
                id: item_id.clone(),
                role,
            }
        };
        events.push(UrpStreamEvent::ItemStart {
            item_index,
            header,
            extra_body: item_extra_body,
        });
        let output_state = output_state_for(index_state, output_index);
        output_state.item_start_sent = true;
        output_state.bridge_item_index = Some(item_index);
    } else if existing_index.is_none() {
        output_state_for(index_state, output_index).bridge_item_index = Some(item_index);
    }

    item_index
}

fn emit_pending_envelope_control_if_needed(
    output_index: u64,
    index_state: &mut ResponsesStreamIndexState,
    events: &mut Vec<UrpStreamEvent>,
) {
    let (should_emit, extra_body) = {
        let output_state = output_state_for(index_state, output_index);
        (
            !output_state.control_emitted && !output_state.item_extra_body.is_empty(),
            output_state.item_extra_body.clone(),
        )
    };
    if !should_emit {
        return;
    }
    let node_index = index_state.allocate_fresh_node_index();
    events.push(UrpStreamEvent::NodeStart {
        node_index,
        header: NodeHeader::NextDownstreamEnvelopeExtra,
        extra_body: extra_body.clone(),
    });
    events.push(UrpStreamEvent::NodeDone {
        node_index,
        node: Node::NextDownstreamEnvelopeExtra {
            extra_body: extra_body.clone(),
        },
        usage: None,
        extra_body,
    });
    output_state_for(index_state, output_index).control_emitted = true;
}

fn emit_node_start_with_bridge(
    output_index: u64,
    node_index: u32,
    node: &Node,
    extra_body: HashMap<String, Value>,
    index_state: &mut ResponsesStreamIndexState,
    events: &mut Vec<UrpStreamEvent>,
) {
    emit_pending_envelope_control_if_needed(output_index, index_state, events);
    events.push(UrpStreamEvent::NodeStart {
        node_index,
        header: node_header_from_node(node),
        extra_body: extra_body.clone(),
    });
    output_state_for(index_state, output_index).emitted_any_node = true;

    let Some(part_header) = part_header_from_node(node) else {
        return;
    };
    let item_index = ensure_bridge_item_start(output_index, index_state, events);
    events.push(UrpStreamEvent::PartStart {
        part_index: node_index,
        item_index,
        header: part_header,
        extra_body,
    });
}

fn emit_node_delta_with_bridge(
    node_index: u32,
    delta: NodeDelta,
    extra_body: HashMap<String, Value>,
    events: &mut Vec<UrpStreamEvent>,
) {
    events.push(UrpStreamEvent::NodeDelta {
        node_index,
        delta: delta.clone(),
        usage: None,
        extra_body: extra_body.clone(),
    });
    if let Some(delta) = part_delta_from_node_delta(&delta) {
        events.push(UrpStreamEvent::Delta {
            part_index: node_index,
            delta,
            usage: None,
            extra_body,
        });
    }
}

fn emit_node_done_with_bridge(
    output_index: u64,
    node_index: u32,
    node: Node,
    extra_body: HashMap<String, Value>,
    index_state: &mut ResponsesStreamIndexState,
    events: &mut Vec<UrpStreamEvent>,
) {
    events.push(UrpStreamEvent::NodeDone {
        node_index,
        node: node.clone(),
        usage: None,
        extra_body: extra_body.clone(),
    });

    match node {
        Node::ToolResult { .. } => {}
        other => {
            let Some(part) = bridge_part_from_node(&other) else {
                return;
            };
            let _item_index = ensure_bridge_item_start(output_index, index_state, events);
            events.push(UrpStreamEvent::PartDone {
                part_index: node_index,
                part,
                usage: None,
                extra_body: extra_body.clone(),
            });
        }
    }
}

fn emit_item_done_bridge(
    output_index: u64,
    item: Item,
    extra_body: HashMap<String, Value>,
    index_state: &mut ResponsesStreamIndexState,
    events: &mut Vec<UrpStreamEvent>,
) {
    let item_index = ensure_bridge_item_start(output_index, index_state, events);
    events.push(UrpStreamEvent::ItemDone {
        item_index,
        item,
        usage: None,
        extra_body,
    });
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
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            output_state.item_id = Some(id.to_string());
        }
        if output_state.item_extra_body.is_empty() {
            output_state.item_extra_body = item_extra_body.clone();
        } else {
            for (key, value) in item_extra_body.clone() {
                output_state.item_extra_body.entry(key).or_insert(value);
            }
        }
        if item_type == "reasoning" {
            merge_reasoning_source(
                &mut output_state.reasoning_source,
                reasoning_source_from_value(item),
            );
        }
    }

    match item_type {
        "reasoning" => {
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::Reasoning {
                id: item.get("id").and_then(Value::as_str).map(|s| s.to_string()),
                content: None,
                encrypted: None,
                summary: None,
                source: None,
                extra_body: part_extra_body_from_value(item),
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            emit_node_start_with_bridge(
                output_index,
                node_index,
                &node,
                part_extra_body_from_value(item),
                index_state,
                &mut events,
            );
        }
        "function_call" => {
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::ToolCall {
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_tool_call_id())),
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
                arguments: item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                extra_body: part_extra_body_from_value(item),
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            emit_node_start_with_bridge(
                output_index,
                node_index,
                &node,
                part_extra_body_from_value(item),
                index_state,
                &mut events,
            );
        }
        "function_call_output" => {
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::ToolResult {
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_tool_result_id())),
                call_id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                is_error: false,
                content: Vec::new(),
                extra_body: item_extra_body_from_value(item),
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            events.push(UrpStreamEvent::NodeStart {
                node_index,
                header: node_header_from_node(&node),
                extra_body: item_extra_body_from_value(item),
            });
            output_state_for(index_state, output_index).emitted_any_node = true;
            ensure_bridge_item_start(output_index, index_state, &mut events);
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
    let node_index = index_state.node_index_for_content(output_index, content_index);
    let role = output_state_for(index_state, output_index)
        .role
        .unwrap_or(Role::Assistant);

    let mut events = Vec::new();
    let node = node_from_part_value(
        part,
        role,
        output_state_for(index_state, output_index)
            .item_id
            .clone()
            .or_else(|| Some(crate::urp::synthetic_message_id())),
    );
    emit_node_start_with_bridge(
        output_index,
        node_index,
        &node,
        part_extra_body_from_value(part),
        index_state,
        &mut events,
    );
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
    let node_index = index_state.node_index_for_content(output_index, content_index);
    let role = output_state_for(index_state, output_index)
        .role
        .unwrap_or(Role::Assistant);
    let node = node_from_part_value(
        part,
        role,
        output_state_for(index_state, output_index)
            .item_id
            .clone()
            .or_else(|| Some(crate::urp::synthetic_message_id())),
    );
    if let Some(output_state) = index_state.output_state_by_index.get_mut(&output_index) {
        output_state.part_done_seen = true;
    }
    let mut events = Vec::new();
    emit_node_done_with_bridge(
        output_index,
        node_index,
        node,
        part_extra_body_from_value(part),
        index_state,
        &mut events,
    );
    events
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
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::ToolResult {
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_tool_result_id())),
                call_id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                is_error: false,
                content: Vec::new(),
                extra_body: item_extra_body_from_value(item),
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            emit_node_done_with_bridge(
                output_index,
                node_index,
                node,
                item_extra_body_from_value(item),
                index_state,
                &mut events,
            );
            emit_item_done_bridge(
                output_index,
                decode_item_from_value(item),
                item_extra_body_from_value(item),
                index_state,
                &mut events,
            );
        }
        "reasoning" | "function_call" => {
            let role = output_state_for(index_state, output_index)
                .role
                .unwrap_or(Role::Assistant);
            let node = first_node_from_item_value(item)
                .unwrap_or_else(|| {
                    node_from_part_value(
                        item,
                        role,
                        output_state_for(index_state, output_index)
                            .item_id
                            .clone()
                            .or_else(|| item.get("id").and_then(Value::as_str).map(|s| s.to_string())),
                    )
                });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            let state = index_state
                .output_state_by_index
                .get(&output_index);
            let emitted_any_node = state.map(|state| state.emitted_any_node).unwrap_or(false);
            let bridge_item_done_already_emitted = state
                .map(|state| state.bridge_item_index.is_some())
                .unwrap_or(false);
            if !emitted_any_node {
                let reasoning_text_delta_seen = state.map(|state| state.reasoning_text_delta_seen).unwrap_or(false);
                let reasoning_summary_delta_seen = state
                    .map(|state| state.reasoning_summary_delta_seen)
                    .unwrap_or(false);
                let reasoning_item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .or_else(|| {
                        state
                            .and_then(|state| state.item_id.clone())
                    });
                if let Node::Reasoning {
                    content,
                    encrypted,
                    summary,
                    source,
                    ..
                } = &node
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
                        let mut delta_extra = HashMap::new();
                        if let Some(id) = &reasoning_item_id {
                            delta_extra.insert(
                                "reasoning_item_id".to_string(),
                                Value::String(id.clone()),
                            );
                        }
                        emit_node_delta_with_bridge(
                            node_index,
                            NodeDelta::Reasoning {
                                content: fallback_content,
                                encrypted: fallback_encrypted,
                                summary: fallback_summary,
                                source: source.clone(),
                            },
                            delta_extra,
                            &mut events,
                        );
                    }
                }
                emit_node_done_with_bridge(
                    output_index,
                    node_index,
                    node.clone(),
                    part_extra_body_from_value(item),
                    index_state,
                    &mut events,
                );
            }
            if !bridge_item_done_already_emitted {
                emit_item_done_bridge(
                    output_index,
                    decode_item_from_value(item),
                    item_extra_body_from_value(item),
                    index_state,
                    &mut events,
                );
            }
        }
        "message" => {
            let (part_done_seen, emitted_any_node, bridge_item_done_already_emitted) = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| {
                    (
                        state.part_done_seen,
                        state.emitted_any_node,
                        state.bridge_item_index.is_some(),
                    )
                })
                .unwrap_or((false, false, false));
            if !part_done_seen && !emitted_any_node {
                let decoded_item = decode_item_from_value(item);
                if let Item::Message { .. } = decoded_item {
                    let nodes = nodes_from_item_value(item);
                    for node in nodes {
                        let node_index = index_state.allocate_fresh_node_index();
                        emit_node_start_with_bridge(
                            output_index,
                            node_index,
                            &node,
                            item_extra_body_from_value(item),
                            index_state,
                            &mut events,
                        );
                        emit_node_done_with_bridge(
                            output_index,
                            node_index,
                            node,
                            HashMap::new(),
                            index_state,
                            &mut events,
                        );
                    }
                }
            }
            if !bridge_item_done_already_emitted {
                emit_item_done_bridge(
                    output_index,
                    decode_item_from_value(item),
                    item_extra_body_from_value(item),
                    index_state,
                    &mut events,
                );
            }
        }
        _ => {
            let (emitted_any_node, bridge_item_done_already_emitted) = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| (state.emitted_any_node, state.bridge_item_index.is_some()))
                .unwrap_or((false, false));
            if !emitted_any_node {
                let role = output_state_for(index_state, output_index)
                    .role
                    .unwrap_or(Role::Assistant);
                let node = first_node_from_item_value(item)
                    .unwrap_or_else(|| {
                        node_from_part_value(
                            item,
                            role,
                            output_state_for(index_state, output_index)
                                .item_id
                                .clone()
                                .or_else(|| item.get("id").and_then(Value::as_str).map(|s| s.to_string())),
                        )
                    });
                let node_index = index_state.synthetic_node_index_for_output(output_index);
                emit_node_done_with_bridge(
                    output_index,
                    node_index,
                    node,
                    part_extra_body_from_value(item),
                    index_state,
                    &mut events,
                );
            }
            if !bridge_item_done_already_emitted {
                emit_item_done_bridge(
                    output_index,
                    decode_item_from_value(item),
                    item_extra_body_from_value(item),
                    index_state,
                    &mut events,
                );
            }
        }
    }

    events
}

fn merge_response_completed_outputs(
    terminal_outputs: Vec<Node>,
    accumulated_outputs: &[Node],
) -> Vec<Node> {
    if accumulated_outputs.is_empty() {
        return terminal_outputs;
    }

    let mut merged = accumulated_outputs.to_vec();
    for terminal in terminal_outputs {
        if let Some(index) = merged
            .iter()
            .position(|candidate| response_completed_nodes_match(candidate, &terminal))
        {
            merged[index] = terminal;
        } else {
            merged.push(terminal);
        }
    }

    merged
}

fn response_completed_nodes_match(left: &Node, right: &Node) -> bool {
    match (left, right) {
        (
            Node::Text {
                id: left_id,
                role: left_role,
                phase: left_phase,
                content: left_content,
                ..
            },
            Node::Text {
                id: right_id,
                role: right_role,
                phase: right_phase,
                content: right_content,
                ..
            },
        ) => {
            (left_id.is_some() && left_id == right_id)
                || (left_role == right_role
                    && left_phase == right_phase
                    && left_content == right_content)
        }
        (Node::Image { id: left_id, .. }, Node::Image { id: right_id, .. })
        | (Node::Audio { id: left_id, .. }, Node::Audio { id: right_id, .. })
        | (Node::File { id: left_id, .. }, Node::File { id: right_id, .. })
        | (Node::Refusal { id: left_id, .. }, Node::Refusal { id: right_id, .. })
        | (Node::ProviderItem { id: left_id, .. }, Node::ProviderItem { id: right_id, .. }) => {
            left_id.is_some() && left_id == right_id
        }
        (
            Node::Reasoning {
                id: left_id,
                content: left_content,
                encrypted: left_encrypted,
                summary: left_summary,
                source: left_source,
                extra_body: left_extra_body,
            },
            Node::Reasoning {
                id: right_id,
                content: right_content,
                encrypted: right_encrypted,
                summary: right_summary,
                source: right_source,
                extra_body: right_extra_body,
            },
        ) => {
            (left_id.is_some() && left_id == right_id)
                || (left_content == right_content
                    && left_encrypted == right_encrypted
                    && left_summary == right_summary
                    && left_source == right_source
                    && left_extra_body == right_extra_body)
        }
        (
            Node::ToolCall {
                id: left_id,
                call_id: left_call_id,
                ..
            },
            Node::ToolCall {
                id: right_id,
                call_id: right_call_id,
                ..
            },
        )
        | (
            Node::ToolResult {
                id: left_id,
                call_id: left_call_id,
                ..
            },
            Node::ToolResult {
                id: right_id,
                call_id: right_call_id,
                ..
            },
        ) => left_call_id == right_call_id || (left_id.is_some() && left_id == right_id),
        _ => false,
    }
}

fn map_response_completed_with_accumulated(
    data_val: Value,
    _index_state: &mut ResponsesStreamIndexState,
    accumulated_outputs: &[Node],
) -> Vec<UrpStreamEvent> {
    let mut events = Vec::new();
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
    let terminal_outputs = decoded
        .as_ref()
        .map(|resp| resp.output.clone())
        .unwrap_or_else(|| {
            response_obj
                .get("output")
                .and_then(|v| v.as_array())
                .map(|items| items.iter().flat_map(nodes_from_item_value).collect())
                .unwrap_or_default()
        });
    let outputs = merge_response_completed_outputs(terminal_outputs, accumulated_outputs);
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
        output: outputs,
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

fn map_response_completed(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    map_response_completed_with_accumulated(data_val, index_state, &[])
}

fn urp_node_index_from_delta(data_val: &Value, index_state: &mut ResponsesStreamIndexState) -> u32 {
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if let Some(content_index) = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
    {
        return index_state.node_index_for_content(output_index, content_index);
    }
    index_state.synthetic_node_index_for_output(output_index)
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
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_message_id())),
                role: role_from_item(item),
                parts,
                extra_body: item_extra_body_from_value(item),
            }
        }
        "function_call_output" => Item::ToolResult {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_tool_result_id())),
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
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_reasoning_id())),
            role: Role::Assistant,
            parts: vec![decode_part_from_value(item)],
            extra_body: HashMap::new(),
        },
        "function_call" => Item::Message {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_tool_call_id())),
            role: Role::Assistant,
            parts: vec![decode_part_from_value(item)],
            extra_body: HashMap::new(),
        },
        other => Item::Message {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
            role: Role::Assistant,
            parts: vec![Part::ProviderItem {
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
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
            id: part
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_reasoning_id())),
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
            id: part
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_tool_call_id())),
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
            id: part
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
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

fn outputs_have_tool_calls(items: &[Node]) -> bool {
    items.iter()
        .any(|item| matches!(item, Node::ToolCall { .. }))
}

#[allow(clippy::too_many_arguments)]
fn build_accumulated_output_nodes(
    reasoning_text: &str,
    reasoning_summary_text: &str,
    reasoning_sig: &str,
    reasoning_source: Option<&str>,
    reasoning_output_index: Option<u64>,
    output_texts_by_output_index: &HashMap<u64, String>,
    message_phases_by_output_index: &HashMap<u64, String>,
    message_item_extra_by_output_index: &HashMap<u64, HashMap<String, Value>>,
    item_ids_by_output_index: &HashMap<u64, String>,
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    call_ids_by_output_index: &HashMap<u64, String>,
) -> Vec<Node> {
    let mut nodes = Vec::new();

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
                nodes.push(Node::Reasoning {
                    id: reasoning_output_index.and_then(|idx| {
                        item_ids_by_output_index
                            .get(&idx)
                            .cloned()
                            .or_else(|| {
                                message_item_extra_by_output_index
                                    .get(&idx)
                                    .and_then(|extra| extra.get("id"))
                                    .and_then(Value::as_str)
                                    .map(|s| s.to_string())
                            })
                    }),
                    content: (!reasoning_text.is_empty()).then(|| reasoning_text.to_string()),
                    summary: (!reasoning_summary_text.is_empty())
                        .then(|| reasoning_summary_text.to_string()),
                    encrypted: (!reasoning_sig.is_empty())
                        .then(|| Value::String(reasoning_sig.to_string())),
                    source: reasoning_source.map(|source| source.to_string()),
                    extra_body: HashMap::new(),
                });
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
                let mut item_extra_body = message_item_extra_by_output_index
                    .get(&output_index)
                    .cloned()
                    .unwrap_or_default();
                if let Some(phase) = text_extra_body.get("phase") {
                    item_extra_body.entry("phase".to_string()).or_insert_with(|| phase.clone());
                }
                let message_id = item_extra_body
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_message_id()));
                nodes.push(Node::NextDownstreamEnvelopeExtra {
                    extra_body: item_extra_body,
                });
                nodes.push(Node::Text {
                    id: message_id,
                    role: OrdinaryRole::Assistant,
                    content: output_text.clone(),
                    phase: text_extra_body
                        .get("phase")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    extra_body: text_extra_body,
                });
            }
            FallbackOutputKind::ToolCall(_, call_id) => {
                if let Some((name, arguments)) = calls.get(&call_id) {
                    nodes.push(Node::ToolCall {
                        id: Some(crate::urp::synthetic_tool_call_id()),
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                        extra_body: HashMap::new(),
                    });
                }
            }
        }
    }

    nodes
}

fn output_item_message_id(item: &Item) -> Option<String> {
    match item {
        Item::Message { id, .. } | Item::ToolResult { id, .. } => id.clone(),
    }
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
    fn build_accumulated_output_nodes_greedily_merge_into_one_compat_message() {
        let calls = HashMap::from([(
            "call_1".to_string(),
            ("lookup".to_string(), "{}".to_string()),
        )]);

        let outputs = build_accumulated_output_nodes(
                    "think",
                    "summary",
                    "sig_1",
                    Some("anthropic"),
                    Some(0),
                    &HashMap::from([(0, "answer".to_string())]),
                    &HashMap::from([(0, "analysis".to_string())]),
                    &HashMap::new(),
                    &HashMap::new(),
                    &["call_1".to_string()],
                    &calls,
                    &HashMap::from([(1, "call_1".to_string())]),
                );

        let outputs = nodes_to_items(&outputs);
        assert_eq!(outputs.len(), 2);
        let Item::Message { parts, extra_body, .. } = &outputs[0] else {
            panic!("expected reasoning compatibility item");
        };
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
        let Item::Message {
            role,
            parts,
            extra_body,
            ..
        } = &outputs[1]
        else {
            panic!("expected phased assistant compatibility item");
        };
        assert_eq!(role, &Role::Assistant);
        assert_eq!(extra_body.get("phase"), Some(&json!("analysis")));
        assert!(matches!(
            &parts[0],
            Part::Text { content, extra_body } if content == "answer" && extra_body.get("phase") == Some(&json!("analysis"))
        ));
        assert!(matches!(
            &parts[1],
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } if call_id == "call_1" && name == "lookup" && arguments == "{}"
        ));
    }

    #[test]
    fn build_accumulated_output_nodes_omit_empty_text_message() {
        let outputs = build_accumulated_output_nodes(
                    "",
                    "",
                    "sig_only",
                    None,
                    Some(0),
                    &HashMap::new(),
                    &HashMap::from([(0, "analysis".to_string())]),
                    &HashMap::new(),
                    &HashMap::new(),
                    &[],
                    &HashMap::new(),
                    &HashMap::new(),
                );

        let outputs = nodes_to_items(&outputs);
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
    fn build_accumulated_output_nodes_preserve_multiple_output_text_phases() {
        let outputs = build_accumulated_output_nodes(
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
                    &HashMap::new(),
                    &HashMap::new(),
                    &[],
                    &HashMap::new(),
                    &HashMap::new(),
                );

        let outputs = nodes_to_items(&outputs);
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
    fn build_accumulated_output_nodes_preserve_real_output_index_order_in_fallback() {
        let calls = HashMap::from([(
            "call_1".to_string(),
            ("lookup".to_string(), "{}".to_string()),
        )]);

        let outputs = build_accumulated_output_nodes(
                    "think",
                    "summary",
                    "",
                    Some("upstream-reasoner"),
                    Some(2),
                    &HashMap::from([(5, "answer".to_string())]),
                    &HashMap::new(),
                    &HashMap::new(),
                    &HashMap::new(),
                    &["call_1".to_string()],
                    &calls,
                    &HashMap::from([(9, "call_1".to_string())]),
                );

        let outputs = nodes_to_items(&outputs);
        assert_eq!(outputs.len(), 2);
        let Item::Message { parts, .. } = &outputs[0] else {
            panic!("expected reasoning compatibility item");
        };
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                source: Some(source),
                ..
            } if source == "upstream-reasoner"
        ));
        let Item::Message { parts, .. } = &outputs[1] else {
            panic!("expected assistant action compatibility item");
        };
        assert!(matches!(&parts[0], Part::Text { content, .. } if content == "answer"));
        assert!(matches!(&parts[1], Part::ToolCall { call_id, .. } if call_id == "call_1"));
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
            output,
            ..
        }) = completed_events.last()
        else {
            panic!("expected response done event");
        };

        assert_eq!(*finish_reason, Some(FinishReason::ToolCalls));
        assert_eq!(output.len(), 3);
        let output_items = nodes_to_items(output);
        let Item::Message {
            parts, extra_body, ..
        } = &output_items[0]
        else {
            panic!("expected assistant message output");
        };
        assert_eq!(extra_body.get("phase"), Some(&json!("analysis")));
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
    fn top_level_reasoning_and_function_call_items_emit_node_lifecycle_in_source_order() {
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
        let reasoning_offset = if matches!(
            reasoning_events.first(),
            Some(UrpStreamEvent::NodeStart {
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                ..
            })
        ) {
            assert!(matches!(
                reasoning_events.get(1),
                Some(UrpStreamEvent::NodeDone {
                    node: Node::NextDownstreamEnvelopeExtra { .. },
                    ..
                })
            ));
            2
        } else {
            0
        };
        assert!(matches!(
            &reasoning_events[reasoning_offset],
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Reasoning { .. },
                ..
            } if *node_index == 0
        ));
        assert!(matches!(
            &reasoning_events[reasoning_offset + 1],
            UrpStreamEvent::ItemStart {
                item_index,
                header: ItemHeader::Message { role: Role::Assistant, .. },
                ..
            } if *item_index == 0
        ));
        assert!(matches!(
            &reasoning_events[reasoning_offset + 2],
            UrpStreamEvent::PartStart {
                item_index,
                part_index,
                header: PartHeader::Reasoning { .. },
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
        let function_offset = if matches!(
            function_events.first(),
            Some(UrpStreamEvent::NodeStart {
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                ..
            })
        ) {
            assert!(matches!(
                function_events.get(1),
                Some(UrpStreamEvent::NodeDone {
                    node: Node::NextDownstreamEnvelopeExtra { .. },
                    ..
                })
            ));
            2
        } else {
            0
        };
        let tool_call_node_index = match &function_events[function_offset] {
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::ToolCall { call_id, name, .. },
                ..
            } if call_id == "call_1" && name == "lookup" => *node_index,
            other => panic!("unexpected tool-call node start: {other:?}"),
        };
        assert!(matches!(
            &function_events[function_offset + 1],
            UrpStreamEvent::ItemStart {
                item_index,
                header: ItemHeader::Message { role: Role::Assistant, .. },
                ..
            } if *item_index == 1
        ));
        assert!(matches!(
            &function_events[function_offset + 2],
            UrpStreamEvent::PartStart {
                item_index,
                part_index,
                header: PartHeader::ToolCall { call_id, name, .. },
                ..
            } if *item_index == 1 && *part_index == tool_call_node_index && call_id == "call_1" && name == "lookup"
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
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::ToolCallArguments { arguments },
                ..
            } if *node_index == tool_call_node_index && arguments == "{}"
        ));
        assert!(matches!(
            &function_delta[1],
            UrpStreamEvent::Delta {
                part_index,
                delta: PartDelta::ToolCallArguments { arguments },
                ..
            } if *part_index == tool_call_node_index && arguments == "{}"
        ));
    }

    #[test]
    fn content_part_done_reuses_normalized_node_index() {
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
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Text { .. },
                ..
            } if *node_index == 0
        ));
        assert!(matches!(
            &added[1],
            UrpStreamEvent::ItemStart {
                item_index,
                header: ItemHeader::Message { role: Role::Assistant, .. },
                ..
            } if *item_index == 0
        ));
        assert!(matches!(
            &added[2],
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
            UrpStreamEvent::NodeDone {
                node_index,
                node: Node::Text { content, .. },
                ..
            } if *node_index == 0 && content == "done"
        ));
        assert!(matches!(
            &done[1],
            UrpStreamEvent::PartDone {
                part_index,
                part: Part::Text { content, .. },
                ..
            } if *part_index == 0 && content == "done"
        ));
    }

    #[test]
    fn reasoning_delta_item_id_overrides_added_item_id() {
        let mut state = ResponsesStreamIndexState::default();

        let added = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 0,
                "item": {
                    "type": "reasoning",
                    "id": "rs_added",
                    "summary": [{ "type": "summary_text", "text": "" }],
                    "text": ""
                }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(added.iter().any(|event| matches!(
            event,
            UrpStreamEvent::NodeStart {
                header: NodeHeader::Reasoning { id },
                ..
            } if id.as_deref() == Some("rs_added")
        )));

        let delta = map_responses_event_to_urp_events_with_state(
            "response.reasoning_summary_text.delta",
            json!({
                "output_index": 0,
                "item_id": "rs_authoritative",
                "summary_index": 0,
                "delta": "summary"
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(matches!(
            &delta[0],
            UrpStreamEvent::NodeDelta {
                delta: NodeDelta::Reasoning { summary: Some(summary), .. },
                extra_body,
                ..
            } if summary == "summary" && extra_body.get("reasoning_item_id") == Some(&json!("rs_authoritative"))
        ));
        assert_eq!(
            state
                .output_state_by_index
                .get(&0)
                .and_then(|output| output.item_id.as_deref()),
            Some("rs_authoritative")
        );

        let _done = map_responses_event_to_urp_events_with_state(
            "response.output_item.done",
            json!({
                "output_index": 0,
                "item": {
                    "type": "reasoning",
                    "id": "rs_authoritative",
                    "summary": [{ "type": "summary_text", "text": "summary" }],
                    "text": "",
                    "encrypted_content": "sig_1"
                }
            }),
            &HashMap::new(),
            &mut state,
        );

        let accumulated = build_accumulated_output_nodes(
            "",
            "summary",
            "sig_1",
            None,
            Some(0),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::from([(0, "rs_authoritative".to_string())]),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(matches!(
            accumulated.first(),
            Some(Node::Reasoning {
                id,
                encrypted: Some(Value::String(sig)),
                summary: Some(summary),
                ..
            }) if id.as_deref() == Some("rs_authoritative") && sig == "sig_1" && summary == "summary"
        ));
    }

    #[test]
    fn synthetic_text_fallback_preserves_multiple_phases_and_allocates_distinct_part_indices() {
        let output_items = nodes_to_items(&build_accumulated_output_nodes(
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
                    &HashMap::new(),
                    &HashMap::new(),
                    &[],
                    &HashMap::new(),
                    &HashMap::new(),
                ));
        let mut state = ResponsesStreamIndexState::default();
        assert_eq!(state.allocate_fresh_item_index(), 0);
        assert_eq!(state.node_index_for_content(11, 4), 0);
        assert_eq!(state.synthetic_node_index_for_output(12), 1);

        let mut observed = Vec::new();

        for (_final_item_index, output_item) in output_items.iter().enumerate() {
            let Item::Message {
                role: Role::Assistant,
                parts,
                extra_body,
                ..
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
                let synthetic_text_item = Item::Message { id: None, role: Role::Assistant, parts: vec![Part::Text {
                    content: content.clone(),
                    extra_body: text_extra_body.clone(),
                }], extra_body: extra_body.clone() };
                let node_index = state.allocate_fresh_node_index();
                observed.push(UrpStreamEvent::NodeStart {
                    node_index,
                    header: NodeHeader::Text {
                        id: None,
                        role: OrdinaryRole::Assistant,
                        phase: text_extra_body
                            .get("phase")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                    },
                    extra_body: item_extra_body_from_item(&synthetic_text_item),
                });
                observed.push(UrpStreamEvent::NodeDelta {
                    node_index,
                    delta: NodeDelta::Text {
                        content: content.clone(),
                    },
                    usage: None,
                    extra_body: text_extra_body.clone(),
                });
            }
        }

        assert!(matches!(
            &observed[1],
            UrpStreamEvent::NodeDelta { node_index, extra_body, .. }
                if *node_index == 2 && extra_body.get("phase") == Some(&json!("commentary"))
        ));
        assert!(matches!(
            &observed[3],
            UrpStreamEvent::NodeDelta { node_index, extra_body, .. }
                if *node_index == 3 && extra_body.get("phase") == Some(&json!("final_answer"))
        ));
    }

    #[test]
    fn build_accumulated_output_nodes_omit_reasoning_source_when_missing() {
        let outputs = build_accumulated_output_nodes(
                    "think",
                    "",
                    "",
                    None,
                    Some(0),
                    &HashMap::new(),
                    &HashMap::new(),
                    &HashMap::new(),
                    &HashMap::new(),
                    &[],
                    &HashMap::new(),
                    &HashMap::new(),
                );

        let outputs = nodes_to_items(&outputs);
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
    fn map_output_item_done_message_fallback_emits_one_node_per_part_then_item_done() {
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
            10,
            "fallback should emit two node lifecycles plus one final item done"
        );
        assert!(matches!(
            &done_events[0],
            UrpStreamEvent::NodeStart {
                node_index: 0,
                header: NodeHeader::Text { .. },
                ..
            }
        ));
        assert!(matches!(
            &done_events[1],
            UrpStreamEvent::ItemStart {
                item_index: 0,
                header: ItemHeader::Message {
                    role: Role::Assistant,
                    ..
                },
                ..
            }
        ));
        assert!(
            done_events.iter().any(|event| matches!(
                event,
                UrpStreamEvent::NodeStart {
                    node_index: 1,
                    header: NodeHeader::ToolCall { call_id, .. },
                    ..
                } if call_id == "call_1"
            ))
        );
        let UrpStreamEvent::ItemDone { item, .. } = done_events
            .last()
            .expect("expected terminal item done")
        else {
            panic!("expected terminal item done");
        };
        let Item::Message { parts, .. } = item else {
            panic!("expected message item");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], Part::Text { content, .. } if content == "answer"));
        assert!(matches!(&parts[1], Part::ToolCall { call_id, .. } if call_id == "call_1"));
    }

    #[test]
    fn accumulated_output_nodes_drive_response_done_before_grouped_projection() {
        let output_nodes = build_accumulated_output_nodes(
                    "think",
                    "summary",
                    "sig_1",
                    Some("anthropic"),
                    Some(0),
                    &HashMap::from([(0, "answer".to_string())]),
                    &HashMap::from([(0, "analysis".to_string())]),
                    &HashMap::new(),
                    &HashMap::new(),
                    &["call_1".to_string()],
                    &HashMap::from([(
                        "call_1".to_string(),
                        ("lookup".to_string(), "{}".to_string()),
                    )]),
                    &HashMap::from([(1, "call_1".to_string())]),
                );

        assert_eq!(output_nodes.len(), 4);
        assert!(matches!(
            &output_nodes[0],
            Node::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                source: Some(source),
                ..
            } if content == "think" && summary == "summary" && sig == "sig_1" && source == "anthropic"
        ));
        assert!(matches!(
            &output_nodes[1],
            Node::NextDownstreamEnvelopeExtra { extra_body }
                if extra_body.get("phase") == Some(&json!("analysis"))
        ));
        assert!(matches!(
            &output_nodes[2],
            Node::Text { role: OrdinaryRole::Assistant, content, phase: Some(phase), .. }
                if content == "answer" && phase == "analysis"
        ));
        assert!(matches!(
            &output_nodes[3],
            Node::ToolCall { call_id, name, arguments, .. }
                if call_id == "call_1" && name == "lookup" && arguments == "{}"
        ));

        let output_items = nodes_to_items(&output_nodes);
        assert_eq!(output_items.len(), 2);
        let Item::Message { parts, extra_body, .. } = &output_items[0] else {
            panic!("expected reasoning compatibility item");
        };
        assert!(extra_body.is_empty());
        assert_eq!(parts.len(), 1);
        let Item::Message { parts, extra_body, .. } = &output_items[1] else {
            panic!("expected phased assistant compatibility item");
        };
        assert_eq!(extra_body.get("phase"), Some(&json!("analysis")));
        assert_eq!(parts.len(), 2);
    }
}
