use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::handlers::usage::usage_to_chat_usage_json;
use crate::urp::encode::sanitize_provider_item_wire_body;
use crate::urp::stream_helpers::*;
use crate::urp::{self, FinishReason, Node, NodeDelta, NodeHeader, UrpStreamEvent};
use axum::response::sse::Event;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

const CHAT_CHOICE_EXTRA_BODY_KEY: &str = "_monoize_chat_choice_extra";
const CHAT_DELTA_EXTRA_BODY_KEY: &str = "_monoize_chat_delta_extra";
const CHAT_ERROR_EVENT_EXTRA_KEY: &str = "_monoize_chat_error_event";
const CHAT_NATIVE_FINISH_REASON_EXTRA_KEY: &str = "_monoize_chat_native_finish_reason";

#[derive(Clone, Debug)]
struct StreamedChatToolCall {
    call_id: String,
    name: String,
    index: usize,
    header_sent: bool,
    arguments_streamed: bool,
}

#[derive(Clone, Debug, Default)]
struct StreamedChatNodeState {
    tool_call: Option<StreamedChatToolCall>,
    saw_node_start: bool,
    saw_node_done: bool,
}

fn merge_chat_delta_extra_preserving_typed(
    delta: &mut Value,
    extra: impl IntoIterator<Item = (String, Value)>,
) {
    let Some(delta) = delta.as_object_mut() else {
        return;
    };
    for (key, value) in extra {
        if !key.starts_with("_monoize_") && !delta.contains_key(&key) {
            delta.insert(key, value);
        }
    }
}

fn native_chat_delta_extra(extra_body: &HashMap<String, Value>) -> Map<String, Value> {
    extra_body
        .get(CHAT_DELTA_EXTRA_BODY_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn chat_delta_with_extras(
    delta: Value,
    event_extra: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
) -> Value {
    let mut event_delta_extra = native_chat_delta_extra(event_extra);
    chat_delta_with_raw_extras(delta, &mut event_delta_extra, pending_envelope_extra)
}

fn chat_delta_with_raw_extras(
    mut delta: Value,
    event_delta_extra: &mut Map<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
) -> Value {
    let mut extra = std::mem::take(pending_envelope_extra);
    for (key, value) in std::mem::take(event_delta_extra) {
        extra.insert(key, value);
    }
    merge_chat_delta_extra_preserving_typed(&mut delta, extra);
    delta
}

fn merge_pending_envelope_extra(
    pending: &mut HashMap<String, Value>,
    extra: &HashMap<String, Value>,
) {
    for (key, value) in extra {
        if !key.starts_with("_monoize_") {
            pending.insert(key.clone(), value.clone());
        }
    }
}

pub(crate) async fn emit_synthetic_chat_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut saw_tool = false;
    let mut tool_idx = 0usize;
    for node in &resp.output {
        match node {
            Node::Reasoning {
                content,
                encrypted,
                summary,
                source,
                extra_body,
                ..
            } => {
                if let Some(detail) = extra_body
                    .get(urp::CHAT_REASONING_DETAIL_EXTRA_KEY)
                    .and_then(Value::as_object)
                {
                    emit_native_chat_reasoning_detail(&tx, &id, created, logical_model, detail)
                        .await?;
                    continue;
                }
                if let Some(rc_value) = extra_body
                    .get("inject_reasoning_content")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    send_chat_chunk_string(
                        &tx,
                        &id,
                        created,
                        logical_model,
                        json!({ "reasoning_content": "" }),
                        rc_value,
                        chat_delta_path_reasoning_content,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                let format = source.as_deref().filter(|format| !format.is_empty());
                if let Some(summary) = summary.as_deref().filter(|summary| !summary.is_empty()) {
                    if extra_body
                        .get("openwebui_reasoning_content")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        send_chat_chunk_string(
                            &tx,
                            &id,
                            created,
                            logical_model,
                            json!({ "reasoning_content": "" }),
                            summary,
                            chat_delta_path_reasoning_content,
                            sse_max_frame_length,
                        )
                        .await?;
                    } else {
                        send_chat_chunk_string(
                            &tx,
                            &id,
                            created,
                            logical_model,
                            chat_reasoning_delta_from_summary("", format),
                            summary,
                            chat_delta_path_reasoning_summary,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                }
                if let Some(content) = content.as_deref().filter(|content| !content.is_empty()) {
                    send_chat_chunk_string(
                        &tx,
                        &id,
                        created,
                        logical_model,
                        chat_reasoning_delta_from_text("", format),
                        content,
                        chat_delta_path_reasoning_text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                if let Some(data) = encrypted {
                    let sig = data
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| data.to_string());
                    if !sig.is_empty() {
                        let reasoning_id = extra_body
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty());
                        send_chat_chunk_string(
                            &tx,
                            &id,
                            created,
                            logical_model,
                            chat_reasoning_delta_from_encrypted("", format, reasoning_id),
                            &sig,
                            chat_delta_path_reasoning_encrypted,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                }
            }
            Node::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                saw_tool = true;
                let chunk = json!({
                    "id": id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": logical_model,
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": tool_idx,
                                "id": call_id,
                                "type": "function",
                                "function": { "name": name, "arguments": "" }
                            }]
                        },
                        "finish_reason": Value::Null
                    }]
                });
                tool_idx += 1;
                send_chat_chunk_string(
                    &tx,
                    &id,
                    created,
                    logical_model,
                    chunk["choices"][0]["delta"].clone(),
                    arguments,
                    chat_delta_path_tool_arguments,
                    sse_max_frame_length,
                )
                .await?;
            }
            Node::Text {
                role: urp::OrdinaryRole::Assistant,
                content,
                ..
            }
            | Node::Refusal { content, .. } => {
                if !content.is_empty() {
                    send_chat_chunk_string(
                        &tx,
                        &id,
                        created,
                        logical_model,
                        json!({ "content": "" }),
                        content,
                        chat_delta_path_content,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            Node::ProviderItem {
                origin_protocol: urp::ProviderProtocol::ChatCompletion,
                body,
                ..
            } => {
                let mut pending_extra = HashMap::new();
                emit_chat_provider_content_part(
                    &tx,
                    &id,
                    created,
                    logical_model,
                    body,
                    &HashMap::new(),
                    &mut pending_extra,
                )
                .await?;
            }
            _ => continue,
        }
    }

    let native_finish_reason = resp
        .extra_body
        .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());
    let finish_reason = if resp.finish_reason == Some(urp::FinishReason::Other) {
        native_finish_reason.unwrap_or("error")
    } else if saw_tool {
        "tool_calls"
    } else {
        finish_reason_to_chat(resp.finish_reason.unwrap_or(urp::FinishReason::Stop))
    };
    emit_chat_terminal_sequence(
        &tx,
        &id,
        created,
        logical_model,
        finish_reason,
        resp.usage.as_ref(),
        resp.extra_body
            .get(CHAT_CHOICE_EXTRA_BODY_KEY)
            .and_then(Value::as_object),
    )
    .await
}

fn finish_reason_to_chat(reason: urp::FinishReason) -> &'static str {
    match reason {
        urp::FinishReason::Stop => "stop",
        urp::FinishReason::Length => "length",
        urp::FinishReason::ToolCalls => "tool_calls",
        urp::FinishReason::ContentFilter => "content_filter",
        urp::FinishReason::Other => "error",
    }
}

async fn emit_chat_terminal_sequence(
    tx: &mpsc::Sender<Event>,
    id: &str,
    created: i64,
    model: &str,
    finish_reason: &str,
    usage: Option<&urp::Usage>,
    choice_extra: Option<&serde_json::Map<String, Value>>,
) -> AppResult<()> {
    let mut finish = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
    });
    if let Some(choice_extra) = choice_extra
        && let Some(choice) = finish
            .get_mut("choices")
            .and_then(Value::as_array_mut)
            .and_then(|choices| choices.first_mut())
            .and_then(Value::as_object_mut)
    {
        for (key, value) in choice_extra {
            if !key.starts_with("_monoize_") {
                choice.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    send_plain_sse_data(tx, finish.to_string()).await?;

    if let Some(usage) = usage {
        let usage_chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [],
            "usage": usage_to_chat_usage_json(usage),
        });
        send_plain_sse_data(tx, usage_chunk.to_string()).await?;
    }

    send_plain_sse_data(tx, "[DONE]".to_string()).await
}

pub(crate) async fn encode_urp_stream_as_chat(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    logical_model: &str,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let mut chat_id = String::new();
    let mut created = 0i64;
    let mut tool_idx = 0usize;
    let mut saw_tool = false;
    let mut node_states: HashMap<u32, StreamedChatNodeState> = HashMap::new();
    let mut finished = false;
    let mut emitted_node_indices: HashSet<u32> = HashSet::new();
    let mut pending_envelope_extra = HashMap::new();

    while let Some(event) = rx.recv().await {
        if finished {
            continue;
        }
        match event {
            UrpStreamEvent::ResponseStart { extra_body, .. } => {
                chat_id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
                created = now_ts();
                let mut delta = json!({ "role": "assistant" });
                merge_chat_delta_extra_preserving_typed(
                    &mut delta,
                    native_chat_delta_extra(&extra_body),
                );
                let chunk = json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": logical_model,
                    "choices": [{
                        "index": 0,
                        "delta": delta,
                        "finish_reason": Value::Null
                    }]
                });
                send_plain_sse_data(&tx, chunk.to_string()).await?;
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                extra_body,
            } => {
                merge_pending_envelope_extra(&mut pending_envelope_extra, &extra_body);
                emitted_node_indices.insert(node_index);
                node_states.entry(node_index).or_default().saw_node_start = true;
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::ToolCall { call_id, name, .. },
                extra_body,
            } => {
                saw_tool = true;
                let idx = tool_idx;
                tool_idx += 1;
                let mut tool_call = StreamedChatToolCall {
                    call_id,
                    name,
                    index: idx,
                    header_sent: false,
                    arguments_streamed: false,
                };
                emit_tool_call_header(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    &mut tool_call,
                    &extra_body,
                    &mut pending_envelope_extra,
                )
                .await?;
                emitted_node_indices.insert(node_index);
                node_states.insert(
                    node_index,
                    StreamedChatNodeState {
                        tool_call: Some(tool_call),
                        saw_node_start: true,
                        saw_node_done: false,
                    },
                );
            }
            UrpStreamEvent::NodeStart { node_index, .. } => {
                node_states.entry(node_index).or_default().saw_node_start = true;
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Text { content },
                extra_body,
                ..
            }
            | UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Refusal { content },
                extra_body,
                ..
            } => {
                let delta = chat_delta_with_extras(
                    json!({ "content": "" }),
                    &extra_body,
                    &mut pending_envelope_extra,
                );
                send_chat_chunk_string(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    delta,
                    &content,
                    chat_delta_path_content,
                    sse_max_frame_length,
                )
                .await?;
                emitted_node_indices.insert(node_index);
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta:
                    NodeDelta::Reasoning {
                        content,
                        encrypted,
                        summary,
                        source,
                    },
                extra_body,
                ..
            } => {
                node_states.entry(node_index).or_default().saw_node_start = true;
                let emits_surface = reasoning_delta_has_chat_surface(
                    content.as_deref(),
                    encrypted.as_ref(),
                    summary.as_deref(),
                    &extra_body,
                );
                emit_reasoning_delta(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    content.as_deref(),
                    encrypted.as_ref(),
                    summary.as_deref(),
                    source.as_deref(),
                    &extra_body,
                    &mut pending_envelope_extra,
                    sse_max_frame_length,
                )
                .await?;
                if emits_surface {
                    emitted_node_indices.insert(node_index);
                }
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::ToolCallArguments { arguments },
                extra_body,
                ..
            } => {
                let Some(node_state) = node_states.get_mut(&node_index) else {
                    continue;
                };
                let Some(tool_call) = node_state.tool_call.as_mut() else {
                    continue;
                };

                saw_tool = true;
                let header_emitted_from_this_delta = !tool_call.header_sent;
                if !tool_call.header_sent {
                    emit_tool_call_header(
                        &tx,
                        &chat_id,
                        created,
                        logical_model,
                        tool_call,
                        &extra_body,
                        &mut pending_envelope_extra,
                    )
                    .await?;
                }
                let empty_delta_extra = HashMap::new();
                let arguments_delta_extra = if header_emitted_from_this_delta {
                    &empty_delta_extra
                } else {
                    &extra_body
                };
                emit_tool_call_arguments_delta(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    tool_call.index,
                    &arguments,
                    arguments_delta_extra,
                    &mut pending_envelope_extra,
                    sse_max_frame_length,
                )
                .await?;
                tool_call.arguments_streamed = true;
            }
            UrpStreamEvent::NodeDelta { node_index, .. } => {
                node_states.entry(node_index).or_default().saw_node_start = true;
            }
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => {
                let state = node_states.entry(node_index).or_default();
                state.saw_node_done = true;
                if let Node::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } = node
                {
                    saw_tool = true;
                    let tool_call = state.tool_call.get_or_insert_with(|| {
                        let idx = tool_idx;
                        tool_idx += 1;
                        StreamedChatToolCall {
                            call_id,
                            name,
                            index: idx,
                            header_sent: false,
                            arguments_streamed: false,
                        }
                    });
                    if !tool_call.header_sent {
                        emit_tool_call_header(
                            &tx,
                            &chat_id,
                            created,
                            logical_model,
                            tool_call,
                            &HashMap::new(),
                            &mut pending_envelope_extra,
                        )
                        .await?;
                    }
                    if !arguments.is_empty() && !tool_call.arguments_streamed {
                        emit_tool_call_arguments_delta(
                            &tx,
                            &chat_id,
                            created,
                            logical_model,
                            tool_call.index,
                            &arguments,
                            &HashMap::new(),
                            &mut pending_envelope_extra,
                            sse_max_frame_length,
                        )
                        .await?;
                        tool_call.arguments_streamed = true;
                    }
                    emitted_node_indices.insert(node_index);
                } else if let Node::ProviderItem {
                    origin_protocol: urp::ProviderProtocol::ChatCompletion,
                    body,
                    ..
                } = node
                {
                    emit_chat_provider_content_part(
                        &tx,
                        &chat_id,
                        created,
                        logical_model,
                        &body,
                        &HashMap::new(),
                        &mut pending_envelope_extra,
                    )
                    .await?;
                    emitted_node_indices.insert(node_index);
                }
            }
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                for (key, value) in native_chat_delta_extra(&extra_body) {
                    pending_envelope_extra.insert(key, value);
                }
                for (node_index, node) in output.iter().enumerate() {
                    if emitted_node_indices.contains(&(node_index as u32)) {
                        continue;
                    }
                    match node {
                        Node::Reasoning {
                            content,
                            encrypted,
                            summary,
                            source,
                            extra_body,
                            ..
                        } => {
                            if !reasoning_delta_has_chat_surface(
                                content.as_deref(),
                                encrypted.as_ref(),
                                summary.as_deref(),
                                extra_body,
                            ) {
                                continue;
                            }
                            emit_reasoning_delta(
                                &tx,
                                &chat_id,
                                created,
                                logical_model,
                                content.as_deref(),
                                encrypted.as_ref(),
                                summary.as_deref(),
                                source.as_deref(),
                                extra_body,
                                &mut pending_envelope_extra,
                                sse_max_frame_length,
                            )
                            .await?;
                        }
                        Node::ToolCall {
                            call_id,
                            name,
                            arguments,
                            ..
                        } => {
                            let mut tool_call = StreamedChatToolCall {
                                call_id: call_id.clone(),
                                name: name.clone(),
                                index: tool_idx,
                                header_sent: false,
                                arguments_streamed: false,
                            };
                            tool_idx += 1;
                            saw_tool = true;
                            emit_tool_call_header(
                                &tx,
                                &chat_id,
                                created,
                                logical_model,
                                &mut tool_call,
                                &HashMap::new(),
                                &mut pending_envelope_extra,
                            )
                            .await?;
                            if !arguments.is_empty() {
                                emit_tool_call_arguments_delta(
                                    &tx,
                                    &chat_id,
                                    created,
                                    logical_model,
                                    tool_call.index,
                                    arguments,
                                    &HashMap::new(),
                                    &mut pending_envelope_extra,
                                    sse_max_frame_length,
                                )
                                .await?;
                            }
                        }
                        Node::Text {
                            role: urp::OrdinaryRole::Assistant,
                            content,
                            ..
                        }
                        | Node::Refusal { content, .. } => {
                            if !content.is_empty() {
                                let delta = chat_delta_with_extras(
                                    json!({ "content": "" }),
                                    &HashMap::new(),
                                    &mut pending_envelope_extra,
                                );
                                send_chat_chunk_string(
                                    &tx,
                                    &chat_id,
                                    created,
                                    logical_model,
                                    delta,
                                    content,
                                    chat_delta_path_content,
                                    sse_max_frame_length,
                                )
                                .await?;
                            }
                        }
                        Node::ProviderItem {
                            origin_protocol: urp::ProviderProtocol::ChatCompletion,
                            body,
                            ..
                        } => {
                            emit_chat_provider_content_part(
                                &tx,
                                &chat_id,
                                created,
                                logical_model,
                                body,
                                &HashMap::new(),
                                &mut pending_envelope_extra,
                            )
                            .await?;
                        }
                        Node::NextDownstreamEnvelopeExtra { extra_body } => {
                            merge_pending_envelope_extra(&mut pending_envelope_extra, extra_body);
                        }
                        _ => {}
                    }
                }
                if !pending_envelope_extra.is_empty() {
                    let delta = chat_delta_with_extras(
                        json!({}),
                        &HashMap::new(),
                        &mut pending_envelope_extra,
                    );
                    let chunk = json!({
                        "id": chat_id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": logical_model,
                        "choices": [{
                            "index": 0,
                            "delta": delta,
                            "finish_reason": Value::Null
                        }]
                    });
                    send_plain_sse_data(&tx, chunk.to_string()).await?;
                }
                let native_finish_reason = extra_body
                    .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
                    .and_then(Value::as_str)
                    .filter(|reason| !reason.is_empty());
                let finish_reason = if finish_reason == Some(FinishReason::Other) {
                    native_finish_reason.unwrap_or("error")
                } else if saw_tool {
                    "tool_calls"
                } else {
                    finish_reason_to_chat(finish_reason.unwrap_or(FinishReason::Stop))
                };
                emit_chat_terminal_sequence(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    finish_reason,
                    usage.as_ref(),
                    extra_body
                        .get(CHAT_CHOICE_EXTRA_BODY_KEY)
                        .and_then(Value::as_object),
                )
                .await?;
                finished = true;
            }
            UrpStreamEvent::ProviderControl { .. } => {}
            UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            } => {
                let payload = if let Some(original) = extra_body.get(CHAT_ERROR_EVENT_EXTRA_KEY) {
                    original.clone()
                } else {
                    let mut error = extra_body
                        .get("error")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    error
                        .entry("message".to_string())
                        .or_insert_with(|| Value::String(message));
                    error
                        .entry("type".to_string())
                        .or_insert_with(|| Value::String("server_error".to_string()));
                    if let Some(code) = code {
                        error
                            .entry("code".to_string())
                            .or_insert_with(|| Value::String(code));
                    }
                    if let Some(param) = extra_body.get("param") {
                        error
                            .entry("param".to_string())
                            .or_insert_with(|| param.clone());
                    }
                    json!({ "error": error })
                };
                send_plain_sse_data(&tx, payload.to_string()).await?;
                send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
                finished = true;
            }
        }
    }

    Ok(())
}

fn reasoning_delta_has_chat_surface(
    content: Option<&str>,
    encrypted: Option<&Value>,
    summary: Option<&str>,
    extra_body: &HashMap<String, Value>,
) -> bool {
    content.is_some_and(|content| !content.is_empty())
        || encrypted.is_some_and(|encrypted| !encrypted.is_null())
        || summary.is_some_and(|summary| !summary.is_empty())
        || extra_body
            .get("inject_reasoning_content")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        || extra_body.contains_key(urp::CHAT_REASONING_DETAIL_EXTRA_KEY)
        || extra_body.contains_key(CHAT_DELTA_EXTRA_BODY_KEY)
}

async fn emit_chat_provider_content_part(
    tx: &mpsc::Sender<Event>,
    chat_id: &str,
    created: i64,
    logical_model: &str,
    body: &Value,
    event_extra: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
) -> AppResult<()> {
    let delta = chat_delta_with_extras(
        json!({ "content": [sanitize_provider_item_wire_body(body)] }),
        event_extra,
        pending_envelope_extra,
    );
    let chunk = json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": Value::Null
        }]
    });
    send_plain_sse_data(tx, chunk.to_string()).await
}

async fn emit_tool_call_header(
    tx: &mpsc::Sender<Event>,
    chat_id: &str,
    created: i64,
    logical_model: &str,
    tool_call: &mut StreamedChatToolCall,
    event_extra: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
) -> AppResult<()> {
    if tool_call.header_sent {
        return Ok(());
    }
    let delta = chat_delta_with_extras(
        json!({
            "tool_calls": [{
                "index": tool_call.index,
                "id": tool_call.call_id,
                "type": "function",
                "function": { "name": tool_call.name, "arguments": "" }
            }]
        }),
        event_extra,
        pending_envelope_extra,
    );
    let chunk = json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": Value::Null
        }]
    });
    send_plain_sse_data(tx, chunk.to_string()).await?;
    tool_call.header_sent = true;
    Ok(())
}

async fn emit_tool_call_arguments_delta(
    tx: &mpsc::Sender<Event>,
    chat_id: &str,
    created: i64,
    logical_model: &str,
    tool_index: usize,
    arguments: &str,
    event_extra: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let delta = chat_delta_with_extras(
        json!({
            "tool_calls": [{
                "index": tool_index,
                "function": { "arguments": "" }
            }]
        }),
        event_extra,
        pending_envelope_extra,
    );
    send_chat_chunk_string(
        tx,
        chat_id,
        created,
        logical_model,
        delta,
        arguments,
        chat_delta_path_tool_arguments,
        sse_max_frame_length,
    )
    .await
}

async fn emit_native_chat_reasoning_detail(
    tx: &mpsc::Sender<Event>,
    chat_id: &str,
    created: i64,
    logical_model: &str,
    detail: &serde_json::Map<String, Value>,
) -> AppResult<()> {
    let chunk = json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{
            "index": 0,
            "delta": { "reasoning_details": [Value::Object(detail.clone())] },
            "finish_reason": Value::Null
        }]
    });
    send_plain_sse_data(tx, chunk.to_string()).await
}

#[allow(clippy::too_many_arguments)]
async fn emit_reasoning_delta(
    tx: &mpsc::Sender<Event>,
    chat_id: &str,
    created: i64,
    logical_model: &str,
    content: Option<&str>,
    encrypted: Option<&Value>,
    summary: Option<&str>,
    source: Option<&str>,
    extra_body: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let mut event_delta_extra = native_chat_delta_extra(extra_body);

    if let Some(detail) = extra_body
        .get(urp::CHAT_REASONING_DETAIL_EXTRA_KEY)
        .and_then(Value::as_object)
    {
        let delta = chat_delta_with_raw_extras(
            json!({ "reasoning_details": [Value::Object(detail.clone())] }),
            &mut event_delta_extra,
            pending_envelope_extra,
        );
        let chunk = json!({
            "id": chat_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": logical_model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": Value::Null
            }]
        });
        return send_plain_sse_data(tx, chunk.to_string()).await;
    }

    if let Some(rc_value) = extra_body
        .get("inject_reasoning_content")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        send_chat_chunk_string(
            tx,
            chat_id,
            created,
            logical_model,
            chat_delta_with_raw_extras(
                json!({ "reasoning_content": "" }),
                &mut event_delta_extra,
                pending_envelope_extra,
            ),
            rc_value,
            chat_delta_path_reasoning_content,
            sse_max_frame_length,
        )
        .await?;
    }
    let format = source.filter(|format| !format.is_empty()).or_else(|| {
        extra_body
            .get("format")
            .and_then(Value::as_str)
            .filter(|format| !format.is_empty())
    });
    let reasoning_id = extra_body
        .get("reasoning_item_id")
        .or_else(|| extra_body.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());

    if let Some(signature) = encrypted.and_then(|value| {
        value
            .as_str()
            .map(str::to_string)
            .or_else(|| (!value.is_null()).then(|| value.to_string()))
            .filter(|signature| !signature.is_empty())
    }) {
        send_chat_chunk_string(
            tx,
            chat_id,
            created,
            logical_model,
            chat_delta_with_raw_extras(
                chat_reasoning_delta_from_encrypted("", format, reasoning_id),
                &mut event_delta_extra,
                pending_envelope_extra,
            ),
            &signature,
            chat_delta_path_reasoning_encrypted,
            sse_max_frame_length,
        )
        .await?;
    }
    if let Some(content) = content.filter(|content| !content.is_empty()) {
        send_chat_chunk_string(
            tx,
            chat_id,
            created,
            logical_model,
            chat_delta_with_raw_extras(
                chat_reasoning_delta_from_text("", format),
                &mut event_delta_extra,
                pending_envelope_extra,
            ),
            content,
            chat_delta_path_reasoning_text,
            sse_max_frame_length,
        )
        .await?;
    }
    if let Some(summary) = summary.filter(|summary| !summary.is_empty()) {
        if extra_body
            .get("openwebui_reasoning_content")
            .and_then(Value::as_bool)
            == Some(true)
        {
            send_chat_chunk_string(
                tx,
                chat_id,
                created,
                logical_model,
                chat_delta_with_raw_extras(
                    json!({ "reasoning_content": "" }),
                    &mut event_delta_extra,
                    pending_envelope_extra,
                ),
                summary,
                chat_delta_path_reasoning_content,
                sse_max_frame_length,
            )
            .await?;
        } else {
            send_chat_chunk_string(
                tx,
                chat_id,
                created,
                logical_model,
                chat_delta_with_raw_extras(
                    chat_reasoning_delta_from_summary("", format),
                    &mut event_delta_extra,
                    pending_envelope_extra,
                ),
                summary,
                chat_delta_path_reasoning_summary,
                sse_max_frame_length,
            )
            .await?;
        }
    }
    if !event_delta_extra.is_empty() || !pending_envelope_extra.is_empty() {
        let delta =
            chat_delta_with_raw_extras(json!({}), &mut event_delta_extra, pending_envelope_extra);
        let chunk = json!({
            "id": chat_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": logical_model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": Value::Null
            }]
        });
        send_plain_sse_data(tx, chunk.to_string()).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_capture::with_sse_capture;
    use crate::urp::decode::openai_chat::decode_response;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::{Mutex, mpsc};

    fn captured_chat_json_frames(frames: &[String]) -> Vec<Value> {
        frames
            .iter()
            .filter_map(|frame| {
                let data = frame.strip_prefix("data: ")?.strip_suffix("\n\n")?;
                (data != "[DONE]").then(|| serde_json::from_str(data).expect("Chat frame JSON"))
            })
            .collect()
    }

    #[tokio::test]
    async fn chat_stream_provider_item_filters_nested_internal_metadata() {
        let (sse_tx, mut sse_rx) = mpsc::channel(4);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let native_body = json!({
            "type": "vendor_part",
            "payload": {
                "keep": 1,
                "_monoize_nested": "drop",
                "rows": [{ "keep_row": true, "_monoize_row": "drop" }]
            },
            "_monoize_top": "drop"
        });
        let mut pending_envelope_extra = HashMap::new();

        with_sse_capture(frames.clone(), async {
            emit_chat_provider_content_part(
                &sse_tx,
                "chatcmpl_provider",
                1,
                "gpt-5.4",
                &native_body,
                &HashMap::new(),
                &mut pending_envelope_extra,
            )
            .await
            .expect("emit Chat provider content part");
        })
        .await;
        drop(sse_tx);
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let json_frames = captured_chat_json_frames(&frames);
        assert_eq!(
            json_frames[0]["choices"][0]["delta"]["content"][0],
            json!({
                "type": "vendor_part",
                "payload": { "keep": 1, "rows": [{ "keep_row": true }] }
            })
        );
        assert_eq!(native_body["_monoize_top"], json!("drop"));
    }

    #[tokio::test]
    async fn encode_chat_stream_emits_reasoning_content_when_summary_is_marked_for_openwebui() {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (sse_tx, mut sse_rx) = mpsc::channel(16);

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_1".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response start");
        event_tx
            .send(UrpStreamEvent::NodeDelta {
                node_index: 0,
                delta: crate::urp::NodeDelta::Reasoning {
                    content: None,
                    encrypted: None,
                    summary: Some("brief summary".to_string()),
                    source: Some("openrouter".to_string()),
                },
                usage: None,
                extra_body: HashMap::from([
                    (
                        "format".to_string(),
                        Value::String("openrouter".to_string()),
                    ),
                    ("openwebui_reasoning_content".to_string(), Value::Bool(true)),
                ]),
            })
            .await
            .expect("reasoning delta");
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                output: Vec::new(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response done");
        drop(event_tx);

        encode_urp_stream_as_chat(event_rx, sse_tx, "gpt-5.4", None)
            .await
            .expect("encode stream");

        let mut text = String::new();
        while let Some(event) = sse_rx.recv().await {
            let debug = format!("{event:?}");
            text.push_str(&debug);
        }
        assert!(text.contains("reasoning_content"));
        assert!(text.contains("brief summary"));
        assert!(text.contains("\\\"delta\\\":{\\\"reasoning_content\\\":\\\"brief summary\\\"}"));
        assert!(!text.contains("data: {\"reasoning_content\":\"brief summary\"}"));
    }

    #[tokio::test]
    async fn synthetic_chat_stream_emits_reasoning_content_inside_delta() {
        let (sse_tx, mut sse_rx) = mpsc::channel(16);
        let response = urp::UrpResponse {
            id: "resp_1".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: vec![urp::Node::Reasoning {
                id: None,
                content: Some("plain_reasoning".to_string()),
                encrypted: Some(Value::String("enc_reasoning".to_string())),
                summary: Some("brief summary".to_string()),
                source: Some("openrouter".to_string()),
                extra_body: HashMap::from([(
                    "inject_reasoning_content".to_string(),
                    Value::String("plain_reasoning".to_string()),
                )]),
            }],
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: HashMap::new(),
        };

        emit_synthetic_chat_stream("gpt-5.4", &response, None, sse_tx)
            .await
            .expect("emit synthetic chat stream");

        let mut text = String::new();
        while let Some(event) = sse_rx.recv().await {
            let debug = format!("{event:?}");
            text.push_str(&debug);
        }

        assert!(text.contains("plain_reasoning"));
        assert!(text.contains("\\\"delta\\\":{\\\"reasoning_content\\\":\\\"plain_reasoning\\\"}"));
        assert!(!text.contains("data: {\"reasoning_content\":\"plain_reasoning\"}"));
    }

    #[tokio::test]
    async fn synthetic_chat_stream_emits_finish_then_empty_choices_usage_then_done() {
        let (sse_tx, mut sse_rx) = mpsc::channel(8);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let response = urp::UrpResponse {
            id: "resp_usage".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: Vec::new(),
            finish_reason: Some(FinishReason::Stop),
            usage: Some(urp::Usage {
                input_tokens: 12,
                output_tokens: 8,
                input_details: None,
                output_details: None,
                extra_body: HashMap::new(),
            }),
            extra_body: HashMap::new(),
        };

        with_sse_capture(frames.clone(), async {
            emit_synthetic_chat_stream("gpt-5.4", &response, None, sse_tx)
                .await
                .expect("emit synthetic chat stream");
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        assert_eq!(frames.len(), 3);
        let finish: Value = serde_json::from_str(
            frames[0]
                .strip_prefix("data: ")
                .and_then(|frame| frame.strip_suffix("\n\n"))
                .expect("finish data frame"),
        )
        .expect("finish JSON");
        let usage: Value = serde_json::from_str(
            frames[1]
                .strip_prefix("data: ")
                .and_then(|frame| frame.strip_suffix("\n\n"))
                .expect("usage data frame"),
        )
        .expect("usage JSON");

        assert_eq!(finish["choices"][0]["finish_reason"], json!("stop"));
        assert_eq!(finish["choices"][0]["delta"], json!({}));
        assert_eq!(finish["usage"], Value::Null);
        assert_eq!(usage["choices"], json!([]));
        assert_eq!(usage["usage"]["prompt_tokens"], json!(12));
        assert_eq!(usage["usage"]["completion_tokens"], json!(8));
        for field in ["id", "object", "created", "model"] {
            assert_eq!(usage[field], finish[field]);
        }
        assert_eq!(frames[2], "data: [DONE]\n\n");
    }

    #[tokio::test]
    async fn encode_chat_stream_maps_live_signature_delta_to_reasoning_encrypted_detail() {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (sse_tx, mut sse_rx) = mpsc::channel(16);

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_1".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response start");
        event_tx
            .send(UrpStreamEvent::NodeDelta {
                node_index: 0,
                delta: crate::urp::NodeDelta::Reasoning {
                    content: None,
                    encrypted: Some(Value::String("live_sig".to_string())),
                    summary: None,
                    source: Some("openrouter".to_string()),
                },
                usage: None,
                extra_body: HashMap::from([(
                    "format".to_string(),
                    Value::String("openrouter".to_string()),
                )]),
            })
            .await
            .expect("reasoning signature delta");
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                output: Vec::new(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response done");
        drop(event_tx);

        encode_urp_stream_as_chat(event_rx, sse_tx, "gpt-5.4", None)
            .await
            .expect("encode stream");

        let mut text = String::new();
        while let Some(event) = sse_rx.recv().await {
            let debug = format!("{event:?}");
            text.push_str(&debug);
        }

        assert!(text.contains("reasoning_details"));
        assert!(text.contains("reasoning.encrypted"));
        assert!(text.contains("live_sig"));
        assert!(!text.contains("\"reasoning\":"));
        assert!(!text.contains("\"signature\":"));
    }

    #[tokio::test]
    async fn response_done_fallback_deduplicates_parallel_tools_by_node_index() {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (sse_tx, mut sse_rx) = mpsc::channel(32);
        let frames = Arc::new(Mutex::new(Vec::new()));

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_tools".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response start");
        event_tx
            .send(UrpStreamEvent::NodeStart {
                node_index: 0,
                header: NodeHeader::ToolCall {
                    id: None,
                    call_id: "call_a".to_string(),
                    name: "tool_a".to_string(),
                },
                extra_body: HashMap::new(),
            })
            .await
            .expect("first tool start");
        event_tx
            .send(UrpStreamEvent::NodeDelta {
                node_index: 0,
                delta: NodeDelta::ToolCallArguments {
                    arguments: "{\"a\":1}".to_string(),
                },
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .expect("first tool arguments");
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                output: vec![
                    Node::ToolCall {
                        id: None,
                        call_id: "call_a".to_string(),
                        name: "tool_a".to_string(),
                        arguments: "{\"a\":1}".to_string(),
                        extra_body: HashMap::new(),
                    },
                    Node::ToolCall {
                        id: None,
                        call_id: "call_b".to_string(),
                        name: "tool_b".to_string(),
                        arguments: "{\"b\":2}".to_string(),
                        extra_body: HashMap::new(),
                    },
                ],
                extra_body: HashMap::new(),
            })
            .await
            .expect("response done");
        drop(event_tx);

        with_sse_capture(frames.clone(), async {
            encode_urp_stream_as_chat(event_rx, sse_tx, "gpt-5.4", None)
                .await
                .expect("encode stream");
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let wire = frames.join("");
        assert_eq!(wire.matches("call_a").count(), 1, "{wire}");
        assert_eq!(wire.matches("call_b").count(), 1, "{wire}");
        assert!(wire.contains("tool_a") && wire.contains("tool_b"), "{wire}");
        let json_frames = captured_chat_json_frames(&frames);
        assert_eq!(
            json_frames
                .iter()
                .filter(|frame| frame["choices"][0]["finish_reason"].as_str() == Some("tool_calls"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn response_done_fallback_emits_later_terminal_only_reasoning_nodes() {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (sse_tx, mut sse_rx) = mpsc::channel(32);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let server_detail = json!({
            "type": "reasoning.server_tool_call",
            "tool_name": "openrouter:fusion",
            "arguments": "{\"q\":1}",
            "result": "{\"ok\":true}",
            "id": "server_1"
        });

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_reasoning".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response start");
        event_tx
            .send(UrpStreamEvent::NodeDelta {
                node_index: 0,
                delta: NodeDelta::Reasoning {
                    content: Some("first reasoning".to_string()),
                    encrypted: None,
                    summary: None,
                    source: Some("openrouter".to_string()),
                },
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .expect("first reasoning delta");
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                output: vec![
                    Node::Reasoning {
                        id: None,
                        content: Some("first reasoning".to_string()),
                        encrypted: None,
                        summary: None,
                        source: Some("openrouter".to_string()),
                        extra_body: HashMap::new(),
                    },
                    Node::Reasoning {
                        id: None,
                        content: None,
                        encrypted: None,
                        summary: Some("terminal summary".to_string()),
                        source: Some("openrouter".to_string()),
                        extra_body: HashMap::new(),
                    },
                    Node::Reasoning {
                        id: Some("server_1".to_string()),
                        content: None,
                        encrypted: None,
                        summary: None,
                        source: Some("openrouter".to_string()),
                        extra_body: HashMap::from([(
                            urp::CHAT_REASONING_DETAIL_EXTRA_KEY.to_string(),
                            server_detail.clone(),
                        )]),
                    },
                ],
                extra_body: HashMap::new(),
            })
            .await
            .expect("response done");
        drop(event_tx);

        with_sse_capture(frames.clone(), async {
            encode_urp_stream_as_chat(event_rx, sse_tx, "gpt-5.4", None)
                .await
                .expect("encode stream");
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let json_frames = captured_chat_json_frames(&frames);
        let wire = frames.join("");
        assert_eq!(wire.matches("first reasoning").count(), 1, "{wire}");
        assert_eq!(wire.matches("terminal summary").count(), 1, "{wire}");
        assert_eq!(
            wire.matches("reasoning.server_tool_call").count(),
            1,
            "{wire}"
        );
        assert!(json_frames.iter().any(|frame| {
            frame["choices"][0]["delta"]["reasoning_details"][0] == server_detail
        }));
    }

    #[tokio::test]
    async fn synthetic_chat_stream_preserves_content_array_tool_call_blocks() {
        let response = decode_response(&json!({
            "id": "chatcmpl_test",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "before tool" },
                        { "type": "tool_call", "id": "call_1", "name": "lookup", "arguments": { "q": 1 } }
                    ]
                }
            }]
        }))
        .expect("decode response");

        let (sse_tx, mut sse_rx) = mpsc::channel(16);
        emit_synthetic_chat_stream("gpt-5.4", &response, None, sse_tx)
            .await
            .expect("emit synthetic chat stream");

        let mut text = String::new();
        while let Some(event) = sse_rx.recv().await {
            let debug = format!("{event:?}");
            text.push_str(&debug);
        }

        assert!(text.contains("before tool"));
        assert!(text.contains("tool_calls"));
        assert!(text.contains("call_1"));
        assert!(text.contains("lookup"));
        assert!(text.contains("finish_reason"));
    }

    #[tokio::test]
    async fn synthetic_chat_stream_preserves_content_array_tool_use_blocks() {
        let response = decode_response(&json!({
            "id": "chatcmpl_test",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "before tool" },
                        { "type": "tool_use", "id": "call_1", "name": "lookup", "input": { "q": 1 } }
                    ]
                }
            }]
        }))
        .expect("decode response");

        let (sse_tx, mut sse_rx) = mpsc::channel(16);
        emit_synthetic_chat_stream("gpt-5.4", &response, None, sse_tx)
            .await
            .expect("emit synthetic chat stream");

        let mut text = String::new();
        while let Some(event) = sse_rx.recv().await {
            let debug = format!("{event:?}");
            text.push_str(&debug);
        }

        assert!(text.contains("before tool"));
        assert!(text.contains("tool_calls"));
        assert!(text.contains("call_1"));
        assert!(text.contains("lookup"));
        assert!(text.contains("finish_reason"));
    }
}
