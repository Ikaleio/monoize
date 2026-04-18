use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_chat_object,
    record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::decode::parse_tool_call_arguments_value;
use crate::urp::stream_helpers::{
    extract_chat_reasoning_content_block, extract_chat_reasoning_delta_chunks,
};
use crate::urp::{
    FinishReason, Node, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent,
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

struct ChatToolCallStreamState<'a> {
    call_order: &'a mut Vec<String>,
    calls: &'a mut HashMap<String, (String, String)>,
    call_id_by_index: &'a mut HashMap<usize, String>,
    response_started: &'a mut bool,
    next_node_index: &'a mut u32,
    tool_node_index_by_call_id: &'a mut HashMap<String, u32>,
}

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
    let mut reasoning_summary = String::new();
    let mut reasoning_source: Option<String> = None;
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut call_id_by_index: HashMap<usize, String> = HashMap::new();

    let mut response_started = false;
    let mut next_node_index = 0u32;
    let mut text_node_index = None;
    let mut reasoning_node_index = None;
    let mut tool_node_index_by_call_id: HashMap<String, u32> = HashMap::new();
    let mut finish_reason = None;

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
            finish_reason = Some(parse_finish_reason(reason));
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

        if delta.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            ensure_response_started(&tx, &response_id, &urp.model, &mut response_started).await?;
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
                    )
                    .await?;
                    continue;
                }

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
                        )
                        .await?;
                    }
                }

                let mut tool_state = ChatToolCallStreamState {
                    call_order: &mut call_order,
                    calls: &mut calls,
                    call_id_by_index: &mut call_id_by_index,
                    response_started: &mut response_started,
                    next_node_index: &mut next_node_index,
                    tool_node_index_by_call_id: &mut tool_node_index_by_call_id,
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
            }
        }

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
            )
            .await?;
        }
        for t in reasoning_text_deltas {
            process_reasoning_text_delta(
                &tx,
                &response_id,
                &urp.model,
                Some(&t.text),
                t.format.as_deref(),
                &mut response_started,
                &mut reasoning_node_index,
                &mut next_node_index,
                &mut reasoning_text,
                &mut reasoning_source,
            )
            .await?;
        }
        for sig in reasoning_sig_deltas {
            process_reasoning_encrypted_delta(
                &tx,
                &response_id,
                &urp.model,
                Some(&Value::String(sig.text)),
                sig.format.as_deref(),
                &mut response_started,
                &mut reasoning_node_index,
                &mut next_node_index,
                &mut reasoning_sig,
                &mut reasoning_source,
            )
            .await?;
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
    }

    if response_started
        || !output_text.is_empty()
        || !reasoning_text.is_empty()
        || !reasoning_summary.is_empty()
        || !call_order.is_empty()
    {
        ensure_response_started(&tx, &response_id, &urp.model, &mut response_started).await?;
    }

    let usage = latest_stream_usage_snapshot(&runtime_metrics).await;
    {
        let total_output_chars =
            (output_text.len() + reasoning_text.len() + reasoning_summary.len()) as u64;
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
        &call_order,
        &calls,
        &tool_node_index_by_call_id,
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
            extra_body: HashMap::new(),
        },
    )
    .await?;

    Ok(())
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
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Text {
            content: text.to_string(),
        },
        HashMap::new(),
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
) -> AppResult<()> {
    let Some(summary) = summary.filter(|summary| !summary.is_empty()) else {
        return Ok(());
    };

    let source = resolve_reasoning_source(reasoning_source, source);
    reasoning_summary.push_str(summary);
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
        HashMap::new(),
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
) -> AppResult<()> {
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return Ok(());
    };

    let source = resolve_reasoning_source(reasoning_source, source);
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
        HashMap::new(),
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
        HashMap::new(),
    )
    .await?;
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
                extra_body: HashMap::new(),
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
            HashMap::new(),
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
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    tool_node_index_by_call_id: &HashMap<String, u32>,
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

    #[tokio::test]
    async fn chat_text_and_tool_nodes_emit_node_first_bridge_events_with_canonical_indices() {
        let (tx, mut rx) = mpsc::channel(32);
        let response_id = "resp_test";
        let model = "gpt-5.4";
        let mut response_started = false;
        let mut next_node_index = 0;
        let mut text_node_index = None;
        let mut output_text = String::new();

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
            ("call_b".to_string(), ("beta".to_string(), "{\"b\":2}".to_string())),
            ("call_a".to_string(), ("alpha".to_string(), "{\"a\":1}".to_string())),
        ]);
        let tool_node_index_by_call_id = HashMap::from([
            ("call_b".to_string(), 5),
            ("call_a".to_string(), 1),
        ]);

        let nodes = sorted_nodes(
            Some("analysis"),
            Some(4),
            "final text",
            Some(0),
            "think",
            "summary",
            "sig",
            Some("anthropic"),
            &call_order,
            &calls,
            &tool_node_index_by_call_id,
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
