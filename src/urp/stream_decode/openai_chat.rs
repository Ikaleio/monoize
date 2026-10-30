use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    StreamChunkMetrics, latest_stream_usage_snapshot, parse_usage_from_chat_object,
    record_stream_chunk_metrics, record_stream_terminal_error, record_stream_terminal_event,
    response_service_tier,
};
use crate::handlers::{StreamRuntimeMetrics, StreamTerminalError, UrpRequest as HandlerUrpRequest};
use crate::urp::decode::{parse_compatible_media_part, parse_tool_call_arguments_value};
use crate::urp::reasoning::chat_reasoning_node_from_detail;
use crate::urp::stream_helpers::{
    extract_chat_reasoning_content_block, extract_chat_reasoning_delta_chunks,
};
use crate::urp::{
    CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY, CHAT_REASONING_DETAIL_EXTRA_KEY, FinishReason, Node,
    NodeDelta, NodeHeader, OrdinaryRole, ProviderProtocol, ToolCallType, UrpStreamEvent,
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

static NULL_DELTA: Value = Value::Null;

const CHAT_CHOICE_EXTRA_BODY_KEY: &str = "_monoize_chat_choice_extra";
const CHAT_DELTA_EXTRA_BODY_KEY: &str = "_monoize_chat_delta_extra";
const CHAT_ERROR_EVENT_EXTRA_KEY: &str = "_monoize_chat_error_event";
const CHAT_NATIVE_FINISH_REASON_EXTRA_KEY: &str = "_monoize_chat_native_finish_reason";

struct ChatToolCallStreamState<'a> {
    call_order: &'a mut Vec<String>,
    calls: &'a mut HashMap<String, (ToolCallType, String, String, bool)>,
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
    // Visible output bytes ride along with the next event's single metrics lock; whatever is
    // still pending when decoding stops (on any exit path) is flushed here.
    let mut pending_visible_output_bytes = 0u64;
    let result = decode_chat_stream(
        urp,
        upstream_resp,
        tx,
        started_at,
        runtime_metrics.clone(),
        idle_timeout_ms,
        &mut pending_visible_output_bytes,
    )
    .await;
    record_stream_chunk_metrics(
        &runtime_metrics,
        StreamChunkMetrics {
            visible_output_bytes: pending_visible_output_bytes,
            ..StreamChunkMetrics::default()
        },
    )
    .await;
    result
}

async fn decode_chat_stream(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    mut started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
    pending_visible_output_bytes: &mut u64,
) -> AppResult<()> {
    let mut response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut output_text = String::new();
    let mut text_logprobs = None;
    let mut refusal_logprobs = None;
    let mut citations = Vec::new();
    let mut refusal_text = String::new();
    let mut refusal_node_index = None;
    let mut audio_fields = Map::new();
    let mut audio_node_index = None;
    let mut latest_usage = None;
    let mut assistant_message_phase: Option<String> = None;
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut reasoning_summary = String::new();
    let mut reasoning_source: Option<String> = None;
    let mut reasoning_surface: Option<&'static str> = None;
    let mut first_chunk_extra: Option<Map<String, Value>> = None;
    let mut reasoning_detail_nodes: Vec<(u32, Node)> = Vec::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (ToolCallType, String, String, bool)> = HashMap::new();
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
    let mut segments = StreamSegments::default();
    let mut held_encrypted_placeholders: Vec<Value> = Vec::new();

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
                    &crate::error::sse_error_chain(&err),
                    None,
                    None,
                )
                .await?;
                return Ok(());
            }
        };
        let ttfb_ms = started_at
            .take()
            .map(|started_at| started_at.elapsed().as_millis() as u64);
        if ev.data.trim() == "[DONE]" {
            record_stream_chunk_metrics(
                &runtime_metrics,
                StreamChunkMetrics {
                    ttfb_ms,
                    done_sentinel: true,
                    visible_output_bytes: std::mem::take(pending_visible_output_bytes),
                    ..StreamChunkMetrics::default()
                },
            )
            .await;
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
                record_stream_chunk_metrics(
                    &runtime_metrics,
                    StreamChunkMetrics {
                        ttfb_ms,
                        ..StreamChunkMetrics::default()
                    },
                )
                .await;
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
        if !response_started && let Some(id) = data_val.get("id").and_then(Value::as_str) {
            response_id = id.into();
        }
        let usage = parse_usage_from_chat_object(&data_val);
        if usage.is_some() {
            latest_usage.clone_from(&usage);
        }
        record_stream_chunk_metrics(
            &runtime_metrics,
            StreamChunkMetrics {
                ttfb_ms,
                done_sentinel: false,
                response_service_tier: response_service_tier(&data_val),
                response_model: data_val.get("model").and_then(Value::as_str),
                usage,
                visible_output_bytes: std::mem::take(pending_visible_output_bytes),
            },
        )
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
            if parse_finish_reason(reason) == FinishReason::Other {
                terminal_extra_body.insert(
                    CHAT_NATIVE_FINISH_REASON_EXTRA_KEY.to_string(),
                    Value::String(reason.to_string()),
                );
            }
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

        let delta = choice
            .and_then(|choice| choice.get("delta"))
            .unwrap_or(&NULL_DELTA);
        for content in [
            delta.get("content"),
            choice
                .and_then(|choice| choice.get("message"))
                .and_then(|message| message.get("content")),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(message) = validate_chat_content_media(content) {
                emit_chat_terminal_error(
                    &tx,
                    &runtime_metrics,
                    "malformed_media",
                    &message,
                    None,
                    None,
                )
                .await?;
                return Ok(());
            }
        }
        let mut delta_extra = std::mem::take(&mut pending_delta_extra);
        for (key, value) in chat_delta_extra(delta) {
            delta_extra.insert(key, value);
        }
        if !protocol_terminal_seen && let Some(choice) = choice {
            let choice_extra = chat_choice_extra(choice);
            if !choice_extra.is_empty() {
                delta_extra.insert(
                    CHAT_CHOICE_EXTRA_BODY_KEY.to_string(),
                    Value::Object(choice_extra),
                );
            }
        }

        if assistant_message_phase.is_none() {
            assistant_message_phase = delta
                .get("phase")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }

        if first_chunk_extra.is_none() {
            let chunk_extra = chat_chunk_extra(&data_val);
            if !chunk_extra.is_empty() {
                first_chunk_extra = Some(chunk_extra);
            }
        }
        if delta.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            let start_extra =
                if !response_started && !chat_delta_has_payload(delta) && !delta_extra.is_empty() {
                    chat_delta_event_extra(std::mem::take(&mut delta_extra))
                } else {
                    HashMap::new()
                };
            ensure_response_started_with_extra(
                &tx,
                &response_id,
                &urp.model,
                &mut response_started,
                with_chunk_extra(start_extra, first_chunk_extra.as_ref()),
            )
            .await?;
        } else if !response_started
            && (chat_delta_has_payload(delta)
                || choice.is_some_and(|choice| choice.contains_key("message")))
        {
            ensure_response_started_with_extra(
                &tx,
                &response_id,
                &urp.model,
                &mut response_started,
                with_chunk_extra(HashMap::new(), first_chunk_extra.as_ref()),
            )
            .await?;
        }

        // PC7h: nodes are sequential; a switch of output kind completes the previous node.
        macro_rules! close_reasoning {
            ($force:expr) => {
                close_reasoning_nodes(
                    &tx,
                    &mut segments,
                    &mut reasoning_node_index,
                    &reasoning_text,
                    &reasoning_summary,
                    &reasoning_sig,
                    &mut reasoning_source,
                    &mut reasoning_surface,
                    &reasoning_detail_nodes,
                    $force,
                )
                .await?
            };
        }
        macro_rules! close_text {
            () => {
                close_text_node(
                    &tx,
                    &mut segments,
                    &mut text_node_index,
                    &output_text,
                    assistant_message_phase.as_deref(),
                    &mut text_logprobs,
                    &mut citations,
                )
                .await?
            };
        }
        if delta
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
            || delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| !calls.is_empty())
            || delta
                .get("function_call")
                .is_some_and(|call| !call.is_null())
        {
            close_reasoning!(false);
        }

        // PC8: an empty first `content` must not open a text node ahead of reasoning.
        if let Some(t) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            let scores = crate::urp::logprobs::decode(
                choice
                    .and_then(|c| c.get("logprobs"))
                    .and_then(|v| v.get("content")),
            );
            crate::urp::logprobs::append(&mut text_logprobs, &scores);
            let node_index = ensure_node_started(
                &tx,
                &response_id,
                &urp.model,
                &mut response_started,
                &mut text_node_index,
                &mut next_node_index,
                NodeHeader::Text {
                    id: None,
                    role: OrdinaryRole::Assistant,
                    phase: assistant_message_phase.clone(),
                    citations: vec![],
                    signature: None,
                },
                HashMap::new(),
            )
            .await?;
            output_text.push_str(t);
            *pending_visible_output_bytes += t.len() as u64;
            send_node_delta(
                &tx,
                node_index,
                NodeDelta::Text {
                    logprobs: scores,
                    content: t.into(),
                    citations: vec![],
                    signature: None,
                },
                chat_delta_event_extra(std::mem::take(&mut delta_extra)),
            )
            .await?;
        }

        if let Some(annotations) = delta.get("annotations").and_then(Value::as_array) {
            let live = place_chat_citations(
                crate::urp::citations::decode(
                    annotations.clone(),
                    crate::urp::ProviderProtocol::ChatCompletion,
                ),
                &output_text,
                &mut segments,
            );
            // Citations that all belong to completed segments must not open an empty node.
            if annotations.is_empty() || !live.is_empty() {
                let node_index = ensure_node_started(
                    &tx,
                    &response_id,
                    &urp.model,
                    &mut response_started,
                    &mut text_node_index,
                    &mut next_node_index,
                    NodeHeader::Text {
                        id: None,
                        role: OrdinaryRole::Assistant,
                        phase: assistant_message_phase.clone(),
                        citations: vec![],
                        signature: None,
                    },
                    HashMap::new(),
                )
                .await?;
                citations.extend(live.iter().cloned());
                send_node_delta(
                    &tx,
                    node_index,
                    NodeDelta::Text {
                        logprobs: None,
                        content: String::new(),
                        citations: live,
                        signature: None,
                    },
                    HashMap::new(),
                )
                .await?;
            }
        }
        if let Some(refusal) = delta.get("refusal").and_then(Value::as_str) {
            let node_index = ensure_node_started(
                &tx,
                &response_id,
                &urp.model,
                &mut response_started,
                &mut refusal_node_index,
                &mut next_node_index,
                NodeHeader::Refusal { id: None },
                HashMap::new(),
            )
            .await?;
            let scores = crate::urp::logprobs::decode(
                choice
                    .and_then(|c| c.get("logprobs"))
                    .and_then(|v| v.get("refusal")),
            );
            crate::urp::logprobs::append(&mut refusal_logprobs, &scores);
            refusal_text.push_str(refusal);
            send_node_delta(
                &tx,
                node_index,
                NodeDelta::Refusal {
                    logprobs: scores,
                    content: refusal.into(),
                },
                chat_delta_event_extra(std::mem::take(&mut delta_extra)),
            )
            .await?;
        }
        if let Some(audio) = delta.get("audio").and_then(Value::as_object) {
            for (key, value) in audio {
                if let ("data" | "transcript", Value::String(chunk)) = (key.as_str(), value) {
                    if let Some(Value::String(accumulated)) = audio_fields.get_mut(key) {
                        accumulated.push_str(chunk);
                    } else {
                        audio_fields.insert(key.clone(), Value::String(chunk.clone()));
                    }
                } else if !key.starts_with("_monoize_") {
                    audio_fields.insert(key.clone(), value.clone());
                }
            }
            ensure_node_started(
                &tx,
                &response_id,
                &urp.model,
                &mut response_started,
                &mut audio_node_index,
                &mut next_node_index,
                NodeHeader::Audio {
                    id: None,
                    role: OrdinaryRole::Assistant,
                    metadata: crate::urp::MediaMetadata {
                        media_type: urp
                            .audio_output_format
                            .as_deref()
                            .and_then(crate::urp::media::audio_mime_for_format)
                            .map(str::to_string),
                        ..Default::default()
                    },
                },
                HashMap::from([(crate::urp::CHAT_MESSAGE_AUDIO_EXTRA_KEY.into(), json!(true))]),
            )
            .await?;
        }

        if let Some(content_blocks) = delta.get("content").and_then(chat_content_blocks) {
            for (content_pos, block) in content_blocks.iter().enumerate() {
                if let Some(text) = block.as_str() {
                    close_reasoning!(false);
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
                        pending_visible_output_bytes,
                    )
                    .await?;
                    continue;
                }

                let Some(block_obj) = block.as_object() else {
                    continue;
                };

                if let Some(reasoning_block) = extract_chat_reasoning_content_block(block) {
                    close_text!();
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
                        None,
                        &mut reasoning_surface,
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
                        None,
                        &mut reasoning_surface,
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
                    if matches!(item_type, Some("input_text" | "text" | "output_text")) {
                        close_reasoning!(false);
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
                            pending_visible_output_bytes,
                        )
                        .await?;
                        recognized = true;
                    }
                }

                let tool_like = chat_stream_block_is_tool_call_like(block_obj);
                if tool_like {
                    close_reasoning!(false);
                    close_text!();
                }
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
                    false,
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
        let mut scalar_delta = Cow::Borrowed(delta);
        if let Some(reasoning_details) = reasoning_details {
            for detail in reasoning_details {
                if is_encrypted_placeholder(detail) {
                    held_encrypted_placeholders.push(detail.clone());
                    continue;
                }
                if !detail_continues_open_node(detail, &reasoning_detail_nodes, &segments) {
                    close_text!();
                }
                process_reasoning_detail_delta(
                    &tx,
                    &response_id,
                    &urp.model,
                    detail,
                    &mut response_started,
                    &mut next_node_index,
                    &mut reasoning_detail_nodes,
                    &segments,
                    &mut delta_extra,
                )
                .await?;
            }
            let scalar_obj = scalar_delta
                .to_mut()
                .as_object_mut()
                .expect("chat delta object");
            scalar_obj.remove("reasoning_details");
            // CHAT-4b: scalar `reasoning` is a view of the chunk's details.
            scalar_obj.remove("reasoning");
            for key in ["reasoning_content", "reasoning_opaque"] {
                let duplicate = delta.get(key).and_then(Value::as_str).is_some_and(|value| {
                    reasoning_details
                        .iter()
                        .any(|detail| match detail["type"].as_str() {
                            Some("reasoning.text") => {
                                key != "reasoning_opaque" && detail["text"].as_str() == Some(value)
                            }
                            Some("reasoning.encrypted") => {
                                key == "reasoning_opaque" && detail["data"].as_str() == Some(value)
                            }
                            _ => false,
                        })
                });
                if duplicate {
                    scalar_obj.remove(key);
                }
            }
        }
        let (reasoning_text_deltas, reasoning_sig_deltas) =
            extract_chat_reasoning_delta_chunks(&scalar_delta);
        if !reasoning_text_deltas.is_empty() || !reasoning_sig_deltas.is_empty() {
            close_text!();
        }
        for text in reasoning_text_deltas {
            process_reasoning_text_delta(
                &tx,
                &response_id,
                &urp.model,
                Some(&text.text),
                text.format.as_deref(),
                text.surface,
                &mut reasoning_surface,
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
                encrypted.surface,
                &mut reasoning_surface,
                &mut response_started,
                &mut reasoning_node_index,
                &mut next_node_index,
                &mut reasoning_sig,
                &mut reasoning_source,
                &mut delta_extra,
            )
            .await?;
        }

        if delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty())
            || delta
                .get("function_call")
                .is_some_and(|call| !call.is_null())
        {
            close_reasoning!(false);
            close_text!();
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
                    false,
                    &mut tool_state,
                )
                .await?;
            }
        }

        if let Some(function_call) = delta
            .get("function_call")
            .and_then(legacy_function_call_as_tool_call)
        {
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
                &function_call,
                0,
                true,
                &mut tool_state,
            )
            .await?;
        }

        if let Some(message) = choice
            .and_then(|choice| choice.get("message"))
            .and_then(Value::as_object)
        {
            if assistant_message_phase.is_none() {
                assistant_message_phase = message
                    .get("phase")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            if !held_encrypted_placeholders.is_empty()
                || ["reasoning_details", "reasoning", "reasoning_content", "reasoning_opaque"]
                .iter()
                .any(|key| match message.get(*key) {
                    Some(Value::String(value)) => !value.is_empty(),
                    Some(Value::Array(value)) => !value.is_empty(),
                    _ => false,
                })
            {
                close_text!();
            }
            process_terminal_reasoning_snapshot(
                &tx,
                &response_id,
                &urp.model,
                message,
                &mut response_started,
                &mut next_node_index,
                &mut reasoning_node_index,
                &mut reasoning_text,
                &mut reasoning_sig,
                &mut reasoning_source,
                &mut reasoning_surface,
                &mut reasoning_detail_nodes,
                &segments,
                &mut delta_extra,
            )
            .await?;
            release_encrypted_placeholders(
                &tx,
                &response_id,
                &urp.model,
                std::mem::take(&mut held_encrypted_placeholders),
                &mut response_started,
                &mut next_node_index,
                &mut reasoning_detail_nodes,
                &segments,
                &mut delta_extra,
            )
            .await?;
            close_reasoning!(true);
            process_terminal_output_snapshot(
                &tx,
                &response_id,
                &urp.model,
                message,
                &mut response_started,
                &mut next_node_index,
                assistant_message_phase.as_deref(),
                &mut text_node_index,
                &mut output_text,
                &mut call_order,
                &mut calls,
                &mut call_id_by_index,
                &mut tool_node_index_by_call_id,
                &mut provider_items,
                &mut delta_extra,
                pending_visible_output_bytes,
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

    if !held_encrypted_placeholders.is_empty() {
        close_text_node(
            &tx,
            &mut segments,
            &mut text_node_index,
            &output_text,
            assistant_message_phase.as_deref(),
            &mut text_logprobs,
            &mut citations,
        )
        .await?;
        release_encrypted_placeholders(
            &tx,
            &response_id,
            &urp.model,
            std::mem::take(&mut held_encrypted_placeholders),
            &mut response_started,
            &mut next_node_index,
            &mut reasoning_detail_nodes,
            &segments,
            &mut pending_delta_extra,
        )
        .await?;
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

    let usage = latest_usage.or(latest_stream_usage_snapshot(&runtime_metrics).await);
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
    let mut output_nodes = sorted_nodes(
        assistant_message_phase.as_deref(),
        text_node_index,
        output_text.get(segments.text_start..).unwrap_or_default(),
        reasoning_node_index,
        reasoning_text.get(segments.reasoning_text_start..).unwrap_or_default(),
        reasoning_summary.get(segments.reasoning_summary_start..).unwrap_or_default(),
        reasoning_sig.get(segments.reasoning_sig_start..).unwrap_or_default(),
        reasoning_source.as_deref(),
        reasoning_surface,
        &reasoning_detail_nodes,
        &call_order,
        &calls,
        &tool_node_index_by_call_id,
        &provider_items,
    );

    if let Some(index) = refusal_node_index {
        output_nodes.push((
            index,
            Node::Refusal {
                logprobs: None,
                id: None,
                content: refusal_text,
                extra_body: HashMap::new(),
            },
        ));
    }
    if let Some(index) = audio_node_index
        && let Some(part) = crate::urp::decode::openai_chat::parse_chat_message_audio_part(
            &Value::Object(audio_fields),
        )
    {
        let mut node = part.into_node(OrdinaryRole::Assistant);
        if let Node::Audio {
            source: crate::urp::AudioSource::Base64 { media_type, .. },
            ..
        } = &mut node
            && let Some(mime) = urp
                .audio_output_format
                .as_deref()
                .and_then(crate::urp::media::audio_mime_for_format)
        {
            *media_type = mime.into();
        }
        output_nodes.push((index, node));
    }
    for (_, node) in &mut output_nodes {
        if let Node::Text {
            citations: target, ..
        } = node
        {
            *target = std::mem::take(&mut citations);
            if let Node::Text { logprobs, .. } = node {
                *logprobs = text_logprobs.take();
            }
            break;
        }
    }
    output_nodes.append(&mut segments.closed);
    output_nodes.sort_by_key(|(index, _)| *index);
    for (node_index, node) in &mut output_nodes {
        if let Node::Refusal { logprobs, .. } = node {
            *logprobs = refusal_logprobs.take();
        }
        if segments.done.contains(node_index) {
            continue;
        }
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

    let output: Vec<Node> = output_nodes.into_iter().map(|(_, node)| node).collect();
    let finish_reason = finish_reason.map(|reason| {
        crate::urp::decode::openai_chat::normalize_tool_call_finish(reason, &output)
    });
    send_event(
        &tx,
        UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason,
            usage,
            output,
            extra_body: terminal_extra_body,
        },
    )
    .await?;

    Ok(())
}

/// DC6a: unknown top-level chunk fields, without OpenRouter's upstream `provider` identity.
fn chat_chunk_extra(chunk: &Value) -> Map<String, Value> {
    let Some(chunk) = chunk.as_object() else {
        return Map::new();
    };
    chunk
        .iter()
        .filter(|(key, value)| {
            !crate::urp::decode::is_internal_extra_key(key)
                && !matches!(
                    key.as_str(),
                    "id" | "object" | "model" | "choices" | "usage" | "error" | "provider"
                )
                // DC7b: an integer `created` is the stream's chunk timestamp.
                && (key.as_str() != "created" || value.is_i64())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn with_chunk_extra(
    mut extra: HashMap<String, Value>,
    chunk_extra: Option<&Map<String, Value>>,
) -> HashMap<String, Value> {
    if let Some(chunk_extra) = chunk_extra {
        extra.insert(
            crate::urp::CHAT_CHUNK_EXTRA_KEY.to_string(),
            Value::Object(chunk_extra.clone()),
        );
    }
    extra
}

fn chat_choice_extra(choice: &Map<String, Value>) -> Map<String, Value> {
    choice
        .iter()
        .filter(|(key, _)| {
            !crate::urp::decode::is_internal_extra_key(key)
                && !matches!(
                    key.as_str(),
                    "index" | "delta" | "message" | "finish_reason" | "logprobs"
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
            !crate::urp::decode::is_internal_extra_key(key)
                && !matches!(
                    key.as_str(),
                    "role"
                        | "content"
                        | "reasoning"
                        | "reasoning_content"
                        | "reasoning_details"
                        | "reasoning_opaque"
                        | "tool_calls"
                        | "function_call"
                        | "refusal"
                        | "audio"
                        | "annotations"
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
        || delta
            .get("function_call")
            .and_then(Value::as_object)
            .is_some_and(|call| !call.is_empty())
}

fn legacy_function_call_as_tool_call(function_call: &Value) -> Option<Value> {
    let mut function = function_call.as_object()?.clone();
    function.retain(|key, _| !crate::urp::decode::is_internal_extra_key(key));
    if function.is_empty() {
        return None;
    }
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let mut normalized = Map::from_iter([
        ("index".to_string(), Value::from(0)),
        ("type".to_string(), Value::String("function".to_string())),
        ("function".to_string(), Value::Object(function)),
    ]);
    if let Some(name) = name {
        normalized.insert(
            "id".to_string(),
            Value::String(format!("legacy_function:{name}")),
        );
    }
    Some(Value::Object(normalized))
}

fn chat_delta_event_extra(mut delta_extra: Map<String, Value>) -> HashMap<String, Value> {
    let choice_extra = delta_extra.remove(CHAT_CHOICE_EXTRA_BODY_KEY);
    let mut event_extra = HashMap::new();
    if !delta_extra.is_empty() {
        event_extra.insert(
            CHAT_DELTA_EXTRA_BODY_KEY.to_string(),
            Value::Object(delta_extra),
        );
    }
    if let Some(choice_extra) = choice_extra {
        event_extra.insert(CHAT_CHOICE_EXTRA_BODY_KEY.to_string(), choice_extra);
    }
    event_extra
}

fn chat_error_code_and_message(error: &Value) -> (Option<String>, String) {
    let code = chat_error_scalar(error, "code", "provider_code");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.is_empty())
        .unwrap_or("upstream Chat Completions error")
        .to_string();
    (code, message)
}

fn chat_error_scalar(error: &Value, direct_key: &str, metadata_key: &str) -> Option<String> {
    error
        .get(direct_key)
        .and_then(json_scalar_string)
        .or_else(|| {
            error
                .get("metadata")
                .and_then(Value::as_object)
                .and_then(|metadata| metadata.get(metadata_key))
                .and_then(json_scalar_string)
        })
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
        .and_then(|error| chat_error_scalar(error, "code", "provider_code"))
        .unwrap_or_else(|| fallback_code.to_string());
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| error.and_then(Value::as_str))
        .filter(|message| !message.is_empty())
        .unwrap_or(fallback_message)
        .to_string();
    let error_type = error.and_then(|error| chat_error_scalar(error, "type", "error_type"));
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
    if let Some(mut original_event) = original_event {
        if let Some(root) = original_event.as_object_mut() {
            if let Some(error) = root.get_mut("error") {
                strip_error_semantics(error);
            }
            if let Some(choices) = root.get_mut("choices").and_then(Value::as_array_mut) {
                for choice in choices {
                    if let Some(error) = choice.get_mut("error") {
                        strip_error_semantics(error);
                    }
                }
            }
        }
        crate::urp::decode::remove_untrusted_internal_keys(&mut original_event);
        extra_body.insert(CHAT_ERROR_EVENT_EXTRA_KEY.to_string(), original_event);
    } else if let Some(mut error) = error_value {
        strip_error_semantics(&mut error);
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
    let parsed_media = parse_compatible_media_part(block_obj).ok().flatten();
    let mut node = parsed_media
        .map(|part| part.into_node(OrdinaryRole::Assistant))
        .unwrap_or_else(|| Node::ProviderItem {
            id: block_obj
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
            origin_protocol: ProviderProtocol::ChatCompletion,
            role: OrdinaryRole::Assistant,
            item_type: block_obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            body: Value::Object(block_obj.clone()),
            extra_body: HashMap::new(),
        });
    let (header, delta) = match &node {
        Node::Image {
            id,
            role,
            metadata,
            source,
            ..
        } => (
            NodeHeader::Image {
                id: id.clone(),
                role: *role,
                metadata: metadata.clone(),
            },
            Some(NodeDelta::Image {
                source: source.clone(),
            }),
        ),
        Node::File {
            id,
            role,
            metadata,
            source,
            ..
        } => (
            NodeHeader::File {
                id: id.clone(),
                role: *role,
                metadata: metadata.clone(),
            },
            Some(NodeDelta::File {
                source: source.clone(),
            }),
        ),
        Node::Audio {
            id,
            role,
            metadata,
            source,
            ..
        } => (
            NodeHeader::Audio {
                id: id.clone(),
                role: *role,
                metadata: metadata.clone(),
            },
            Some(NodeDelta::Audio {
                source: source.clone(),
            }),
        ),
        Node::ProviderItem {
            id,
            origin_protocol,
            role,
            item_type,
            body,
            ..
        } => (
            NodeHeader::ProviderItem {
                id: id.clone(),
                origin_protocol: *origin_protocol,
                role: *role,
                item_type: item_type.clone(),
                body: Some(body.clone()),
            },
            None,
        ),
        _ => unreachable!("media parser returned a non-media node"),
    };
    let mut extra_body = node.extra_body_mut().clone();
    extra_body.extend(chat_delta_event_extra(std::mem::take(delta_extra)));
    send_event(
        tx,
        UrpStreamEvent::NodeStart {
            node_index,
            header,
            extra_body,
        },
    )
    .await?;
    if let Some(delta) = delta {
        send_node_delta(tx, node_index, delta, HashMap::new()).await?;
    }
    provider_items.push((node_index, node));
    Ok(())
}

fn chat_stream_block_is_tool_call_like(block_obj: &Map<String, Value>) -> bool {
    matches!(
        block_obj.get("type").and_then(|v| v.as_str()),
        Some("tool_call" | "function_call" | "tool_use" | "custom_tool_call" | "custom")
    ) || block_obj.contains_key("function")
        || block_obj.contains_key("custom")
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
    pending_visible_output_bytes: &mut u64,
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
            signature: None,
            citations: Vec::new(),
            id: None,
            role: OrdinaryRole::Assistant,
            phase: phase.map(str::to_string),
        },
        HashMap::new(),
    )
    .await?;
    output_text.push_str(text);
    *pending_visible_output_bytes += text.len() as u64;
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Text {
            logprobs: None,
            signature: None,
            citations: Vec::new(),
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

/// CHAT-4d fallback: a fragment without `id` and `index` continues the most recently started
/// node when that node is an unidentified detail of the same type and format.
fn continues_unidentified_detail(
    existing: &Map<String, Value>,
    fragment: &Map<String, Value>,
) -> bool {
    let unidentified = |detail: &Map<String, Value>| {
        !detail
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
            && detail.get("index").and_then(Value::as_u64).is_none()
    };
    unidentified(existing)
        && unidentified(fragment)
        && existing.get("type") == fragment.get("type")
        && existing.get("format") == fragment.get("format")
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
    segments: &StreamSegments,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    let Some(detail) = detail.as_object() else {
        return Ok(());
    };
    let Some(node) = chat_reasoning_node_from_detail(detail) else {
        return Ok(());
    };
    let Node::Reasoning {
        metadata,
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
    let latest_node_index = next_node_index.checked_sub(1);
    let is_text_or_summary = matches!(
        detail.get("type").and_then(Value::as_str),
        Some("reasoning.text" | "reasoning.summary")
    );
    let matching_position = is_text_or_summary
        .then(|| {
            reasoning_detail_nodes
                .iter()
                .position(|(node_index, existing)| {
                    !segments.done.contains(node_index)
                        && reasoning_detail_raw(existing).is_some_and(|existing| {
                            reasoning_detail_identity_matches(&existing, detail)
                        })
                })
                .or_else(|| {
                    let (node_index, existing) = reasoning_detail_nodes.last()?;
                    (Some(*node_index) == latest_node_index
                        && !segments.done.contains(node_index)
                        && reasoning_detail_raw(existing).is_some_and(|existing| {
                            continues_unidentified_detail(&existing, detail)
                        }))
                    .then(|| reasoning_detail_nodes.len() - 1)
                })
        })
        .flatten();
    if let Some(position) = matching_position {
        let (node_index, existing_node) = &mut reasoning_detail_nodes[position];
        let mut accumulated = reasoning_detail_raw(existing_node).unwrap();
        let payload_key = reasoning_detail_payload_key(detail["type"].as_str().unwrap()).unwrap();
        let mut text = accumulated
            .get(payload_key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        text.push_str(
            detail
                .get(payload_key)
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        accumulated.extend(detail.clone());
        accumulated.insert(payload_key.to_string(), Value::String(text));
        *existing_node = chat_reasoning_node_from_detail(&accumulated).unwrap();
        let mut event_extra = extra_body.clone();
        event_extra.extend(chat_delta_event_extra(std::mem::take(delta_extra)));
        send_node_delta(
            tx,
            *node_index,
            NodeDelta::Reasoning {
                metadata: crate::urp::ReasoningMetadata {
                    item_id: id.clone(),
                    ..metadata.clone()
                },
                content: content.clone(),
                encrypted: encrypted.clone(),
                summary: summary.clone(),
                source: source.clone(),
            },
            event_extra,
        )
        .await?;
        return Ok(());
    }
    let node_index = *next_node_index;
    *next_node_index += 1;
    send_event(
        tx,
        UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::Reasoning {
                metadata: metadata.clone(),
                id: id.clone(),
            },
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
            metadata: crate::urp::ReasoningMetadata {
                item_id: id.clone(),
                ..metadata.clone()
            },
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
        NodeHeader::Reasoning {
            metadata: Default::default(),
            id: None,
        },
        HashMap::new(),
    )
    .await?;
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Reasoning {
            metadata: Default::default(),
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

fn surface_rank(surface: &str) -> u8 {
    match surface {
        crate::urp::CHAT_REASONING_SURFACE_REASONING_CONTENT => 3,
        crate::urp::CHAT_REASONING_SURFACE_REASONING => 2,
        _ => 1,
    }
}

/// Records the fragment's scalar surface on the node and returns the event extras for it.
fn reasoning_surface_extra(
    surface: Option<&'static str>,
    node_surface: &mut Option<&'static str>,
) -> HashMap<String, Value> {
    let Some(surface) = surface else {
        return HashMap::new();
    };
    if node_surface.is_none_or(|current| surface_rank(surface) > surface_rank(current)) {
        *node_surface = Some(surface);
    }
    HashMap::from([(
        crate::urp::CHAT_REASONING_SURFACE_EXTRA_KEY.to_string(),
        Value::String(surface.to_string()),
    )])
}

#[allow(clippy::too_many_arguments)]
async fn process_reasoning_text_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    content: Option<&str>,
    source: Option<&str>,
    surface: Option<&'static str>,
    node_surface: &mut Option<&'static str>,
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
    let surface_extra = reasoning_surface_extra(surface, node_surface);
    let mut event_extra = chat_delta_event_extra(std::mem::take(delta_extra));
    event_extra.extend(surface_extra.clone());
    let node_index = ensure_node_started(
        tx,
        response_id,
        model,
        response_started,
        reasoning_node_index,
        next_node_index,
        NodeHeader::Reasoning {
            metadata: Default::default(),
            id: None,
        },
        surface_extra,
    )
    .await?;
    reasoning_text.push_str(content);
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Reasoning {
            metadata: Default::default(),
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
    surface: Option<&'static str>,
    node_surface: &mut Option<&'static str>,
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
    let surface_extra = reasoning_surface_extra(surface, node_surface);
    let mut event_extra = chat_delta_event_extra(std::mem::take(delta_extra));
    event_extra.extend(surface_extra.clone());
    let node_index = ensure_node_started(
        tx,
        response_id,
        model,
        response_started,
        reasoning_node_index,
        next_node_index,
        NodeHeader::Reasoning {
            metadata: Default::default(),
            id: None,
        },
        surface_extra,
    )
    .await?;
    reasoning_sig.push_str(&sig);
    send_node_delta(
        tx,
        node_index,
        NodeDelta::Reasoning {
            metadata: Default::default(),
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

fn apply_terminal_opaque_snapshot(current: &mut String, snapshot: &str) -> Option<String> {
    if let Some(suffix) = snapshot_suffix(current, snapshot) {
        return Some(suffix.to_string());
    }
    if current != snapshot {
        current.clear();
        current.push_str(snapshot);
    }
    None
}

fn reasoning_detail_raw(node: &Node) -> Option<Map<String, Value>> {
    let Node::Reasoning {
        id,
        content,
        summary,
        encrypted,
        source,
        extra_body,
        ..
    } = node
    else {
        return None;
    };
    let native = extra_body
        .get(CHAT_REASONING_DETAIL_EXTRA_KEY)?
        .as_object()?;
    crate::urp::reasoning::chat_details(
        content.as_deref(),
        summary.as_deref(),
        encrypted.as_ref(),
        id.as_deref(),
        source.as_deref(),
        Some(native),
    )
    .into_iter()
    .next()?
    .as_object()
    .cloned()
}

fn reasoning_detail_payload_key(detail_type: &str) -> Option<&'static str> {
    match detail_type {
        "reasoning.text" => Some("text"),
        "reasoning.summary" => Some("summary"),
        "reasoning.encrypted" => Some("data"),
        _ => None,
    }
}

fn reasoning_detail_identity_matches(
    existing: &Map<String, Value>,
    terminal: &Map<String, Value>,
) -> bool {
    let has_identity = existing
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        || existing.get("index").and_then(Value::as_u64).is_some();
    has_identity
        && existing.get("type") == terminal.get("type")
        && ["id", "index", "tool_call_id"]
            .iter()
            .all(|key| existing.get(*key) == terminal.get(*key))
        && match (existing.get("format"), terminal.get("format")) {
            (Some(left), Some(right)) => left == right,
            _ => true,
        }
}

fn reasoning_detail_matches(existing: &Node, terminal: &Map<String, Value>) -> bool {
    reasoning_detail_raw(existing)
        .is_some_and(|existing| reasoning_detail_identity_matches(&existing, terminal))
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
        if !crate::urp::decode::is_internal_extra_key(key) {
            merged_raw.insert(key.clone(), value.clone());
        }
    }
    let merged_node = chat_reasoning_node_from_detail(&merged_raw)?;

    let (content, encrypted, summary, payload_delta) = match payload_key {
        "text" => {
            let existing_text = existing_raw
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("");
            let terminal = merged_raw.get("text").and_then(Value::as_str).unwrap_or("");
            let suffix = snapshot_suffix(existing_text, terminal);
            // PC8: a signature first seen (or changed) in the snapshot has no other delta.
            let signature = match (existing, &merged_node) {
                (
                    Node::Reasoning { encrypted: old, .. },
                    Node::Reasoning { encrypted: new, .. },
                ) if new != old => new.clone(),
                _ => None,
            };
            if suffix.is_none() && signature.is_none() {
                return None;
            }
            (
                suffix.map(str::to_string),
                signature,
                None,
                Value::String(suffix.unwrap_or_default().to_string()),
            )
        }
        "data" => {
            // A terminal snapshot replaces encrypted data atomically.
            let data = terminal.get("data").filter(|data| match data {
                Value::Null => false,
                Value::String(data) => !data.is_empty(),
                _ => true,
            })?;
            let Node::Reasoning { encrypted, .. } = existing else {
                return None;
            };
            if encrypted.as_ref() == Some(data) {
                return None;
            }
            (None, Some(data.clone()), None, data.clone())
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
        _ => return None,
    };

    let mut delta_detail = terminal.clone();
    delta_detail.retain(|key, _| !crate::urp::decode::is_internal_extra_key(key));
    delta_detail.insert(payload_key.to_string(), payload_delta);
    let extra_body = HashMap::from([(
        CHAT_REASONING_DETAIL_EXTRA_KEY.to_string(),
        Value::Object(crate::urp::reasoning::detail_metadata(&delta_detail)),
    )]);
    Some((
        merged_node,
        NodeDelta::Reasoning {
            metadata: crate::urp::ReasoningMetadata {
                item_id: terminal
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                ..Default::default()
            },
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
    segments: &StreamSegments,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    let existing_len = reasoning_detail_nodes.len();
    let mut matched = vec![false; existing_len];
    for detail in details {
        let Some(detail_obj) = detail.as_object() else {
            continue;
        };
        // One logical detail can span several nodes after a PC7h switch.
        let group: Vec<usize> = reasoning_detail_nodes[..existing_len]
            .iter()
            .enumerate()
            .filter(|(position, (_, node))| {
                !matched[*position] && reasoning_detail_matches(node, detail_obj)
            })
            .map(|(position, _)| position)
            .collect();
        let Some(&position) = group.last() else {
            process_reasoning_detail_delta(
                tx,
                response_id,
                model,
                detail,
                response_started,
                next_node_index,
                reasoning_detail_nodes,
                segments,
                delta_extra,
            )
            .await?;
            continue;
        };
        for position in &group {
            matched[*position] = true;
        }
        // PC7h: terminal encrypted data that no completed node carries becomes a new node.
        if segments.done.contains(&reasoning_detail_nodes[position].0)
            && detail_obj.get("type").and_then(Value::as_str) == Some("reasoning.encrypted")
        {
            let data = detail_obj.get("data").filter(|data| match data {
                Value::Null => false,
                Value::String(data) => !data.is_empty(),
                _ => true,
            });
            let known = group.iter().any(|position| {
                matches!(
                    &reasoning_detail_nodes[*position].1,
                    Node::Reasoning { encrypted, .. } if encrypted.as_ref() == data
                )
            });
            if data.is_some() && !known {
                process_reasoning_detail_delta(
                    tx,
                    response_id,
                    model,
                    detail,
                    response_started,
                    next_node_index,
                    reasoning_detail_nodes,
                    segments,
                    delta_extra,
                )
                .await?;
            }
            continue;
        }
        let Some(detail_obj) =
            terminal_detail_for_last_segment(detail_obj, &group, reasoning_detail_nodes)
        else {
            continue;
        };
        let (node_index, existing_node) = &mut reasoning_detail_nodes[position];
        let node_index = *node_index;
        let completion = reasoning_detail_completion(existing_node, &detail_obj);
        if segments.done.contains(&node_index) {
            // PC7h: a completed node takes no more deltas; the remainder is a new node.
            if let Some((_, NodeDelta::Reasoning {
                content,
                encrypted,
                summary,
                ..
            }, _)) = completion
            {
                let mut fragment = detail_obj;
                let payload_key = fragment
                    .get("type")
                    .and_then(Value::as_str)
                    .and_then(reasoning_detail_payload_key)
                    .unwrap_or("text");
                fragment.insert(
                    payload_key.to_string(),
                    json!(content.or(summary).unwrap_or_default()),
                );
                if encrypted.is_none() {
                    fragment.remove("signature");
                }
                process_reasoning_detail_delta(
                    tx,
                    response_id,
                    model,
                    &Value::Object(fragment),
                    response_started,
                    next_node_index,
                    reasoning_detail_nodes,
                    segments,
                    delta_extra,
                )
                .await?;
            }
            continue;
        }
        if let Some((merged_node, delta, mut event_extra)) = completion {
            event_extra.extend(chat_delta_event_extra(std::mem::take(delta_extra)));
            send_node_delta(tx, node_index, delta, event_extra).await?;
            *existing_node = merged_node;
        } else {
            let mut merged_raw = reasoning_detail_raw(existing_node).unwrap_or_default();
            for (key, value) in &detail_obj {
                if !crate::urp::decode::is_internal_extra_key(key) {
                    merged_raw.insert(key.clone(), value.clone());
                }
            }
            if let Some(merged_node) = chat_reasoning_node_from_detail(&merged_raw) {
                *existing_node = merged_node;
            }
        }
    }
    Ok(())
}

/// The terminal detail as seen by the last node of its group: earlier segments' text removed.
/// `None` when the snapshot does not extend the earlier segments.
fn terminal_detail_for_last_segment(
    terminal: &Map<String, Value>,
    group: &[usize],
    nodes: &[(u32, Node)],
) -> Option<Map<String, Value>> {
    let mut terminal = terminal.clone();
    let Some((_, earlier)) = group.split_last() else {
        return Some(terminal);
    };
    if earlier.is_empty() {
        return Some(terminal);
    }
    let payload_key = terminal
        .get("type")
        .and_then(Value::as_str)
        .and_then(reasoning_detail_payload_key)?;
    let prior: String = earlier
        .iter()
        .filter_map(|position| reasoning_detail_raw(&nodes[*position].1))
        .filter_map(|raw| raw.get(payload_key).and_then(Value::as_str).map(str::to_string))
        .collect();
    let rest = terminal
        .get(payload_key)?
        .as_str()?
        .strip_prefix(prior.as_str())?
        .to_string();
    terminal.insert(payload_key.to_string(), Value::String(rest));
    Some(terminal)
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
    } else if let Some(custom) = obj.get_mut("custom").and_then(Value::as_object_mut) {
        custom.insert("input".to_string(), Value::String(arguments.to_string()));
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
    legacy_function_call: bool,
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
    if let Some((tool_type, name, streamed_arguments, stored_legacy_function_call)) =
        state.calls.get_mut(&call_id)
    {
        *stored_legacy_function_call |= legacy_function_call;
        if chat_tool_call_type(tool_call) == ToolCallType::Custom {
            *tool_type = ToolCallType::Custom;
        }
        let terminal_name = tool_call
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| {
                tool_call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                tool_call
                    .get("custom")
                    .and_then(|custom| custom.get("name"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("");
        if name.is_empty() && !terminal_name.is_empty() {
            *name = terminal_name.to_string();
        }
        if let Some(suffix) = snapshot_suffix(streamed_arguments, &terminal_arguments) {
            let delta_call = tool_call_with_arguments_delta(raw_tool_call, suffix);
            process_tool_call_delta(
                tx,
                response_id,
                model,
                &delta_call,
                tool_call_pos,
                legacy_function_call,
                state,
            )
            .await?;
        }
        return Ok(());
    }
    process_tool_call_delta(
        tx,
        response_id,
        model,
        raw_tool_call,
        tool_call_pos,
        legacy_function_call,
        state,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn process_terminal_reasoning_snapshot(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    message: &Map<String, Value>,
    response_started: &mut bool,
    next_node_index: &mut u32,
    reasoning_node_index: &mut Option<u32>,
    reasoning_text: &mut String,
    reasoning_sig: &mut String,
    reasoning_source: &mut Option<String>,
    reasoning_surface: &mut Option<&'static str>,
    reasoning_detail_nodes: &mut Vec<(u32, Node)>,
    segments: &StreamSegments,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
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
                    | "function_call"
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
            segments,
            delta_extra,
        )
        .await?;
    } else if reasoning_detail_nodes.is_empty() {
        if let Some((snapshot, surface)) = [
            ("reasoning_content", crate::urp::CHAT_REASONING_SURFACE_REASONING_CONTENT),
            ("reasoning", crate::urp::CHAT_REASONING_SURFACE_REASONING),
        ]
        .into_iter()
        .find_map(|(key, surface)| {
            message
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| (value, surface))
        }) && let Some(suffix) = snapshot_suffix(reasoning_text, snapshot)
        {
            process_reasoning_text_delta(
                tx,
                response_id,
                model,
                Some(suffix),
                None,
                Some(surface),
                reasoning_surface,
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
        {
            if let Some(suffix) = apply_terminal_opaque_snapshot(reasoning_sig, snapshot) {
                process_reasoning_encrypted_delta(
                    tx,
                    response_id,
                    model,
                    Some(&Value::String(suffix.to_string())),
                    None,
                    Some(crate::urp::CHAT_REASONING_SURFACE_REASONING_OPAQUE),
                    reasoning_surface,
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
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_terminal_output_snapshot(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    message: &Map<String, Value>,
    response_started: &mut bool,
    next_node_index: &mut u32,
    assistant_message_phase: Option<&str>,
    text_node_index: &mut Option<u32>,
    output_text: &mut String,
    call_order: &mut Vec<String>,
    calls: &mut HashMap<String, (ToolCallType, String, String, bool)>,
    call_id_by_index: &mut HashMap<usize, String>,
    tool_node_index_by_call_id: &mut HashMap<String, u32>,
    provider_items: &mut Vec<(u32, Node)>,
    delta_extra: &mut Map<String, Value>,
    pending_visible_output_bytes: &mut u64,
) -> AppResult<()> {
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
                    assistant_message_phase,
                    response_started,
                    text_node_index,
                    next_node_index,
                    output_text,
                    delta_extra,
                    pending_visible_output_bytes,
                )
                .await?;
            }
        } else if let Some(blocks) = chat_content_blocks(content) {
            for (block_pos, block) in blocks.iter().enumerate() {
                if let Some(text) = block.as_str() {
                    snapshot_text.push_str(text);
                    if let Some(suffix) = snapshot_suffix(output_text, &snapshot_text) {
                        process_text_delta(
                            tx,
                            response_id,
                            model,
                            suffix,
                            assistant_message_phase,
                            response_started,
                            text_node_index,
                            next_node_index,
                            output_text,
                            delta_extra,
                            pending_visible_output_bytes,
                        )
                        .await?;
                    }
                    continue;
                }
                let Some(block_obj) = block.as_object() else {
                    continue;
                };
                if let Some(text) = block_obj.get("text").and_then(Value::as_str)
                    && matches!(
                        block_obj.get("type").and_then(Value::as_str),
                        Some("input_text" | "text" | "output_text")
                    )
                {
                    snapshot_text.push_str(text);
                    if let Some(suffix) = snapshot_suffix(output_text, &snapshot_text) {
                        process_text_delta(
                            tx,
                            response_id,
                            model,
                            suffix,
                            assistant_message_phase,
                            response_started,
                            text_node_index,
                            next_node_index,
                            output_text,
                            delta_extra,
                            pending_visible_output_bytes,
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
                        false,
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
                false,
                &mut tool_state,
            )
            .await?;
        }
    }
    if let Some(function_call) = message
        .get("function_call")
        .and_then(legacy_function_call_as_tool_call)
    {
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
            &function_call,
            0,
            true,
            &mut tool_state,
        )
        .await?;
    }
    Ok(())
}

async fn process_tool_call_delta(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    raw_tool_call: &Value,
    tool_call_pos: usize,
    legacy_function_call: bool,
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

    let tool_type = chat_tool_call_type(tc_obj);
    let name = tc_obj
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| {
            tc_obj
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            tc_obj
                .get("custom")
                .and_then(|custom| custom.get("name"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();
    let args_delta = tool_call_arguments_delta_text(tc_obj).unwrap_or_default();

    if !state.calls.contains_key(&call_id) {
        state.call_order.push(call_id.clone());
        state.calls.insert(
            call_id.clone(),
            (tool_type, name.clone(), String::new(), legacy_function_call),
        );
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
        let mut extra_body = chat_delta_event_extra(std::mem::take(state.delta_extra));
        if legacy_function_call {
            extra_body.insert(
                CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY.to_string(),
                Value::Bool(true),
            );
        }
        send_event(
            tx,
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::ToolCall {
                    namespace: None,
                    signature: None,

                    id: None,
                    tool_type,
                    call_id: call_id.clone(),
                    name: name.clone(),
                },
                extra_body,
            },
        )
        .await?;
        node_index
    };

    let Some(entry) = state.calls.get_mut(&call_id) else {
        tracing::warn!(call_id = %call_id, "unknown call_id in tool call stream delta, skipping");
        return Ok(());
    };

    if tool_type == ToolCallType::Custom {
        entry.0 = ToolCallType::Custom;
    }
    entry.3 |= legacy_function_call;
    if !name.is_empty() && entry.1.is_empty() {
        entry.1 = name.clone();
    }
    if !args_delta.is_empty() {
        entry.2.push_str(&args_delta);
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

fn chat_tool_call_type(tc_obj: &Map<String, Value>) -> ToolCallType {
    if matches!(
        tc_obj.get("type").and_then(Value::as_str),
        Some("custom" | "custom_tool_call")
    ) || tc_obj.contains_key("custom")
    {
        ToolCallType::Custom
    } else {
        ToolCallType::Function
    }
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
                usage: None,
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

/// PC7h bookkeeping. `reasoning_text`, `reasoning_summary`, `reasoning_sig`, and `output_text`
/// stay cumulative for terminal snapshot comparison; the offsets mark the open node's start.
#[derive(Default)]
struct StreamSegments {
    closed: Vec<(u32, Node)>,
    done: HashSet<u32>,
    reasoning_text_start: usize,
    reasoning_summary_start: usize,
    reasoning_sig_start: usize,
    text_start: usize,
}

/// PC7h: an unsigned Anthropic thinking detail stays open, because its signature must reach
/// the same node before a Messages downstream closes the block.
fn detail_awaits_signature(node: &Node) -> bool {
    let Node::Reasoning {
        encrypted,
        source,
        extra_body,
        ..
    } = node
    else {
        return false;
    };
    encrypted.is_none()
        && source
            .as_deref()
            .is_some_and(|source| source.starts_with("anthropic"))
        && extra_body
            .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
            .and_then(|detail| detail.get("type"))
            .and_then(Value::as_str)
            == Some("reasoning.text")
}

/// PC7h: a `reasoning.encrypted` entry whose data has not arrived yet.
fn is_encrypted_placeholder(detail: &Value) -> bool {
    detail.get("type").and_then(Value::as_str) == Some("reasoning.encrypted")
        && match detail.get("data") {
            None | Some(Value::Null) => true,
            Some(Value::String(data)) => data.is_empty(),
            Some(_) => false,
        }
}

/// Starts nodes for held placeholders that no later entry of the same identity supplied.
#[allow(clippy::too_many_arguments)]
async fn release_encrypted_placeholders(
    tx: &mpsc::Sender<UrpStreamEvent>,
    response_id: &str,
    model: &str,
    held: Vec<Value>,
    response_started: &mut bool,
    next_node_index: &mut u32,
    reasoning_detail_nodes: &mut Vec<(u32, Node)>,
    segments: &StreamSegments,
    delta_extra: &mut Map<String, Value>,
) -> AppResult<()> {
    for detail in held {
        let supplied = detail.as_object().is_some_and(|detail| {
            reasoning_detail_nodes
                .iter()
                .any(|(_, node)| reasoning_detail_matches(node, detail))
        });
        if !supplied {
            process_reasoning_detail_delta(
                tx,
                response_id,
                model,
                &detail,
                response_started,
                next_node_index,
                reasoning_detail_nodes,
                segments,
                delta_extra,
            )
            .await?;
        }
    }
    Ok(())
}

/// True when a detail fragment only extends an open node or carries no payload.
fn detail_continues_open_node(
    detail: &Value,
    nodes: &[(u32, Node)],
    segments: &StreamSegments,
) -> bool {
    let Some(detail) = detail.as_object() else {
        return true;
    };
    nodes.iter().any(|(node_index, node)| {
        !segments.done.contains(node_index) && reasoning_detail_matches(node, detail)
    }) || !["text", "summary", "data", "signature"].iter().any(|key| {
        match detail.get(*key) {
            Some(Value::String(value)) => !value.is_empty(),
            Some(Value::Null) | None => false,
            Some(_) => true,
        }
    })
}

fn scalar_reasoning_node(
    reasoning_text: &str,
    reasoning_summary: &str,
    reasoning_sig: &str,
    reasoning_source: Option<&str>,
    reasoning_surface: Option<&str>,
) -> Node {
    Node::Reasoning {
        metadata: Default::default(),
        id: Some(crate::urp::synthetic_reasoning_id()),
        content: (!reasoning_text.is_empty()).then(|| reasoning_text.to_string()),
        encrypted: (!reasoning_sig.is_empty()).then(|| Value::String(reasoning_sig.to_string())),
        summary: (!reasoning_summary.is_empty()).then(|| reasoning_summary.to_string()),
        source: reasoning_source.map(|source| source.to_string()),
        extra_body: reasoning_surface
            .map(|surface| {
                HashMap::from([(
                    crate::urp::CHAT_REASONING_SURFACE_EXTRA_KEY.to_string(),
                    Value::String(surface.to_string()),
                )])
            })
            .unwrap_or_default(),
    }
}

fn text_segment_node(content: &str, phase: Option<&str>) -> Node {
    Node::Text {
        logprobs: None,
        signature: None,
        citations: Vec::new(),
        id: Some(crate::urp::synthetic_message_id()),
        role: OrdinaryRole::Assistant,
        content: content.to_string(),
        phase: phase.map(str::to_string),
        extra_body: HashMap::new(),
        cache_breakpoint: None,
    }
}

/// PC7h: Chat annotation ranges are message-global and URP text citations are node-local.
/// Returns the citations of the open text node; a range wholly inside a completed segment
/// joins that segment's terminal node.
fn place_chat_citations(
    citations: Vec<crate::urp::Citation>,
    output_text: &str,
    segments: &mut StreamSegments,
) -> Vec<crate::urp::Citation> {
    let base = output_text
        .get(..segments.text_start)
        .unwrap_or_default()
        .chars()
        .count() as u64;
    if base == 0 {
        return citations;
    }
    let mut live = Vec::with_capacity(citations.len());
    for mut citation in citations {
        match &mut citation.answer_range {
            Some(range) if range.end <= base && range.start < base => {
                let mut segment_end = 0u64;
                let owner = segments.closed.iter_mut().find_map(|(_, node)| {
                    let Node::Text {
                        content, citations, ..
                    } = node
                    else {
                        return None;
                    };
                    let segment_start = segment_end;
                    segment_end += content.chars().count() as u64;
                    (range.start < segment_end).then_some((segment_start, segment_end, citations))
                });
                if let Some((segment_start, segment_end, target)) = owner {
                    range.start -= segment_start;
                    range.end = range.end.min(segment_end) - segment_start;
                    target.push(citation);
                }
            }
            Some(range) => {
                range.start = range.start.saturating_sub(base);
                range.end = range.end.saturating_sub(base);
                live.push(citation);
            }
            None => live.push(citation),
        }
    }
    live
}

/// `force` also completes details awaiting a signature, at the terminal snapshot.
#[allow(clippy::too_many_arguments)]
async fn close_reasoning_nodes(
    tx: &mpsc::Sender<UrpStreamEvent>,
    segments: &mut StreamSegments,
    reasoning_node_index: &mut Option<u32>,
    reasoning_text: &str,
    reasoning_summary: &str,
    reasoning_sig: &str,
    reasoning_source: &mut Option<String>,
    reasoning_surface: &mut Option<&'static str>,
    reasoning_detail_nodes: &[(u32, Node)],
    force: bool,
) -> AppResult<()> {
    let mut completed = Vec::new();
    if let Some(node_index) = reasoning_node_index.take() {
        let node = scalar_reasoning_node(
            reasoning_text.get(segments.reasoning_text_start..).unwrap_or_default(),
            reasoning_summary.get(segments.reasoning_summary_start..).unwrap_or_default(),
            reasoning_sig.get(segments.reasoning_sig_start..).unwrap_or_default(),
            reasoning_source.as_deref(),
            *reasoning_surface,
        );
        segments.reasoning_text_start = reasoning_text.len();
        segments.reasoning_summary_start = reasoning_summary.len();
        segments.reasoning_sig_start = reasoning_sig.len();
        *reasoning_source = None;
        *reasoning_surface = None;
        segments.closed.push((node_index, node.clone()));
        completed.push((node_index, node));
    }
    for (node_index, node) in reasoning_detail_nodes {
        if !segments.done.contains(node_index) && (force || !detail_awaits_signature(node)) {
            completed.push((*node_index, node.clone()));
        }
    }
    completed.sort_by_key(|(node_index, _)| *node_index);
    for (node_index, node) in completed {
        segments.done.insert(node_index);
        send_event(
            tx,
            UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage: None,
                extra_body: HashMap::new(),
            },
        )
        .await?;
    }
    Ok(())
}

async fn close_text_node(
    tx: &mpsc::Sender<UrpStreamEvent>,
    segments: &mut StreamSegments,
    text_node_index: &mut Option<u32>,
    output_text: &str,
    phase: Option<&str>,
    text_logprobs: &mut Option<Vec<crate::urp::TokenLogprob>>,
    citations: &mut Vec<crate::urp::Citation>,
) -> AppResult<()> {
    let Some(node_index) = text_node_index.take() else {
        return Ok(());
    };
    let mut node = text_segment_node(
        output_text.get(segments.text_start..).unwrap_or_default(),
        phase,
    );
    if let Node::Text {
        logprobs,
        citations: target,
        ..
    } = &mut node
    {
        *logprobs = text_logprobs.take();
        *target = std::mem::take(citations);
    }
    segments.text_start = output_text.len();
    segments.done.insert(node_index);
    segments.closed.push((node_index, node.clone()));
    send_event(
        tx,
        UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body: HashMap::new(),
        },
    )
    .await
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
    reasoning_surface: Option<&str>,
    reasoning_detail_nodes: &[(u32, Node)],
    call_order: &[String],
    calls: &HashMap<String, (ToolCallType, String, String, bool)>,
    tool_node_index_by_call_id: &HashMap<String, u32>,
    provider_items: &[(u32, Node)],
) -> Vec<(u32, Node)> {
    let mut nodes = Vec::new();

    if let Some(node_index) = reasoning_node_index {
        nodes.push((
            node_index,
            scalar_reasoning_node(
                reasoning_text,
                reasoning_summary,
                reasoning_sig,
                reasoning_source,
                reasoning_surface,
            ),
        ));
    }

    nodes.extend(reasoning_detail_nodes.iter().cloned());

    if let Some(node_index) = text_node_index {
        nodes.push((
            node_index,
            text_segment_node(output_text, assistant_message_phase),
        ));
    }

    for call_id in call_order {
        let Some(node_index) = tool_node_index_by_call_id.get(call_id).copied() else {
            continue;
        };
        let Some((tool_type, name, arguments, legacy_function_call)) = calls.get(call_id) else {
            continue;
        };
        let extra_body = if *legacy_function_call {
            HashMap::from([(
                CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY.to_string(),
                Value::Bool(true),
            )])
        } else {
            HashMap::new()
        };
        nodes.push((
            node_index,
            Node::ToolCall {
                namespace: None,
                signature: None,

                id: Some(crate::urp::synthetic_tool_call_id()),
                tool_type: *tool_type,
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                extra_body,
                cache_breakpoint: None,
            },
        ));
    }

    nodes.extend(provider_items.iter().cloned());
    nodes.sort_by_key(|(node_index, _)| *node_index);
    nodes
}

fn strip_error_semantics(error: &mut Value) {
    let Some(object) = error.as_object_mut() else {
        *error = json!({});
        return;
    };
    for key in ["message", "code", "type", "param"] {
        object.remove(key);
    }
    if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata.remove("provider_code");
        metadata.remove("error_type");
    }
}

fn chat_content_blocks(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(parts) => Some(parts),
        Value::Object(_) => Some(std::slice::from_ref(value)),
        _ => None,
    }
}

fn validate_chat_content_media(value: &Value) -> Result<(), String> {
    for part in chat_content_blocks(value).into_iter().flatten() {
        if let Some(obj) = part.as_object() {
            parse_compatible_media_part(obj)?;
        }
    }
    Ok(())
}
