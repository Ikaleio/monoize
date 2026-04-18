use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::handlers::usage::usage_to_chat_usage_json;
use crate::urp::stream_helpers::*;
use crate::urp::{self, FinishReason, Node, NodeDelta, NodeHeader, UrpStreamEvent};
use axum::response::sse::Event;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum ChatNodeKind {
    Content,
    ReasoningText,
    ReasoningSummary,
    ReasoningEncrypted,
    ToolCall,
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
                            if let Some(summary) =
                                summary.as_deref().filter(|summary| !summary.is_empty())
                            {
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
                            if let Some(content) =
                                content.as_deref().filter(|content| !content.is_empty())
                            {
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
                                        chat_reasoning_delta_from_encrypted(
                                            "",
                                            format,
                                            reasoning_id,
                                        ),
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
            _ => continue,
        }
    }

    let finish_reason = if saw_tool {
        "tool_calls"
    } else {
        finish_reason_to_chat(resp.finish_reason.unwrap_or(urp::FinishReason::Stop))
    };
    let mut done = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
    });
    if let Some(usage) = resp.usage.as_ref() {
        done["usage"] = usage_to_chat_usage_json(usage);
    }
    send_plain_sse_data(&tx, done.to_string()).await?;
    send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
    Ok(())
}

fn finish_reason_to_chat(reason: urp::FinishReason) -> &'static str {
    match reason {
        urp::FinishReason::Stop => "stop",
        urp::FinishReason::Length => "length",
        urp::FinishReason::ToolCalls => "tool_calls",
        urp::FinishReason::ContentFilter => "content_filter",
        urp::FinishReason::Other => "stop",
    }
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
    let mut emitted_terminal_for_tools = false;
    let mut node_owned_kinds: HashSet<ChatNodeKind> = HashSet::new();

    while let Some(event) = rx.recv().await {
        if finished {
            continue;
        }
        match event {
            UrpStreamEvent::ResponseStart { .. } => {
                chat_id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
                created = now_ts();
                let chunk = json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": logical_model,
                    "choices": [{
                        "index": 0,
                        "delta": { "role": "assistant" },
                        "finish_reason": Value::Null
                    }]
                });
                send_plain_sse_data(&tx, chunk.to_string()).await?;
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::ToolCall { call_id, name, .. },
                ..
            } => {
                node_owned_kinds.insert(ChatNodeKind::ToolCall);
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
                )
                .await?;
                node_states.insert(
                    node_index,
                    StreamedChatNodeState {
                        tool_call: Some(tool_call),
                        saw_node_start: true,
                        saw_node_done: false,
                    },
                );
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header,
                ..
            } => {
                if node_header_streams_as_content(&header) {
                    node_owned_kinds.insert(ChatNodeKind::Content);
                }
                node_states.entry(node_index).or_default().saw_node_start = true;
            }
            UrpStreamEvent::NodeDelta {
                delta: NodeDelta::Text { content },
                ..
            }
            | UrpStreamEvent::NodeDelta {
                delta: NodeDelta::Refusal { content },
                ..
            } => {
                node_owned_kinds.insert(ChatNodeKind::Content);
                send_chat_chunk_string(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    json!({ "content": "" }),
                    &content,
                    chat_delta_path_content,
                    sse_max_frame_length,
                )
                .await?;
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
                let mut transform_plaintext_to_summary = false;
                if summary.as_deref().is_some_and(|summary| !summary.is_empty()) {
                    node_owned_kinds.insert(ChatNodeKind::ReasoningSummary);
                }
                if content.as_deref().is_some_and(|content| !content.is_empty()) {
                    if extra_body
                        .get("inject_reasoning_content")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty())
                    {
                        node_owned_kinds.insert(ChatNodeKind::ReasoningText);
                    } else if source.as_deref() == Some("openrouter") {
                        node_owned_kinds.insert(ChatNodeKind::ReasoningText);
                    } else {
                        node_owned_kinds.insert(ChatNodeKind::ReasoningSummary);
                        transform_plaintext_to_summary = true;
                    }
                }
                if encrypted.as_ref().is_some_and(|value| !value.is_null()) {
                    node_owned_kinds.insert(ChatNodeKind::ReasoningEncrypted);
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
                    &extra_body,
                    transform_plaintext_to_summary,
                    sse_max_frame_length,
                )
                .await?;
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::ToolCallArguments { arguments },
                ..
            } => {
                let Some(node_state) = node_states.get_mut(&node_index) else {
                    continue;
                };
                node_owned_kinds.insert(ChatNodeKind::ToolCall);
                let Some(tool_call) = node_state.tool_call.as_mut() else {
                    continue;
                };

                saw_tool = true;
                if !tool_call.header_sent {
                    emit_tool_call_header(
                        &tx,
                        &chat_id,
                        created,
                        logical_model,
                        tool_call,
                    )
                    .await?;
                }
                emit_tool_call_arguments_delta(
                    &tx,
                    &chat_id,
                    created,
                    logical_model,
                    tool_call.index,
                    &arguments,
                    sse_max_frame_length,
                )
                .await?;
                tool_call.arguments_streamed = true;
            }
            UrpStreamEvent::NodeDelta { node_index, .. } => {
                node_states.entry(node_index).or_default().saw_node_start = true;
            }
            UrpStreamEvent::NodeDone {
                node_index,
                node,
                ..
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
                    node_owned_kinds.insert(ChatNodeKind::ToolCall);
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
                            sse_max_frame_length,
                        )
                        .await?;
                        tool_call.arguments_streamed = true;
                    }
                }
            }
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                output,
                ..
            } => {
                let missing_reasoning_summary =
                    !node_owned_kinds.contains(&ChatNodeKind::ReasoningSummary);
                let missing_reasoning_text =
                    !node_owned_kinds.contains(&ChatNodeKind::ReasoningText);
                let missing_reasoning_encrypted =
                    !node_owned_kinds.contains(&ChatNodeKind::ReasoningEncrypted);
                if missing_reasoning_summary || missing_reasoning_text || missing_reasoning_encrypted {
                    for node in &output {
                        let Node::Reasoning {
                            content,
                            encrypted,
                            summary,
                            source,
                            extra_body,
                            ..
                        } = node
                        else {
                            continue;
                        };

                        let original_content = content.as_deref().filter(|content| !content.is_empty());

                        let allow_text_surface = missing_reasoning_text;
                        let allow_summary_surface = missing_reasoning_summary;
                        let allow_encrypted_surface = missing_reasoning_encrypted;
                        let content = original_content.filter(|_| allow_text_surface);
                        let summary = summary
                            .as_deref()
                            .filter(|summary| !summary.is_empty() && allow_summary_surface);
                        let encrypted = encrypted
                            .as_ref()
                            .filter(|value| !value.is_null() && allow_encrypted_surface);
                        let transform_plaintext_to_summary = allow_summary_surface
                            && !allow_text_surface
                            && source.as_deref().is_none_or(|source| source != "openrouter")
                            && content.is_none()
                            && summary.is_none()
                            && original_content.is_some()
                            && extra_body
                                .get("inject_reasoning_content")
                                .and_then(Value::as_str)
                                .is_none_or(|value| value.is_empty());
                        let content = if transform_plaintext_to_summary {
                            None
                        } else {
                            content.or_else(|| {
                                (allow_text_surface
                                    && source.as_deref() == Some("openrouter")
                                    && summary.is_none())
                                    .then_some(original_content)
                                    .flatten()
                            })
                        };
                        let summary = if transform_plaintext_to_summary {
                            original_content
                        } else {
                            summary
                        };
                        if content.is_none() && summary.is_none() && encrypted.is_none() {
                            continue;
                        }
                        emit_reasoning_delta(
                            &tx,
                            &chat_id,
                            created,
                            logical_model,
                            content,
                            encrypted,
                            summary,
                            source.as_deref(),
                            extra_body,
                            false,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                }
                if !emitted_terminal_for_tools && !node_owned_kinds.contains(&ChatNodeKind::ToolCall)
                {
                    for node in output {
                        if let Node::ToolCall {
                            call_id,
                            name,
                            arguments,
                            ..
                        } = node
                        {
                            let mut tool_call = StreamedChatToolCall {
                                call_id,
                                name,
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
                            )
                            .await?;
                            if !arguments.is_empty() {
                                emit_tool_call_arguments_delta(
                                    &tx,
                                    &chat_id,
                                    created,
                                    logical_model,
                                    tool_call.index,
                                    &arguments,
                                    sse_max_frame_length,
                                )
                                .await?;
                            }
                        }
                    }
                }
                let finish_reason = if saw_tool {
                    "tool_calls"
                } else {
                    finish_reason_to_chat(finish_reason.unwrap_or(FinishReason::Stop))
                };
                let mut done = json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": logical_model,
                    "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
                });
                if let Some(usage) = usage.as_ref() {
                    done["usage"] = usage_to_chat_usage_json(usage);
                }
                send_plain_sse_data(&tx, done.to_string()).await?;
                send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
                emitted_terminal_for_tools = true;
                finished = true;
            }
            UrpStreamEvent::Error { code, message, .. } => {
                let error = json!({
                    "error": {
                        "message": message,
                        "type": "server_error",
                        "code": code
                    }
                });
                send_plain_sse_data(&tx, error.to_string()).await?;
                finished = true;
            }
            _ => {}
        }
    }

    Ok(())
}

fn node_header_streams_as_content(header: &NodeHeader) -> bool {
    matches!(
        header,
        NodeHeader::Text { .. }
            | NodeHeader::Refusal { .. }
            | NodeHeader::Image { .. }
            | NodeHeader::Audio { .. }
            | NodeHeader::File { .. }
            | NodeHeader::ProviderItem { .. }
    )
}

async fn emit_tool_call_header(
    tx: &mpsc::Sender<Event>,
    chat_id: &str,
    created: i64,
    logical_model: &str,
    tool_call: &mut StreamedChatToolCall,
) -> AppResult<()> {
    if tool_call.header_sent {
        return Ok(());
    }
    let chunk = json!({
        "id": chat_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": tool_call.index,
                    "id": tool_call.call_id,
                    "type": "function",
                    "function": { "name": tool_call.name, "arguments": "" }
                }]
            },
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
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let delta = json!({
        "tool_calls": [{
            "index": tool_index,
            "function": { "arguments": "" }
        }]
    });
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
    transform_plaintext_to_summary: bool,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let (content, summary) = if transform_plaintext_to_summary {
        chat_reasoning_transform_parity(content, summary)
    } else {
        (content, summary)
    };
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
            json!({ "reasoning_content": "" }),
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
            chat_reasoning_delta_from_encrypted("", format, reasoning_id),
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
            chat_reasoning_delta_from_text("", format),
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
                json!({ "reasoning_content": "" }),
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
                chat_reasoning_delta_from_summary("", format),
                summary,
                chat_delta_path_reasoning_summary,
                sse_max_frame_length,
            )
            .await?;
        }
    }
    Ok(())
}

fn chat_reasoning_transform_parity<'a>(
    content: Option<&'a str>,
    summary: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    if let Some(content) = content.filter(|content| !content.is_empty()) {
        return (None, Some(content));
    }
    (content, summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::openai_chat::decode_response;
    use serde_json::json;
    use tokio::sync::mpsc;

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
            .send(UrpStreamEvent::ResponseDone { finish_reason: Some(FinishReason::Stop), usage: None, output: Vec::new(), extra_body: HashMap::new() })
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
            .send(UrpStreamEvent::ResponseDone { finish_reason: Some(FinishReason::Stop), usage: None, output: Vec::new(), extra_body: HashMap::new() })
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
