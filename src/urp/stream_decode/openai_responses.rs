use crate::error::{AppError, AppResult};
use crate::handlers::routing::{now_ts, wrap_responses_event};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_responses_object,
    record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present,
    usage_to_responses_usage_json,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::stream_helpers::*;
use axum::http::StatusCode;
use axum::response::sse::Event;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_responses_sse_as_responses(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut message_phases_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, arguments)
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_text_delta = false;

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
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
        // For responses upstream, we forward event names and data into our wrapper.
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::String(ev.data));
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_responses_object(&data_val),
        )
        .await;
        // Try to extract text deltas for final output.
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
        let data_val = if ev.event == "response.output_text.delta" {
            let mut payload = data_val;
            if let Some(idx) = payload.get("output_index").and_then(|v| v.as_u64()) {
                if let Some(phase) = message_phases_by_output_index.get(&idx) {
                    if let Some(obj) = payload.as_object_mut() {
                        obj.entry("phase".to_string())
                            .or_insert_with(|| Value::String(phase.clone()));
                    }
                }
            }
            payload
        } else {
            data_val
        };
        let name = if ev.event.is_empty() {
            "message"
        } else {
            ev.event.as_str()
        };
        let _ = tx
            .send(wrap_responses_event(&mut seq, name, data_val))
            .await;
    }

    // Minimal completion sequence.
    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            output_items.push(json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            }));
        }
    }
    let output_item = if let Some((_, phase)) = message_phases_by_output_index.iter().min_by_key(|(idx, _)| *idx) {
        json!({
            "type": "message",
            "role": "assistant",
            "phase": phase,
            "content": [{ "type": "output_text", "text": output_text }]
        })
    } else {
        json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": output_text }]
        })
    };
    output_items.push(output_item.clone());
    let mut final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    if let Some(usage) = latest_stream_usage_snapshot(&runtime_metrics).await {
        final_response["usage"] = usage_to_responses_usage_json(&usage);
    }
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.added",
            json!({ "output_index": output_items.len() - 1, "item": output_item.clone() }),
        ))
        .await;
    if !saw_text_delta {
        if let Some(text) = output_item
            .get("content")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|p| p.get("text"))
            .and_then(|v| v.as_str())
        {
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_text.delta",
                    responses_text_delta_payload(
                        text,
                        extract_responses_message_phase(&output_item).as_deref(),
                    ),
                ))
                .await;
        }
    }
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            json!({ "output_index": output_items.len() - 1, "item": output_item }),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}
