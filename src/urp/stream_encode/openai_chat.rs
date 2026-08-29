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
    tool_type: urp::ToolCallType,
    call_id: String,
    name: String,
    index: usize,
    legacy_function_call: bool,
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

fn retain_chat_error_owner_fields(obj: &mut Map<String, Value>) {
    obj.retain(|key, _| !key.starts_with("_monoize_"));
}

fn sanitize_chat_error_object(error: &mut Map<String, Value>) {
    retain_chat_error_owner_fields(error);
    if let Some(metadata) = error.get_mut("metadata").and_then(Value::as_object_mut) {
        retain_chat_error_owner_fields(metadata);
    }
}

fn sanitize_chat_error_replay_owners(payload: &mut Value) {
    let Some(root) = payload.as_object_mut() else {
        return;
    };
    retain_chat_error_owner_fields(root);
    if let Some(error) = root.get_mut("error").and_then(Value::as_object_mut) {
        sanitize_chat_error_object(error);
    }
    let Some(choices) = root.get_mut("choices").and_then(Value::as_array_mut) else {
        return;
    };
    for choice in choices {
        let Some(choice) = choice.as_object_mut() else {
            continue;
        };
        retain_chat_error_owner_fields(choice);
        for key in ["delta", "message"] {
            if let Some(owner) = choice.get_mut(key).and_then(Value::as_object_mut) {
                retain_chat_error_owner_fields(owner);
            }
        }
        if let Some(error) = choice.get_mut("error").and_then(Value::as_object_mut) {
            sanitize_chat_error_object(error);
        }
    }
}

fn nonempty_json_scalar(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Number(_)))
        || matches!(value, Some(Value::String(value)) if !value.is_empty())
}

fn materialize_chat_error_fields(
    error: &mut Map<String, Value>,
    code: Option<&str>,
    message: &str,
    extra_body: &HashMap<String, Value>,
) {
    if error
        .get("message")
        .and_then(Value::as_str)
        .is_none_or(|value| value.is_empty())
    {
        error.insert("message".to_string(), Value::String(message.to_string()));
    }
    if !nonempty_json_scalar(error.get("code")) {
        if let Some(code) = code {
            error.insert("code".to_string(), Value::String(code.to_string()));
        }
    }
    if !nonempty_json_scalar(error.get("type")) {
        let error_type = extra_body
            .get("type")
            .filter(|value| nonempty_json_scalar(Some(value)))
            .cloned()
            .unwrap_or_else(|| Value::String("server_error".to_string()));
        error.insert("type".to_string(), error_type);
    }
    if !error.contains_key("param") {
        if let Some(param) = extra_body.get("param") {
            error.insert("param".to_string(), param.clone());
        }
    }
}

fn chat_error_payload(
    original: Option<&Value>,
    code: Option<&str>,
    message: &str,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut payload = original.cloned().unwrap_or_else(|| json!({}));
    sanitize_chat_error_replay_owners(&mut payload);

    let Some(root) = payload.as_object_mut() else {
        return chat_error_payload(None, code, message, extra_body);
    };
    if let Some(error) = root.get_mut("error").and_then(Value::as_object_mut) {
        materialize_chat_error_fields(error, code, message, extra_body);
        return payload;
    }
    if let Some(error) = root
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|choices| choices.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("error"))
        .and_then(Value::as_object_mut)
    {
        materialize_chat_error_fields(error, code, message, extra_body);
        return payload;
    }

    let mut error = extra_body
        .get("error")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    sanitize_chat_error_object(&mut error);
    materialize_chat_error_fields(&mut error, code, message, extra_body);
    root.insert("error".to_string(), Value::Object(error));
    payload
}

fn chat_delta_with_extras(
    delta: Value,
    event_extra: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
) -> Value {
    let mut event_delta_extra = native_chat_delta_extra(event_extra);
    chat_delta_with_raw_extras(delta, &mut event_delta_extra, pending_envelope_extra)
}

async fn emit_chat_choice_extra_chunk(
    tx: &mpsc::Sender<Event>,
    id: &str,
    created: i64,
    model: &str,
    extra_body: &HashMap<String, Value>,
) -> AppResult<()> {
    let Some(choice_extra) = extra_body
        .get(CHAT_CHOICE_EXTRA_BODY_KEY)
        .and_then(Value::as_object)
        .filter(|extra| !extra.is_empty())
    else {
        return Ok(());
    };
    let mut choice = json!({ "index": 0, "delta": {}, "finish_reason": Value::Null });
    if let Some(choice) = choice.as_object_mut() {
        for (key, value) in choice_extra {
            if !key.starts_with("_monoize_") {
                choice.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    let chunk = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [choice]
    });
    send_plain_sse_data(tx, chunk.to_string()).await
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
    let mut saw_legacy_function_call = false;
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
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
                ..
            } => {
                let legacy_function_call = *tool_type == urp::ToolCallType::Function
                    && extra_body
                        .get(urp::CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                        .and_then(Value::as_bool)
                        == Some(true);
                if legacy_function_call {
                    saw_legacy_function_call = true;
                    send_chat_chunk_string(
                        &tx,
                        &id,
                        created,
                        logical_model,
                        json!({
                            "function_call": { "name": name, "arguments": "" }
                        }),
                        arguments,
                        chat_delta_path_function_call_arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                    continue;
                }
                saw_tool = true;
                let (wire_type, payload_key, argument_key) = match tool_type {
                    urp::ToolCallType::Function => ("function", "function", "arguments"),
                    urp::ToolCallType::Custom => ("custom", "custom", "input"),
                };
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
                                "type": wire_type,
                                (payload_key): { "name": name, (argument_key): "" }
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
                    if *tool_type == urp::ToolCallType::Custom {
                        chat_delta_path_custom_tool_input
                    } else {
                        chat_delta_path_tool_arguments
                    },
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
    } else if saw_legacy_function_call {
        "function_call"
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
    mask_sensitive_info: bool,
) -> AppResult<()> {
    let mut chat_id = String::new();
    let mut created = 0i64;
    let mut tool_idx = 0usize;
    let mut saw_tool = false;
    let mut saw_legacy_function_call = false;
    let mut node_states: HashMap<u32, StreamedChatNodeState> = HashMap::new();
    let mut finished = false;
    let mut emitted_node_indices: HashSet<u32> = HashSet::new();
    let mut pending_envelope_extra = HashMap::new();

    while let Some(event) = rx.recv().await {
        if finished {
            continue;
        }
        if let UrpStreamEvent::NodeDelta { extra_body, .. } = &event {
            emit_chat_choice_extra_chunk(&tx, &chat_id, created, logical_model, extra_body).await?;
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
                header:
                    NodeHeader::ToolCall {
                        tool_type,
                        call_id,
                        name,
                        ..
                    },
                extra_body,
            } => {
                let legacy_function_call = tool_type == urp::ToolCallType::Function
                    && extra_body
                        .get(urp::CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                        .and_then(Value::as_bool)
                        == Some(true);
                if legacy_function_call {
                    saw_legacy_function_call = true;
                } else {
                    saw_tool = true;
                }
                let idx = tool_idx;
                tool_idx += 1;
                let mut tool_call = StreamedChatToolCall {
                    tool_type,
                    call_id,
                    name,
                    index: idx,
                    legacy_function_call,
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

                if tool_call.legacy_function_call {
                    saw_legacy_function_call = true;
                } else {
                    saw_tool = true;
                }
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
                    tool_call,
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
                    tool_type,
                    call_id,
                    name,
                    arguments,
                    extra_body,
                    ..
                } = node
                {
                    let legacy_function_call = tool_type == urp::ToolCallType::Function
                        && extra_body
                            .get(urp::CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                            .and_then(Value::as_bool)
                            == Some(true);
                    if legacy_function_call {
                        saw_legacy_function_call = true;
                    } else {
                        saw_tool = true;
                    }
                    let tool_call = state.tool_call.get_or_insert_with(|| {
                        let idx = tool_idx;
                        tool_idx += 1;
                        StreamedChatToolCall {
                            tool_type,
                            call_id,
                            name,
                            index: idx,
                            legacy_function_call,
                            header_sent: false,
                            arguments_streamed: false,
                        }
                    });
                    if tool_type == urp::ToolCallType::Custom {
                        tool_call.tool_type = urp::ToolCallType::Custom;
                    }
                    tool_call.legacy_function_call |= legacy_function_call;
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
                            tool_call,
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
                            tool_type,
                            call_id,
                            name,
                            arguments,
                            extra_body,
                            ..
                        } => {
                            let legacy_function_call = *tool_type == urp::ToolCallType::Function
                                && extra_body
                                    .get(urp::CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                                    .and_then(Value::as_bool)
                                    == Some(true);
                            let mut tool_call = StreamedChatToolCall {
                                tool_type: *tool_type,
                                call_id: call_id.clone(),
                                name: name.clone(),
                                index: tool_idx,
                                legacy_function_call,
                                header_sent: false,
                                arguments_streamed: false,
                            };
                            tool_idx += 1;
                            if legacy_function_call {
                                saw_legacy_function_call = true;
                            } else {
                                saw_tool = true;
                            }
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
                                    &tool_call,
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
                } else if saw_legacy_function_call {
                    "function_call"
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
                // SAN-11 / SAN-CFG5: decoder-origin error text may embed
                // upstream URLs; masking is gated by the runtime setting.
                let message =
                    crate::error_sanitize::maybe_mask_sensitive_text(&message, mask_sensitive_info);
                let payload = chat_error_payload(
                    extra_body.get(CHAT_ERROR_EVENT_EXTRA_KEY),
                    code.as_deref(),
                    &message,
                    &extra_body,
                );
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
        if tool_call.legacy_function_call {
            json!({
                "function_call": {
                    "name": tool_call.name,
                    "arguments": ""
                }
            })
        } else {
            match tool_call.tool_type {
                urp::ToolCallType::Function => json!({
                    "tool_calls": [{
                        "index": tool_call.index,
                        "id": tool_call.call_id,
                        "type": "function",
                        "function": { "name": tool_call.name, "arguments": "" }
                    }]
                }),
                urp::ToolCallType::Custom => json!({
                    "tool_calls": [{
                        "index": tool_call.index,
                        "id": tool_call.call_id,
                        "type": "custom",
                        "custom": { "name": tool_call.name, "input": "" }
                    }]
                }),
            }
        },
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
    tool_call: &StreamedChatToolCall,
    arguments: &str,
    event_extra: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let delta = chat_delta_with_extras(
        if tool_call.legacy_function_call {
            json!({
                "function_call": { "arguments": "" }
            })
        } else {
            match tool_call.tool_type {
                urp::ToolCallType::Function => json!({
                    "tool_calls": [{
                        "index": tool_call.index,
                        "function": { "arguments": "" }
                    }]
                }),
                urp::ToolCallType::Custom => json!({
                    "tool_calls": [{
                        "index": tool_call.index,
                        "custom": { "input": "" }
                    }]
                }),
            }
        },
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
        if tool_call.legacy_function_call {
            chat_delta_path_function_call_arguments
        } else if tool_call.tool_type == urp::ToolCallType::Custom {
            chat_delta_path_custom_tool_input
        } else {
            chat_delta_path_tool_arguments
        },
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
