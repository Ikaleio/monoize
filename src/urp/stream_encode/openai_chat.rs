use crate::error::AppResult;
use crate::handlers::routing::now_ts;
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
    text: String,
    scored_bytes: usize,
    scored_tokens: usize,
}

fn chat_delta_scores<'a>(
    state: &mut StreamedChatNodeState,
    content: &str,
    scores: &'a Option<Vec<urp::TokenLogprob>>,
) -> Option<&'a [urp::TokenLogprob]> {
    let previous_len = state.text.len();
    state.text.push_str(content);
    let scores = scores.as_deref().filter(|scores| !scores.is_empty())?;
    let bytes: Vec<u8> = scores
        .iter()
        .flat_map(|entry| {
            entry
                .score
                .bytes
                .as_deref()
                .unwrap_or(entry.score.token.as_bytes())
                .iter()
                .copied()
        })
        .collect();
    let unscored = state.text.as_bytes().get(state.scored_bytes..)?;
    if !unscored.starts_with(&bytes)
        || (!content.is_empty()
            && (previous_len != state.scored_bytes || bytes != content.as_bytes()))
    {
        return None;
    }
    state.scored_bytes += bytes.len();
    state.scored_tokens += scores.len();
    Some(scores)
}

async fn emit_chat_terminal_scores(
    tx: &ChatOut,
    id: &str,
    created: i64,
    model: &str,
    node: &Node,
    state: &mut StreamedChatNodeState,
    max_frame_length: Option<usize>,
) -> AppResult<()> {
    let (content, delta, patch) = match node {
        Node::Text { content, .. } => (
            content,
            json!({"content":""}),
            chat_delta_path_content as fn(&mut Value, &str),
        ),
        Node::Refusal { content, .. } => (
            content,
            json!({"refusal":""}),
            chat_delta_path_refusal as fn(&mut Value, &str),
        ),
        _ => return Ok(()),
    };
    let Some(scores) = node
        .token_scores()
        .filter(|scores| state.text == *content && scores.len() > state.scored_tokens)
    else {
        return Ok(());
    };
    let prefix_bytes = scores[..state.scored_tokens]
        .iter()
        .map(|entry| {
            entry
                .score
                .bytes
                .as_deref()
                .unwrap_or(entry.score.token.as_bytes())
                .len()
        })
        .sum::<usize>();
    if prefix_bytes != state.scored_bytes {
        return Ok(());
    }
    send_chat_text_chunk(
        tx,
        id,
        created,
        model,
        delta,
        "",
        patch,
        max_frame_length,
        Some(&scores[state.scored_tokens..]),
    )
    .await?;
    state.scored_bytes = content.len();
    state.scored_tokens = scores.len();
    Ok(())
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
    error.insert("message".to_string(), Value::String(message.to_string()));
    error.remove("code");
    if let Some(code) = code {
        error.insert("code".to_string(), Value::String(code.to_string()));
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

/// Downstream Chat chunk sink. It repeats upstream top-level chunk fields on every chunk
/// (DC6a) and attaches pending choice-level fields to the next chunk that has a choice (DC6).
pub(crate) struct ChatOut {
    tx: mpsc::Sender<Event>,
    envelope_extra: std::sync::Mutex<Map<String, Value>>,
    pending_choice_extra: std::sync::Mutex<Map<String, Value>>,
}

impl ChatOut {
    fn new(tx: mpsc::Sender<Event>) -> Self {
        Self {
            tx,
            envelope_extra: Default::default(),
            pending_choice_extra: Default::default(),
        }
    }

    fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    fn set_envelope_extra(&self, extra: Option<&Map<String, Value>>) {
        let mut envelope = self.envelope_extra.lock().expect("chat envelope lock");
        *envelope = extra
            .into_iter()
            .flatten()
            .filter(|(key, _)| !key.starts_with("_monoize_"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
    }

    fn stash_choice_extra(&self, extra_body: &HashMap<String, Value>) {
        let Some(choice_extra) = extra_body
            .get(CHAT_CHOICE_EXTRA_BODY_KEY)
            .and_then(Value::as_object)
        else {
            return;
        };
        let mut pending = self.pending_choice_extra.lock().expect("chat choice lock");
        for (key, value) in choice_extra {
            if !key.starts_with("_monoize_") {
                pending.insert(key.clone(), value.clone());
            }
        }
    }

    fn has_pending_choice_extra(&self) -> bool {
        !self
            .pending_choice_extra
            .lock()
            .expect("chat choice lock")
            .is_empty()
    }

    async fn send_chunk(&self, chunk: &Value) -> AppResult<()> {
        let mut chunk = chunk.clone();
        if let Some(obj) = chunk.as_object_mut()
            && obj.get("object").and_then(Value::as_str) == Some("chat.completion.chunk")
        {
            for (key, value) in self.envelope_extra.lock().expect("chat envelope lock").iter() {
                obj.entry(key.clone()).or_insert_with(|| value.clone());
            }
            if let Some(choice) = obj
                .get_mut("choices")
                .and_then(Value::as_array_mut)
                .and_then(|choices| choices.first_mut())
                .and_then(Value::as_object_mut)
            {
                let pending =
                    std::mem::take(&mut *self.pending_choice_extra.lock().expect("chat choice lock"));
                for (key, value) in pending {
                    choice.entry(key).or_insert(value);
                }
            }
        }
        send_plain_sse_json(&self.tx, &chunk).await
    }

    async fn send_done(&self) -> AppResult<()> {
        send_plain_sse_data(&self.tx, "[DONE]").await
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_chat_chunk_string(
    tx: &ChatOut,
    id: &str,
    created: i64,
    logical_model: &str,
    delta_template: Value,
    content: &str,
    patch: fn(&mut Value, &str),
    max_frame_length: Option<usize>,
) -> AppResult<()> {
    let base = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{ "index": 0, "delta": delta_template, "finish_reason": Value::Null }]
    });
    for chunk in split_json_value_by_string_patch(base, content, patch, max_frame_length) {
        tx.send_chunk(&chunk).await?;
    }
    Ok(())
}

/// Flushes choice-level fields that no delta chunk carried before the terminal chunk.
async fn flush_pending_choice_extra(
    tx: &ChatOut,
    id: &str,
    created: i64,
    model: &str,
) -> AppResult<()> {
    if !tx.has_pending_choice_extra() {
        return Ok(());
    }
    tx.send_chunk(&json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": Value::Null }]
    }))
    .await
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

fn validate_chat_media_event(event: &UrpStreamEvent) -> Result<(), String> {
    match event {
        UrpStreamEvent::NodeStart {
            header: NodeHeader::ProviderItem { item_type, .. },
            ..
        } if matches!(
            item_type.as_str(),
            "input_image"
                | "output_image"
                | "image_url"
                | "input_file"
                | "output_file"
                | "file"
                | "input_audio"
        ) =>
        {
            Err("Native response content cannot contain input-only media items".into())
        }
        UrpStreamEvent::NodeStart {
            header:
                NodeHeader::Audio {
                    role: urp::OrdinaryRole::Assistant,
                    ..
                },
            extra_body,
            ..
        } if extra_body
            .get(urp::CHAT_MESSAGE_AUDIO_EXTRA_KEY)
            .and_then(Value::as_bool)
            == Some(true) =>
        {
            Ok(())
        }
        UrpStreamEvent::NodeStart {
            header: NodeHeader::Image { .. } | NodeHeader::File { .. } | NodeHeader::Audio { .. },
            ..
        }
        | UrpStreamEvent::NodeDelta {
            delta:
                NodeDelta::Image { .. }
                | NodeDelta::File { .. }
                | NodeDelta::Audio {
                    source: urp::AudioSource::Url { .. },
                },
            ..
        } => Err("Chat Completions responses cannot represent ordinary media content".into()),
        UrpStreamEvent::NodeDone { node, .. } => {
            urp::encode::openai_chat::validate_response_nodes(std::slice::from_ref(node))
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            urp::encode::openai_chat::validate_response_nodes(output)
        }
        _ => Ok(()),
    }
}

async fn emit_chat_media_error(tx: &ChatOut, message: &str) -> AppResult<()> {
    tx.send_chunk(&urp::media::error_body(message)).await?;
    tx.send_done().await?;
    Err(crate::error::AppError::new(
        axum::http::StatusCode::BAD_GATEWAY,
        "unsupported_media",
        message,
    )
    .with_downstream_stream_terminal_sent(!tx.is_closed()))
}

pub(crate) async fn emit_synthetic_chat_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    include_usage: bool,
    sender: mpsc::Sender<Event>,
) -> AppResult<()> {
    let tx = ChatOut::new(sender);
    if let Some(body) = resp
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.failure_body(false))
    {
        tx.send_chunk(&body).await?;
        tx.send_done().await?;
        return Ok(());
    }

    if let Err(error) = urp::encode::openai_chat::validate_response_nodes(&resp.output) {
        return emit_chat_media_error(&tx, &error).await;
    }
    let projected = crate::urp::tool_signature::project_response(resp);
    let resp = &projected;
    let id = urp::encode::openai_chat::chat_completion_id(&resp.id);
    let created = resp.created_at.unwrap_or_else(now_ts);
    tx.set_envelope_extra(Some(
        &resp
            .extra_body
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    ));
    tx.send_chunk(&json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{ "index": 0, "delta": first_chunk_delta(), "finish_reason": Value::Null }]
    }))
    .await?;
    let mut saw_tool = false;
    let mut saw_legacy_function_call = false;
    let mut text_offset = 0u64;
    let mut tool_idx = 0usize;
    let mut reasoning_indexes = ReasoningDetailIndexes::default();
    for (node_index, node) in resp.output.iter().enumerate() {
        match node {
            Node::Reasoning {
                metadata,
                content,
                encrypted,
                summary,
                source,
                extra_body,
                ..
            } => {
                let mut no_envelope_extra = HashMap::new();
                emit_reasoning_delta(
                    &tx,
                    &id,
                    created,
                    logical_model,
                    node_index as u32,
                    node.id().map(String::as_str),
                    content.as_deref(),
                    encrypted.as_ref(),
                    summary.as_deref(),
                    source.as_deref(),
                    metadata,
                    extra_body,
                    &mut no_envelope_extra,
                    &mut reasoning_indexes,
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
                    send_chat_text_chunk(
                        &tx,
                        &id,
                        created,
                        logical_model,
                        {
                            let delta = chat_text_node_delta(node, text_offset);
                            if matches!(node, Node::Text { .. }) {
                                text_offset += content.chars().count() as u64;
                            }
                            delta
                        },
                        content,
                        if matches!(node, Node::Refusal { .. }) {
                            chat_delta_path_refusal
                        } else {
                            chat_delta_path_content
                        },
                        sse_max_frame_length,
                        node.token_scores(),
                    )
                    .await?;
                }
            }
            Node::Image { .. } | Node::File { .. } | Node::Audio { .. } => {
                emit_chat_semantic_node(&tx, &id, created, logical_model, node).await?;
            }
            Node::ProviderItem {
                origin_protocol: urp::ProviderProtocol::ChatCompletion,
                ..
            } => {
                let mut pending_extra = HashMap::new();
                emit_chat_provider_content_part(
                    &tx,
                    &id,
                    created,
                    logical_model,
                    node,
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
        .and_then(Value::as_str);
    let finish_reason = chat_stream_finish_reason(
        resp.finish_reason,
        native_finish_reason,
        saw_tool,
        saw_legacy_function_call,
    );
    emit_chat_terminal_sequence(
        &tx,
        &id,
        created,
        logical_model,
        finish_reason,
        resp.usage.as_ref().filter(|_| include_usage),
        resp.extra_body
            .get(CHAT_CHOICE_EXTRA_BODY_KEY)
            .and_then(Value::as_object),
    )
    .await
}

/// The first chunk of every Chat stream (DC7b), as OpenAI sends it.
fn first_chunk_delta() -> Value {
    json!({ "role": "assistant", "content": "", "refusal": Value::Null })
}

fn chat_stream_finish_reason<'a>(
    finish_reason: Option<FinishReason>,
    native_finish_reason: Option<&'a str>,
    saw_tool: bool,
    saw_legacy_function_call: bool,
) -> &'a str {
    match finish_reason {
        Some(FinishReason::Other) => native_finish_reason
            .filter(|reason| !reason.is_empty())
            .unwrap_or("stop"),
        Some(FinishReason::ToolCalls) if saw_legacy_function_call => "function_call",
        Some(reason) => urp::encode::openai_chat::finish_reason_to_chat(reason),
        None if saw_tool => "tool_calls",
        None if saw_legacy_function_call => "function_call",
        None => "stop",
    }
}

async fn emit_chat_terminal_sequence(
    tx: &ChatOut,
    id: &str,
    created: i64,
    model: &str,
    finish_reason: &str,
    usage: Option<&urp::Usage>,
    choice_extra: Option<&serde_json::Map<String, Value>>,
) -> AppResult<()> {
    flush_pending_choice_extra(tx, id, created, model).await?;
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
    tx.send_chunk(&finish).await?;

    if let Some(usage) = usage {
        let usage_chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [],
            "usage": urp::encode::openai_chat::encode_chat_usage(usage),
        });
        tx.send_chunk(&usage_chunk).await?;
    }

    tx.send_done().await
}

pub(crate) async fn encode_urp_stream_as_chat(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    sender: mpsc::Sender<Event>,
    logical_model: &str,
    sse_max_frame_length: Option<usize>,
    mask_sensitive_info: bool,
    include_usage: bool,
) -> AppResult<()> {
    let tx = ChatOut::new(sender);
    let mut reasoning_indexes = ReasoningDetailIndexes::default();
    let mut signature_projection = crate::urp::tool_signature::SignatureProjection::default();
    let mut chat_id = String::new();
    let mut created = 0i64;
    let mut text_offset = 0u64;
    let mut tool_idx = 0usize;
    let mut saw_tool = false;
    let mut saw_legacy_function_call = false;
    let mut node_states: HashMap<u32, StreamedChatNodeState> = HashMap::new();
    let mut finished = false;
    let mut emitted_node_indices: HashSet<u32> = HashSet::new();
    let mut pending_envelope_extra = HashMap::new();
    let mut native_audio_nodes = HashSet::new();
    let mut node_text_offsets = HashMap::new();

    while let Some(event) = signature_projection.recv(&mut rx).await {
        if finished {
            continue;
        }
        if let UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::Audio { .. },
            extra_body,
            ..
        } = &event
            && extra_body
                .get(urp::CHAT_MESSAGE_AUDIO_EXTRA_KEY)
                .and_then(Value::as_bool)
                == Some(true)
        {
            native_audio_nodes.insert(*node_index);
        }
        if let UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::Audio { .. },
            ..
        } = &event
            && !native_audio_nodes.contains(node_index)
        {
            return emit_chat_media_error(
                &tx,
                "Chat audio fragments require a native message.audio lifecycle",
            )
            .await;
        }
        if let Err(error) = validate_chat_media_event(&event) {
            return emit_chat_media_error(&tx, &error).await;
        }
        if let UrpStreamEvent::NodeDelta { extra_body, .. } = &event {
            tx.stash_choice_extra(extra_body);
        }
        match event {
            UrpStreamEvent::ResponseStart { id, extra_body, .. } => {
                chat_id = urp::encode::openai_chat::chat_completion_id(&id);
                created = extra_body
                    .get(urp::CHAT_CHUNK_EXTRA_KEY)
                    .and_then(|chunk| chunk.get("created"))
                    .and_then(Value::as_i64)
                    .unwrap_or_else(now_ts);
                tx.set_envelope_extra(
                    extra_body
                        .get(urp::CHAT_CHUNK_EXTRA_KEY)
                        .and_then(Value::as_object),
                );
                tx.stash_choice_extra(&extra_body);
                let mut delta = first_chunk_delta();
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
                tx.send_chunk(&chunk).await?;
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
                        ..Default::default()
                    },
                );
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Text { phase, .. },
                ..
            } => {
                if let Some(phase) = phase {
                    pending_envelope_extra.insert("phase".into(), json!(phase));
                }
                node_states.entry(node_index).or_default().saw_node_start = true;
            }
            UrpStreamEvent::NodeStart { node_index, .. } => {
                node_states.entry(node_index).or_default().saw_node_start = true;
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta:
                    NodeDelta::Text {
                        logprobs,
                        signature: _,
                        citations,
                        content,
                    },
                extra_body,
                ..
            } => {
                let node_text_offset = *node_text_offsets.entry(node_index).or_insert(text_offset);
                text_offset = text_offset.saturating_add(content.chars().count() as u64);
                let mut native_delta = json!({"content":""});
                if !citations.is_empty() {
                    native_delta["annotations"] = json!(crate::urp::citations::encode(
                        &citations,
                        crate::urp::ProviderProtocol::ChatCompletion,
                        node_text_offset
                    ));
                }
                let delta =
                    chat_delta_with_extras(native_delta, &extra_body, &mut pending_envelope_extra);
                let scores = chat_delta_scores(
                    node_states.entry(node_index).or_default(),
                    &content,
                    &logprobs,
                );
                send_chat_text_chunk(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    delta,
                    &content,
                    chat_delta_path_content,
                    sse_max_frame_length,
                    scores,
                )
                .await?;
                emitted_node_indices.insert(node_index);
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Refusal { logprobs, content },
                extra_body,
                ..
            } => {
                let delta = chat_delta_with_extras(
                    json!({"refusal":""}),
                    &extra_body,
                    &mut pending_envelope_extra,
                );
                let scores = chat_delta_scores(
                    node_states.entry(node_index).or_default(),
                    &content,
                    &logprobs,
                );
                send_chat_text_chunk(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    delta,
                    &content,
                    chat_delta_path_refusal,
                    sse_max_frame_length,
                    scores,
                )
                .await?;
                emitted_node_indices.insert(node_index);
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta:
                    NodeDelta::Reasoning {
                        metadata,
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
                    node_index,
                    metadata.item_id.as_deref(),
                    content.as_deref(),
                    encrypted.as_ref(),
                    summary.as_deref(),
                    source.as_deref(),
                    &metadata,
                    &extra_body,
                    &mut pending_envelope_extra,
                    &mut reasoning_indexes,
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
                emit_chat_terminal_scores(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    &node,
                    state,
                    sse_max_frame_length,
                )
                .await?;
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
                } else if matches!(
                    &node,
                    Node::Image { .. } | Node::File { .. } | Node::Audio { .. }
                ) {
                    emit_chat_semantic_node(&tx, &chat_id, created, logical_model, &node).await?;
                    emitted_node_indices.insert(node_index);
                } else if let Node::ProviderItem {
                    origin_protocol: urp::ProviderProtocol::ChatCompletion,
                    ..
                } = &node
                {
                    emit_chat_provider_content_part(
                        &tx,
                        &chat_id,
                        created,
                        logical_model,
                        &node,
                        &HashMap::new(),
                        &mut pending_envelope_extra,
                    )
                    .await?;
                    emitted_node_indices.insert(node_index);
                }
            }
            UrpStreamEvent::ResponseDone {
                outcome,
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                if let Some(mut body) = outcome
                    .as_ref()
                    .and_then(|outcome| outcome.failure_body(false))
                {
                    if let Some(message) = body["error"]["message"].as_str() {
                        body["error"]["message"] =
                            json!(crate::error_sanitize::maybe_mask_sensitive_text(
                                message,
                                mask_sensitive_info
                            ));
                    }
                    tx.send_chunk(&body).await?;
                    tx.send_done().await?;
                    return Ok(());
                }

                for (key, value) in native_chat_delta_extra(&extra_body) {
                    pending_envelope_extra.insert(key, value);
                }
                for (node_index, node) in output.iter().enumerate() {
                    if emitted_node_indices.contains(&(node_index as u32)) {
                        emit_chat_terminal_scores(
                            &tx,
                            &chat_id,
                            created,
                            logical_model,
                            node,
                            node_states.entry(node_index as u32).or_default(),
                            sse_max_frame_length,
                        )
                        .await?;
                        continue;
                    }
                    if let Node::ToolCall { call_id, .. } = node
                        && node_states.values().any(|state| {
                            state
                                .tool_call
                                .as_ref()
                                .is_some_and(|call| call.header_sent && call.call_id == *call_id)
                        })
                    {
                        continue;
                    }
                    match node {
                        Node::Reasoning {
                            metadata,
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
                                node_index as u32,
                                node.id().map(String::as_str),
                                content.as_deref(),
                                encrypted.as_ref(),
                                summary.as_deref(),
                                source.as_deref(),
                                metadata,
                                extra_body,
                                &mut pending_envelope_extra,
                                &mut reasoning_indexes,
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
                                    chat_text_node_delta(node, text_offset),
                                    &HashMap::new(),
                                    &mut pending_envelope_extra,
                                );
                                if matches!(node, Node::Text { .. }) {
                                    text_offset += content.chars().count() as u64;
                                }
                                send_chat_text_chunk(
                                    &tx,
                                    &chat_id,
                                    created,
                                    logical_model,
                                    delta,
                                    content,
                                    if matches!(node, Node::Refusal { .. }) {
                                        chat_delta_path_refusal
                                    } else {
                                        chat_delta_path_content
                                    },
                                    sse_max_frame_length,
                                    node.token_scores(),
                                )
                                .await?;
                            }
                        }
                        Node::Image { .. } | Node::File { .. } | Node::Audio { .. } => {
                            emit_chat_semantic_node(&tx, &chat_id, created, logical_model, node)
                                .await?;
                        }
                        Node::ProviderItem {
                            origin_protocol: urp::ProviderProtocol::ChatCompletion,
                            ..
                        } => {
                            emit_chat_provider_content_part(
                                &tx,
                                &chat_id,
                                created,
                                logical_model,
                                node,
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
                    tx.send_chunk(&chunk).await?;
                }
                let finish_reason = chat_stream_finish_reason(
                    finish_reason,
                    extra_body
                        .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
                        .and_then(Value::as_str),
                    saw_tool,
                    saw_legacy_function_call,
                );
                emit_chat_terminal_sequence(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    finish_reason,
                    usage.as_ref().filter(|_| include_usage),
                    extra_body
                        .get(CHAT_CHOICE_EXTRA_BODY_KEY)
                        .and_then(Value::as_object),
                )
                .await?;
                finished = true;
            }
            UrpStreamEvent::ProviderControl {
                protocol,
                event_name,
                data,
                ..
            } => {
                // PC7h: message-global citations of a completed text segment.
                if protocol == "chat_completion"
                    && event_name == urp::CHAT_ANNOTATIONS_CONTROL
                    && let Some(annotations) = data.get("annotations")
                {
                    tx.send_chunk(&json!({
                        "id": chat_id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": logical_model,
                        "choices": [{
                            "index": 0,
                            "delta": { "annotations": annotations },
                            "finish_reason": Value::Null,
                        }],
                    }))
                    .await?;
                }
            }
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
                tx.send_chunk(&payload).await?;
                tx.send_done().await?;
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
        || extra_body.contains_key(urp::CHAT_REASONING_DETAIL_EXTRA_KEY)
        || extra_body.contains_key(CHAT_DELTA_EXTRA_BODY_KEY)
}

async fn emit_chat_provider_content_part(
    tx: &ChatOut,
    chat_id: &str,
    created: i64,
    logical_model: &str,
    node: &Node,
    event_extra: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
) -> AppResult<()> {
    let delta = chat_delta_with_extras(
        Value::Object(
            crate::urp::encode::openai_chat::encode_assistant_chat_message_from_nodes(
                std::slice::from_ref(node),
            ),
        ),
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
    tx.send_chunk(&chunk).await
}

async fn emit_tool_call_header(
    tx: &ChatOut,
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
    tx.send_chunk(&chunk).await?;
    tool_call.header_sent = true;
    Ok(())
}

async fn emit_tool_call_arguments_delta(
    tx: &ChatOut,
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

/// DC5: one stable `index` per logical synthesized detail (node, detail type) in a stream.
#[derive(Default)]
struct ReasoningDetailIndexes {
    next: u64,
    assigned: HashMap<(u32, String), u64>,
}

impl ReasoningDetailIndexes {
    fn index_for(&mut self, node_index: u32, kind: &str) -> u64 {
        let next = &mut self.next;
        *self
            .assigned
            .entry((node_index, kind.to_string()))
            .or_insert_with(|| {
                let index = *next;
                *next += 1;
                index
            })
    }

    fn observe_native(&mut self, index: u64) {
        self.next = self.next.max(index + 1);
    }
}

fn chat_delta_path_reasoning_detail_text(value: &mut Value, content: &str) {
    let delta = &mut value["choices"][0]["delta"];
    delta["reasoning"] = json!(content);
    delta["reasoning_details"][0]["text"] = json!(content);
}

fn chat_delta_path_reasoning_detail_summary(value: &mut Value, content: &str) {
    let delta = &mut value["choices"][0]["delta"];
    delta["reasoning"] = json!(content);
    delta["reasoning_details"][0]["summary"] = json!(content);
}

fn chat_delta_path_reasoning_detail_data(value: &mut Value, content: &str) {
    value["choices"][0]["delta"]["reasoning_details"][0]["data"] = json!(content);
}

fn chat_delta_path_scalar(field: &'static str) -> fn(&mut Value, &str) {
    match field {
        "reasoning_content" => |value, content| {
            value["choices"][0]["delta"]["reasoning_content"] = json!(content);
        },
        _ => |value, content| {
            value["choices"][0]["delta"]["reasoning_opaque"] = json!(content);
        },
    }
}

/// Emits one reasoning delta, or one terminal-only reasoning node, under DC5.
#[allow(clippy::too_many_arguments)]
async fn emit_reasoning_delta(
    tx: &ChatOut,
    chat_id: &str,
    created: i64,
    logical_model: &str,
    node_index: u32,
    item_id: Option<&str>,
    content: Option<&str>,
    encrypted: Option<&Value>,
    summary: Option<&str>,
    source: Option<&str>,
    metadata: &urp::ReasoningMetadata,
    extra_body: &HashMap<String, Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
    indexes: &mut ReasoningDetailIndexes,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let mut event_delta_extra = native_chat_delta_extra(extra_body);
    let surface = extra_body
        .get(urp::CHAT_REASONING_SURFACE_EXTRA_KEY)
        .and_then(Value::as_str);
    let mut content = content;
    let mut encrypted = encrypted;
    let scalar = match surface {
        Some(urp::CHAT_REASONING_SURFACE_REASONING_CONTENT) => content
            .take()
            .map(|content| ("reasoning_content", content.to_string())),
        Some(urp::CHAT_REASONING_SURFACE_REASONING_OPAQUE) => encrypted.take().and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| (!value.is_null()).then(|| value.to_string()))
                .map(|value| ("reasoning_opaque", value))
        }),
        _ => None,
    };
    if let Some((field, value)) = scalar
        && !value.is_empty()
    {
        send_chat_chunk_string(
            tx,
            chat_id,
            created,
            logical_model,
            chat_delta_with_raw_extras(
                json!({ (field): "" }),
                &mut event_delta_extra,
                pending_envelope_extra,
            ),
            &value,
            chat_delta_path_scalar(field),
            sse_max_frame_length,
        )
        .await?;
    }
    let plain = content
        .filter(|content| !content.is_empty())
        .or(summary.filter(|summary| !summary.is_empty()));
    {
        if let Some(rc_value) = metadata.chat_content.then_some(plain).flatten() {
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
                chat_delta_path_scalar("reasoning_content"),
                sse_max_frame_length,
            )
            .await?;
        }
        let native = extra_body.contains_key(urp::CHAT_REASONING_DETAIL_EXTRA_KEY);
        for mut detail in urp::reasoning::chat_node_details(
            metadata, item_id, content, summary, encrypted, source, extra_body,
        ) {
            let kind = detail
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match detail.get("index").and_then(Value::as_u64) {
                Some(index) => indexes.observe_native(index),
                None if !native => {
                    detail.insert("index".into(), json!(indexes.index_for(node_index, &kind)));
                }
                None => {}
            }
            let (field, patch): (&str, Option<fn(&mut Value, &str)>) = match kind.as_str() {
                "reasoning.text" => ("text", Some(chat_delta_path_reasoning_detail_text)),
                "reasoning.summary" => ("summary", Some(chat_delta_path_reasoning_detail_summary)),
                "reasoning.encrypted" => ("data", Some(chat_delta_path_reasoning_detail_data)),
                _ => ("", None),
            };
            let payload = detail
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_string);
            let alias = matches!(kind.as_str(), "reasoning.text" | "reasoning.summary");
            match (payload, patch) {
                // An Anthropic signature is opaque and must not be split across chunks.
                (Some(payload), Some(patch)) if !detail.contains_key("signature") => {
                    detail.insert(field.into(), json!(""));
                    let mut delta = json!({ "reasoning_details": [Value::Object(detail)] });
                    if alias {
                        delta["reasoning"] = json!("");
                    }
                    send_chat_chunk_string(
                        tx,
                        chat_id,
                        created,
                        logical_model,
                        chat_delta_with_raw_extras(
                            delta,
                            &mut event_delta_extra,
                            pending_envelope_extra,
                        ),
                        &payload,
                        patch,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                (payload, _) => {
                    let mut delta = json!({ "reasoning_details": [Value::Object(detail)] });
                    if alias && let Some(payload) = payload.filter(|payload| !payload.is_empty()) {
                        delta["reasoning"] = json!(payload);
                    }
                    let delta = chat_delta_with_raw_extras(
                        delta,
                        &mut event_delta_extra,
                        pending_envelope_extra,
                    );
                    tx.send_chunk(&json!({
                        "id": chat_id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": logical_model,
                        "choices": [{ "index": 0, "delta": delta, "finish_reason": Value::Null }]
                    }))
                    .await?;
                }
            }
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
        tx.send_chunk(&chunk).await?;
    }
    Ok(())
}

fn chat_delta_path_refusal(value: &mut Value, content: &str) {
    value["choices"][0]["delta"]["refusal"] = json!(content);
}

fn chat_text_node_delta(node: &Node, text_offset: u64) -> Value {
    match node {
        Node::Refusal { .. } => json!({"refusal":""}),
        Node::Text {
            citations, phase, ..
        } => {
            let mut delta = json!({"content":""});
            if !citations.is_empty() {
                delta["annotations"] = json!(crate::urp::citations::encode(
                    &citations,
                    crate::urp::ProviderProtocol::ChatCompletion,
                    text_offset
                ));
            }
            if let Some(phase) = phase {
                delta["phase"] = json!(phase);
            }
            delta
        }
        _ => json!({}),
    }
}

async fn emit_chat_semantic_node(
    tx: &ChatOut,
    id: &str,
    created: i64,
    model: &str,
    node: &Node,
) -> AppResult<()> {
    let delta = crate::urp::encode::openai_chat::encode_assistant_chat_message_from_nodes(
        std::slice::from_ref(node),
    );
    tx.send_chunk(&json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":delta,"finish_reason":null}]})).await
}

async fn send_chat_text_chunk(
    tx: &ChatOut,
    id: &str,
    created: i64,
    model: &str,
    mut delta: Value,
    content: &str,
    patch: fn(&mut Value, &str),
    max_frame_length: Option<usize>,
    scores: Option<&[urp::TokenLogprob]>,
) -> AppResult<()> {
    let annotations = delta
        .as_object_mut()
        .and_then(|object| object.remove("annotations"));
    if let Some(scores) = scores {
        let field = if delta.get("refusal").is_some() {
            "refusal"
        } else {
            "content"
        };
        let mut chunk = json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":delta,"finish_reason":null,"logprobs":{field:urp::logprobs::encode_openai(scores)}}]});
        patch(&mut chunk, content);
        if max_frame_length.is_some_and(|limit| chunk.to_string().len() + 8 > limit) {
            for (text, scores) in urp::logprobs::fragments(scores) {
                let mut part = chunk.clone();
                patch(&mut part, if content.is_empty() { "" } else { &text });
                part["choices"][0]["logprobs"][field] = urp::logprobs::encode_openai(&scores);
                tx.send_chunk(&part).await?;
            }
        } else {
            tx.send_chunk(&chunk).await?;
        }
    } else {
        send_chat_chunk_string(
            tx,
            id,
            created,
            model,
            delta,
            content,
            patch,
            max_frame_length,
        )
        .await?;
    }

    if let Some(annotations) = annotations
        .and_then(|value| value.as_array().cloned())
        .filter(|values| !values.is_empty())
    {
        tx.send_chunk(&json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":{"annotations":annotations},"finish_reason":null}]})).await?;
    }
    Ok(())
}
