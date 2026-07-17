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

    #[test]
    fn chat_raw_error_replay_materializes_metadata_fallbacks_and_filters_reserved_owners() {
        let original = json!({
            "id": "chatcmpl_error",
            "vendor_frame": true,
            "_monoize_frame_spoof": true,
            "error": {
                "message": "provider failed",
                "metadata": {
                    "provider_code": "P529",
                    "error_type": "provider_error",
                    "vendor_metadata": 7,
                    "_monoize_metadata_spoof": true
                },
                "vendor_error": 8,
                "_monoize_error_spoof": true
            }
        });
        let extra_body = HashMap::from([
            ("type".to_string(), json!("provider_error")),
            ("param".to_string(), json!("route")),
        ]);

        let replay = chat_error_payload(Some(&original), Some("P529"), "fallback", &extra_body);
        assert_eq!(replay["vendor_frame"], json!(true));
        assert_eq!(replay["error"]["message"], json!("provider failed"));
        assert_eq!(replay["error"]["code"], json!("P529"));
        assert_eq!(replay["error"]["type"], json!("provider_error"));
        assert_eq!(replay["error"]["param"], json!("route"));
        assert_eq!(replay["error"]["vendor_error"], json!(8));
        assert_eq!(replay["error"]["metadata"]["vendor_metadata"], json!(7));
        assert!(!replay.to_string().contains("_monoize_"));

        let direct = chat_error_payload(
            Some(&json!({
                "choices": [{
                    "index": 0,
                    "vendor_choice": 9,
                    "_monoize_choice_spoof": true,
                    "error": { "message": "native", "code": 503, "type": "native_error" }
                }]
            })),
            Some("P529"),
            "fallback",
            &extra_body,
        );
        assert_eq!(direct["choices"][0]["error"]["code"], json!(503));
        assert_eq!(direct["choices"][0]["error"]["type"], json!("native_error"));
        assert_eq!(direct["choices"][0]["vendor_choice"], json!(9));
        assert!(!direct.to_string().contains("_monoize_"));
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
    async fn chat_stream_emits_choice_level_logprobs_on_token_frame() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (sse_tx, mut sse_rx) = mpsc::channel(8);
        let frames = Arc::new(Mutex::new(Vec::new()));

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_logprobs".to_string(),
                model: "deepseek-chat".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::NodeDelta {
                node_index: 0,
                delta: NodeDelta::Text {
                    content: "A".to_string(),
                },
                usage: None,
                extra_body: HashMap::from([(
                    CHAT_CHOICE_EXTRA_BODY_KEY.to_string(),
                    json!({ "logprobs": { "content": [{ "token": "A", "logprob": -0.1 }] } }),
                )]),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                output: Vec::new(),
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        drop(event_tx);

        with_sse_capture(frames.clone(), async {
            encode_urp_stream_as_chat(event_rx, sse_tx, "deepseek-chat", None)
                .await
                .unwrap();
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let json_frames = captured_chat_json_frames(&frames);
        let logprobs_frame = json_frames
            .iter()
            .find(|frame| frame["choices"][0].get("logprobs").is_some())
            .expect("choice-level logprobs frame");
        assert_eq!(
            logprobs_frame["choices"][0]["logprobs"]["content"][0]["token"],
            json!("A")
        );
        assert_eq!(logprobs_frame["choices"][0]["delta"], json!({}));
        assert!(
            logprobs_frame["choices"][0]["delta"]
                .get("logprobs")
                .is_none()
        );
        assert_eq!(logprobs_frame["choices"][0]["finish_reason"], Value::Null);
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
    async fn chat_stream_terminal_usage_preserves_nested_unknown_details_and_typed_counters_win() {
        let (event_tx, event_rx) = mpsc::channel(4);
        let (sse_tx, mut sse_rx) = mpsc::channel(8);
        let frames = Arc::new(Mutex::new(Vec::new()));

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_nested_usage".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response start");
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: Some(urp::Usage {
                    input_tokens: 12,
                    output_tokens: 8,
                    input_details: Some(urp::InputDetails {
                        cache_read_tokens: 3,
                        ..urp::InputDetails::default()
                    }),
                    output_details: Some(urp::OutputDetails {
                        reasoning_tokens: 5,
                        ..urp::OutputDetails::default()
                    }),
                    extra_body: HashMap::from([
                        (
                            "prompt_tokens_details".to_string(),
                            json!({
                                "cached_tokens": 999,
                                "future_prompt_detail": { "kind": "warm" },
                                "_monoize_hidden": true
                            }),
                        ),
                        (
                            "completion_tokens_details".to_string(),
                            json!({
                                "reasoning_tokens": 999,
                                "future_completion_detail": [1, 2]
                            }),
                        ),
                    ]),
                }),
                output: Vec::new(),
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
        let usage = json_frames
            .iter()
            .find(|frame| frame["choices"] == json!([]))
            .and_then(|frame| frame.get("usage"))
            .expect("terminal Chat usage frame");
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], json!(3));
        assert_eq!(
            usage["prompt_tokens_details"]["future_prompt_detail"],
            json!({ "kind": "warm" })
        );
        assert!(
            usage["prompt_tokens_details"]
                .get("_monoize_hidden")
                .is_none()
        );
        assert_eq!(
            usage["completion_tokens_details"]["reasoning_tokens"],
            json!(5)
        );
        assert_eq!(
            usage["completion_tokens_details"]["future_completion_detail"],
            json!([1, 2])
        );
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
                    tool_type: urp::ToolCallType::Function,
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
                        tool_type: urp::ToolCallType::Function,
                        call_id: "call_a".to_string(),
                        name: "tool_a".to_string(),
                        arguments: "{\"a\":1}".to_string(),
                        extra_body: HashMap::new(),
                    },
                    Node::ToolCall {
                        id: None,
                        tool_type: urp::ToolCallType::Function,
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
    async fn legacy_function_call_stream_replays_deprecated_shape_and_finish_reason() {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (sse_tx, mut sse_rx) = mpsc::channel(32);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let legacy_extra = HashMap::from([(
            urp::CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY.to_string(),
            Value::Bool(true),
        )]);

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_legacy".to_string(),
                model: "gpt-4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::NodeStart {
                node_index: 0,
                header: NodeHeader::ToolCall {
                    id: None,
                    tool_type: urp::ToolCallType::Function,
                    call_id: "legacy_function:lookup".to_string(),
                    name: "lookup".to_string(),
                },
                extra_body: legacy_extra.clone(),
            })
            .await
            .unwrap();
        for arguments in ["{\"q\":", "1}"] {
            event_tx
                .send(UrpStreamEvent::NodeDelta {
                    node_index: 0,
                    delta: NodeDelta::ToolCallArguments {
                        arguments: arguments.to_string(),
                    },
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .await
                .unwrap();
        }
        let node = Node::ToolCall {
            id: None,
            tool_type: urp::ToolCallType::Function,
            call_id: "legacy_function:lookup".to_string(),
            name: "lookup".to_string(),
            arguments: "{\"q\":1}".to_string(),
            extra_body: legacy_extra,
        };
        event_tx
            .send(UrpStreamEvent::NodeDone {
                node_index: 0,
                node: node.clone(),
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                output: vec![node],
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        drop(event_tx);

        with_sse_capture(frames.clone(), async {
            encode_urp_stream_as_chat(event_rx, sse_tx, "gpt-4", None)
                .await
                .unwrap();
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let json_frames = captured_chat_json_frames(&frames);
        assert!(
            json_frames
                .iter()
                .all(|frame| !frame.to_string().contains("tool_calls"))
        );
        assert_eq!(
            json_frames
                .iter()
                .find_map(|frame| frame["choices"][0]["delta"]["function_call"]["name"].as_str()),
            Some("lookup")
        );
        let arguments = json_frames
            .iter()
            .filter_map(|frame| frame["choices"][0]["delta"]["function_call"]["arguments"].as_str())
            .collect::<String>();
        assert_eq!(arguments, "{\"q\":1}");
        assert_eq!(
            json_frames
                .iter()
                .filter(|frame| {
                    frame["choices"][0]["finish_reason"].as_str() == Some("function_call")
                })
                .count(),
            1
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.contains("[DONE]"))
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
