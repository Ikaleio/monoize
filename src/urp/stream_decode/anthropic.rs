use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_messages_object,
    record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{
    FinishReason, Item, ItemHeader, Node, NodeDelta, NodeHeader, OrdinaryRole, Part, PartDelta,
    PartHeader, Role, UrpStreamEvent, Usage,
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

#[derive(Debug, Default)]
struct AnthropicMessagesStreamState {
    node_order: Vec<u32>,
    active_nodes: HashMap<u32, ActiveNodeState>,
    completed_nodes: HashMap<u32, Node>,
    bridge_item_started: bool,
    bridge_item_done_sent: bool,
}

#[derive(Debug, Clone)]
struct ActiveNodeState {
    kind: ActiveNodeKind,
    extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
enum ActiveNodeKind {
    Text {
        content: String,
        phase: Option<String>,
    },
    Reasoning {
        content: String,
        encrypted: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
        replace_on_next_delta: bool,
    },
}

pub(crate) async fn stream_messages_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut response_model = urp.model.clone();
    let mut response_extra = HashMap::new();
    let mut state = AnthropicMessagesStreamState::default();

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
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_messages_object(&data_val),
        )
        .await;

        match data_val.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "message_start" => {
                let message = data_val.get("message").cloned().unwrap_or(Value::Null);
                if let Some(id) = message.get("id").and_then(|v| v.as_str()) {
                    response_id = id.to_string();
                }
                if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
                    response_model = model.to_string();
                }
                response_extra = object_without_keys(
                    &message,
                    &[
                        "id",
                        "type",
                        "role",
                        "model",
                        "content",
                        "stop_reason",
                        "stop_sequence",
                        "usage",
                    ],
                );
                let _ = tx
                    .send(UrpStreamEvent::ResponseStart {
                        id: response_id.clone(),
                        model: response_model.clone(),
                        extra_body: response_extra.clone(),
                    })
                    .await;
            }
            "content_block_start" => {
                let node_index = data_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let cb = data_val
                    .get("content_block")
                    .cloned()
                    .unwrap_or(Value::Null);
                for event in handle_content_block_start(node_index, cb, &mut state) {
                    let _ = tx.send(event).await;
                }
            }
            "content_block_delta" => {
                let node_index = data_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                for event in handle_content_block_delta(node_index, delta, &mut state) {
                    let _ = tx.send(event).await;
                }
            }
            "content_block_stop" => {
                let node_index = data_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                for event in handle_content_block_stop(node_index, &mut state) {
                    let _ = tx.send(event).await;
                }
            }
            "message_delta" => {
                let finish_reason = data_val
                    .get("delta")
                    .and_then(|v| v.get("stop_reason"))
                    .and_then(|v| v.as_str())
                    .and_then(map_finish_reason);
                let usage = latest_stream_usage_snapshot(&runtime_metrics).await;
                let output_nodes = ordered_completed_nodes(&state);
                let item = build_completed_message_item(&output_nodes);
                crate::handlers::usage::increment_estimated_output_tokens(
                    &runtime_metrics,
                    estimated_output_chars(&item),
                )
                .await;

                for event in build_message_completion_events(
                    &mut state,
                    finish_reason,
                    usage.clone(),
                    item.clone(),
                    response_extra.clone(),
                ) {
                    let _ = tx.send(event).await;
                }
                record_stream_terminal_event(
                    &runtime_metrics,
                    "response_done",
                    finish_reason.as_ref().map(finish_reason_name),
                )
                .await;
            }
            "message_stop" => {
                record_stream_terminal_event(&runtime_metrics, "message_stop", None).await;
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

fn handle_content_block_start(
    node_index: u32,
    content_block: Value,
    state: &mut AnthropicMessagesStreamState,
) -> Vec<UrpStreamEvent> {
    let Some(active_node) = active_node_from_content_block(&content_block) else {
        return Vec::new();
    };

    if !state.node_order.contains(&node_index) {
        state.node_order.push(node_index);
    }
    let node = node_from_active(&active_node);
    let extra_body = active_node.extra_body.clone();
    state.active_nodes.insert(node_index, active_node);

    let mut events = Vec::new();
    emit_node_start_with_bridge(node_index, &node, extra_body, state, &mut events);
    events
}

fn handle_content_block_delta(
    node_index: u32,
    delta_value: Value,
    state: &mut AnthropicMessagesStreamState,
) -> Vec<UrpStreamEvent> {
    let Some(active_node) = state.active_nodes.get_mut(&node_index) else {
        return Vec::new();
    };

    let delta_type = delta_value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let delta_extra = object_without_keys(
        &delta_value,
        &["type", "text", "thinking", "signature", "partial_json"],
    );

    let stream_delta = match (&mut active_node.kind, delta_type) {
        (ActiveNodeKind::Text { content, .. }, "text_delta") => {
            let Some(text) = delta_value.get("text").and_then(|v| v.as_str()) else {
                return Vec::new();
            };
            if text.is_empty() {
                return Vec::new();
            }
            content.push_str(text);
            NodeDelta::Text {
                content: text.to_string(),
            }
        }
        (ActiveNodeKind::Reasoning { content, .. }, "thinking_delta") => {
            let Some(text) = delta_value.get("thinking").and_then(|v| v.as_str()) else {
                return Vec::new();
            };
            if text.is_empty() {
                return Vec::new();
            }
            content.push_str(text);
            NodeDelta::Reasoning {
                content: Some(text.to_string()),
                encrypted: None,
                summary: None,
                source: None,
            }
        }
        (ActiveNodeKind::Reasoning { encrypted, .. }, "signature_delta") => {
            let Some(signature) = delta_value.get("signature").and_then(|v| v.as_str()) else {
                return Vec::new();
            };
            if signature.is_empty() {
                return Vec::new();
            }
            encrypted.push_str(signature);
            NodeDelta::Reasoning {
                content: None,
                encrypted: Some(Value::String(signature.to_string())),
                summary: None,
                source: None,
            }
        }
        (
            ActiveNodeKind::ToolCall {
                arguments,
                replace_on_next_delta,
                ..
            },
            "input_json_delta",
        ) => {
            let Some(arguments_delta) = delta_value.get("partial_json").and_then(|v| v.as_str())
            else {
                return Vec::new();
            };
            if arguments_delta.is_empty() {
                return Vec::new();
            }
            if *replace_on_next_delta {
                arguments.clear();
                *replace_on_next_delta = false;
            }
            arguments.push_str(arguments_delta);
            NodeDelta::ToolCallArguments {
                arguments: arguments_delta.to_string(),
            }
        }
        _ => return Vec::new(),
    };

    let mut events = Vec::new();
    emit_node_delta_with_bridge(node_index, stream_delta, delta_extra, &mut events);
    events
}

fn handle_content_block_stop(
    node_index: u32,
    state: &mut AnthropicMessagesStreamState,
) -> Vec<UrpStreamEvent> {
    let Some(active_node) = state.active_nodes.remove(&node_index) else {
        return Vec::new();
    };

    let node = node_from_active(&active_node);
    let extra_body = active_node.extra_body.clone();
    state.completed_nodes.insert(node_index, node.clone());

    let mut events = Vec::new();
    emit_node_done_with_bridge(node_index, node, extra_body, &mut events);
    events
}

fn active_node_from_content_block(content_block: &Value) -> Option<ActiveNodeState> {
    let content_type = content_block.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match content_type {
        "text" => {
            let phase = content_block
                .get("phase")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let mut extra_body = object_without_keys(content_block, &["type", "text", "phase"]);
            if let Some(phase) = phase.as_ref() {
                extra_body.insert("phase".to_string(), Value::String(phase.clone()));
            }
            Some(ActiveNodeState {
                kind: ActiveNodeKind::Text {
                    content: content_block
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    phase,
                },
                extra_body,
            })
        }
        "thinking" => {
            let extra_body =
                object_without_keys(content_block, &["type", "thinking", "signature"]);
            Some(ActiveNodeState {
                kind: ActiveNodeKind::Reasoning {
                    content: content_block
                        .get("thinking")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    encrypted: content_block
                        .get("signature")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                },
                extra_body,
            })
        }
        "redacted_thinking" => {
            let mut extra_body = object_without_keys(content_block, &["type", "data"]);
            extra_body.insert(
                crate::urp::REASONING_KIND_EXTRA_KEY.to_string(),
                Value::String(crate::urp::REASONING_KIND_REDACTED_THINKING.to_string()),
            );
            Some(ActiveNodeState {
                kind: ActiveNodeKind::Reasoning {
                    content: String::new(),
                    encrypted: content_block
                        .get("data")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                },
                extra_body,
            })
        }
        "tool_use" => Some(ActiveNodeState {
            kind: ActiveNodeKind::ToolCall {
                call_id: content_block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                name: content_block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                arguments: json_value_to_string(content_block.get("input")),
                replace_on_next_delta: tool_use_input_is_placeholder(content_block.get("input")),
            },
            extra_body: object_without_keys(content_block, &["type", "id", "name", "input"]),
        }),
        _ => None,
    }
}

fn node_from_active(active_node: &ActiveNodeState) -> Node {
    match &active_node.kind {
        ActiveNodeKind::Text { content, phase } => Node::Text {
            id: None,
            role: OrdinaryRole::Assistant,
            content: content.clone(),
            phase: phase.clone(),
            extra_body: active_node.extra_body.clone(),
        },
        ActiveNodeKind::Reasoning { content, encrypted } => {
            let extra_body = active_node.extra_body.clone();
            let (id, encrypted_value) = if encrypted.is_empty() {
                (None, None)
            } else {
                match crate::urp::unwrap_reasoning_signature_sigil(encrypted) {
                    Some((item_id, original)) => {
                        (Some(item_id), Some(Value::String(original)))
                    }
                    None => (None, Some(Value::String(encrypted.clone()))),
                }
            };
            Node::Reasoning {
                id,
                content: (!content.is_empty()).then(|| content.clone()),
                encrypted: encrypted_value,
                summary: None,
                source: None,
                extra_body,
            }
        }
        ActiveNodeKind::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } => Node::ToolCall {
            id: Some(call_id.clone()),
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            extra_body: active_node.extra_body.clone(),
        },
    }
}

fn node_header_from_node(node: &Node) -> NodeHeader {
    match node {
        Node::Text { role, phase, .. } => NodeHeader::Text {
            id: node.id().cloned(),
            role: *role,
            phase: phase.clone(),
        },
        Node::Reasoning { .. } => NodeHeader::Reasoning { id: node.id().cloned() },
        Node::ToolCall { call_id, name, .. } => NodeHeader::ToolCall {
            id: node.id().cloned(),
            call_id: call_id.clone(),
            name: name.clone(),
        },
        Node::Image { role, .. } => NodeHeader::Image { id: node.id().cloned(), role: *role },
        Node::Audio { role, .. } => NodeHeader::Audio { id: node.id().cloned(), role: *role },
        Node::File { role, .. } => NodeHeader::File { id: node.id().cloned(), role: *role },
        Node::Refusal { .. } => NodeHeader::Refusal { id: node.id().cloned() },
        Node::ProviderItem { role, item_type, .. } => NodeHeader::ProviderItem {
            id: node.id().cloned(),
            role: *role,
            item_type: item_type.clone(),
        },
        Node::ToolResult { call_id, .. } => NodeHeader::ToolResult {
            id: node.id().cloned(),
            call_id: call_id.clone(),
        },
        Node::NextDownstreamEnvelopeExtra { .. } => NodeHeader::NextDownstreamEnvelopeExtra,
    }
}

fn part_header_from_node(node: &Node) -> Option<PartHeader> {
    match node {
        Node::Text { .. } => Some(PartHeader::Text),
        Node::Reasoning { .. } => Some(PartHeader::Reasoning { id: node.id().cloned() }),
        Node::ToolCall { call_id, name, .. } => Some(PartHeader::ToolCall {
            id: node.id().cloned(),
            call_id: call_id.clone(),
            name: name.clone(),
        }),
        Node::Refusal { .. } => Some(PartHeader::Refusal),
        Node::Image { source: _, extra_body, .. } => Some(PartHeader::Image {
            extra_body: extra_body.clone(),
        }),
        Node::Audio { source: _, extra_body, .. } => Some(PartHeader::Audio {
            extra_body: extra_body.clone(),
        }),
        Node::File { source: _, extra_body, .. } => Some(PartHeader::File {
            extra_body: extra_body.clone(),
        }),
        Node::ProviderItem {
            item_type, body, ..
        } => Some(PartHeader::ProviderItem {
            id: node.id().cloned(),
            item_type: item_type.clone(),
            body: body.clone(),
        }),
        Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

fn part_delta_from_node_delta(delta: &NodeDelta) -> Option<PartDelta> {
    match delta {
        NodeDelta::Text { content } => Some(PartDelta::Text {
            content: content.clone(),
        }),
        NodeDelta::Reasoning {
            content,
            encrypted,
            summary,
            source,
        } => Some(PartDelta::Reasoning {
            content: content.clone(),
            encrypted: encrypted.clone(),
            summary: summary.clone(),
            source: source.clone(),
        }),
        NodeDelta::Refusal { content } => Some(PartDelta::Refusal {
            content: content.clone(),
        }),
        NodeDelta::ToolCallArguments { arguments } => Some(PartDelta::ToolCallArguments {
            arguments: arguments.clone(),
        }),
        NodeDelta::Image { source } => Some(PartDelta::Image {
            source: source.clone(),
        }),
        NodeDelta::Audio { source } => Some(PartDelta::Audio {
            source: source.clone(),
        }),
        NodeDelta::File { source } => Some(PartDelta::File {
            source: source.clone(),
        }),
        NodeDelta::ProviderItem { data } => Some(PartDelta::ProviderItem { data: data.clone() }),
    }
}

fn bridge_part_from_node(node: &Node) -> Option<Part> {
    match node {
        Node::Text { content, extra_body, .. } => Some(Part::Text {
            content: content.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::Reasoning {
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => Some(Part::Reasoning {
            id: id.clone(),
            content: content.clone(),
            encrypted: encrypted.clone(),
            summary: summary.clone(),
            source: source.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::ToolCall {
            id,
            call_id,
            name,
            arguments,
            extra_body,
        } => Some(Part::ToolCall {
            id: id.clone(),
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::Refusal { content, extra_body, .. } => Some(Part::Refusal {
            content: content.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::Image { source, extra_body, .. } => Some(Part::Image {
            source: source.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::Audio { source, extra_body, .. } => Some(Part::Audio {
            source: source.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::File { source, extra_body, .. } => Some(Part::File {
            source: source.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::ProviderItem {
            item_type,
            body,
            extra_body,
            ..
        } => Some(Part::ProviderItem {
            id: node.id().cloned(),
            item_type: item_type.clone(),
            body: body.clone(),
            extra_body: extra_body.clone(),
        }),
        Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

fn emit_node_start_with_bridge(
    node_index: u32,
    node: &Node,
    extra_body: HashMap<String, Value>,
    state: &mut AnthropicMessagesStreamState,
    events: &mut Vec<UrpStreamEvent>,
) {
    events.push(UrpStreamEvent::NodeStart {
        node_index,
        header: node_header_from_node(node),
        extra_body: extra_body.clone(),
    });

    let Some(part_header) = part_header_from_node(node) else {
        return;
    };

    if !state.bridge_item_started {
        events.push(UrpStreamEvent::ItemStart {
            item_index: 0,
            header: ItemHeader::Message {
                id: None,
                role: Role::Assistant,
            },
            extra_body: HashMap::new(),
        });
        state.bridge_item_started = true;
    }
    events.push(UrpStreamEvent::PartStart {
        part_index: node_index,
        item_index: 0,
        header: part_header,
        extra_body,
    });
}

fn emit_node_delta_with_bridge(
    node_index: u32,
    delta: NodeDelta,
    extra_body: HashMap<String, Value>,
    events: &mut Vec<UrpStreamEvent>,
) {
    events.push(UrpStreamEvent::NodeDelta {
        node_index,
        delta: delta.clone(),
        usage: None,
        extra_body: extra_body.clone(),
    });
    if let Some(part_delta) = part_delta_from_node_delta(&delta) {
        events.push(UrpStreamEvent::Delta {
            part_index: node_index,
            delta: part_delta,
            usage: None,
            extra_body,
        });
    }
}

fn emit_node_done_with_bridge(
    node_index: u32,
    node: Node,
    extra_body: HashMap<String, Value>,
    events: &mut Vec<UrpStreamEvent>,
) {
    events.push(UrpStreamEvent::NodeDone {
        node_index,
        node: node.clone(),
        usage: None,
        extra_body: extra_body.clone(),
    });
    if let Some(part) = bridge_part_from_node(&node) {
        events.push(UrpStreamEvent::PartDone {
            part_index: node_index,
            part,
            usage: None,
            extra_body,
        });
    }
}

fn ordered_completed_nodes(state: &AnthropicMessagesStreamState) -> Vec<Node> {
    state
        .node_order
        .iter()
        .filter_map(|node_index| state.completed_nodes.get(node_index).cloned())
        .collect()
}

fn build_completed_message_item(nodes: &[Node]) -> Item {
    Item::Message {
        id: None,
        role: Role::Assistant,
        parts: nodes.iter().filter_map(bridge_part_from_node).collect(),
        extra_body: HashMap::new(),
    }
}

fn build_message_completion_events(
    state: &mut AnthropicMessagesStreamState,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
    item: Item,
    response_extra: HashMap<String, Value>,
) -> Vec<UrpStreamEvent> {
    let mut events = Vec::new();

    if !state.bridge_item_started {
        events.push(UrpStreamEvent::ItemStart {
            item_index: 0,
            header: ItemHeader::Message {
                id: None,
                role: Role::Assistant,
            },
            extra_body: HashMap::new(),
        });
        state.bridge_item_started = true;
    }
    if !state.bridge_item_done_sent {
        events.push(UrpStreamEvent::ItemDone {
            item_index: 0,
            item: item.clone(),
            usage: usage.clone(),
            extra_body: HashMap::new(),
        });
        state.bridge_item_done_sent = true;
    }

    events.push(UrpStreamEvent::ResponseDone {
        finish_reason,
        usage,
        output: crate::urp::items_to_nodes(vec![item]),
        extra_body: response_extra,
    });
    events
}

fn estimated_output_chars(item: &Item) -> u64 {
    match item {
        Item::Message { parts, .. } => parts
            .iter()
            .map(|part| match part {
                Part::Text { content, .. } | Part::Refusal { content, .. } => content.len() as u64,
                Part::Reasoning {
                    content, summary, ..
                } => {
                    content.as_ref().map_or(0, |content| content.len() as u64)
                        + summary.as_ref().map_or(0, |summary| summary.len() as u64)
                }
                _ => 0,
            })
            .sum(),
        Item::ToolResult { .. } => 0,
    }
}

fn json_value_to_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

fn tool_use_input_is_placeholder(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null))
        || value
            .and_then(Value::as_object)
            .is_some_and(|obj| obj.is_empty())
}

fn object_without_keys(value: &Value, ignored: &[&str]) -> HashMap<String, Value> {
    let Some(obj) = value.as_object() else {
        return HashMap::new();
    };
    obj.iter()
        .filter(|(key, _)| !ignored.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn map_finish_reason(reason: &str) -> Option<FinishReason> {
    match reason {
        "end_turn" => Some(FinishReason::Stop),
        "max_tokens" => Some(FinishReason::Length),
        "tool_use" => Some(FinishReason::ToolCalls),
        "refusal" => Some(FinishReason::ContentFilter),
        "stop_sequence" => Some(FinishReason::Stop),
        "" => None,
        _ => Some(FinishReason::Other),
    }
}

fn finish_reason_name(reason: &FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::nodes_to_items;
    use serde_json::json;

    #[test]
    fn anthropic_block_lifecycle_emits_node_first_then_bridge_events() {
        let mut state = AnthropicMessagesStreamState::default();

        let start_events = handle_content_block_start(
            0,
            json!({
                "type": "thinking",
                "thinking": "",
                "signature": ""
            }),
            &mut state,
        );
        assert_eq!(start_events.len(), 3);
        assert!(matches!(
            &start_events[0],
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Reasoning { .. },
                ..
            } if *node_index == 0
        ));
        assert!(matches!(
            &start_events[1],
            UrpStreamEvent::ItemStart {
                item_index,
                header: ItemHeader::Message { role: Role::Assistant, .. },
                ..
            } if *item_index == 0
        ));
        assert!(matches!(
            &start_events[2],
            UrpStreamEvent::PartStart {
                item_index,
                part_index,
                header: PartHeader::Reasoning { .. },
                ..
            } if *item_index == 0 && *part_index == 0
        ));

        let thinking_delta_events = handle_content_block_delta(
            0,
            json!({
                "type": "thinking_delta",
                "thinking": "mock_reasoning"
            }),
            &mut state,
        );
        assert_eq!(thinking_delta_events.len(), 2);
        assert!(matches!(
            &thinking_delta_events[0],
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Reasoning { content: Some(content), encrypted: None, .. },
                ..
            } if *node_index == 0 && content == "mock_reasoning"
        ));
        assert!(matches!(
            &thinking_delta_events[1],
            UrpStreamEvent::Delta {
                part_index,
                delta: PartDelta::Reasoning { content: Some(content), encrypted: None, .. },
                ..
            } if *part_index == 0 && content == "mock_reasoning"
        ));

        let signature_delta_events = handle_content_block_delta(
            0,
            json!({
                "type": "signature_delta",
                "signature": "mock_sig"
            }),
            &mut state,
        );
        assert_eq!(signature_delta_events.len(), 2);
        assert!(matches!(
            &signature_delta_events[0],
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Reasoning { content: None, encrypted: Some(Value::String(signature)), .. },
                ..
            } if *node_index == 0 && signature == "mock_sig"
        ));
        assert!(matches!(
            &signature_delta_events[1],
            UrpStreamEvent::Delta {
                part_index,
                delta: PartDelta::Reasoning { content: None, encrypted: Some(Value::String(signature)), .. },
                ..
            } if *part_index == 0 && signature == "mock_sig"
        ));

        let done_events = handle_content_block_stop(0, &mut state);
        assert_eq!(done_events.len(), 2);
        assert!(matches!(
            &done_events[0],
            UrpStreamEvent::NodeDone {
                node_index,
                node: Node::Reasoning {
                    content: Some(content),
                    encrypted: Some(Value::String(signature)),
                    ..
                },
                ..
            } if *node_index == 0 && content == "mock_reasoning" && signature == "mock_sig"
        ));
        assert!(matches!(
            &done_events[1],
            UrpStreamEvent::PartDone {
                part_index,
                part: Part::Reasoning {
                    content: Some(content),
                    encrypted: Some(Value::String(signature)),
                    ..
                },
                ..
            } if *part_index == 0 && content == "mock_reasoning" && signature == "mock_sig"
        ));
    }

    #[test]
    fn anthropic_message_completion_uses_completed_nodes_as_authoritative_state() {
        let mut state = AnthropicMessagesStreamState::default();

        for event in handle_content_block_start(
            0,
            json!({ "type": "text", "text": "" }),
            &mut state,
        ) {
            assert!(!matches!(event, UrpStreamEvent::ResponseDone { .. }));
        }
        handle_content_block_delta(
            0,
            json!({ "type": "text_delta", "text": "hello" }),
            &mut state,
        );
        handle_content_block_stop(0, &mut state);

        handle_content_block_start(
            1,
            json!({
                "type": "tool_use",
                "id": "call_1",
                "name": "lookup",
                "input": {}
            }),
            &mut state,
        );
        handle_content_block_delta(
            1,
            json!({ "type": "input_json_delta", "partial_json": "{\"a\":1}" }),
            &mut state,
        );
        handle_content_block_stop(1, &mut state);

        let item = build_completed_message_item(&ordered_completed_nodes(&state));
        let completion_events = build_message_completion_events(
            &mut state,
            Some(FinishReason::ToolCalls),
            Some(Usage {
                input_tokens: 12,
                output_tokens: 8,
                input_details: None,
                output_details: None,
                extra_body: HashMap::new(),
            }),
            item,
            HashMap::new(),
        );

        assert!(matches!(
            &completion_events[0],
            UrpStreamEvent::ItemDone {
                item_index,
                item: Item::Message { parts, .. },
                ..
            }
            if *item_index == 0
                && matches!(&parts[0], Part::Text { content, .. } if content == "hello")
                && matches!(&parts[1], Part::ToolCall { call_id, name, arguments, .. } if call_id == "call_1" && name == "lookup" && arguments == "{\"a\":1}")
        ));
        assert!(matches!(
            &completion_events[1],
            UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::ToolCalls),
                output,
                ..
            }
            if {
                let output_items = nodes_to_items(output);
                matches!(
                    &output_items[0],
                    Item::Message { parts, .. }
                        if matches!(&parts[0], Part::Text { content, .. } if content == "hello")
                            && matches!(&parts[1], Part::ToolCall { call_id, arguments, .. } if call_id == "call_1" && arguments == "{\"a\":1}")
                )
            }
        ));
    }

    #[test]
    fn anthropic_tool_use_without_input_delta_keeps_non_placeholder_input() {
        let mut state = AnthropicMessagesStreamState::default();

        handle_content_block_start(
            0,
            json!({
                "type": "tool_use",
                "id": "call_1",
                "name": "lookup",
                "input": { "a": 1 }
            }),
            &mut state,
        );

        let done_events = handle_content_block_stop(0, &mut state);
        assert!(matches!(
            &done_events[0],
            UrpStreamEvent::NodeDone {
                node: Node::ToolCall { arguments, .. },
                ..
            } if arguments == "{\"a\":1}"
        ));
        assert!(matches!(
            &done_events[1],
            UrpStreamEvent::PartDone {
                part: Part::ToolCall { arguments, .. },
                ..
            } if arguments == "{\"a\":1}"
        ));
    }
}
