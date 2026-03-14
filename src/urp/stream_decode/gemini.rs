use crate::error::{AppError, AppResult};
use crate::handlers::routing::{now_ts, wrap_responses_event};
use crate::handlers::usage::{latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_gemini_object, record_stream_done_sentinel, record_stream_synthetic_terminal_emitted, record_stream_terminal_event, record_stream_usage_if_present, usage_to_chat_usage_json, usage_to_messages_usage_json, usage_to_responses_usage_json};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::stream_helpers::*;
use axum::http::StatusCode;
use axum::response::sse::Event;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_gemini_sse_as_responses(
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
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();

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
        if let Some(parts) = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                        if !text.is_empty() {
                            reasoning_text.push_str(text);
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.reasoning_text.delta",
                                    json!({ "delta": text }),
                                ))
                                .await;
                        }
                        if let Some(sig) = part.get("thoughtSignature") {
                            let sig_text = sig
                                .as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| sig.to_string());
                            if !sig_text.is_empty() {
                                reasoning_sig.push_str(&sig_text);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_signature.delta",
                                        json!({ "delta": sig_text }),
                                    ))
                                    .await;
                            }
                        }
                    } else if !text.is_empty() {
                        output_text.push_str(text);
                        let _ = tx
                            .send(wrap_responses_event(
                                &mut seq,
                                "response.output_text.delta",
                                json!({ "text": text }),
                            ))
                            .await;
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
                            format!("call_{}", call_order.len() + 1)
                        } else {
                            call_id
                        };
                        if !calls.contains_key(&key) {
                            call_order.push(key.clone());
                            calls.insert(key.clone(), (name.clone(), String::new()));
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.output_item.added",
                                    json!({
                                        "type": "function_call",
                                        "call_id": key,
                                        "name": name,
                                        "arguments": ""
                                    }),
                                ))
                                .await;
                        }
                        if !arguments.is_empty() {
                            if let Some(entry) = calls.get_mut(&key) {
                                entry.1.push_str(&arguments);
                            }
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.function_call_arguments.delta",
                                    json!({ "call_id": key, "name": name, "delta": arguments }),
                                ))
                                .await;
                        }
                    }
                }
            }
        }
    }

    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            let item = json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            });
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.done",
                    item.clone(),
                ))
                .await;
            output_items.push(item);
        }
    }

    let output_item = json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": output_text }]
    });
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
            "response.output_item.done",
            output_item,
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

pub(crate) async fn stream_gemini_sse_as_chat(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut saw_tool_call = false;

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

        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(text), "finish_reason": Value::Null }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                    if let Some(sig) = part.get("thoughtSignature") {
                        let sig_text = sig
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sig.to_string());
                        if !sig_text.is_empty() {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&sig_text), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                } else if !text.is_empty() {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
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
                let args = serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                    .unwrap_or_else(|_| "{}".to_string());
                if !name.is_empty() {
                    saw_tool_call = true;
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": 0,
                                    "id": call_id,
                                    "type": "function",
                                    "function": { "name": name, "arguments": args }
                                }]
                            },
                            "finish_reason": Value::Null
                        }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                }
            }
        }
    }

    let finish_reason = if saw_tool_call { "tool_calls" } else { "stop" };
    let mut done = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": urp.model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
    });
    if let Some(usage) = latest_stream_usage_snapshot(&runtime_metrics).await {
        done["usage"] = usage_to_chat_usage_json(&usage);
    }
    record_stream_synthetic_terminal_emitted(&runtime_metrics).await;
    let _ = tx.send(Event::default().data(done.to_string())).await;
    record_stream_terminal_event(&runtime_metrics, "chat.completion.chunk", Some(finish_reason))
        .await;
    let _ = tx.send(Event::default().data("[DONE]")).await;
    Ok(())
}

pub(crate) async fn stream_gemini_sse_as_messages(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    let start = json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": urp.model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0
            }
        }
    });
    let _ = tx.send(Event::default().data(start.to_string())).await;

    let mut next_index: u32 = 0;
    let mut text_index: Option<u32> = None;
    let mut thinking_index: Option<u32> = None;
    let mut tool_indices: HashMap<String, u32> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut started: Vec<u32> = Vec::new();
    let mut saw_tool_use = false;

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

        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                    let idx = ensure_anthropic_thinking_block(
                        &tx,
                        &mut thinking_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "thinking_delta", "thinking": text }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                    if let Some(sig) = part.get("thoughtSignature") {
                        let sig_text = sig
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sig.to_string());
                        if !sig_text.is_empty() {
                            let d = json!({
                                "type": "content_block_delta",
                                "index": idx,
                                "delta": { "type": "signature_delta", "signature": sig_text }
                            });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                } else if !text.is_empty() {
                    let idx = ensure_anthropic_text_block(
                        &tx,
                        &mut text_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "text_delta", "text": text }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }
            }

            if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                let call_id = fc
                    .get("id")
                    .or_else(|| fc.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let name = fc.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                    .unwrap_or_else(|_| "{}".to_string());
                if !name.is_empty() {
                    saw_tool_use = true;
                    let idx = ensure_anthropic_tool_block(
                        &tx,
                        &mut tool_indices,
                        &mut tool_names,
                        &mut next_index,
                        &mut started,
                        call_id,
                        name,
                    )
                    .await?;
                    if !args.is_empty() {
                        let d = json!({
                            "type": "content_block_delta",
                            "index": idx,
                            "delta": { "type": "input_json_delta", "partial_json": args }
                        });
                        let _ = tx.send(Event::default().data(d.to_string())).await;
                    }
                }
            }
        }
    }

    for idx in started.iter() {
        let stop = json!({ "type": "content_block_stop", "index": idx });
        let _ = tx.send(Event::default().data(stop.to_string())).await;
    }
    let message_usage = latest_stream_usage_snapshot(&runtime_metrics)
        .await
        .map(|usage| usage_to_messages_usage_json(&usage))
        .unwrap_or_else(|| json!({ "input_tokens": 0, "output_tokens": 0 }));
    let message_delta = json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": if saw_tool_use { "tool_use" } else { "end_turn" },
            "stop_sequence": Value::Null
        },
        "usage": message_usage
    });
    let _ = tx
        .send(Event::default().data(message_delta.to_string()))
        .await;
    let stop = json!({ "type": "message_stop" });
    let _ = tx.send(Event::default().data(stop.to_string())).await;
    Ok(())
}
