use crate::config::ProviderType;
use crate::error::{AppError, AppResult};
use crate::handlers::routing::{now_ts, wrap_responses_event};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_chat_object,
    parse_usage_from_gemini_object, parse_usage_from_messages_object,
    parse_usage_from_responses_object, record_stream_done_sentinel,
    record_stream_synthetic_terminal_emitted, record_stream_terminal_event,
    record_stream_usage_if_present, usage_to_chat_usage_json, usage_to_responses_usage_json,
};
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

pub(crate) async fn stream_chat_sse_as_responses(
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
    let mut assistant_message_phase: Option<String> = None;
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, arguments)
    let mut call_id_by_index: HashMap<usize, String> = HashMap::new();

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
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.added",
            json!({
                "output_index": 0,
                "item": {"type":"message","role":"assistant","content":[]}
            }),
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
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_chat_object(&data_val))
            .await;
        if let Some(reason) = data_val
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|v| v.as_str())
            .filter(|reason| !reason.is_empty())
        {
            record_stream_terminal_event(&runtime_metrics, "chat.completion.chunk", Some(reason))
                .await;
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

        if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
            if !t.is_empty() {
                output_text.push_str(t);
                let _ = tx
                    .send(wrap_responses_event(
                        &mut seq,
                        "response.output_text.delta",
                        responses_text_delta_payload(t, assistant_message_phase.as_deref()),
                    ))
                    .await;
            }
        }

        let (reasoning_text_deltas, reasoning_sig_deltas) = extract_chat_reasoning_deltas(&delta);
        for t in reasoning_text_deltas {
            reasoning_text.push_str(&t);
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.reasoning_text.delta",
                    json!({ "delta": t }),
                ))
                .await;
        }
        for sig in reasoning_sig_deltas {
            reasoning_sig.push_str(&sig);
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.reasoning_signature.delta",
                    json!({ "delta": sig }),
                ))
                .await;
        }

        // Tool call deltas (OpenAI chat format).
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
                    let item = json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": ""
                    });
                    let _ = tx
                        .send(wrap_responses_event(
                            &mut seq,
                            "response.output_item.added",
                            json!({
                                "output_index": call_order.len(),
                                "item": item
                            }),
                        ))
                        .await;
                }

                let Some(entry) = calls.get_mut(&call_id) else {
                    tracing::warn!(call_id = %call_id, "unknown call_id in tool call stream delta, skipping");
                    continue;
                };

                if !name.is_empty() && entry.0.is_empty() {
                    entry.0 = name.clone();
                }
                if !args_delta.is_empty() {
                    entry.1.push_str(&args_delta);
                    let _ = tx
                        .send(wrap_responses_event(
                            &mut seq,
                            "response.function_call_arguments.delta",
                            json!({ "call_id": call_id, "name": entry.0, "delta": args_delta }),
                        ))
                        .await;
                }
            }
        }
    }

    // Finalize any function calls encountered in the chat stream.
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
                    json!({
                        "output_index": output_items.len(),
                        "item": item.clone()
                    }),
                ))
                .await;
            output_items.push(item);
        }
    }

    let output_item = if let Some(phase) = assistant_message_phase.as_deref() {
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
            "response.output_item.done",
            json!({
                "output_index": output_items.len() - 1,
                "item": output_item
            }),
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

pub(crate) async fn stream_any_sse_as_chat(
    urp: &HandlerUrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut out_text = String::new();
    let mut saw_tool_call = false;
    let mut saw_responses_text_delta = false;
    let mut saw_responses_tool_delta = false;
    let mut saw_responses_reasoning_delta = false;
    let mut call_order: Vec<String> = Vec::new();
    let mut call_names: HashMap<String, String> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_upstream_terminal_finish = false;

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
            break;
        }
        match provider_type {
            ProviderType::ChatCompletion => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_chat_object(&data_val),
                )
                .await;
                let mut chunk = data_val;
                if let Some(obj) = chunk.as_object_mut() {
                    obj.insert("model".to_string(), Value::String(urp.model.clone()));
                    if !obj.contains_key("id") {
                        obj.insert("id".to_string(), Value::String(id.clone()));
                    }
                    if !obj.contains_key("object") {
                        obj.insert(
                            "object".to_string(),
                            Value::String("chat.completion.chunk".to_string()),
                        );
                    }
                    if !obj.contains_key("created") {
                        obj.insert("created".to_string(), Value::Number(created.into()));
                    }
                    if let Some(delta) = obj
                        .get_mut("choices")
                        .and_then(|v| v.as_array_mut())
                        .and_then(|arr| arr.first_mut())
                        .and_then(|v| v.get_mut("delta"))
                        .and_then(|v| v.as_object_mut())
                    {
                        normalize_chat_reasoning_delta_object(delta);
                        // Upstream may send explicit nulls (role: null, content: null, etc.)
                        // that violate the OpenAI streaming schema. Strip them — null semantically
                        // means "no update" and is equivalent to the field being absent.
                        delta.retain(|_, v| !v.is_null());
                    }
                }

                if let Some(t) = chunk
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
                {
                    out_text.push_str(t);
                }
                let choice_snapshot = chunk
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .cloned();
                if let Some(choice) = choice_snapshot {
                    let has_tool_delta = choice
                        .get("delta")
                        .and_then(|d| d.get("tool_calls"))
                        .and_then(|v| v.as_array())
                        .map(|arr| !arr.is_empty())
                        .unwrap_or(false);
                    if has_tool_delta {
                        saw_tool_call = true;
                    }

                    if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                        if !reason.is_empty() {
                            saw_upstream_terminal_finish = true;
                            record_stream_terminal_event(
                                &runtime_metrics,
                                "chat.completion.chunk",
                                Some(reason),
                            )
                            .await;
                            if reason == "tool_calls" {
                                saw_tool_call = true;
                            }
                            if reason == "stop" && saw_tool_call {
                                if let Some(choice_obj) = chunk
                                    .get_mut("choices")
                                    .and_then(|v| v.as_array_mut())
                                    .and_then(|arr| arr.first_mut())
                                    .and_then(|v| v.as_object_mut())
                                {
                                    choice_obj.insert(
                                        "finish_reason".to_string(),
                                        Value::String("tool_calls".to_string()),
                                    );
                                }
                            }
                        }
                    }
                }

                let _ = tx.send(Event::default().data(chunk.to_string())).await;
            }
            ProviderType::Responses | ProviderType::Grok => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_responses_object(&data_val),
                )
                .await;
                match ev.event.as_str() {
                    "response.output_text.delta" => {
                        let t = data_val
                            .get("text")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_text_delta = true;
                            out_text.push_str(t);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": { "content": t }, "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.reasoning_text.delta" => {
                        let t = data_val
                            .get("delta")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_reasoning_delta = true;
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(t), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.reasoning_signature.delta" => {
                        let t = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_reasoning_delta = true;
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(t), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.output_item.added" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !call_id.is_empty() {
                                saw_responses_tool_delta = true;
                                if !call_order.contains(&call_id) {
                                    call_order.push(call_id.clone());
                                }
                                if !name.is_empty() {
                                    call_names.insert(call_id.clone(), name.clone());
                                }
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index.insert(output_index, call_id.clone());
                                }
                                let idx =
                                    call_order.iter().position(|x| x == &call_id).unwrap_or(0);
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
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": "" }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let call_id = data_val
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                data_val
                                    .get("output_index")
                                    .and_then(|v| v.as_u64())
                                    .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                            })
                            .unwrap_or_default();
                        if call_id.is_empty() {
                            continue;
                        }
                        saw_responses_tool_delta = true;
                        if !call_order.contains(&call_id) {
                            call_order.push(call_id.clone());
                        }
                        let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                        let name = data_val
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| call_names.get(&call_id).cloned())
                            .unwrap_or_default();
                        if !name.is_empty() {
                            call_names.insert(call_id.clone(), name.clone());
                        }
                        let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
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
                                        "index": idx,
                                        "id": call_id,
                                        "type": "function",
                                        "function": { "name": name, "arguments": delta }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        });
                        let _ = tx.send(Event::default().data(chunk.to_string())).await;
                    }
                    "response.output_item.done" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if call_id.is_empty() {
                                continue;
                            }
                            saw_responses_tool_delta = true;
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .or_else(|| call_names.get(&call_id).cloned())
                                .unwrap_or_default();
                            if !call_order.contains(&call_id) {
                                call_order.push(call_id.clone());
                            }
                            if !name.is_empty() {
                                call_names.insert(call_id.clone(), name.clone());
                            }
                            if let Some(output_index) =
                                data_val.get("output_index").and_then(|v| v.as_u64())
                            {
                                call_ids_by_output_index.insert(output_index, call_id.clone());
                            }
                            let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                            let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                            if !args.is_empty() {
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
                                                "index": idx,
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
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                            if !saw_responses_text_delta {
                                let text = extract_responses_message_text(item);
                                if !text.is_empty() {
                                    out_text.push_str(&text);
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
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                            let (reasoning_text, reasoning_sig) =
                                extract_reasoning_text_and_signature(item);
                            if !reasoning_text.is_empty() {
                                saw_responses_reasoning_delta = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(&reasoning_text), "finish_reason": Value::Null }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                            if !reasoning_sig.is_empty() {
                                saw_responses_reasoning_delta = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&reasoning_sig), "finish_reason": Value::Null }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    "response.completed" => {
                        record_stream_terminal_event(&runtime_metrics, "response.completed", None)
                            .await;
                        let completed = data_val.get("response").unwrap_or(&data_val);
                        let Some(output) = completed.get("output").and_then(|v| v.as_array())
                        else {
                            continue;
                        };
                        for item in output {
                            match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                                "function_call" => {
                                    if saw_responses_tool_delta {
                                        continue;
                                    }
                                    let call_id = item
                                        .get("call_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if call_id.is_empty() {
                                        continue;
                                    }
                                    let name = item
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let args = item
                                        .get("arguments")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    if !call_order.contains(&call_id) {
                                        call_order.push(call_id.clone());
                                    }
                                    if !name.is_empty() {
                                        call_names.insert(call_id.clone(), name.clone());
                                    }
                                    let idx =
                                        call_order.iter().position(|x| x == &call_id).unwrap_or(0);
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
                                                    "index": idx,
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
                                "message" => {
                                    if saw_responses_text_delta {
                                        continue;
                                    }
                                    let text = extract_responses_message_text(item);
                                    if !text.is_empty() {
                                        saw_responses_text_delta = true;
                                        out_text.push_str(&text);
                                        let chunk = json!({
                                            "id": id,
                                            "object": "chat.completion.chunk",
                                            "created": created,
                                            "model": urp.model,
                                            "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                                        });
                                        let _ =
                                            tx.send(Event::default().data(chunk.to_string())).await;
                                    }
                                }
                                "reasoning" => {
                                    if saw_responses_reasoning_delta {
                                        continue;
                                    }
                                    let (reasoning_text, reasoning_sig) =
                                        extract_reasoning_text_and_signature(item);
                                    if !reasoning_text.is_empty() {
                                        saw_responses_reasoning_delta = true;
                                        let chunk = json!({
                                            "id": id,
                                            "object": "chat.completion.chunk",
                                            "created": created,
                                            "model": urp.model,
                                            "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(&reasoning_text), "finish_reason": Value::Null }]
                                        });
                                        let _ =
                                            tx.send(Event::default().data(chunk.to_string())).await;
                                    }
                                    if !reasoning_sig.is_empty() {
                                        saw_responses_reasoning_delta = true;
                                        let chunk = json!({
                                            "id": id,
                                            "object": "chat.completion.chunk",
                                            "created": created,
                                            "model": urp.model,
                                            "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&reasoning_sig), "finish_reason": Value::Null }]
                                        });
                                        let _ =
                                            tx.send(Event::default().data(chunk.to_string())).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Messages => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_messages_object(&data_val),
                )
                .await;
                let t = data_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match t {
                    "content_block_delta" => {
                        let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                        let dt = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match dt {
                            "text_delta" => {
                                let txt = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                if !txt.is_empty() {
                                    out_text.push_str(txt);
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": { "content": txt }, "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "thinking_delta" => {
                                let txt =
                                    delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                                if !txt.is_empty() {
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(txt), "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "signature_delta" => {
                                let txt = delta
                                    .get("signature")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !txt.is_empty() {
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(txt), "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "input_json_delta" => {
                                let call_id = call_order.last().cloned().unwrap_or_default();
                                let partial = delta
                                    .get("partial_json")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !call_id.is_empty() && !partial.is_empty() {
                                    let idx =
                                        call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                    let name =
                                        call_names.get(&call_id).cloned().unwrap_or_default();
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
                                                    "index": idx,
                                                    "id": call_id,
                                                    "type": "function",
                                                    "function": { "name": name, "arguments": partial }
                                                }]
                                            },
                                            "finish_reason": Value::Null
                                        }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_start" => {
                        let cb = data_val
                            .get("content_block")
                            .cloned()
                            .unwrap_or(Value::Null);
                        if cb.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
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
                            if !call_id.is_empty() {
                                if !call_order.contains(&call_id) {
                                    call_order.push(call_id.clone());
                                }
                                if !name.is_empty() {
                                    call_names.insert(call_id.clone(), name.clone());
                                }
                                let idx =
                                    call_order.iter().position(|x| x == &call_id).unwrap_or(0);
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
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": "" }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Gemini => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_gemini_object(&data_val),
                )
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
                            out_text.push_str(text);
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
                        let args =
                            serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                                .unwrap_or_else(|_| "{}".to_string());
                        if !name.is_empty() {
                            if !call_order.contains(&call_id) {
                                call_order.push(call_id.clone());
                            }
                            saw_tool_call = true;
                            let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {
                                        "tool_calls": [{
                                            "index": idx,
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
            ProviderType::Group => {}
        }
    }

    let needs_synthetic_terminal =
        provider_type != ProviderType::ChatCompletion || !saw_upstream_terminal_finish;
    if needs_synthetic_terminal {
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
    }
    let _ = tx.send(Event::default().data("[DONE]")).await;
    Ok(())
}