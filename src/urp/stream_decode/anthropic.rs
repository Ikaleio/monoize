use crate::config::ProviderType;
use crate::error::{AppError, AppResult};
use crate::handlers::routing::{now_ts, wrap_responses_event};
use crate::handlers::usage::{latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_chat_object, parse_usage_from_messages_object, parse_usage_from_responses_object, record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present, usage_to_messages_usage_json, usage_to_responses_usage_json};
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

pub(crate) async fn stream_messages_sse_as_responses(
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
    let mut current_tool_call_id: Option<String> = None;

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
            json!({"type":"message","role":"assistant","content":[]}),
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
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_messages_object(&data_val),
        )
        .await;
        let event_type = data_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match event_type {
            "content_block_start" => {
                let cb = data_val
                    .get("content_block")
                    .cloned()
                    .unwrap_or(Value::Null);
                let cb_type = cb.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if cb_type == "tool_use" {
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
                                item,
                            ))
                            .await;
                    }
                }
            }
            "content_block_delta" => {
                let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                            if !text.is_empty() {
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
                    }
                    "thinking_delta" => {
                        if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                            if !t.is_empty() {
                                reasoning_text.push_str(t);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_text.delta",
                                        json!({ "delta": t }),
                                    ))
                                    .await;
                            }
                        }
                    }
                    "signature_delta" => {
                        if let Some(s) = delta.get("signature").and_then(|v| v.as_str()) {
                            if !s.is_empty() {
                                reasoning_sig.push_str(s);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_signature.delta",
                                        json!({ "delta": s }),
                                    ))
                                    .await;
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            if let Some(call_id) = current_tool_call_id.clone() {
                                if let Some(entry) = calls.get_mut(&call_id) {
                                    entry.1.push_str(partial);
                                    let _ = tx
                                        .send(wrap_responses_event(
                                            &mut seq,
                                            "response.function_call_arguments.delta",
                                            json!({ "call_id": call_id, "name": entry.0, "delta": partial }),
                                        ))
                                        .await;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                current_tool_call_id = None;
            }
            "message_stop" => {
                record_stream_terminal_event(&runtime_metrics, "message_stop", None).await;
                break;
            }
            _ => {}
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

pub(crate) async fn stream_any_sse_as_messages(
    urp: &HandlerUrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    if provider_type == ProviderType::Messages {
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
            if data_val.get("type").and_then(|v| v.as_str()) == Some("message_stop") {
                record_stream_terminal_event(&runtime_metrics, "message_stop", None).await;
            }
            let _ = tx.send(Event::default().data(ev.data)).await;
        }
        return Ok(());
    }

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
    let mut saw_responses_text_delta = false;
    let mut saw_responses_tool_delta = false;
    let mut saw_responses_reasoning_delta = false;
    let mut tool_indices: HashMap<String, u32> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut started: Vec<u32> = Vec::new();

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

        match provider_type {
            ProviderType::ChatCompletion => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_chat_object(&data_val),
                )
                .await;
                let delta = data_val
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("delta"))
                    .cloned()
                    .unwrap_or(Value::Null);

                let (reasoning_text_deltas, reasoning_sig_deltas) =
                    extract_chat_reasoning_deltas(&delta);
                for t in reasoning_text_deltas {
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
                        "delta": { "type": "thinking_delta", "thinking": t }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }
                for s in reasoning_sig_deltas {
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
                        "delta": { "type": "signature_delta", "signature": s }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }

                if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
                    if !t.is_empty() {
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
                            "delta": { "type": "text_delta", "text": t }
                        });
                        let _ = tx.send(Event::default().data(d.to_string())).await;
                    }
                }

                if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tool_calls {
                        let call_id = tc
                            .get("id")
                            .or_else(|| tc.get("call_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if call_id.is_empty() {
                            continue;
                        }
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
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
                            let idx = ensure_anthropic_text_block(
                                &tx,
                                &mut text_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "text_delta", "text": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
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
                            let idx = ensure_anthropic_thinking_block(
                                &tx,
                                &mut thinking_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "thinking_delta", "thinking": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                    "response.reasoning_signature.delta" => {
                        let t = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_reasoning_delta = true;
                            let idx = ensure_anthropic_thinking_block(
                                &tx,
                                &mut thinking_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "signature_delta", "signature": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                    "response.output_item.added" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id =
                                item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if !call_id.is_empty() {
                                saw_responses_tool_delta = true;
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index
                                        .insert(output_index, call_id.to_string());
                                }
                                let _ = ensure_anthropic_tool_block(
                                    &tx,
                                    &mut tool_indices,
                                    &mut tool_names,
                                    &mut next_index,
                                    &mut started,
                                    call_id,
                                    name,
                                )
                                .await?;
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let call_id = data_val
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                data_val
                                    .get("output_index")
                                    .and_then(|v| v.as_u64())
                                    .and_then(|idx| {
                                        call_ids_by_output_index.get(&idx).map(|s| s.as_str())
                                    })
                            })
                            .unwrap_or("");
                        let name = data_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !call_id.is_empty() {
                            saw_responses_tool_delta = true;
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
                            if !delta.is_empty() {
                                let d = json!({
                                    "type": "content_block_delta",
                                    "index": idx,
                                    "delta": { "type": "input_json_delta", "partial_json": delta }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                        }
                    }
                    "response.output_item.done" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id =
                                item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                            if !call_id.is_empty() {
                                saw_responses_tool_delta = true;
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index
                                        .insert(output_index, call_id.to_string());
                                }
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
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                            if !saw_responses_text_delta {
                                let text = extract_responses_message_text(item);
                                if !text.is_empty() {
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
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                            let (reasoning_text, reasoning_sig) =
                                extract_reasoning_text_and_signature(item);
                            if !reasoning_text.is_empty() {
                                saw_responses_reasoning_delta = true;
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
                                    "delta": { "type": "thinking_delta", "thinking": reasoning_text }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                            if !reasoning_sig.is_empty() {
                                saw_responses_reasoning_delta = true;
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
                                    "delta": { "type": "signature_delta", "signature": reasoning_sig }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                        }
                    }
                    "response.completed" => {
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
                                    let call_id =
                                        item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                                    if call_id.is_empty() {
                                        continue;
                                    }
                                    let name =
                                        item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    let args = item
                                        .get("arguments")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
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
                                "message" => {
                                    if saw_responses_text_delta {
                                        continue;
                                    }
                                    let text = extract_responses_message_text(item);
                                    if !text.is_empty() {
                                        saw_responses_text_delta = true;
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
                                "reasoning" => {
                                    if saw_responses_reasoning_delta {
                                        continue;
                                    }
                                    let (reasoning_text, reasoning_sig) =
                                        extract_reasoning_text_and_signature(item);
                                    if !reasoning_text.is_empty() {
                                        saw_responses_reasoning_delta = true;
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
                                            "delta": { "type": "thinking_delta", "thinking": reasoning_text }
                                        });
                                        let _ = tx.send(Event::default().data(d.to_string())).await;
                                    }
                                    if !reasoning_sig.is_empty() {
                                        saw_responses_reasoning_delta = true;
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
                                            "delta": { "type": "signature_delta", "signature": reasoning_sig }
                                        });
                                        let _ = tx.send(Event::default().data(d.to_string())).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Gemini | ProviderType::Group | ProviderType::Messages => {}
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
            "stop_reason": if tool_indices.is_empty() { "end_turn" } else { "tool_use" },
            "stop_sequence": Value::Null
        },
        "usage": message_usage
    });
    let _ = tx
        .send(Event::default().data(message_delta.to_string()))
        .await;
    let stop = json!({ "type": "message_stop" });
    let _ = tx.send(Event::default().data(stop.to_string())).await;
    record_stream_terminal_event(
        &runtime_metrics,
        "message_stop",
        Some(if tool_indices.is_empty() {
            "end_turn"
        } else {
            "tool_use"
        }),
    )
    .await;
    Ok(())
}
