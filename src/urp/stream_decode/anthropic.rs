use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, record_cumulative_stream_usage_snapshot,
    record_stream_done_sentinel, record_stream_terminal_error, record_stream_terminal_event,
    record_visible_stream_event_delta,
};
use crate::handlers::{StreamRuntimeMetrics, StreamTerminalError, UrpRequest as HandlerUrpRequest};
use crate::urp::{
    FinishReason, InputDetails, MESSAGES_STREAM_START_USAGE_EXTRA_KEY, Node, NodeDelta, NodeHeader,
    OrdinaryRole, OutputDetails, ProviderProtocol, UrpStreamEvent, Usage,
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
    wire_to_node_index: HashMap<u32, u32>,
    next_node_index: u32,
    active_nodes: HashMap<u32, ActiveNodeState>,
    completed_nodes: HashMap<u32, Node>,
    usage: AnthropicStreamUsageAccumulator,
    finish_reason: Option<FinishReason>,
    exact_stop_reason: Option<String>,
    saw_terminal_delta: bool,
    response_done_sent: bool,
}

#[derive(Debug, Default)]
struct AnthropicStreamUsageAccumulator {
    saw_usage: bool,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
    cache_creation_5m_tokens: Option<u64>,
    cache_creation_1h_tokens: Option<u64>,
    tool_prompt_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    accepted_prediction_tokens: Option<u64>,
    rejected_prediction_tokens: Option<u64>,
    extra_body: HashMap<String, Value>,
}

impl AnthropicStreamUsageAccumulator {
    fn merge_event(&mut self, event: &Value) -> Option<Usage> {
        let usage = event
            .get("usage")
            .or_else(|| {
                event
                    .get("message")
                    .and_then(|message| message.get("usage"))
            })?
            .as_object()?;
        self.saw_usage = true;

        replace_numeric_counter(
            usage,
            &["input_tokens", "prompt_tokens"],
            &mut self.input_tokens,
        );
        replace_numeric_counter(
            usage,
            &["output_tokens", "completion_tokens"],
            &mut self.output_tokens,
        );
        replace_numeric_counter(
            usage,
            &[
                "cache_read_input_tokens",
                "cache_read_tokens",
                "cached_tokens",
            ],
            &mut self.cache_read_tokens,
        );
        replace_numeric_counter(
            usage,
            &[
                "cache_creation_input_tokens",
                "cache_creation_tokens",
                "cache_write_tokens",
            ],
            &mut self.cache_creation_tokens,
        );
        replace_numeric_counter(
            usage,
            &["tool_prompt_tokens", "tool_prompt_input_tokens"],
            &mut self.tool_prompt_tokens,
        );
        replace_numeric_counter(
            usage,
            &["reasoning_tokens", "reasoning_output_tokens"],
            &mut self.reasoning_tokens,
        );
        replace_numeric_counter(
            usage,
            &[
                "accepted_prediction_tokens",
                "accepted_prediction_output_tokens",
            ],
            &mut self.accepted_prediction_tokens,
        );
        replace_numeric_counter(
            usage,
            &[
                "rejected_prediction_tokens",
                "rejected_prediction_output_tokens",
            ],
            &mut self.rejected_prediction_tokens,
        );

        if let Some(cache_creation) = usage.get("cache_creation").and_then(Value::as_object) {
            replace_numeric_counter(
                cache_creation,
                &["ephemeral_5m_input_tokens"],
                &mut self.cache_creation_5m_tokens,
            );
            replace_numeric_counter(
                cache_creation,
                &["ephemeral_1h_input_tokens"],
                &mut self.cache_creation_1h_tokens,
            );
        }
        if let Some(output_details) = usage
            .get("output_tokens_details")
            .and_then(Value::as_object)
        {
            replace_numeric_counter(
                output_details,
                &["thinking_tokens"],
                &mut self.reasoning_tokens,
            );
            let mut unknown_output_details = output_details.clone();
            unknown_output_details.remove("thinking_tokens");
            if !unknown_output_details.is_empty() {
                let incoming = Value::Object(unknown_output_details);
                match self.extra_body.get_mut("output_tokens_details") {
                    Some(existing) => merge_cumulative_usage_value(existing, &incoming),
                    None => {
                        self.extra_body
                            .insert("output_tokens_details".to_string(), incoming);
                    }
                }
            }
        }

        const KNOWN_USAGE_KEYS: &[&str] = &[
            "input_tokens",
            "prompt_tokens",
            "output_tokens",
            "completion_tokens",
            "cache_read_input_tokens",
            "cache_read_tokens",
            "cached_tokens",
            "cache_creation_input_tokens",
            "cache_creation_tokens",
            "cache_write_tokens",
            "cache_creation",
            "tool_prompt_tokens",
            "tool_prompt_input_tokens",
            "reasoning_tokens",
            "reasoning_output_tokens",
            "output_tokens_details",
            "accepted_prediction_tokens",
            "accepted_prediction_output_tokens",
            "rejected_prediction_tokens",
            "rejected_prediction_output_tokens",
        ];
        for (key, value) in usage {
            if !KNOWN_USAGE_KEYS.contains(&key.as_str()) {
                match self.extra_body.get_mut(key) {
                    Some(existing) => merge_cumulative_usage_value(existing, value),
                    None => {
                        self.extra_body.insert(key.clone(), value.clone());
                    }
                }
            }
        }

        self.snapshot()
    }

    fn snapshot(&self) -> Option<Usage> {
        if !self.saw_usage {
            return None;
        }

        let wire_input_tokens = self.input_tokens.unwrap_or(0);
        let cache_read_tokens = self.cache_read_tokens.unwrap_or(0);
        let cache_creation_tokens = self.cache_creation_tokens.unwrap_or(0);
        let cache_creation_5m_tokens = self.cache_creation_5m_tokens.unwrap_or(0);
        let cache_creation_1h_tokens = self.cache_creation_1h_tokens.unwrap_or(0);
        let tool_prompt_tokens = self.tool_prompt_tokens.unwrap_or(0);
        let reasoning_tokens = self.reasoning_tokens.unwrap_or(0);
        let accepted_prediction_tokens = self.accepted_prediction_tokens.unwrap_or(0);
        let rejected_prediction_tokens = self.rejected_prediction_tokens.unwrap_or(0);

        let input_details = (cache_read_tokens > 0
            || cache_creation_tokens > 0
            || cache_creation_5m_tokens > 0
            || cache_creation_1h_tokens > 0
            || tool_prompt_tokens > 0)
            .then_some(InputDetails {
                standard_tokens: 0,
                cache_read_tokens,
                cache_read_modality_breakdown: None,
                cache_creation_tokens,
                cache_creation_5m_tokens,
                cache_creation_1h_tokens,
                tool_prompt_tokens,
                modality_breakdown: None,
            });
        let output_details = (reasoning_tokens > 0
            || accepted_prediction_tokens > 0
            || rejected_prediction_tokens > 0)
            .then_some(OutputDetails {
                standard_tokens: 0,
                reasoning_tokens,
                accepted_prediction_tokens,
                rejected_prediction_tokens,
                modality_breakdown: None,
            });

        Some(Usage {
            input_tokens: wire_input_tokens
                .saturating_add(cache_read_tokens)
                .saturating_add(cache_creation_tokens),
            output_tokens: self.output_tokens.unwrap_or(0),
            input_details,
            output_details,
            extra_body: self.extra_body.clone(),
        })
    }
}

fn replace_numeric_counter(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    destination: &mut Option<u64>,
) {
    if let Some(value) = keys
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_u64))
    {
        *destination = Some(value);
    }
}

fn merge_cumulative_usage_value(existing: &mut Value, incoming: &Value) {
    if incoming.is_null() {
        return;
    }
    if let (Some(existing), Some(incoming)) = (existing.as_object_mut(), incoming.as_object()) {
        for (key, value) in incoming {
            match existing.get_mut(key) {
                Some(existing_value) => merge_cumulative_usage_value(existing_value, value),
                None => {
                    existing.insert(key.clone(), value.clone());
                }
            }
        }
    } else {
        *existing = incoming.clone();
    }
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
        summary: String,
        encrypted: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
        replace_on_next_delta: bool,
    },
    ProviderItem {
        id: Option<String>,
        item_type: String,
        body: Value,
    },
}

pub(crate) async fn stream_messages_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut response_model = urp.model.clone();
    let mut response_extra = HashMap::new();
    let mut state = AnthropicMessagesStreamState::default();
    let mut explicit_terminal_event: Option<&'static str> = None;
    let mut downstream_closed = false;

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
            Ok(event) => event,
            Err(error) => {
                emit_messages_terminal_protocol_error(
                    &tx,
                    &runtime_metrics,
                    "upstream_stream_decode_failed",
                    error.to_string(),
                    HashMap::new(),
                )
                .await;
                return Ok(());
            }
        };
        if tx.is_closed() {
            downstream_closed = true;
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            explicit_terminal_event = Some("[DONE]");
            break;
        }

        let data_val: Value = match serde_json::from_str(&ev.data) {
            Ok(value) => value,
            Err(error) => {
                emit_messages_terminal_protocol_error(
                    &tx,
                    &runtime_metrics,
                    "messages_invalid_sse_json",
                    format!("invalid JSON in upstream Messages event: {error}"),
                    HashMap::from([
                        ("event_name".to_string(), Value::String(ev.event)),
                        ("raw_data".to_string(), Value::String(ev.data)),
                    ]),
                )
                .await;
                return Ok(());
            }
        };
        let cumulative_usage = state.usage.merge_event(&data_val);
        record_cumulative_stream_usage_snapshot(&runtime_metrics, cumulative_usage).await;

        match data_val.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "error" => {
                let (code, message, extra_body, terminal_error) =
                    messages_stream_error_parts(&data_val);
                let _ = tx
                    .send(UrpStreamEvent::Error {
                        code,
                        message,
                        extra_body,
                    })
                    .await;
                record_stream_terminal_error(&runtime_metrics, "error", terminal_error).await;
                return Ok(());
            }
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
                let mut response_start_extra = response_extra.clone();
                if let Some(usage) = state.usage.snapshot()
                    && let Ok(usage_value) = serde_json::to_value(usage)
                {
                    response_start_extra.insert(
                        MESSAGES_STREAM_START_USAGE_EXTRA_KEY.to_string(),
                        usage_value,
                    );
                }
                let _ = tx
                    .send(UrpStreamEvent::ResponseStart {
                        id: response_id.clone(),
                        model: response_model.clone(),
                        extra_body: response_start_extra,
                    })
                    .await;
            }
            "content_block_start" => {
                let wire_index = data_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let node_index = canonical_node_index_for_start(&mut state, wire_index);
                let cb = data_val
                    .get("content_block")
                    .cloned()
                    .unwrap_or(Value::Null);
                for event in handle_content_block_start(node_index, cb, &mut state) {
                    let _ = tx.send(event).await;
                }
            }
            "content_block_delta" => {
                let wire_index = data_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let Some(node_index) = state.wire_to_node_index.get(&wire_index).copied() else {
                    continue;
                };
                let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                for event in handle_content_block_delta(node_index, delta, &mut state) {
                    record_visible_stream_event_delta(started_at, &runtime_metrics, &event).await;
                    let _ = tx.send(event).await;
                }
            }
            "content_block_stop" => {
                let wire_index = data_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let Some(node_index) = state.wire_to_node_index.get(&wire_index).copied() else {
                    continue;
                };
                for event in handle_content_block_stop(node_index, &mut state) {
                    let _ = tx.send(event).await;
                }
            }
            "message_delta" => {
                merge_message_delta_state(&mut state, &data_val);
            }
            "ping" => {
                let _ = tx
                    .send(UrpStreamEvent::ProviderControl {
                        protocol: "messages".to_string(),
                        event_name: "ping".to_string(),
                        data: data_val,
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
            "message_stop" => {
                explicit_terminal_event = Some("message_stop");
                break;
            }
            _ => {}
        }
    }

    let terminal_event = explicit_terminal_event.or_else(|| {
        state
            .saw_terminal_delta
            .then_some("message_delta_stream_end")
    });
    if !downstream_closed {
        if let Some(terminal_event) = terminal_event {
            let output_nodes = ordered_completed_nodes(&state);
            crate::handlers::usage::increment_estimated_output_tokens(
                &runtime_metrics,
                estimated_output_chars(&output_nodes),
            )
            .await;
            record_stream_terminal_event(
                &runtime_metrics,
                terminal_event,
                state.finish_reason.as_ref().map(finish_reason_name),
            )
            .await;
            if let Some(event) = take_response_done(&mut state, &response_extra) {
                let _ = tx.send(event).await;
            }
        } else {
            emit_messages_terminal_protocol_error(
                &tx,
                &runtime_metrics,
                "upstream_stream_missing_terminal",
                "upstream Messages stream ended without message_stop, [DONE], or a non-null stop_reason"
                    .to_string(),
                HashMap::new(),
            )
            .await;
        }
    }

    Ok(())
}

fn canonical_node_index_for_start(
    state: &mut AnthropicMessagesStreamState,
    wire_index: u32,
) -> u32 {
    if let Some(node_index) = state.wire_to_node_index.get(&wire_index) {
        return *node_index;
    }
    let node_index = state.next_node_index;
    state.next_node_index = state.next_node_index.saturating_add(1);
    state.wire_to_node_index.insert(wire_index, node_index);
    node_index
}

async fn emit_messages_terminal_protocol_error(
    tx: &mpsc::Sender<UrpStreamEvent>,
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    code: &str,
    message: String,
    extra_body: HashMap<String, Value>,
) {
    let _ = tx
        .send(UrpStreamEvent::Error {
            code: Some(code.to_string()),
            message: message.clone(),
            extra_body,
        })
        .await;
    record_stream_terminal_error(
        runtime_metrics,
        code,
        StreamTerminalError {
            code: code.to_string(),
            message,
            http_status: StatusCode::BAD_GATEWAY.as_u16(),
            error_type: Some("upstream_protocol_error".to_string()),
            param: None,
        },
    )
    .await;
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

    vec![UrpStreamEvent::NodeStart {
        node_index,
        header: node_header_from_node(&node),
        extra_body,
    }]
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
    let mut delta_extra = object_without_keys(
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
        (ActiveNodeKind::Reasoning { summary, .. }, "thinking_delta") => {
            let Some(text) = delta_value.get("thinking").and_then(|v| v.as_str()) else {
                return Vec::new();
            };
            if text.is_empty() {
                return Vec::new();
            }
            summary.push_str(text);
            delta_extra.insert(
                "_monoize_summary_from_messages_thinking".to_string(),
                Value::Bool(true),
            );
            NodeDelta::Reasoning {
                content: None,
                encrypted: None,
                summary: Some(text.to_string()),
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

    vec![UrpStreamEvent::NodeDelta {
        node_index,
        delta: stream_delta,
        usage: None,
        extra_body: delta_extra,
    }]
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

    vec![UrpStreamEvent::NodeDone {
        node_index,
        node,
        usage: None,
        extra_body,
    }]
}

fn active_node_from_content_block(content_block: &Value) -> Option<ActiveNodeState> {
    let content_type = content_block
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

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
            let extra_body = object_without_keys(content_block, &["type", "thinking", "signature"]);
            Some(ActiveNodeState {
                kind: ActiveNodeKind::Reasoning {
                    summary: content_block
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
                    summary: String::new(),
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
        _ => Some(ActiveNodeState {
            kind: ActiveNodeKind::ProviderItem {
                id: content_block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                item_type: content_type.to_string(),
                body: content_block.clone(),
            },
            extra_body: HashMap::new(),
        }),
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
        ActiveNodeKind::Reasoning { summary, encrypted } => {
            let extra_body = active_node.extra_body.clone();
            let (id, encrypted_value) = if encrypted.is_empty() {
                (None, None)
            } else {
                match crate::urp::unwrap_reasoning_signature_sigil(encrypted) {
                    Some((item_id, original)) => (Some(item_id), Some(Value::String(original))),
                    None => (None, Some(Value::String(encrypted.clone()))),
                }
            };
            Node::Reasoning {
                id,
                content: None,
                encrypted: encrypted_value,
                summary: (!summary.is_empty()).then(|| summary.clone()),
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
        ActiveNodeKind::ProviderItem {
            id,
            item_type,
            body,
        } => Node::ProviderItem {
            id: id.clone(),
            origin_protocol: ProviderProtocol::Messages,
            role: OrdinaryRole::Assistant,
            item_type: item_type.clone(),
            body: body.clone(),
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
        Node::Reasoning { .. } => NodeHeader::Reasoning {
            id: node.id().cloned(),
        },
        Node::ToolCall { call_id, name, .. } => NodeHeader::ToolCall {
            id: node.id().cloned(),
            call_id: call_id.clone(),
            name: name.clone(),
        },
        Node::Image { role, .. } => NodeHeader::Image {
            id: node.id().cloned(),
            role: *role,
        },
        Node::Audio { role, .. } => NodeHeader::Audio {
            id: node.id().cloned(),
            role: *role,
        },
        Node::File { role, .. } => NodeHeader::File {
            id: node.id().cloned(),
            role: *role,
        },
        Node::Refusal { .. } => NodeHeader::Refusal {
            id: node.id().cloned(),
        },
        Node::ProviderItem {
            role,
            origin_protocol,
            item_type,
            ..
        } => NodeHeader::ProviderItem {
            id: node.id().cloned(),
            origin_protocol: *origin_protocol,
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

fn ordered_completed_nodes(state: &AnthropicMessagesStreamState) -> Vec<Node> {
    state
        .node_order
        .iter()
        .filter_map(|node_index| state.completed_nodes.get(node_index).cloned())
        .collect()
}

fn take_response_done(
    state: &mut AnthropicMessagesStreamState,
    response_extra: &HashMap<String, Value>,
) -> Option<UrpStreamEvent> {
    if state.response_done_sent {
        return None;
    }
    state.response_done_sent = true;
    let mut extra_body = response_extra.clone();
    if let Some(stop_reason) = state.exact_stop_reason.as_ref() {
        extra_body.insert(
            "stop_reason".to_string(),
            Value::String(stop_reason.clone()),
        );
    }
    Some(UrpStreamEvent::ResponseDone {
        finish_reason: state.finish_reason.clone(),
        usage: state.usage.snapshot(),
        output: ordered_completed_nodes(state),
        extra_body,
    })
}

fn estimated_output_chars(nodes: &[Node]) -> u64 {
    nodes
        .iter()
        .map(|node| match node {
            Node::Text { content, .. } | Node::Refusal { content, .. } => content.len() as u64,
            Node::Reasoning {
                content, summary, ..
            } => {
                content.as_ref().map_or(0, |content| content.len() as u64)
                    + summary.as_ref().map_or(0, |summary| summary.len() as u64)
            }
            _ => 0,
        })
        .sum()
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

fn messages_stream_error_parts(
    data_val: &Value,
) -> (
    Option<String>,
    String,
    HashMap<String, Value>,
    StreamTerminalError,
) {
    let error_value = data_val
        .get("error")
        .cloned()
        .unwrap_or_else(|| data_val.clone());
    let code = error_value
        .get("type")
        .and_then(|v| v.as_str())
        .or_else(|| error_value.get("code").and_then(|v| v.as_str()))
        .or_else(|| data_val.get("code").and_then(|v| v.as_str()))
        .map(str::to_string);
    let message = error_value
        .get("message")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("message").and_then(|v| v.as_str()))
        .unwrap_or_else(|| data_val.as_str().unwrap_or("upstream error"))
        .to_string();
    let param = error_value
        .get("param")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("param").and_then(|v| v.as_str()))
        .map(str::to_string);
    let http_status = error_value
        .get("status")
        .and_then(|v| v.as_u64())
        .or_else(|| data_val.get("status").and_then(|v| v.as_u64()))
        .filter(|status| (400..=599).contains(status))
        .and_then(|status| u16::try_from(status).ok())
        .unwrap_or(StatusCode::BAD_REQUEST.as_u16());
    let mut extra_body = object_without_keys(&error_value, &["type", "code", "message", "param"]);
    extra_body.extend(object_without_keys(
        data_val,
        &["type", "error", "code", "message", "param"],
    ));
    let terminal_error = StreamTerminalError {
        code: code
            .clone()
            .unwrap_or_else(|| "upstream_stream_error".to_string()),
        message: message.clone(),
        http_status,
        error_type: code.clone(),
        param: param.clone(),
    };
    (code, message, extra_body, terminal_error)
}

fn merge_message_delta_state(state: &mut AnthropicMessagesStreamState, event: &Value) {
    let Some(stop_reason) = event
        .get("delta")
        .and_then(|value| value.get("stop_reason"))
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    else {
        return;
    };
    state.exact_stop_reason = Some(stop_reason.to_string());
    state.finish_reason = map_finish_reason(stop_reason);
    state.saw_terminal_delta = true;
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
    use crate::urp::Usage;
    use serde_json::json;

    #[test]
    fn anthropic_block_lifecycle_emits_only_node_events() {
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
        assert_eq!(start_events.len(), 1);
        assert!(matches!(
            &start_events[0],
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Reasoning { .. },
                ..
            } if *node_index == 0
        ));
        let thinking_delta_events = handle_content_block_delta(
            0,
            json!({
                "type": "thinking_delta",
                "thinking": "mock_reasoning"
            }),
            &mut state,
        );
        assert_eq!(thinking_delta_events.len(), 1);
        assert!(matches!(
            &thinking_delta_events[0],
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Reasoning { content: None, encrypted: None, summary: Some(summary), .. },
                ..
            } if *node_index == 0 && summary == "mock_reasoning"
        ));
        let signature_delta_events = handle_content_block_delta(
            0,
            json!({
                "type": "signature_delta",
                "signature": "mock_sig"
            }),
            &mut state,
        );
        assert_eq!(signature_delta_events.len(), 1);
        assert!(matches!(
            &signature_delta_events[0],
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Reasoning { content: None, encrypted: Some(Value::String(signature)), .. },
                ..
            } if *node_index == 0 && signature == "mock_sig"
        ));
        let done_events = handle_content_block_stop(0, &mut state);
        assert_eq!(done_events.len(), 1);
        assert!(matches!(
            &done_events[0],
            UrpStreamEvent::NodeDone {
                node_index,
                node: Node::Reasoning {
                    content: None,
                    encrypted: Some(Value::String(signature)),
                    summary: Some(summary),
                    ..
                },
                ..
            } if *node_index == 0 && summary == "mock_reasoning" && signature == "mock_sig"
        ));
    }

    #[test]
    fn anthropic_wire_indices_map_to_contiguous_canonical_node_indices() {
        let mut state = AnthropicMessagesStreamState::default();

        assert_eq!(canonical_node_index_for_start(&mut state, 4), 0);
        assert_eq!(canonical_node_index_for_start(&mut state, 9), 1);
        assert_eq!(canonical_node_index_for_start(&mut state, 4), 0);
        assert_eq!(state.next_node_index, 2);
        assert_eq!(state.wire_to_node_index.get(&4), Some(&0));
        assert_eq!(state.wire_to_node_index.get(&9), Some(&1));
    }

    #[test]
    fn anthropic_omitted_thinking_block_completes_with_signature_only() {
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
        assert_eq!(start_events.len(), 1);

        let signature_events = handle_content_block_delta(
            0,
            json!({
                "type": "signature_delta",
                "signature": "sig_omitted"
            }),
            &mut state,
        );
        assert_eq!(signature_events.len(), 1);
        assert!(matches!(
            &signature_events[0],
            UrpStreamEvent::NodeDelta {
                delta: NodeDelta::Reasoning {
                    content: None,
                    encrypted: Some(Value::String(signature)),
                    summary: None,
                    ..
                },
                ..
            } if signature == "sig_omitted"
        ));

        let done_events = handle_content_block_stop(0, &mut state);
        assert_eq!(done_events.len(), 1);
        assert!(matches!(
            &done_events[0],
            UrpStreamEvent::NodeDone {
                node: Node::Reasoning {
                    content: None,
                    encrypted: Some(Value::String(signature)),
                    summary: None,
                    ..
                },
                ..
            } if signature == "sig_omitted"
        ));
    }

    #[test]
    fn anthropic_message_completion_uses_completed_nodes_as_authoritative_state() {
        let mut state = AnthropicMessagesStreamState::default();

        for event in
            handle_content_block_start(0, json!({ "type": "text", "text": "" }), &mut state)
        {
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

        let completion_event = UrpStreamEvent::ResponseDone {
            finish_reason: Some(FinishReason::ToolCalls),
            usage: Some(Usage {
                input_tokens: 12,
                output_tokens: 8,
                input_details: None,
                output_details: None,
                extra_body: HashMap::new(),
            }),
            output: ordered_completed_nodes(&state),
            extra_body: HashMap::new(),
        };

        assert!(matches!(
            &completion_event,
            UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::ToolCalls),
                output,
                ..
            }
            if matches!(&output[0], Node::Text { content, .. } if content == "hello")
                && matches!(&output[1], Node::ToolCall { call_id, name, arguments, .. } if call_id == "call_1" && name == "lookup" && arguments == "{\"a\":1}")
        ));
    }

    #[test]
    fn anthropic_partial_usage_is_merged_cumulatively() {
        let mut usage = AnthropicStreamUsageAccumulator::default();

        let start = usage
            .merge_event(&json!({
                "type": "message_start",
                "message": {
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 0,
                        "cache_read_input_tokens": 3,
                        "cache_creation_input_tokens": 2,
                        "cache_creation": {
                            "ephemeral_5m_input_tokens": 2,
                            "ephemeral_1h_input_tokens": 0
                        },
                        "output_tokens_details": { "future_detail": { "start": 1 } },
                        "server_tool_use": {
                            "web_fetch_requests": 1,
                            "web_search_requests": 0
                        },
                        "custom_start_counter": 7
                    }
                }
            }))
            .expect("start usage");
        assert_eq!(start.input_tokens, 15);
        assert_eq!(start.output_tokens, 0);

        let first_delta = usage
            .merge_event(&json!({
                "type": "message_delta",
                "usage": {
                    "input_tokens": Value::Null,
                    "output_tokens": 4,
                    "output_tokens_details": {
                        "thinking_tokens": 2,
                        "future_detail": { "delta": 2 }
                    },
                    "cache_read_input_tokens": Value::Null,
                    "server_tool_use": { "web_search_requests": 2 },
                    "custom_delta_counter": 11
                }
            }))
            .expect("first delta usage");
        assert_eq!(first_delta.input_tokens, 15);
        assert_eq!(first_delta.output_tokens, 4);

        let final_usage = usage
            .merge_event(&json!({
                "type": "message_delta",
                "usage": { "output_tokens": 9 }
            }))
            .expect("final usage");
        assert_eq!(final_usage.input_tokens, 15);
        assert_eq!(final_usage.output_tokens, 9);
        let input_details = final_usage.input_details.expect("input details");
        assert_eq!(input_details.cache_read_tokens, 3);
        assert_eq!(input_details.cache_creation_tokens, 2);
        assert_eq!(input_details.cache_creation_5m_tokens, 2);
        assert_eq!(input_details.cache_creation_1h_tokens, 0);
        assert_eq!(
            final_usage
                .output_details
                .expect("output details")
                .reasoning_tokens,
            2
        );
        assert_eq!(final_usage.extra_body["custom_start_counter"], json!(7));
        assert_eq!(final_usage.extra_body["custom_delta_counter"], json!(11));
        assert_eq!(
            final_usage.extra_body["server_tool_use"],
            json!({ "web_fetch_requests": 1, "web_search_requests": 2 })
        );
        assert_eq!(
            final_usage.extra_body["output_tokens_details"],
            json!({ "future_detail": { "start": 1, "delta": 2 } })
        );
    }

    #[test]
    fn anthropic_terminal_event_is_taken_once_after_multiple_deltas() {
        let mut state = AnthropicMessagesStreamState::default();
        state.usage.merge_event(&json!({
            "type": "message_start",
            "message": { "usage": { "input_tokens": 10, "output_tokens": 0 } }
        }));
        merge_message_delta_state(
            &mut state,
            &json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": { "output_tokens": 4 }
            }),
        );
        state.usage.merge_event(&json!({
            "type": "message_delta",
            "delta": { "stop_reason": Value::Null },
            "usage": { "input_tokens": Value::Null, "output_tokens": 9 }
        }));

        let first = take_response_done(&mut state, &HashMap::new()).expect("terminal event");
        assert!(matches!(
            first,
            UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: Some(Usage {
                    input_tokens: 10,
                    output_tokens: 9,
                    ..
                }),
                ..
            }
        ));
        assert!(take_response_done(&mut state, &HashMap::new()).is_none());
    }

    #[test]
    fn anthropic_terminal_event_preserves_exact_messages_stop_reason() {
        let mut state = AnthropicMessagesStreamState::default();
        merge_message_delta_state(
            &mut state,
            &json!({
                "type": "message_delta",
                "delta": { "stop_reason": "pause_turn" }
            }),
        );

        let event = take_response_done(&mut state, &HashMap::new()).expect("terminal event");
        assert!(matches!(
            event,
            UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Other),
                extra_body,
                ..
            } if extra_body.get("stop_reason") == Some(&json!("pause_turn"))
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
        assert_eq!(done_events.len(), 1);
        assert!(matches!(
            &done_events[0],
            UrpStreamEvent::NodeDone {
                node: Node::ToolCall { arguments, .. },
                ..
            } if arguments == "{\"a\":1}"
        ));
    }
}
