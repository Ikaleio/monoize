use crate::error::AppResult;
use crate::urp::encode::anthropic::anthropic_native_input_tokens;
use crate::urp::stream_helpers::*;
use crate::urp::{
    self, FinishReason, Node, NodeDelta, NodeHeader, Part, PartDelta, PartHeader,
    REASONING_KIND_EXTRA_KEY, REASONING_KIND_REDACTED_THINKING, UrpStreamEvent, Usage,
    wrap_reasoning_signature_with_item_id,
};
use axum::response::sse::Event;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MessagesSurfaceKind {
    Text,
    Reasoning,
    ToolUse,
}

#[derive(Debug, Clone)]
enum AnthropicBlockPayload {
    Text {
        content: String,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
        item_id: Option<String>,
        extra: HashMap<String, Value>,
    },
    ToolUse {
        call_id: String,
        name: String,
        arguments: String,
    },
}

#[derive(Debug, Clone)]
struct PendingAnthropicBlock {
    block_index: u32,
    payload: AnthropicBlockPayload,
}

impl PendingAnthropicBlock {
    fn effective_signature(&self) -> Option<String> {
        let AnthropicBlockPayload::Thinking {
            signature, item_id, ..
        } = &self.payload
        else {
            return None;
        };
        let raw = signature.as_deref().filter(|s| !s.is_empty())?;
        match item_id.as_deref().filter(|s| !s.is_empty()) {
            Some(id) => wrap_reasoning_signature_with_item_id(id, raw).or_else(|| Some(raw.to_string())),
            None => Some(raw.to_string()),
        }
    }

    fn content_block(&self, saw_tool_use: &mut bool) -> Value {
        match &self.payload {
            AnthropicBlockPayload::Text { .. } => json!({ "type": "text", "text": "" }),
            AnthropicBlockPayload::Thinking { extra, .. } => {
                let sig_for_start = self.effective_signature().unwrap_or_default();
                if payload_is_redacted(extra) {
                    json!({
                        "type": "redacted_thinking",
                        "data": sig_for_start
                    })
                } else {
                    json!({
                        "type": "thinking",
                        "thinking": "",
                        "signature": sig_for_start
                    })
                }
            }
            AnthropicBlockPayload::ToolUse { call_id, name, .. } => {
                *saw_tool_use = true;
                json!({ "type": "tool_use", "id": call_id, "name": name, "input": {} })
            }
        }
    }

    async fn emit(
        &self,
        tx: &mpsc::Sender<Event>,
        saw_tool_use: &mut bool,
        sse_max_frame_length: Option<usize>,
    ) -> AppResult<()> {
        if let AnthropicBlockPayload::Thinking {
            signature, item_id, ..
        } = &self.payload
        {
            tracing::info!(
                target: "monoize::urp::reasoning_trace",
                block_index = self.block_index,
                item_id = item_id.as_deref().unwrap_or(""),
                raw_signature_len = signature.as_ref().map(|s| s.len()).unwrap_or(0),
                effective_signature_len = self.effective_signature().as_ref().map(|s| s.len()).unwrap_or(0),
                "anthropic thinking block emit"
            );
        }
        let start = json!({
            "type": "content_block_start",
            "index": self.block_index,
            "content_block": self.content_block(saw_tool_use)
        });
        send_named_messages_event(tx, start).await?;

        match &self.payload {
            AnthropicBlockPayload::Text { content } => {
                if !content.is_empty() {
                    send_messages_delta_string(
                        tx,
                        json!({
                            "type": "content_block_delta",
                            "index": self.block_index,
                            "delta": { "type": "text_delta", "text": "" }
                        }),
                        messages_delta_path_text,
                        content,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            AnthropicBlockPayload::Thinking {
                thinking, extra, ..
            } => {
                // `redacted_thinking` blocks carry their opaque payload in the initial
                // `content_block_start.content_block.data` field, per Anthropic wire contract.
                // No `thinking_delta` or `signature_delta` events exist for this block type.
                if !payload_is_redacted(extra) {
                    if !thinking.is_empty() {
                        send_messages_delta_string(
                            tx,
                            json!({
                                "type": "content_block_delta",
                                "index": self.block_index,
                                "delta": { "type": "thinking_delta", "thinking": "" }
                            }),
                            messages_delta_path_thinking,
                            thinking,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                    if let Some(signature) = self
                        .effective_signature()
                        .filter(|signature| !signature.is_empty())
                    {
                        send_messages_delta_string(
                            tx,
                            json!({
                                "type": "content_block_delta",
                                "index": self.block_index,
                                "delta": { "type": "signature_delta", "signature": "" }
                            }),
                            messages_delta_path_signature,
                            &signature,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                }
            }
            AnthropicBlockPayload::ToolUse { arguments, .. } => {
                if !arguments.is_empty() {
                    send_messages_delta_string(
                        tx,
                        json!({
                            "type": "content_block_delta",
                            "index": self.block_index,
                            "delta": { "type": "input_json_delta", "partial_json": "" }
                        }),
                        messages_delta_path_partial_json,
                        arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
        }

        let stop = json!({ "type": "content_block_stop", "index": self.block_index });
        send_named_messages_event(tx, stop).await?;
        Ok(())
    }
}

fn payload_is_redacted(extra: &HashMap<String, Value>) -> bool {
    extra
        .get(REASONING_KIND_EXTRA_KEY)
        .and_then(Value::as_str)
        == Some(REASONING_KIND_REDACTED_THINKING)
}

#[derive(Debug, Clone)]
struct LiveNodeBlockState {
    block_index: u32,
    payload: AnthropicBlockPayload,
}

fn reasoning_signature_value(
    encrypted: Option<&Value>,
    extra_body: &HashMap<String, Value>,
) -> Option<String> {
    encrypted
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        })
        .or_else(|| {
            extra_body
                .get("signature")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|signature| !signature.is_empty())
}

fn reasoning_is_redacted_extra(extra_body: &HashMap<String, Value>) -> bool {
    payload_is_redacted(extra_body)
}

fn reasoning_item_id(id: Option<&str>) -> Option<String> {
    id.map(str::to_owned).filter(|s| !s.is_empty())
}

fn reasoning_kind_marker(extra_body: &HashMap<String, Value>) -> HashMap<String, Value> {
    let mut extra = HashMap::new();
    if payload_is_redacted(extra_body) {
        extra.insert(
            REASONING_KIND_EXTRA_KEY.to_string(),
            Value::String(REASONING_KIND_REDACTED_THINKING.to_string()),
        );
    }
    extra
}

fn surface_kind_for_payload(payload: &AnthropicBlockPayload) -> MessagesSurfaceKind {
    match payload {
        AnthropicBlockPayload::Text { .. } => MessagesSurfaceKind::Text,
        AnthropicBlockPayload::Thinking { .. } => MessagesSurfaceKind::Reasoning,
        AnthropicBlockPayload::ToolUse { .. } => MessagesSurfaceKind::ToolUse,
    }
}

fn anthropic_block_from_node(node: &Node) -> Option<AnthropicBlockPayload> {
    match node {
        Node::Text { content, .. } | Node::Refusal { content, .. } => {
            Some(AnthropicBlockPayload::Text {
                content: content.clone(),
            })
        }
        Node::Reasoning {
            id,
            content,
            summary,
            encrypted,
            extra_body,
            ..
        } => {
            let thinking = content
                .as_deref()
                .filter(|content| !content.is_empty())
                .or_else(|| summary.as_deref().filter(|summary| !summary.is_empty()))
                .unwrap_or_default()
                .to_string();
            let raw_signature = reasoning_signature_value(encrypted.as_ref(), extra_body);
            let is_redacted = reasoning_is_redacted_extra(extra_body);
            if thinking.is_empty() && !is_redacted {
                return None;
            }
            if is_redacted && raw_signature.is_none() {
                return None;
            }
            let extra = reasoning_kind_marker(extra_body);
            Some(AnthropicBlockPayload::Thinking {
                thinking,
                signature: raw_signature,
                item_id: reasoning_item_id(id.as_deref()),
                extra,
            })
        }
        Node::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } => Some(AnthropicBlockPayload::ToolUse {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        Node::Image { .. }
        | Node::Audio { .. }
        | Node::File { .. }
        | Node::ProviderItem { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

fn anthropic_block_from_node_header(
    header: &NodeHeader,
    extra_body: &HashMap<String, Value>,
) -> Option<AnthropicBlockPayload> {
    match header {
        NodeHeader::Text { .. } | NodeHeader::Refusal { .. } => Some(AnthropicBlockPayload::Text {
            content: String::new(),
        }),
        NodeHeader::Reasoning { .. } => Some(AnthropicBlockPayload::Thinking {
            thinking: String::new(),
            signature: reasoning_signature_value(None, extra_body),
            item_id: None,
            extra: reasoning_kind_marker(extra_body),
        }),
        NodeHeader::ToolCall { call_id, name, .. } => Some(AnthropicBlockPayload::ToolUse {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: String::new(),
        }),
        NodeHeader::Image { .. }
        | NodeHeader::Audio { .. }
        | NodeHeader::File { .. }
        | NodeHeader::ProviderItem { .. }
        | NodeHeader::ToolResult { .. }
        | NodeHeader::NextDownstreamEnvelopeExtra => None,
    }
}

fn merge_json_extra_preserving_typed(obj: &mut Map<String, Value>, extra: &HashMap<String, Value>) {
    for (key, value) in extra {
        if !obj.contains_key(key) {
            obj.insert(key.clone(), value.clone());
        }
    }
}

fn merge_hashmap_extra_preserving_typed(
    dst: &mut HashMap<String, Value>,
    extra: &HashMap<String, Value>,
) {
    for (key, value) in extra {
        if !dst.contains_key(key) {
            dst.insert(key.clone(), value.clone());
        }
    }
}

fn message_start_payload(
    message_id: &str,
    logical_model: &str,
    input_tokens: u64,
    output_tokens: u64,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut message = Map::new();
    message.insert("id".to_string(), json!(message_id));
    message.insert("type".to_string(), json!("message"));
    message.insert("role".to_string(), json!("assistant"));
    message.insert("model".to_string(), json!(logical_model));
    message.insert("content".to_string(), json!([]));
    message.insert("stop_reason".to_string(), Value::Null);
    message.insert("stop_sequence".to_string(), Value::Null);
    message.insert(
        "usage".to_string(),
        json!({
            "input_tokens": input_tokens,
            "output_tokens": output_tokens
        }),
    );
    merge_json_extra_preserving_typed(&mut message, extra_body);
    json!({
        "type": "message_start",
        "message": Value::Object(message)
    })
}

fn anthropic_block_from_part(part: &Part) -> Option<AnthropicBlockPayload> {
    match part {
        Part::Text { content, .. } | Part::Refusal { content, .. } => {
            Some(AnthropicBlockPayload::Text {
                content: content.clone(),
            })
        }
        Part::Reasoning {
            id,
            summary,
            encrypted,
            extra_body,
            ..
        } => {
            let thinking = match part {
                Part::Reasoning { content, .. } => content
                    .as_deref()
                    .filter(|content| !content.is_empty())
                    .or_else(|| summary.as_deref().filter(|summary| !summary.is_empty()))
                    .unwrap_or_default()
                    .to_string(),
                _ => String::new(),
            };
            let raw_signature = reasoning_signature_value(encrypted.as_ref(), extra_body);
            let is_redacted = reasoning_is_redacted_extra(extra_body);
            if thinking.is_empty() && !is_redacted {
                return None;
            }
            if is_redacted && raw_signature.is_none() {
                return None;
            }
            let extra = reasoning_kind_marker(extra_body);
            Some(AnthropicBlockPayload::Thinking {
                thinking,
                signature: raw_signature,
                item_id: reasoning_item_id(id.as_deref()),
                extra,
            })
        }
        Part::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } => Some(AnthropicBlockPayload::ToolUse {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        Part::Image { .. }
        | Part::Audio { .. }
        | Part::File { .. }
        | Part::ProviderItem { .. } => None,
    }
}

fn anthropic_block_from_part_header(
    header: &PartHeader,
    extra_body: &HashMap<String, Value>,
) -> Option<AnthropicBlockPayload> {
    match header {
        PartHeader::Text | PartHeader::Refusal => Some(AnthropicBlockPayload::Text {
            content: String::new(),
        }),
        PartHeader::Reasoning { .. } => Some(AnthropicBlockPayload::Thinking {
            thinking: String::new(),
            signature: reasoning_signature_value(None, extra_body),
            item_id: None,
            extra: reasoning_kind_marker(extra_body),
        }),
        PartHeader::ToolCall { call_id, name, .. } => Some(AnthropicBlockPayload::ToolUse {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: String::new(),
        }),
        PartHeader::Image { .. }
        | PartHeader::Audio { .. }
        | PartHeader::File { .. }
        | PartHeader::ProviderItem { .. } => None,
    }
}

fn apply_node_delta_to_block(payload: &mut AnthropicBlockPayload, delta: &NodeDelta) {
    match (payload, delta) {
        (AnthropicBlockPayload::Text { content }, NodeDelta::Text { content: delta })
        | (AnthropicBlockPayload::Text { content }, NodeDelta::Refusal { content: delta }) => {
            content.push_str(delta);
        }
        (
            AnthropicBlockPayload::Thinking {
                thinking, signature, ..
            },
            NodeDelta::Reasoning {
                content,
                encrypted,
                summary,
                ..
            },
        ) => {
            if let Some(delta) = content.as_deref().filter(|content| !content.is_empty()) {
                thinking.push_str(delta);
            } else if thinking.is_empty()
                && let Some(delta) = summary.as_deref().filter(|summary| !summary.is_empty())
            {
                thinking.push_str(delta);
            }
            if let Some(signature_delta) = encrypted
                .as_ref()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                })
                .filter(|signature| !signature.is_empty())
            {
                signature
                    .get_or_insert_with(String::new)
                    .push_str(&signature_delta);
            }
        }
        (
            AnthropicBlockPayload::ToolUse { arguments, .. },
            NodeDelta::ToolCallArguments { arguments: delta },
        ) => {
            arguments.push_str(delta);
        }
        _ => {}
    }
}

fn maybe_override_reasoning_item_id(
    payload: &mut AnthropicBlockPayload,
    extra_body: &HashMap<String, Value>,
) {
    let AnthropicBlockPayload::Thinking { item_id, .. } = payload else {
        return;
    };
    let Some(reasoning_item_id) = extra_body
        .get("reasoning_item_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    *item_id = Some(reasoning_item_id.to_string());
}

fn merge_node_payload_with_terminal(payload: &mut AnthropicBlockPayload, node: &Node) {
    match (payload, node) {
        (AnthropicBlockPayload::Thinking { thinking, signature, item_id, .. }, Node::Reasoning {
            id,
            content,
            summary,
            encrypted,
            extra_body,
            ..
        }) => {
            if let Some(content) = content.as_deref().filter(|content| !content.is_empty()) {
                *thinking = content.to_string();
            } else if thinking.is_empty()
                && let Some(summary) = summary.as_deref().filter(|summary| !summary.is_empty())
            {
                *thinking = summary.to_string();
            }
            if let Some(sig) = reasoning_signature_value(encrypted.as_ref(), extra_body) {
                *signature = Some(sig);
            }
            if item_id.is_none() {
                *item_id = reasoning_item_id(id.as_deref());
            }
        }
        (AnthropicBlockPayload::ToolUse { arguments, .. }, Node::ToolCall { arguments: done_args, .. }) => {
            if !done_args.is_empty() {
                *arguments = done_args.clone();
            }
        }
        (AnthropicBlockPayload::Text { content }, Node::Text { content: done, .. })
        | (AnthropicBlockPayload::Text { content }, Node::Refusal { content: done, .. }) => {
            if !done.is_empty() {
                *content = done.clone();
            }
        }
        _ => {}
    }
}

fn apply_part_delta_to_block(payload: &mut AnthropicBlockPayload, delta: &PartDelta) {
    match (payload, delta) {
        (AnthropicBlockPayload::Text { content }, PartDelta::Text { content: delta })
        | (AnthropicBlockPayload::Text { content }, PartDelta::Refusal { content: delta }) => {
            content.push_str(delta);
        }
        (
            AnthropicBlockPayload::Thinking { thinking, signature, .. },
            PartDelta::Reasoning {
                content,
                encrypted,
                summary,
                ..
            },
        ) => {
            if let Some(delta) = content.as_deref().filter(|content| !content.is_empty()) {
                thinking.push_str(delta);
            } else if thinking.is_empty()
                && let Some(delta) = summary.as_deref().filter(|summary| !summary.is_empty())
            {
                thinking.push_str(delta);
            }
            if let Some(signature_delta) = encrypted
                .as_ref()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                })
                .filter(|signature| !signature.is_empty())
            {
                signature
                    .get_or_insert_with(String::new)
                    .push_str(&signature_delta);
            }
        }
        (
            AnthropicBlockPayload::ToolUse { arguments, .. },
            PartDelta::ToolCallArguments { arguments: delta },
        ) => {
            arguments.push_str(delta);
        }
        _ => {}
    }
}

async fn flush_ready_node_blocks(
    tx: &mpsc::Sender<Event>,
    pending_blocks: &mut HashMap<u32, PendingAnthropicBlock>,
    next_flush_node_index: &mut u32,
    saw_tool_use: &mut bool,
    emitted_node_owned_surfaces: &mut HashSet<MessagesSurfaceKind>,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    while let Some(block) = pending_blocks.remove(next_flush_node_index) {
        emitted_node_owned_surfaces.insert(surface_kind_for_payload(&block.payload));
        block.emit(tx, saw_tool_use, sse_max_frame_length).await?;
        *next_flush_node_index += 1;
    }
    Ok(())
}

pub(crate) async fn emit_synthetic_messages_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    let mut saw_tool_use = false;
    let usage = resp.usage.clone().unwrap_or(urp::Usage {
        input_tokens: 0,
        output_tokens: 0,
        input_details: None,
        output_details: None,
        extra_body: HashMap::new(),
    });
    let message_nodes = resp.output.clone();
    let mut pending_envelope_extra = HashMap::new();
    for node in &message_nodes {
        if let Node::NextDownstreamEnvelopeExtra { extra_body } = node {
            merge_hashmap_extra_preserving_typed(&mut pending_envelope_extra, extra_body);
            continue;
        }
        break;
    }
    let start = message_start_payload(
        &message_id,
        logical_model,
        anthropic_native_input_tokens(&usage),
        usage.output_tokens,
        &pending_envelope_extra,
    );
    send_named_messages_event(&tx, start).await?;

    let mut index = 0u32;
    for node in &message_nodes {
        match node {
            Node::NextDownstreamEnvelopeExtra { .. } => continue,
            Node::Text {
                role: urp::OrdinaryRole::Assistant,
                ..
            }
            | Node::Refusal { .. }
            | Node::Reasoning { .. }
            | Node::ToolCall { .. } => {
                let Some(payload) = anthropic_block_from_node(node) else {
                    continue;
                };
                PendingAnthropicBlock {
                    block_index: index,
                    payload,
                }
                .emit(&tx, &mut saw_tool_use, sse_max_frame_length)
                .await?;
                index += 1;
            }
            _ => continue,
        }
    }

    let message_delta = json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": if saw_tool_use { "tool_use" } else { "end_turn" },
            "stop_sequence": Value::Null
        },
        "usage": {
            "input_tokens": anthropic_native_input_tokens(&usage),
            "output_tokens": usage.output_tokens
        }
    });
    send_named_messages_event(&tx, message_delta).await?;
    send_named_messages_event(&tx, json!({ "type": "message_stop" })).await?;
    Ok(())
}

pub(crate) async fn encode_urp_stream_as_messages(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    logical_model: &str,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let mut next_content_block_index = 0u32;
    let mut saw_tool_use = false;
    let mut response_usage: Option<Usage> = None;
    let mut node_owned_surfaces: HashSet<MessagesSurfaceKind> = HashSet::new();
    let mut emitted_node_owned_surfaces: HashSet<MessagesSurfaceKind> = HashSet::new();
    let mut completed_node_owned_surfaces: HashSet<MessagesSurfaceKind> = HashSet::new();
    let mut completed_node_indices: HashSet<u32> = HashSet::new();
    let mut live_node_blocks: HashMap<u32, LiveNodeBlockState> = HashMap::new();
    let mut pending_node_blocks: HashMap<u32, PendingAnthropicBlock> = HashMap::new();
    let mut next_flush_node_index = 0u32;
    let mut live_bridge_blocks: HashMap<u32, PendingAnthropicBlock> = HashMap::new();
    let mut response_id: Option<String> = None;
    let mut message_start_sent = false;
    let mut pending_envelope_extra: HashMap<String, Value> = HashMap::new();
    let mut should_emit_terminal_message = false;
    let terminal_message_emitted = false;

    async fn ensure_message_start(
        tx: &mpsc::Sender<Event>,
        response_id: &str,
        logical_model: &str,
        response_usage: Option<&Usage>,
        pending_envelope_extra: &HashMap<String, Value>,
        message_start_sent: &mut bool,
    ) -> AppResult<()> {
        if *message_start_sent {
            return Ok(());
        }
        let usage = response_usage.cloned().unwrap_or(Usage {
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: HashMap::new(),
        });
        send_named_messages_event(
            tx,
            message_start_payload(
                response_id,
                logical_model,
                anthropic_native_input_tokens(&usage),
                usage.output_tokens,
                pending_envelope_extra,
            ),
        )
        .await?;
        *message_start_sent = true;
        Ok(())
    }

    while let Some(event) = rx.recv().await {
        match event {
            UrpStreamEvent::ResponseStart { id, extra_body, .. } => {
                response_id = Some(id);
                merge_hashmap_extra_preserving_typed(&mut pending_envelope_extra, &extra_body);
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header,
                extra_body,
            } => {
                if matches!(header, NodeHeader::NextDownstreamEnvelopeExtra) {
                    merge_hashmap_extra_preserving_typed(&mut pending_envelope_extra, &extra_body);
                    continue;
                }
                let Some(payload) = anthropic_block_from_node_header(&header, &extra_body) else {
                    continue;
                };
                should_emit_terminal_message = true;
                ensure_message_start(
                    &tx,
                    response_id.as_deref().unwrap_or("msg_mock"),
                    logical_model,
                    response_usage.as_ref(),
                    &pending_envelope_extra,
                    &mut message_start_sent,
                )
                .await?;
                pending_envelope_extra.clear();
                let surface = surface_kind_for_payload(&payload);
                if matches!(surface, MessagesSurfaceKind::ToolUse) {
                    saw_tool_use = true;
                }
                live_node_blocks.insert(
                    node_index,
                    LiveNodeBlockState {
                        block_index: next_content_block_index,
                        payload,
                    },
                );
                next_content_block_index += 1;
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                usage,
                extra_body,
            } => {
                if let Some(usage) = usage {
                    response_usage = Some(usage);
                }
                let Some(block_state) = live_node_blocks.get_mut(&node_index) else {
                    continue;
                };
                maybe_override_reasoning_item_id(&mut block_state.payload, &extra_body);
                apply_node_delta_to_block(&mut block_state.payload, &delta);
            }
            UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage,
                ..
            } => {
                if let Some(usage) = usage {
                    response_usage = Some(usage);
                }
                if matches!(node, Node::NextDownstreamEnvelopeExtra { .. }) {
                    continue;
                }
                completed_node_indices.insert(node_index);
                let block_index = live_node_blocks
                    .get(&node_index)
                    .map(|state| state.block_index)
                    .unwrap_or(next_content_block_index);
                let mut payload = live_node_blocks
                    .get(&node_index)
                    .map(|state| state.payload.clone())
                    .or_else(|| anthropic_block_from_node(&node));
                let Some(mut payload) = payload.take() else {
                    live_node_blocks.remove(&node_index);
                    continue;
                };
                merge_node_payload_with_terminal(&mut payload, &node);
                let used_existing_index = live_node_blocks.remove(&node_index).is_some();
                if !used_existing_index {
                    next_content_block_index += 1;
                }
                if matches!(surface_kind_for_payload(&payload), MessagesSurfaceKind::ToolUse) {
                    saw_tool_use = true;
                }
                let surface = surface_kind_for_payload(&payload);
                node_owned_surfaces.insert(surface);
                completed_node_owned_surfaces.insert(surface);
                pending_node_blocks.insert(
                    node_index,
                    PendingAnthropicBlock {
                        block_index,
                        payload,
                    },
                );
                flush_ready_node_blocks(
                    &tx,
                    &mut pending_node_blocks,
                    &mut next_flush_node_index,
                    &mut saw_tool_use,
                    &mut emitted_node_owned_surfaces,
                    sse_max_frame_length,
                )
                .await?;
            }
            UrpStreamEvent::PartStart {
                part_index,
                header,
                extra_body,
                ..
            } => {
                if completed_node_indices.contains(&part_index) {
                    continue;
                }
                let Some(payload) = anthropic_block_from_part_header(&header, &extra_body) else {
                    continue;
                };
                should_emit_terminal_message = true;
                ensure_message_start(
                    &tx,
                    response_id.as_deref().unwrap_or("msg_mock"),
                    logical_model,
                    response_usage.as_ref(),
                    &pending_envelope_extra,
                    &mut message_start_sent,
                )
                .await?;
                pending_envelope_extra.clear();
                let surface = surface_kind_for_payload(&payload);
                if completed_node_owned_surfaces.contains(&surface) {
                    continue;
                }
                if matches!(surface, MessagesSurfaceKind::ToolUse) {
                    saw_tool_use = true;
                }
                live_bridge_blocks.insert(
                    part_index,
                    PendingAnthropicBlock {
                        block_index: next_content_block_index,
                        payload,
                    },
                );
                next_content_block_index += 1;
            }
            UrpStreamEvent::Delta {
                part_index,
                delta,
                usage,
                extra_body,
            } => {
                if let Some(usage) = usage {
                    response_usage = Some(usage);
                }
                let Some(block_state) = live_bridge_blocks.get_mut(&part_index) else {
                    continue;
                };
                maybe_override_reasoning_item_id(&mut block_state.payload, &extra_body);
                apply_part_delta_to_block(&mut block_state.payload, &delta);
            }
            UrpStreamEvent::PartDone {
                part_index,
                part,
                usage,
                ..
            } => {
                if let Some(usage) = usage {
                    response_usage = Some(usage);
                }
                let payload_from_part = anthropic_block_from_part(&part);
                let Some(surface) = payload_from_part
                    .as_ref()
                    .map(surface_kind_for_payload)
                    .or_else(|| {
                        live_bridge_blocks
                            .get(&part_index)
                            .map(|block| surface_kind_for_payload(&block.payload))
                    })
                else {
                    live_bridge_blocks.remove(&part_index);
                    continue;
                };
                if completed_node_owned_surfaces.contains(&surface) {
                    live_bridge_blocks.remove(&part_index);
                    continue;
                }
                let Some(mut block) = live_bridge_blocks.remove(&part_index) else {
                    if let Some(payload) = payload_from_part {
                        PendingAnthropicBlock {
                            block_index: next_content_block_index,
                            payload,
                        }
                        .emit(&tx, &mut saw_tool_use, sse_max_frame_length)
                        .await?;
                        next_content_block_index += 1;
                    }
                    continue;
                };
                if let Some(payload) = payload_from_part {
                    block.payload = payload;
                }
                block.emit(&tx, &mut saw_tool_use, sse_max_frame_length).await?;
            }
            UrpStreamEvent::ItemStart { .. } | UrpStreamEvent::ItemDone { .. } => {}
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                output,
                ..
            } => {
                if let Some(usage) = &usage {
                    response_usage = Some(usage.clone());
                }
                should_emit_terminal_message = should_emit_terminal_message
                    || !pending_node_blocks.is_empty()
                    || !live_node_blocks.is_empty()
                    || !live_bridge_blocks.is_empty()
                    || output.iter().any(|node| anthropic_block_from_node(node).is_some());
                if !should_emit_terminal_message && !message_start_sent {
                    pending_envelope_extra.clear();
                    continue;
                }
                ensure_message_start(
                    &tx,
                    response_id.as_deref().unwrap_or("msg_mock"),
                    logical_model,
                    response_usage.as_ref(),
                    &pending_envelope_extra,
                    &mut message_start_sent,
                )
                .await?;
                pending_envelope_extra.clear();
                let mut remaining_live_node_blocks: Vec<(u32, LiveNodeBlockState)> =
                    live_node_blocks.drain().collect();
                remaining_live_node_blocks.sort_by_key(|(node_index, _)| *node_index);
                for (node_index, mut block_state) in remaining_live_node_blocks {
                    if let Some(node) = output.get(node_index as usize) {
                        merge_node_payload_with_terminal(&mut block_state.payload, node);
                    }
                    if matches!(surface_kind_for_payload(&block_state.payload), MessagesSurfaceKind::ToolUse) {
                        saw_tool_use = true;
                    }
                    completed_node_owned_surfaces.insert(surface_kind_for_payload(&block_state.payload));
                    pending_node_blocks.insert(
                        node_index,
                        PendingAnthropicBlock {
                            block_index: block_state.block_index,
                            payload: block_state.payload,
                        },
                    );
                }
                flush_ready_node_blocks(
                    &tx,
                    &mut pending_node_blocks,
                    &mut next_flush_node_index,
                    &mut saw_tool_use,
                    &mut emitted_node_owned_surfaces,
                    sse_max_frame_length,
                )
                .await?;

                emit_messages_response_done_fallback(
                    &tx,
                    &mut next_content_block_index,
                    &mut saw_tool_use,
                    &output,
                    &emitted_node_owned_surfaces,
                    sse_max_frame_length,
                )
                .await?;

                let usage = usage.or_else(|| response_usage.clone()).unwrap_or(Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    input_details: None,
                    output_details: None,
                    extra_body: HashMap::new(),
                });
                let stop_reason = match finish_reason {
                    Some(FinishReason::Stop) => "end_turn",
                    Some(FinishReason::ToolCalls) => "tool_use",
                    Some(FinishReason::Length) => "max_tokens",
                    Some(FinishReason::Other | FinishReason::ContentFilter) => "end_turn",
                    None if saw_tool_use => "tool_use",
                    None => "end_turn",
                };
                let message_delta = json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": stop_reason,
                        "stop_sequence": Value::Null
                    },
                    "usage": {
                        "input_tokens": anthropic_native_input_tokens(&usage),
                        "output_tokens": usage.output_tokens
                    }
                });
                send_named_messages_event(&tx, message_delta).await?;
                send_named_messages_event(&tx, json!({ "type": "message_stop" })).await?;
                return Ok(());
            }
            UrpStreamEvent::Error { code, message, .. } => {
                should_emit_terminal_message = true;
                ensure_message_start(
                    &tx,
                    response_id.as_deref().unwrap_or("msg_mock"),
                    logical_model,
                    response_usage.as_ref(),
                    &pending_envelope_extra,
                    &mut message_start_sent,
                )
                .await?;
                let error = json!({
                    "type": "error",
                    "error": {
                        "type": code.unwrap_or_else(|| "server_error".to_string()),
                        "message": message
                    }
                });
                send_named_messages_event(&tx, error).await?;
            }
        }
    }

    if should_emit_terminal_message && !terminal_message_emitted {
        ensure_message_start(
            &tx,
            response_id.as_deref().unwrap_or("msg_mock"),
            logical_model,
            response_usage.as_ref(),
            &pending_envelope_extra,
            &mut message_start_sent,
        )
        .await?;
        let usage = response_usage.unwrap_or(Usage {
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: HashMap::new(),
        });
        let message_delta = json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": if saw_tool_use { "tool_use" } else { "end_turn" },
                "stop_sequence": Value::Null
            },
            "usage": {
                "input_tokens": anthropic_native_input_tokens(&usage),
                "output_tokens": usage.output_tokens
            }
        });
        send_named_messages_event(&tx, message_delta).await?;
        send_named_messages_event(&tx, json!({ "type": "message_stop" })).await?;
    }

    Ok(())
}

async fn emit_messages_response_done_fallback(
    tx: &mpsc::Sender<Event>,
    next_content_block_index: &mut u32,
    saw_tool_use: &mut bool,
    output: &[Node],
    emitted_node_owned_surfaces: &HashSet<MessagesSurfaceKind>,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    for node in output {
        let Some(payload) = anthropic_block_from_node(node) else {
            continue;
        };
        let surface = surface_kind_for_payload(&payload);
        if emitted_node_owned_surfaces.contains(&surface) {
            continue;
        }
        PendingAnthropicBlock {
            block_index: *next_content_block_index,
            payload,
        }
        .emit(tx, saw_tool_use, sse_max_frame_length)
        .await?;
        *next_content_block_index += 1;
    }
    Ok(())
}

async fn send_named_messages_event(tx: &mpsc::Sender<Event>, payload: Value) -> AppResult<()> {
    let event_name = payload
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            crate::error::AppError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "stream_encode_failed",
                "messages stream payload missing type field",
            )
        })?;
    send_named_sse_json(tx, &event_name, payload).await
}
