use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_chat_object,
    record_stream_done_sentinel, record_stream_terminal_error, record_stream_terminal_event,
    record_stream_usage_if_present, record_visible_output_delta,
};
use crate::handlers::{StreamRuntimeMetrics, StreamTerminalError, UrpRequest as HandlerUrpRequest};
use crate::urp::decode::parse_tool_call_arguments_value;
use crate::urp::stream_helpers::{
    extract_chat_reasoning_content_block, extract_chat_reasoning_delta_chunks,
};
use crate::urp::{
    CHAT_REASONING_DETAIL_EXTRA_KEY, FinishReason, Node, NodeDelta, NodeHeader, OrdinaryRole,
    ProviderProtocol, UrpStreamEvent,
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

const CHAT_CHOICE_EXTRA_BODY_KEY: &str = "_monoize_chat_choice_extra";
const CHAT_DELTA_EXTRA_BODY_KEY: &str = "_monoize_chat_delta_extra";
const CHAT_ERROR_EVENT_EXTRA_KEY: &str = "_monoize_chat_error_event";
const CHAT_NATIVE_FINISH_REASON_EXTRA_KEY: &str = "_monoize_chat_native_finish_reason";

struct ChatToolCallStreamState<'a> {
    call_order: &'a mut Vec<String>,
    calls: &'a mut HashMap<String, (String, String)>,
    call_id_by_index: &'a mut HashMap<usize, String>,
    response_started: &'a mut bool,
    next_node_index: &'a mut u32,
    tool_node_index_by_call_id: &'a mut HashMap<String, u32>,
    delta_extra: &'a mut Map<String, Value>,
}

pub(crate) async fn stream_chat_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut output_text = String::new();
    let mut assistant_message_phase: Option<String> = None;
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut reasoning_summary = String::new();
    let mut reasoning_source: Option<String> = None;
    let mut reasoning_detail_nodes: Vec<(u32, Node)> = Vec::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut call_id_by_index: HashMap<usize, String> = HashMap::new();
    let mut provider_items: Vec<(u32, Node)> = Vec::new();

    let mut response_started = false;
    let mut next_node_index = 0u32;
    let mut text_node_index = None;
    let mut reasoning_node_index = None;
    let mut tool_node_index_by_call_id: HashMap<String, u32> = HashMap::new();
    let mut finish_reason = None;
    let mut protocol_terminal_seen = false;
    let mut terminal_extra_body = HashMap::new();
    let mut pending_delta_extra = Map::new();

    let idle_timeout = std::time::Duration::from_millis(idle_timeout_ms.max(1));
    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                format!("upstream stream idle for {idle_timeout_ms}ms without data"),
            )
        })?
    {
        let ev = match ev {
            Ok(ev) => ev,
            Err(err) => {
                emit_chat_terminal_error(
                    &tx,
                    &runtime_metrics,
                    "upstream_stream_decode_failed",
                    &err.to_string(),
                    None,
                    None,
                )
                .await?;
                return Ok(());
            }
        };
        if tx.is_closed() {
            return Ok(());
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            if !protocol_terminal_seen {
                emit_chat_terminal_error(
                    &tx,
                    &runtime_metrics,
                    "upstream_stream_missing_terminal",
                    "upstream Chat Completions stream sent [DONE] before a terminal finish_reason",
                    None,
                    None,
                )
                .await?;
                return Ok(());
            }
            break;
        }

        let data_val: Value = match serde_json::from_str(&ev.data) {
            Ok(value) => value,
            Err(err) => {
                emit_chat_terminal_error(
                    &tx,
                    &runtime_metrics,
                    "invalid_upstream_sse_json",
                    &format!("invalid Chat Completions SSE JSON: {err}"),
                    None,
                    Some(Value::String(ev.data)),
                )
                .await?;
                return Ok(());
            }
        };
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_chat_object(&data_val))
            .await;

        let choice = data_val
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(Value::as_object);
        if let Some(error) = data_val
            .get("error")
            .filter(|error| !error.is_null())
            .or_else(|| {
                choice
                    .and_then(|choice| choice.get("error"))
                    .filter(|error| !error.is_null())
            })
        {
            let (code, message) = chat_error_code_and_message(error);
            emit_chat_terminal_error(
                &tx,
                &runtime_metrics,
                code.as_deref().unwrap_or("upstream_chat_error"),
                &message,
                Some(data_val.clone()),
                Some(error.clone()),
            )
            .await?;
            return Ok(());
        }

        if let Some(reason) = choice
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|v| v.as_str())
            .filter(|reason| !reason.is_empty())
        {
            if reason == "error" {
                emit_chat_terminal_error(
                    &tx,
                    &runtime_metrics,
                    "upstream_chat_error",
                    "upstream Chat Completions stream terminated with finish_reason=error",
                    Some(data_val.clone()),
                    None,
                )
                .await?;
                return Ok(());
            }
            protocol_terminal_seen = true;
            finish_reason = Some(parse_finish_reason(reason));
            terminal_extra_body.insert(
                CHAT_NATIVE_FINISH_REASON_EXTRA_KEY.to_string(),
                Value::String(reason.to_string()),
            );
            if let Some(choice) = choice {
                let choice_extra = chat_choice_extra(choice);
                if !choice_extra.is_empty() {
                    terminal_extra_body.insert(
                        CHAT_CHOICE_EXTRA_BODY_KEY.to_string(),
                        Value::Object(choice_extra),
                    );
                }
            }
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
        let mut delta_extra = std::mem::take(&mut pending_delta_extra);
        for (key, value) in chat_delta_extra(&delta) {
            delta_extra.insert(key, value);
        }

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
            if !response_started && !chat_delta_has_payload(&delta) && !delta_extra.is_empty() {
                ensure_response_started_with_extra(
                    &tx,
                    &response_id,
                    &urp.model,
                    &mut response_started,
                    chat_delta_event_extra(std::mem::take(&mut delta_extra)),
                )
                .await?;
            } else {
                ensure_response_started(&tx, &response_id, &urp.model, &mut response_started)
                    .await?;
            }
        }

        if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
            process_text_delta(
                &tx,
                &response_id,
                &urp.model,
                t,
                assistant_message_phase.as_deref(),
                &mut response_started,
                &mut text_node_index,
                &mut next_node_index,
                &mut output_text,
                &mut delta_extra,
                started_at,
                &runtime_metrics,
            )
            .await?;
        }

        if let Some(content_blocks) = delta.get("content").and_then(|v| v.as_array()) {
            for (content_pos, block) in content_blocks.iter().enumerate() {
                if let Some(text) = block.as_str() {
                    process_text_delta(
                        &tx,
                        &response_id,
                        &urp.model,
                        text,
                        assistant_message_phase.as_deref(),
                        &mut response_started,
                        &mut text_node_index,
                        &mut next_node_index,
                        &mut output_text,
                        &mut delta_extra,
                        started_at,
                        &runtime_metrics,
                    )
                    .await?;
                    continue;
                }

                let Some(block_obj) = block.as_object() else {
                    continue;
                };

                if let Some(reasoning_block) = extract_chat_reasoning_content_block(block) {
                    process_reasoning_summary_delta(
                        &tx,
                        &response_id,
                        &urp.model,
                        reasoning_block.summary.as_deref(),
                        reasoning_block.format.as_deref(),
                        &mut response_started,
                        &mut reasoning_node_index,
                        &mut next_node_index,
                        &mut reasoning_summary,
                        &mut reasoning_source,
                        &mut delta_extra,
                    )
                    .await?;
                    process_reasoning_text_delta(
                        &tx,
                        &response_id,
                        &urp.model,
                        reasoning_block.content.as_deref(),
                        reasoning_block.format.as_deref(),
                        &mut response_started,
                        &mut reasoning_node_index,
                        &mut next_node_index,
                        &mut reasoning_text,
                        &mut reasoning_source,
                        &mut delta_extra,
                    )
                    .await?;
                    process_reasoning_encrypted_delta(
                        &tx,
                        &response_id,
                        &urp.model,
                        reasoning_block.encrypted.as_ref(),
                        reasoning_block.format.as_deref(),
                        &mut response_started,
                        &mut reasoning_node_index,
                        &mut next_node_index,
                        &mut reasoning_sig,
                        &mut reasoning_source,
                        &mut delta_extra,
                    )
                    .await?;
                    continue;
                }

                let mut recognized = false;
                if let Some(text) = block_obj.get("text").and_then(|v| v.as_str()) {
                    let item_type = block_obj.get("type").and_then(|v| v.as_str());
                    if !matches!(item_type, Some("tool_call" | "function_call" | "tool_use")) {
                        process_text_delta(
                            &tx,
                            &response_id,
                            &urp.model,
                            text,
                            assistant_message_phase.as_deref(),
                            &mut response_started,
                            &mut text_node_index,
                            &mut next_node_index,
                            &mut output_text,
                            &mut delta_extra,
                            started_at,
                            &runtime_metrics,
                        )
                        .await?;
                        recognized = true;
                    }
                }

                let tool_like = chat_stream_block_is_tool_call_like(block_obj);
                let mut tool_state = ChatToolCallStreamState {
                    call_order: &mut call_order,
                    calls: &mut calls,
                    call_id_by_index: &mut call_id_by_index,
                    response_started: &mut response_started,
                    next_node_index: &mut next_node_index,
                    tool_node_index_by_call_id: &mut tool_node_index_by_call_id,
                    delta_extra: &mut delta_extra,
                };
                process_tool_call_delta(
                    &tx,
                    &response_id,
                    &urp.model,
                    block,
                    content_pos,
                    &mut tool_state,
                )
                .await?;
                if tool_like {
                    recognized = true;
                }
                if !recognized {
                    process_provider_item_block(
                        &tx,
                        &response_id,
                        &urp.model,
                        block_obj,
                        &mut response_started,
                        &mut next_node_index,
                        &mut provider_items,
                        &mut delta_extra,
                    )
                    .await?;
                }
            }
        }

        let reasoning_details = delta
            .get("reasoning_details")
            .and_then(Value::as_array)
            .filter(|details| !details.is_empty());
        if let Some(reasoning_details) = reasoning_details {
            for detail in reasoning_details {
                process_reasoning_detail_delta(
                    &tx,
                    &response_id,
                    &urp.model,
                    detail,
                    &mut response_started,
                    &mut next_node_index,
                    &mut reasoning_detail_nodes,
                    &mut delta_extra,
                )
                .await?;
            }
        } else {
            let (reasoning_text_deltas, reasoning_summary_deltas, reasoning_sig_deltas) =
                extract_chat_reasoning_delta_chunks(&delta);
            for summary in reasoning_summary_deltas {
                process_reasoning_summary_delta(
                    &tx,
                    &response_id,
                    &urp.model,
                    Some(&summary.text),
                    summary.format.as_deref(),
                    &mut response_started,
                    &mut reasoning_node_index,
                    &mut next_node_index,
                    &mut reasoning_summary,
                    &mut reasoning_source,
                    &mut delta_extra,
                )
                .await?;
            }
            for text in reasoning_text_deltas {
                process_reasoning_text_delta(
                    &tx,
                    &response_id,
                    &urp.model,
                    Some(&text.text),
                    text.format.as_deref(),
                    &mut response_started,
                    &mut reasoning_node_index,
                    &mut next_node_index,
                    &mut reasoning_text,
                    &mut reasoning_source,
                    &mut delta_extra,
                )
                .await?;
            }
            for encrypted in reasoning_sig_deltas {
                process_reasoning_encrypted_delta(
                    &tx,
                    &response_id,
                    &urp.model,
                    Some(&Value::String(encrypted.text)),
                    encrypted.format.as_deref(),
                    &mut response_started,
                    &mut reasoning_node_index,
                    &mut next_node_index,
                    &mut reasoning_sig,
                    &mut reasoning_source,
                    &mut delta_extra,
                )
                .await?;
            }
        }

        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for (tool_call_pos, tc) in tool_calls.iter().enumerate() {
                let mut tool_state = ChatToolCallStreamState {
                    call_order: &mut call_order,
                    calls: &mut calls,
                    call_id_by_index: &mut call_id_by_index,
                    response_started: &mut response_started,
                    next_node_index: &mut next_node_index,
                    tool_node_index_by_call_id: &mut tool_node_index_by_call_id,
                    delta_extra: &mut delta_extra,
                };
                process_tool_call_delta(
                    &tx,
                    &response_id,
                    &urp.model,
                    tc,
                    tool_call_pos,
                    &mut tool_state,
                )
                .await?;
            }
        }

        if let Some(message) = choice
            .and_then(|choice| choice.get("message"))
            .and_then(Value::as_object)
        {
            process_terminal_message_snapshot(
                &tx,
                &response_id,
                &urp.model,
                message,
                &mut response_started,
                &mut next_node_index,
                &mut assistant_message_phase,
                &mut text_node_index,
                &mut output_text,
                &mut reasoning_node_index,
                &mut reasoning_text,
                &mut reasoning_sig,
                &mut reasoning_source,
                &mut reasoning_detail_nodes,
                &mut call_order,
                &mut calls,
                &mut call_id_by_index,
                &mut tool_node_index_by_call_id,
                &mut provider_items,
                &mut delta_extra,
                started_at,
                &runtime_metrics,
            )
            .await?;
        }

        for (key, value) in delta_extra {
            pending_delta_extra.insert(key, value);
        }
    }

    if !protocol_terminal_seen {
        emit_chat_terminal_error(
            &tx,
            &runtime_metrics,
            "upstream_stream_missing_terminal",
            "upstream Chat Completions stream ended before a terminal finish_reason",
            None,
            None,
        )
        .await?;
        return Ok(());
    }

    if !pending_delta_extra.is_empty() {
        terminal_extra_body.insert(
            CHAT_DELTA_EXTRA_BODY_KEY.to_string(),
            Value::Object(pending_delta_extra),
        );
    }

    if response_started
        || !output_text.is_empty()
        || !reasoning_text.is_empty()
        || !reasoning_summary.is_empty()
        || !reasoning_detail_nodes.is_empty()
        || !call_order.is_empty()
    {
        ensure_response_started(&tx, &response_id, &urp.model, &mut response_started).await?;
    }

    let usage = latest_stream_usage_snapshot(&runtime_metrics).await;
    {
        let total_output_chars = (output_text.len()
            + reasoning_text.len()
            + reasoning_summary.len()
            + reasoning_detail_text_len(&reasoning_detail_nodes))
            as u64;
        crate::handlers::usage::increment_estimated_output_tokens(
            &runtime_metrics,
            total_output_chars,
        )
        .await;
    }
    let output_nodes = sorted_nodes(
        assistant_message_phase.as_deref(),
        text_node_index,
        &output_text,
        reasoning_node_index,
        &reasoning_text,
        &reasoning_summary,
        &reasoning_sig,
        reasoning_source.as_deref(),
        &reasoning_detail_nodes,
        &call_order,
        &calls,
        &tool_node_index_by_call_id,
        &provider_items,
    );

    for (node_index, node) in &output_nodes {
        send_event(
            &tx,
            UrpStreamEvent::NodeDone {
                node_index: *node_index,
                node: node.clone(),
                usage: None,
                extra_body: HashMap::new(),
            },
        )
        .await?;
    }

    send_event(
        &tx,
        UrpStreamEvent::ResponseDone {
            finish_reason,
            usage,
            output: output_nodes.into_iter().map(|(_, node)| node).collect(),
            extra_body: terminal_extra_body,
        },
    )
    .await?;

    Ok(())
}

fn chat_choice_extra(choice: &Map<String, Value>) -> Map<String, Value> {
    choice
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "index" | "delta" | "message" | "finish_reason"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn chat_delta_extra(delta: &Value) -> Map<String, Value> {
    let Some(delta) = delta.as_object() else {
        return Map::new();
    };
    delta
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "role"
                    | "content"
                    | "reasoning"
                    | "reasoning_content"
                    | "reasoning_details"
                    | "reasoning_opaque"
                    | "tool_calls"
                    | "refusal"
                    | "phase"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn chat_delta_has_payload(delta: &Value) -> bool {
    delta.get("content").is_some_and(|value| !value.is_null())
        || delta
            .get("reasoning_details")
            .and_then(Value::as_array)
            .is_some_and(|details| !details.is_empty())
        || [
            "reasoning",
            "reasoning_content",
            "reasoning_opaque",
            "refusal",
        ]
        .iter()
        .any(|field| {
            delta
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
        || delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty())
}

fn chat_delta_event_extra(delta_extra: Map<String, Value>) -> HashMap<String, Value> {
    if delta_extra.is_empty() {
        HashMap::new()
    } else {
        HashMap::from([(
            CHAT_DELTA_EXTRA_BODY_KEY.to_string(),
            Value::Object(delta_extra),
        )])
    }
}

fn chat_error_code_and_message(error: &Value) -> (Option<String>, String) {
    let code = error.get("code").and_then(json_scalar_string);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.is_empty())
        .unwrap_or("upstream Chat Completions error")
        .to_string();
    (code, message)
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

async fn emit_chat_terminal_error(
    tx: &mpsc::Sender<UrpStreamEvent>,
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    fallback_code: &str,
    fallback_message: &str,
    original_event: Option<Value>,
    error_value: Option<Value>,
) -> AppResult<()> {
    let error = error_value.as_ref();
    let code = error
        .and_then(|error| error.get("code"))
        .and_then(json_scalar_string)
        .unwrap_or_else(|| fallback_code.to_string());
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| error.and_then(Value::as_str))
        .filter(|message| !message.is_empty())
        .unwrap_or(fallback_message)
        .to_string();
    let error_type = error
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let param = error
        .and_then(|error| error.get("param"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let http_status = error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_u64)
        .filter(|status| (400..=599).contains(status))
        .map(|status| status as u16)
        .unwrap_or(StatusCode::BAD_GATEWAY.as_u16());

    let mut extra_body = HashMap::new();
    if let Some(original_event) = original_event {
        extra_body.insert(CHAT_ERROR_EVENT_EXTRA_KEY.to_string(), original_event);
    }
    if let Some(error) = error_value {
        extra_body.insert("error".to_string(), error);
    }
    if let Some(error_type) = &error_type {
        extra_body.insert("type".to_string(), Value::String(error_type.clone()));
    }
    if let Some(param) = &param {
        extra_body.insert("param".to_string(), Value::String(param.clone()));
    }

    send_event(
        tx,
        UrpStreamEvent::Error {
            code: Some(code.clone()),
            message: message.clone(),
            extra_body,
        },
    )
    .await?;
    record_stream_terminal_error(
        runtime_metrics,
        "chat.completion.error",
        StreamTerminalError {
            code,
            message,
            http_status,
            error_type,
            param,
        },
    )
    .await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_provider_item_block(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    block_obj: &Map<String, Value>,
    response_started: &mut bool,
    next_node_index: &mut u32,
    provider_items: &mut Vec<(u32, Node)>,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    ensure_response_started(tx, response_id, model, response_started).await?;
    let node_index = *next_node_index;
    *next_node_index += 1;
    let node = Node::ProviderItem {
        id: block_obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        origin_protocol: ProviderProtocol::ChatCompletion,
        role: OrdinaryRole::Assistant,
        item_type: block_obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        body: Value::Object(block_obj.clone()),
        extra_body: HashMap::new(),
    };
    send_event(
        tx,
        UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::ProviderItem {
                id: node.id().cloned(),
                origin_protocol: ProviderProtocol::ChatCompletion,
                role: OrdinaryRole::Assistant,
                item_type: block_obj
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            },
            extra_body: chat_delta_event_extra(std::mem::take(delta_extra)),
        },
    )
    .await?;
    provider_items.push((node_index, node));
    Ok(())
}

fn chat_stream_block_is_tool_call_like(block_obj: &Map<String, Value>) -> bool {
    matches!(
        block_obj.get("type").and_then(|v| v.as_str()),
        Some("tool_call" | "function_call" | "tool_use")
    ) || block_obj.contains_key("function")
        || block_obj.contains_key("call_id")
        || block_obj.contains_key("input")
}

#[allow(clippy::too_many_arguments)]
async fn process_text_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    text: &str,
    phase: Option<&str>,
    response_started: &mut bool,
    text_node_index: &mut Option<u32>,
    next_node_index: &mut u32,
    output_text: &mut String,
    delta_extra: &mut Map<String, Value>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    if text.is_empty() {
        return Ok(());
    }

    let node_index = ensure_node_started(
        tx,
        response_id,
        model,
        response_started,
        text_node_index,
        next_node_index,
        NodeHeader::Text {
            id: None,
            role: OrdinaryRole::Assistant,
            phase: phase.map(str::to_string),
        },
        HashMap::new(),
    )
    .await?;
    output_text.push_str(text);
    record_visible_output_delta(started_at, runtime_metrics, text).await;
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Text {
            content: text.to_string(),
        },
        chat_delta_event_extra(std::mem::take(delta_extra)),
    )
    .await?;
    Ok(())
}

fn resolve_reasoning_source(
    reasoning_source: &mut Option<String>,
    source: Option<&str>,
) -> Option<String> {
    if let Some(source) = source
        .filter(|source| !source.is_empty() && *source != "openrouter")
        .map(|source| source.to_string())
    {
        *reasoning_source = Some(source);
    }
    reasoning_source.clone()
}

fn chat_reasoning_node_from_detail(detail: &Map<String, Value>) -> Option<Node> {
    let detail_type = detail.get("type").and_then(Value::as_str)?;
    if !detail_type.starts_with("reasoning.") {
        return None;
    }

    let id = detail
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let source = detail
        .get("format")
        .and_then(Value::as_str)
        .filter(|format| !format.is_empty())
        .map(str::to_string);
    let content = (detail_type == "reasoning.text")
        .then(|| detail.get("text").and_then(Value::as_str))
        .flatten()
        .map(str::to_string);
    let summary = (detail_type == "reasoning.summary")
        .then(|| detail.get("summary").and_then(Value::as_str))
        .flatten()
        .map(str::to_string);
    let encrypted = (detail_type == "reasoning.encrypted")
        .then(|| detail.get("data"))
        .flatten()
        .filter(|value| !value.is_null())
        .cloned();
    let extra_body = HashMap::from([(
        CHAT_REASONING_DETAIL_EXTRA_KEY.to_string(),
        Value::Object(detail.clone()),
    )]);

    Some(Node::Reasoning {
        id,
        content,
        encrypted,
        summary,
        source,
        extra_body,
    })
}

#[allow(clippy::too_many_arguments)]
async fn process_reasoning_detail_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    detail: &Value,
    response_started: &mut bool,
    next_node_index: &mut u32,
    reasoning_detail_nodes: &mut Vec<(u32, Node)>,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    let Some(detail) = detail.as_object() else {
        return Ok(());
    };
    let Some(node) = chat_reasoning_node_from_detail(detail) else {
        return Ok(());
    };
    let Node::Reasoning {
        id,
        content,
        encrypted,
        summary,
        source,
        extra_body,
    } = &node
    else {
        unreachable!("chat reasoning detail decoder must produce a reasoning node");
    };

    ensure_response_started(tx, response_id, model, response_started).await?;
    let node_index = *next_node_index;
    *next_node_index += 1;
    send_event(
        tx,
        UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::Reasoning { id: id.clone() },
            extra_body: extra_body.clone(),
        },
    )
    .await?;
    let mut event_extra = extra_body.clone();
    event_extra.extend(chat_delta_event_extra(std::mem::take(delta_extra)));
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Reasoning {
            content: content.clone(),
            encrypted: encrypted.clone(),
            summary: summary.clone(),
            source: source.clone(),
        },
        event_extra,
    )
    .await?;
    reasoning_detail_nodes.push((node_index, node));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_reasoning_summary_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    summary: Option<&str>,
    source: Option<&str>,
    response_started: &mut bool,
    reasoning_node_index: &mut Option<u32>,
    next_node_index: &mut u32,
    reasoning_summary: &mut String,
    reasoning_source: &mut Option<String>,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    let Some(summary) = summary.filter(|summary| !summary.is_empty()) else {
        return Ok(());
    };

    let source = resolve_reasoning_source(reasoning_source, source);
    reasoning_summary.push_str(summary);
    let event_extra = chat_delta_event_extra(std::mem::take(delta_extra));
    let node_index = ensure_node_started(
        tx,
        response_id,
        model,
        response_started,
        reasoning_node_index,
        next_node_index,
        NodeHeader::Reasoning { id: None },
        HashMap::new(),
    )
    .await?;
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Reasoning {
            content: None,
            encrypted: None,
            summary: Some(summary.to_string()),
            source,
        },
        event_extra,
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_reasoning_text_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    content: Option<&str>,
    source: Option<&str>,
    response_started: &mut bool,
    reasoning_node_index: &mut Option<u32>,
    next_node_index: &mut u32,
    reasoning_text: &mut String,
    reasoning_source: &mut Option<String>,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return Ok(());
    };

    let source = resolve_reasoning_source(reasoning_source, source);
    let event_extra = chat_delta_event_extra(std::mem::take(delta_extra));
    let node_index = ensure_node_started(
        tx,
        response_id,
        model,
        response_started,
        reasoning_node_index,
        next_node_index,
        NodeHeader::Reasoning { id: None },
        HashMap::new(),
    )
    .await?;
    reasoning_text.push_str(content);
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Reasoning {
            content: Some(content.to_string()),
            encrypted: None,
            summary: None,
            source,
        },
        event_extra,
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_reasoning_encrypted_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    encrypted: Option<&Value>,
    source: Option<&str>,
    response_started: &mut bool,
    reasoning_node_index: &mut Option<u32>,
    next_node_index: &mut u32,
    reasoning_sig: &mut String,
    reasoning_source: &mut Option<String>,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    let Some(encrypted) = encrypted else {
        return Ok(());
    };
    if matches!(encrypted, Value::Null) {
        return Ok(());
    }

    let sig = encrypted
        .as_str()
        .map(|value| value.to_string())
        .unwrap_or_else(|| encrypted.to_string());
    if sig.is_empty() {
        return Ok(());
    }

    let source = resolve_reasoning_source(reasoning_source, source);
    let event_extra = chat_delta_event_extra(std::mem::take(delta_extra));
    let node_index = ensure_node_started(
        tx,
        response_id,
        model,
        response_started,
        reasoning_node_index,
        next_node_index,
        NodeHeader::Reasoning { id: None },
        HashMap::new(),
    )
    .await?;
    reasoning_sig.push_str(&sig);
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Reasoning {
            content: None,
            encrypted: Some(encrypted.clone()),
            summary: None,
            source,
        },
        event_extra,
    )
    .await?;
    Ok(())
}

fn snapshot_suffix<'a>(streamed: &str, snapshot: &'a str) -> Option<&'a str> {
    snapshot
        .strip_prefix(streamed)
        .filter(|suffix| !suffix.is_empty())
}

fn reasoning_detail_raw(node: &Node) -> Option<&Map<String, Value>> {
    let Node::Reasoning { extra_body, .. } = node else {
        return None;
    };
    extra_body
        .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
        .and_then(Value::as_object)
}

fn reasoning_detail_payload_key(detail_type: &str) -> Option<&'static str> {
    match detail_type {
        "reasoning.text" => Some("text"),
        "reasoning.summary" => Some("summary"),
        "reasoning.encrypted" => Some("data"),
        _ => None,
    }
}

fn reasoning_detail_matches(existing: &Node, terminal: &Map<String, Value>) -> bool {
    let Some(existing) = reasoning_detail_raw(existing) else {
        return false;
    };
    if existing == terminal {
        return true;
    }
    let existing_type = existing.get("type").and_then(Value::as_str);
    let terminal_type = terminal.get("type").and_then(Value::as_str);
    if existing_type != terminal_type {
        return false;
    }
    for identity_key in ["id", "index", "tool_call_id"] {
        if (existing.contains_key(identity_key) || terminal.contains_key(identity_key))
            && existing.get(identity_key) != terminal.get(identity_key)
        {
            return false;
        }
    }
    if let (Some(existing_format), Some(terminal_format)) =
        (existing.get("format"), terminal.get("format"))
        && existing_format != terminal_format
    {
        return false;
    }

    let Some(payload_key) = terminal_type.and_then(reasoning_detail_payload_key) else {
        return false;
    };
    match (existing.get(payload_key), terminal.get(payload_key)) {
        (Some(Value::String(existing)), Some(Value::String(terminal))) => {
            existing.starts_with(terminal) || terminal.starts_with(existing)
        }
        (None, _) | (_, None) => true,
        (Some(existing), Some(terminal)) => existing == terminal,
    }
}

fn reasoning_detail_completion(
    existing: &Node,
    terminal: &Map<String, Value>,
) -> Option<(Node, NodeDelta, HashMap<String, Value>)> {
    let existing_raw = reasoning_detail_raw(existing)?;
    let detail_type = terminal.get("type").and_then(Value::as_str)?;
    let payload_key = reasoning_detail_payload_key(detail_type)?;
    let mut merged_raw = existing_raw.clone();
    for (key, value) in terminal {
        merged_raw.insert(key.clone(), value.clone());
    }
    let merged_node = chat_reasoning_node_from_detail(&merged_raw)?;

    let (content, encrypted, summary, payload_delta) = match payload_key {
        "text" => {
            let existing = existing_raw
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("");
            let terminal = merged_raw.get("text").and_then(Value::as_str).unwrap_or("");
            let suffix = snapshot_suffix(existing, terminal)?;
            (
                Some(suffix.to_string()),
                None,
                None,
                Value::String(suffix.to_string()),
            )
        }
        "summary" => {
            let existing = existing_raw
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("");
            let terminal = merged_raw
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("");
            let suffix = snapshot_suffix(existing, terminal)?;
            (
                None,
                None,
                Some(suffix.to_string()),
                Value::String(suffix.to_string()),
            )
        }
        "data" => match (existing_raw.get("data"), merged_raw.get("data")) {
            (Some(Value::String(existing)), Some(Value::String(terminal))) => {
                let suffix = snapshot_suffix(existing, terminal)?;
                (
                    None,
                    Some(Value::String(suffix.to_string())),
                    None,
                    Value::String(suffix.to_string()),
                )
            }
            (None, Some(terminal)) if !terminal.is_null() => {
                (None, Some(terminal.clone()), None, terminal.clone())
            }
            _ => return None,
        },
        _ => return None,
    };

    let mut delta_detail = terminal.clone();
    delta_detail.insert(payload_key.to_string(), payload_delta);
    let extra_body = HashMap::from([(
        CHAT_REASONING_DETAIL_EXTRA_KEY.to_string(),
        Value::Object(delta_detail),
    )]);
    Some((
        merged_node,
        NodeDelta::Reasoning {
            content,
            encrypted,
            summary,
            source: terminal
                .get("format")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        extra_body,
    ))
}

async fn process_terminal_reasoning_details(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    details: &[Value],
    response_started: &mut bool,
    next_node_index: &mut u32,
    reasoning_detail_nodes: &mut Vec<(u32, Node)>,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    let existing_len = reasoning_detail_nodes.len();
    let mut matched = vec![false; existing_len];
    for detail in details {
        let Some(detail_obj) = detail.as_object() else {
            continue;
        };
        let matching_position = reasoning_detail_nodes[..existing_len]
            .iter()
            .enumerate()
            .find_map(|(position, (_, node))| {
                (!matched[position] && reasoning_detail_matches(node, detail_obj))
                    .then_some(position)
            });
        let Some(position) = matching_position else {
            process_reasoning_detail_delta(
                tx,
                response_id,
                model,
                detail,
                response_started,
                next_node_index,
                reasoning_detail_nodes,
                delta_extra,
            )
            .await?;
            continue;
        };
        matched[position] = true;
        let (node_index, existing_node) = &mut reasoning_detail_nodes[position];
        if let Some((merged_node, delta, mut event_extra)) =
            reasoning_detail_completion(existing_node, detail_obj)
        {
            event_extra.extend(chat_delta_event_extra(std::mem::take(delta_extra)));
            send_node_delta(tx, *node_index, delta, event_extra).await?;
            *existing_node = merged_node;
        } else {
            let mut merged_raw = reasoning_detail_raw(existing_node)
                .cloned()
                .unwrap_or_default();
            for (key, value) in detail_obj {
                merged_raw.insert(key.clone(), value.clone());
            }
            if let Some(merged_node) = chat_reasoning_node_from_detail(&merged_raw) {
                *existing_node = merged_node;
            }
        }
    }
    Ok(())
}

fn terminal_tool_call_id(
    tool_call: &Map<String, Value>,
    tool_call_pos: usize,
    call_order: &[String],
    call_id_by_index: &HashMap<usize, String>,
) -> Option<String> {
    tool_call
        .get("id")
        .or_else(|| tool_call.get("call_id"))
        .and_then(Value::as_str)
        .filter(|call_id| !call_id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            tool_call
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|index| call_id_by_index.get(&(index as usize)).cloned())
        })
        .or_else(|| call_order.get(tool_call_pos).cloned())
}

fn tool_call_with_arguments_delta(raw: &Value, arguments: &str) -> Value {
    let mut raw = raw.clone();
    let Some(obj) = raw.as_object_mut() else {
        return raw;
    };
    if let Some(function) = obj.get_mut("function").and_then(Value::as_object_mut) {
        function.insert(
            "arguments".to_string(),
            Value::String(arguments.to_string()),
        );
    } else {
        obj.remove("input");
        obj.insert(
            "arguments".to_string(),
            Value::String(arguments.to_string()),
        );
    }
    raw
}

async fn process_terminal_tool_call_snapshot(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    raw_tool_call: &Value,
    tool_call_pos: usize,
    state: &mut ChatToolCallStreamState<'_>,
) -> AppResult<()> {
    let Some(tool_call) = raw_tool_call.as_object() else {
        return Ok(());
    };
    let Some(call_id) = terminal_tool_call_id(
        tool_call,
        tool_call_pos,
        state.call_order,
        state.call_id_by_index,
    ) else {
        return Ok(());
    };
    let terminal_arguments = tool_call_arguments_delta_text(tool_call).unwrap_or_default();
    if let Some((name, streamed_arguments)) = state.calls.get_mut(&call_id) {
        let terminal_name = tool_call
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| {
                tool_call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("");
        if name.is_empty() && !terminal_name.is_empty() {
            *name = terminal_name.to_string();
        }
        if let Some(suffix) = snapshot_suffix(streamed_arguments, &terminal_arguments) {
            let delta_call = tool_call_with_arguments_delta(raw_tool_call, suffix);
            process_tool_call_delta(tx, response_id, model, &delta_call, tool_call_pos, state)
                .await?;
        }
        return Ok(());
    }
    process_tool_call_delta(tx, response_id, model, raw_tool_call, tool_call_pos, state).await
}

#[allow(clippy::too_many_arguments)]
async fn process_terminal_message_snapshot(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    message: &Map<String, Value>,
    response_started: &mut bool,
    next_node_index: &mut u32,
    assistant_message_phase: &mut Option<String>,
    text_node_index: &mut Option<u32>,
    output_text: &mut String,
    reasoning_node_index: &mut Option<u32>,
    reasoning_text: &mut String,
    reasoning_sig: &mut String,
    reasoning_source: &mut Option<String>,
    reasoning_detail_nodes: &mut Vec<(u32, Node)>,
    call_order: &mut Vec<String>,
    calls: &mut HashMap<String, (String, String)>,
    call_id_by_index: &mut HashMap<usize, String>,
    tool_node_index_by_call_id: &mut HashMap<String, u32>,
    provider_items: &mut Vec<(u32, Node)>,
    delta_extra: &mut Map<String, Value>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    if assistant_message_phase.is_none() {
        *assistant_message_phase = message
            .get("phase")
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    let message_extra = message
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "role"
                    | "content"
                    | "reasoning"
                    | "reasoning_content"
                    | "reasoning_details"
                    | "reasoning_opaque"
                    | "tool_calls"
                    | "refusal"
                    | "phase"
            )
        })
        .filter(|(key, _)| !key.starts_with("_monoize_"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();
    for (key, value) in message_extra {
        delta_extra.insert(key, value);
    }

    if let Some(details) = message
        .get("reasoning_details")
        .and_then(Value::as_array)
        .filter(|details| !details.is_empty())
    {
        process_terminal_reasoning_details(
            tx,
            response_id,
            model,
            details,
            response_started,
            next_node_index,
            reasoning_detail_nodes,
            delta_extra,
        )
        .await?;
    } else if reasoning_detail_nodes.is_empty() {
        if let Some(snapshot) = message
            .get("reasoning")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                message
                    .get("reasoning_content")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            })
            && let Some(suffix) = snapshot_suffix(reasoning_text, snapshot)
        {
            process_reasoning_text_delta(
                tx,
                response_id,
                model,
                Some(suffix),
                None,
                response_started,
                reasoning_node_index,
                next_node_index,
                reasoning_text,
                reasoning_source,
                delta_extra,
            )
            .await?;
        }
        if let Some(snapshot) = message
            .get("reasoning_opaque")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            && let Some(suffix) = snapshot_suffix(reasoning_sig, snapshot)
        {
            process_reasoning_encrypted_delta(
                tx,
                response_id,
                model,
                Some(&Value::String(suffix.to_string())),
                None,
                response_started,
                reasoning_node_index,
                next_node_index,
                reasoning_sig,
                reasoning_source,
                delta_extra,
            )
            .await?;
        }
    }

    let mut snapshot_text = String::new();
    if let Some(content) = message.get("content") {
        if let Some(text) = content.as_str() {
            snapshot_text.push_str(text);
            if let Some(suffix) = snapshot_suffix(output_text, &snapshot_text) {
                process_text_delta(
                    tx,
                    response_id,
                    model,
                    suffix,
                    assistant_message_phase.as_deref(),
                    response_started,
                    text_node_index,
                    next_node_index,
                    output_text,
                    delta_extra,
                    started_at,
                    runtime_metrics,
                )
                .await?;
            }
        } else if let Some(blocks) = content.as_array() {
            for (block_pos, block) in blocks.iter().enumerate() {
                if let Some(text) = block.as_str() {
                    snapshot_text.push_str(text);
                    if let Some(suffix) = snapshot_suffix(output_text, &snapshot_text) {
                        process_text_delta(
                            tx,
                            response_id,
                            model,
                            suffix,
                            assistant_message_phase.as_deref(),
                            response_started,
                            text_node_index,
                            next_node_index,
                            output_text,
                            delta_extra,
                            started_at,
                            runtime_metrics,
                        )
                        .await?;
                    }
                    continue;
                }
                let Some(block_obj) = block.as_object() else {
                    continue;
                };
                if let Some(text) = block_obj.get("text").and_then(Value::as_str)
                    && !matches!(
                        block_obj.get("type").and_then(Value::as_str),
                        Some("tool_call" | "function_call" | "tool_use")
                    )
                {
                    snapshot_text.push_str(text);
                    if let Some(suffix) = snapshot_suffix(output_text, &snapshot_text) {
                        process_text_delta(
                            tx,
                            response_id,
                            model,
                            suffix,
                            assistant_message_phase.as_deref(),
                            response_started,
                            text_node_index,
                            next_node_index,
                            output_text,
                            delta_extra,
                            started_at,
                            runtime_metrics,
                        )
                        .await?;
                    }
                    continue;
                }
                if chat_stream_block_is_tool_call_like(block_obj) {
                    let mut tool_state = ChatToolCallStreamState {
                        call_order,
                        calls,
                        call_id_by_index,
                        response_started,
                        next_node_index,
                        tool_node_index_by_call_id,
                        delta_extra,
                    };
                    process_terminal_tool_call_snapshot(
                        tx,
                        response_id,
                        model,
                        block,
                        block_pos,
                        &mut tool_state,
                    )
                    .await?;
                } else if !provider_items.iter().any(
                    |(_, node)| matches!(node, Node::ProviderItem { body, .. } if body == block),
                ) {
                    process_provider_item_block(
                        tx,
                        response_id,
                        model,
                        block_obj,
                        response_started,
                        next_node_index,
                        provider_items,
                        delta_extra,
                    )
                    .await?;
                }
            }
        }
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (tool_call_pos, tool_call) in tool_calls.iter().enumerate() {
            let mut tool_state = ChatToolCallStreamState {
                call_order,
                calls,
                call_id_by_index,
                response_started,
                next_node_index,
                tool_node_index_by_call_id,
                delta_extra,
            };
            process_terminal_tool_call_snapshot(
                tx,
                response_id,
                model,
                tool_call,
                tool_call_pos,
                &mut tool_state,
            )
            .await?;
        }
    }
    Ok(())
}

async fn process_tool_call_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    raw_tool_call: &Value,
    tool_call_pos: usize,
    state: &mut ChatToolCallStreamState<'_>,
) -> AppResult<()> {
    let Some(tc_obj) = raw_tool_call.as_object() else {
        return Ok(());
    };

    let tc_index = tc_obj
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let mut call_id = tc_obj
        .get("id")
        .or_else(|| tc_obj.get("call_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if call_id.is_empty() {
        if let Some(idx) = tc_index {
            if let Some(existing) = state.call_id_by_index.get(&idx) {
                call_id = existing.clone();
            }
        }
    }
    if call_id.is_empty() && tool_call_pos == 0 {
        if let Some(last) = state.call_order.last() {
            call_id = last.clone();
        }
    }
    if call_id.is_empty() {
        if let Some(existing) = state.call_order.get(tool_call_pos) {
            call_id = existing.clone();
        }
    }
    if call_id.is_empty() {
        return Ok(());
    }
    if let Some(idx) = tc_index {
        state.call_id_by_index.insert(idx, call_id.clone());
    }

    let name = tc_obj
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| {
            tc_obj
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();
    let args_delta = tool_call_arguments_delta_text(tc_obj).unwrap_or_default();

    if !state.calls.contains_key(&call_id) {
        state.call_order.push(call_id.clone());
        state
            .calls
            .insert(call_id.clone(), (name.clone(), String::new()));
    }

    ensure_response_started(tx, response_id, model, state.response_started).await?;

    let node_index = if let Some(node_index) = state.tool_node_index_by_call_id.get(&call_id) {
        *node_index
    } else {
        let node_index = *state.next_node_index;
        *state.next_node_index += 1;
        state
            .tool_node_index_by_call_id
            .insert(call_id.clone(), node_index);
        send_event(
            tx,
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::ToolCall {
                    id: None,
                    call_id: call_id.clone(),
                    name: name.clone(),
                },
                extra_body: chat_delta_event_extra(std::mem::take(state.delta_extra)),
            },
        )
        .await?;
        node_index
    };

    let Some(entry) = state.calls.get_mut(&call_id) else {
        tracing::warn!(call_id = %call_id, "unknown call_id in tool call stream delta, skipping");
        return Ok(());
    };

    if !name.is_empty() && entry.0.is_empty() {
        entry.0 = name.clone();
    }
    if !args_delta.is_empty() {
        entry.1.push_str(&args_delta);
        send_node_delta(
            tx,
            node_index,
            NodeDelta::ToolCallArguments {
                arguments: args_delta,
            },
            chat_delta_event_extra(std::mem::take(state.delta_extra)),
        )
        .await?;
    }

    Ok(())
}

fn tool_call_arguments_delta_text(tc_obj: &Map<String, Value>) -> Option<String> {
    let value = parse_tool_call_arguments_value(tc_obj)?;
    if tc_obj.get("type").and_then(|v| v.as_str()) == Some("tool_use")
        && value.as_object().map(|obj| obj.is_empty()).unwrap_or(false)
    {
        return None;
    }
    value
        .as_str()
        .map(|text| text.to_string())
        .or_else(|| Some(value.to_string()))
        .filter(|text| !text.is_empty())
}

async fn ensure_response_started(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    response_started: &mut bool,
) -> AppResult<()> {
    ensure_response_started_with_extra(tx, response_id, model, response_started, HashMap::new())
        .await
}

async fn ensure_response_started_with_extra(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    response_started: &mut bool,
    extra_body: HashMap<String, Value>,
) -> AppResult<()> {
    if !*response_started {
        send_event(
            tx,
            UrpStreamEvent::ResponseStart {
                id: response_id.to_string(),
                model: model.to_string(),
                extra_body,
            },
        )
        .await?;
        *response_started = true;
    }
    Ok(())
}

async fn ensure_node_started(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    response_started: &mut bool,
    slot: &mut Option<u32>,
    next_node_index: &mut u32,
    node_header: NodeHeader,
    extra_body: HashMap<String, Value>,
) -> AppResult<u32> {
    if let Some(node_index) = *slot {
        return Ok(node_index);
    }
    ensure_response_started(tx, response_id, model, response_started).await?;
    let node_index = *next_node_index;
    *next_node_index += 1;
    *slot = Some(node_index);
    send_event(
        tx,
        UrpStreamEvent::NodeStart {
            node_index,
            header: node_header,
            extra_body: extra_body.clone(),
        },
    )
    .await?;
    Ok(node_index)
}

async fn send_node_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    node_index: u32,
    delta: NodeDelta,
    extra_body: HashMap<String, Value>,
) -> AppResult<()> {
    send_event(
        tx,
        UrpStreamEvent::NodeDelta {
            node_index,
            delta: delta.clone(),
            usage: None,
            extra_body: extra_body.clone(),
        },
    )
    .await
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

fn reasoning_detail_text_len(nodes: &[(u32, Node)]) -> usize {
    nodes
        .iter()
        .map(|(_, node)| match node {
            Node::Reasoning {
                content, summary, ..
            } => {
                content.as_deref().map(str::len).unwrap_or_default()
                    + summary.as_deref().map(str::len).unwrap_or_default()
            }
            _ => 0,
        })
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn sorted_nodes(
    assistant_message_phase: Option<&str>,
    text_node_index: Option<u32>,
    output_text: &str,
    reasoning_node_index: Option<u32>,
    reasoning_text: &str,
    reasoning_summary: &str,
    reasoning_sig: &str,
    reasoning_source: Option<&str>,
    reasoning_detail_nodes: &[(u32, Node)],
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    tool_node_index_by_call_id: &HashMap<String, u32>,
    provider_items: &[(u32, Node)],
) -> Vec<(u32, Node)> {
    let mut nodes = Vec::new();

    if let Some(node_index) = reasoning_node_index {
        nodes.push((
            node_index,
            Node::Reasoning {
                id: Some(crate::urp::synthetic_reasoning_id()),
                content: (!reasoning_text.is_empty()).then(|| reasoning_text.to_string()),
                encrypted: (!reasoning_sig.is_empty())
                    .then(|| Value::String(reasoning_sig.to_string())),
                summary: (!reasoning_summary.is_empty()).then(|| reasoning_summary.to_string()),
                source: reasoning_source.map(|source| source.to_string()),
                extra_body: HashMap::new(),
            },
        ));
    }

    nodes.extend(reasoning_detail_nodes.iter().cloned());

    if let Some(node_index) = text_node_index {
        nodes.push((
            node_index,
            Node::Text {
                id: Some(crate::urp::synthetic_message_id()),
                role: OrdinaryRole::Assistant,
                content: output_text.to_string(),
                phase: assistant_message_phase.map(str::to_string),
                extra_body: HashMap::new(),
            },
        ));
    }

    for call_id in call_order {
        let Some(node_index) = tool_node_index_by_call_id.get(call_id).copied() else {
            continue;
        };
        let Some((name, arguments)) = calls.get(call_id) else {
            continue;
        };
        nodes.push((
            node_index,
            Node::ToolCall {
                id: Some(crate::urp::synthetic_tool_call_id()),
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                extra_body: HashMap::new(),
            },
        ));
    }

    nodes.extend(provider_items.iter().cloned());
    nodes.sort_by_key(|(node_index, _)| *node_index);
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn resolve_reasoning_source_preserves_none_without_fallback() {
        let mut reasoning_source = None;

        assert_eq!(resolve_reasoning_source(&mut reasoning_source, None), None);
        assert_eq!(reasoning_source, None);
    }

    #[test]
    fn resolve_reasoning_source_preserves_explicit_upstream_value() {
        let mut reasoning_source = None;

        assert_eq!(
            resolve_reasoning_source(&mut reasoning_source, Some("anthropic")),
            Some("anthropic".to_string())
        );
        assert_eq!(reasoning_source, Some("anthropic".to_string()));
        assert_eq!(
            resolve_reasoning_source(&mut reasoning_source, None),
            Some("anthropic".to_string())
        );
    }

    #[test]
    fn chat_reasoning_details_map_to_distinct_ordered_nodes_with_raw_entries() {
        let details = serde_json::json!([
            { "type": "reasoning.summary", "summary": "first", "id": "sum_1", "format": "openrouter", "index": 0, "future": "s" },
            { "type": "reasoning.text", "text": "second", "signature": "native", "id": "txt_1", "format": "openrouter", "index": 1 },
            { "type": "reasoning.text", "text": "second", "signature": "native", "id": "txt_1", "format": "openrouter", "index": 1 },
            { "type": "reasoning.encrypted", "data": "third", "id": "enc_1", "format": "openrouter", "index": 2 },
            { "type": "reasoning.server_tool_call", "tool_name": "openrouter:fusion", "arguments": "{\"q\":1}", "result": "{\"ok\":true}", "id": "srv_1", "index": 3 }
        ]);
        let detail_values = details.as_array().expect("detail array");
        let nodes = detail_values
            .iter()
            .map(|detail| {
                chat_reasoning_node_from_detail(detail.as_object().expect("detail object"))
                    .expect("reasoning node")
            })
            .collect::<Vec<_>>();

        assert_eq!(nodes.len(), detail_values.len());
        for (node, expected) in nodes.iter().zip(detail_values) {
            let Node::Reasoning { extra_body, .. } = node else {
                panic!("expected reasoning node");
            };
            assert_eq!(
                extra_body.get(CHAT_REASONING_DETAIL_EXTRA_KEY),
                Some(expected)
            );
        }
        assert_eq!(nodes[1], nodes[2], "byte-identical details must repeat");
        assert!(matches!(
            &nodes[4],
            Node::Reasoning {
                content: None,
                encrypted: None,
                summary: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn chat_text_and_tool_nodes_emit_node_first_bridge_events_with_canonical_indices() {
        let (tx, mut rx) = mpsc::channel(32);
        let response_id = "resp_test";
        let model = "gpt-5.4";
        let mut response_started = false;
        let mut next_node_index = 0;
        let mut text_node_index = None;
        let mut output_text = String::new();
        let mut delta_extra = Map::new();

        process_text_delta(
            &tx,
            response_id,
            model,
            "hello",
            Some("analysis"),
            &mut response_started,
            &mut text_node_index,
            &mut next_node_index,
            &mut output_text,
            &mut delta_extra,
            None,
            &None,
        )
        .await
        .expect("text delta should succeed");

        let mut call_order = Vec::new();
        let mut calls = HashMap::new();
        let mut call_id_by_index = HashMap::new();
        let mut tool_node_index_by_call_id = HashMap::new();
        let mut tool_state = ChatToolCallStreamState {
            call_order: &mut call_order,
            calls: &mut calls,
            call_id_by_index: &mut call_id_by_index,
            response_started: &mut response_started,
            next_node_index: &mut next_node_index,
            tool_node_index_by_call_id: &mut tool_node_index_by_call_id,
            delta_extra: &mut delta_extra,
        };

        process_tool_call_delta(
            &tx,
            response_id,
            model,
            &serde_json::json!({
                "index": 0,
                "id": "call_1",
                "function": {
                    "name": "lookup",
                    "arguments": "{\"a\":1}"
                }
            }),
            0,
            &mut tool_state,
        )
        .await
        .expect("tool call delta should succeed");

        drop(tx);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        assert!(matches!(
            &events[0],
            UrpStreamEvent::ResponseStart { id, model, .. }
                if id == response_id && model == "gpt-5.4"
        ));
        assert!(matches!(
            &events[1],
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Text { role: OrdinaryRole::Assistant, phase, .. },
                ..
            } if *node_index == 0 && phase.as_deref() == Some("analysis")
        ));
        assert!(matches!(
            &events[2],
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Text { content },
                ..
            } if *node_index == 0 && content == "hello"
        ));
        assert!(matches!(
            &events[3],
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::ToolCall { call_id, name, .. },
                ..
            } if *node_index == 1 && call_id == "call_1" && name == "lookup"
        ));
        assert!(matches!(
            &events[4],
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::ToolCallArguments { arguments },
                ..
            } if *node_index == 1 && arguments == "{\"a\":1}"
        ));
    }

    #[test]
    fn chat_completion_builds_terminal_nodes_from_sorted_node_state() {
        let call_order = vec!["call_b".to_string(), "call_a".to_string()];
        let calls = HashMap::from([
            (
                "call_b".to_string(),
                ("beta".to_string(), "{\"b\":2}".to_string()),
            ),
            (
                "call_a".to_string(),
                ("alpha".to_string(), "{\"a\":1}".to_string()),
            ),
        ]);
        let tool_node_index_by_call_id =
            HashMap::from([("call_b".to_string(), 5), ("call_a".to_string(), 1)]);

        let nodes = sorted_nodes(
            Some("analysis"),
            Some(4),
            "final text",
            Some(0),
            "think",
            "summary",
            "sig",
            Some("anthropic"),
            &[],
            &call_order,
            &calls,
            &tool_node_index_by_call_id,
            &[],
        );

        assert_eq!(nodes.len(), 4);
        assert!(matches!(
            &nodes[0],
            (0, Node::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                source: Some(source),
                ..
            }) if content == "think" && summary == "summary" && sig == "sig" && source == "anthropic"
        ));
        assert!(matches!(
            &nodes[1],
            (1, Node::ToolCall { call_id, name, arguments, .. })
                if call_id == "call_a" && name == "alpha" && arguments == "{\"a\":1}"
        ));
        assert!(matches!(
            &nodes[2],
            (4, Node::Text { content, phase: Some(phase), .. }) if content == "final text" && phase == "analysis"
        ));
        assert!(matches!(
            &nodes[3],
            (5, Node::ToolCall { call_id, name, arguments, .. })
                if call_id == "call_b" && name == "beta" && arguments == "{\"b\":2}"
        ));
    }
}
