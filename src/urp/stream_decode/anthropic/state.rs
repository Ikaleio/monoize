//! Cross-event state of one upstream Messages stream and the handler of each event kind.

use super::blocks::{ActiveBlock, start_error_code};
use super::object_without_keys;
use super::usage::AnthropicStreamUsageAccumulator;
use crate::handlers::usage::{
    increment_estimated_output_tokens, mark_stream_ttfb_if_needed,
    record_cumulative_stream_usage_snapshot, record_observed_upstream_response_model,
    record_stream_done_sentinel, record_stream_response_service_tier, record_stream_terminal_error,
    record_stream_terminal_event, record_visible_stream_event_delta,
};
use crate::handlers::{StreamRuntimeMetrics, StreamTerminalError, UrpRequest as HandlerUrpRequest};
use crate::urp::encode::anthropic::messages_finish_reason;
use crate::urp::{
    FinishReason, Node, NodeDelta, TerminalNode, ToolCallType, UrpStreamEvent, is_internal_key,
};
use axum::http::StatusCode;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};

/// What the driver loop does after one upstream event.
pub(super) enum Flow {
    Continue,
    /// The upstream stream reached its terminal event with this metrics name.
    Terminal(&'static str),
    /// The stream already ended with an error event.
    Stop,
}

/// One upstream content block, in `content_block_start` order. `node_index` equals its
/// position, so URP indices are sequential in first-seen order (STR-4).
struct ContentBlockSlot {
    node_index: u32,
    phase: BlockPhase,
}

enum BlockPhase {
    /// A start block that decodes to no node; later events for it are lifecycle errors.
    Skipped,
    Active(Box<ActiveBlock>),
    Done(Box<Node>),
}

pub(super) struct AnthropicMessagesStreamState<'a> {
    custom_tool_names: &'a HashSet<String>,
    tx: mpsc::Sender<UrpStreamEvent>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    started_at: Option<Instant>,
    downstream_closed: bool,
    response_started: bool,
    response_id: String,
    response_model: String,
    /// Unknown `message_start.message` members, replayed on `ResponseDone`.
    response_extra: HashMap<String, Value>,
    blocks: Vec<ContentBlockSlot>,
    block_by_wire_index: HashMap<u32, usize>,
    usage: AnthropicStreamUsageAccumulator,
    finish_reason: Option<FinishReason>,
    /// Native `stop_reason`; set exactly when a terminal `message_delta` arrived.
    stop_reason: Option<String>,
    stop_sequence: Option<Value>,
    /// PM6d: latest non-stop `message_delta.delta` members such as `stop_details` and `container`.
    message_delta_extra: HashMap<String, Value>,
}

impl<'a> AnthropicMessagesStreamState<'a> {
    pub(super) fn new(
        urp: &'a HandlerUrpRequest,
        tx: mpsc::Sender<UrpStreamEvent>,
        started_at: Option<Instant>,
        runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    ) -> Self {
        Self {
            custom_tool_names: &urp.messages_custom_tool_names,
            tx,
            runtime_metrics,
            started_at,
            downstream_closed: false,
            response_started: false,
            response_id: format!("resp_{}", uuid::Uuid::new_v4()),
            response_model: urp.model.clone(),
            response_extra: HashMap::new(),
            blocks: Vec::new(),
            block_by_wire_index: HashMap::new(),
            usage: AnthropicStreamUsageAccumulator::default(),
            finish_reason: None,
            stop_reason: None,
            stop_sequence: None,
            message_delta_extra: HashMap::new(),
        }
    }

    async fn send(&self, event: UrpStreamEvent) {
        let _ = self.tx.send(event).await;
    }

    /// Emits a terminal URP error for an upstream protocol violation.
    pub(super) async fn protocol_error(
        &self,
        code: &str,
        message: String,
        extra_body: HashMap<String, Value>,
    ) -> Flow {
        self.send(UrpStreamEvent::Error {
            code: Some(code.to_string()),
            message: message.clone(),
            extra_body,
        })
        .await;
        record_stream_terminal_error(
            &self.runtime_metrics,
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
        Flow::Stop
    }

    pub(super) async fn on_sse_event(&mut self, event: crate::payload::sse::Frame) -> Flow {
        if self.tx.is_closed() {
            self.downstream_closed = true;
        }
        mark_stream_ttfb_if_needed(&mut self.started_at, &self.runtime_metrics).await;
        if event.data.is_done() {
            record_stream_done_sentinel(&self.runtime_metrics).await;
            return Flow::Terminal("[DONE]");
        }
        let data: Value = match event.data.into_json() {
            Ok(data) => data,
            Err((error, raw_data)) => {
                return self
                    .protocol_error(
                        "messages_invalid_sse_json",
                        format!("invalid JSON in upstream Messages event: {error}"),
                        HashMap::from([
                            ("event_name".to_string(), Value::String(event.event)),
                            ("raw_data".to_string(), Value::String(raw_data)),
                        ]),
                    )
                    .await;
            }
        };
        record_stream_response_service_tier(&self.runtime_metrics, &data).await;
        let cumulative_usage = self.usage.merge_event(&data);
        record_cumulative_stream_usage_snapshot(&self.runtime_metrics, cumulative_usage).await;
        self.on_event(data).await
    }

    async fn on_event(&mut self, data: Value) -> Flow {
        let event_type = data.get("type").and_then(Value::as_str).unwrap_or("");
        let in_message = matches!(
            event_type,
            "content_block_start"
                | "content_block_delta"
                | "content_block_stop"
                | "message_delta"
                | "message_stop"
        );
        if (event_type == "message_start" && self.response_started)
            || (in_message && !self.response_started)
        {
            return self
                .protocol_error(
                    "messages_invalid_message_lifecycle",
                    format!("{event_type} occurred outside its message lifecycle"),
                    HashMap::new(),
                )
                .await;
        }
        match event_type {
            "error" => self.on_error(&data).await,
            "message_start" => self.on_message_start(&data).await,
            "content_block_start" | "content_block_delta" | "content_block_stop" => {
                self.on_content_block_event(event_type, &data).await
            }
            "message_delta" => self.on_message_delta(&data),
            "ping" => {
                self.send(UrpStreamEvent::ProviderControl {
                    protocol: "messages".to_string(),
                    event_name: "ping".to_string(),
                    data,
                    extra_body: HashMap::new(),
                })
                .await;
                Flow::Continue
            }
            "message_stop" => Flow::Terminal("message_stop"),
            _ => Flow::Continue,
        }
    }

    async fn on_error(&self, data: &Value) -> Flow {
        let (code, message, extra_body, terminal_error) = messages_stream_error_parts(data);
        self.send(UrpStreamEvent::Error {
            code,
            message,
            extra_body,
        })
        .await;
        record_stream_terminal_error(&self.runtime_metrics, "error", terminal_error).await;
        Flow::Stop
    }

    async fn on_message_start(&mut self, data: &Value) -> Flow {
        self.response_started = true;
        let message = data.get("message").unwrap_or(&Value::Null);
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            self.response_id = id.to_string();
        }
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            self.response_model = model.to_string();
            record_observed_upstream_response_model(&self.runtime_metrics, model, false).await;
        }
        self.response_extra = object_without_keys(
            message,
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
        self.send(UrpStreamEvent::ResponseStart {
            usage: self.usage.snapshot(),
            id: self.response_id.clone(),
            model: self.response_model.clone(),
            // Messages carries no creation time.
            created_at: None,
            extra_body: self.response_extra.clone(),
        })
        .await;
        Flow::Continue
    }

    async fn on_content_block_event(&mut self, event_type: &str, data: &Value) -> Flow {
        let wire_index = data
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|index| u32::try_from(index).ok());
        let position = wire_index.and_then(|index| self.block_by_wire_index.get(&index).copied());
        let valid = match event_type {
            "content_block_start" => wire_index.is_some() && position.is_none(),
            _ => position.is_some_and(|position| {
                matches!(self.blocks[position].phase, BlockPhase::Active(_))
            }),
        };
        let (Some(wire_index), true) = (wire_index, valid) else {
            return self
                .protocol_error(
                    "messages_invalid_block_lifecycle",
                    format!("{event_type} has an invalid, reused, or inactive content block index"),
                    HashMap::new(),
                )
                .await;
        };
        match (event_type, position) {
            ("content_block_start", _) => self.on_content_block_start(wire_index, data).await,
            ("content_block_delta", Some(position)) => {
                self.on_content_block_delta(position, data).await
            }
            (_, Some(position)) => self.on_content_block_stop(position).await,
            (_, None) => Flow::Continue,
        }
    }

    async fn on_content_block_start(&mut self, wire_index: u32, data: &Value) -> Flow {
        let content_block = data.get("content_block").unwrap_or(&Value::Null);
        let block = match ActiveBlock::from_content_block(content_block, self.custom_tool_names) {
            Ok(block) => block,
            Err(message) => {
                return self
                    .protocol_error(start_error_code(content_block), message, HashMap::new())
                    .await;
            }
        };
        let node_index = u32::try_from(self.blocks.len()).unwrap_or(u32::MAX);
        self.block_by_wire_index
            .insert(wire_index, self.blocks.len());
        let phase = match block {
            Some(block) => {
                for event in block.start_events(node_index) {
                    self.send(event).await;
                }
                BlockPhase::Active(Box::new(block))
            }
            None => BlockPhase::Skipped,
        };
        self.blocks.push(ContentBlockSlot { node_index, phase });
        Flow::Continue
    }

    async fn on_content_block_delta(&mut self, position: usize, data: &Value) -> Flow {
        let slot = &mut self.blocks[position];
        let BlockPhase::Active(block) = &mut slot.phase else {
            return Flow::Continue;
        };
        let node_index = slot.node_index;
        let delta = match block.apply_delta(data.get("delta").unwrap_or(&Value::Null)) {
            Ok(Some((delta, extra_body))) => delta_event(node_index, delta, extra_body),
            Ok(None) => return Flow::Continue,
            Err(message) => {
                return self
                    .protocol_error(
                        "messages_custom_tool_input_invalid",
                        message,
                        HashMap::new(),
                    )
                    .await;
            }
        };
        record_visible_stream_event_delta(&self.runtime_metrics, &delta).await;
        self.send(delta).await;
        Flow::Continue
    }

    async fn on_content_block_stop(&mut self, position: usize) -> Flow {
        let slot = &mut self.blocks[position];
        let BlockPhase::Active(block) = std::mem::replace(&mut slot.phase, BlockPhase::Skipped)
        else {
            return Flow::Continue;
        };
        let node_index = slot.node_index;
        let (node, extra_body) = match block.finish() {
            Ok(done) => done,
            Err(message) => {
                return self
                    .protocol_error(
                        "messages_custom_tool_input_invalid",
                        message,
                        HashMap::new(),
                    )
                    .await;
            }
        };
        self.blocks[position].phase = BlockPhase::Done(Box::new(node.clone()));
        self.send(UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body,
        })
        .await;
        Flow::Continue
    }

    fn on_message_delta(&mut self, data: &Value) -> Flow {
        let Some(delta) = data.get("delta").and_then(Value::as_object) else {
            return Flow::Continue;
        };
        if let Some(stop_sequence) = delta.get("stop_sequence") {
            self.stop_sequence = Some(stop_sequence.clone());
        }
        for (key, value) in delta {
            if !matches!(key.as_str(), "stop_reason" | "stop_sequence") && !is_internal_key(key) {
                self.message_delta_extra.insert(key.clone(), value.clone());
            }
        }
        let Some(stop_reason) = delta
            .get("stop_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        else {
            return Flow::Continue;
        };
        self.finish_reason = messages_finish_reason(stop_reason);
        self.stop_reason = Some(stop_reason.to_string());
        Flow::Terminal("message_delta.stop_reason")
    }

    /// Validates the closed stream and emits `ResponseDone`, or the terminal error.
    pub(super) async fn finish(self, terminal_event: Option<&'static str>) {
        let Some(terminal_event) = terminal_event else {
            if !self.downstream_closed {
                self.protocol_error(
                    "upstream_stream_missing_terminal",
                    "upstream Messages stream ended without message_stop, [DONE], or a non-null stop_reason"
                        .to_string(),
                    HashMap::new(),
                )
                .await;
            }
            return;
        };
        let has_open_block = self
            .blocks
            .iter()
            .any(|slot| matches!(slot.phase, BlockPhase::Active(_)));
        if !self.downstream_closed && has_open_block {
            self.protocol_error(
                "messages_invalid_block_lifecycle",
                "upstream Messages stream ended before a content block closed".to_string(),
                HashMap::new(),
            )
            .await;
            return;
        }
        if self.has_invalid_tool_input() {
            self.protocol_error(
                "messages_tool_input_invalid",
                "completed Messages tool input must be a JSON object".to_string(),
                HashMap::new(),
            )
            .await;
            return;
        }

        let output: Vec<TerminalNode> = self
            .blocks
            .iter()
            .filter_map(|slot| match &slot.phase {
                BlockPhase::Done(node) => Some(TerminalNode::streamed(
                    slot.node_index,
                    node.as_ref().clone(),
                )),
                _ => None,
            })
            .collect();
        increment_estimated_output_tokens(&self.runtime_metrics, estimated_output_chars(&output))
            .await;
        record_stream_terminal_event(
            &self.runtime_metrics,
            terminal_event,
            self.finish_reason.as_ref().map(finish_reason_name),
        )
        .await;
        if self.downstream_closed {
            return;
        }
        let mut extra_body = self.response_extra.clone();
        extra_body.extend(self.message_delta_extra.clone());
        if let Some(stop_reason) = &self.stop_reason {
            extra_body.insert(
                "stop_reason".to_string(),
                Value::String(stop_reason.clone()),
            );
        }
        if let Some(stop_sequence) = &self.stop_sequence {
            extra_body.insert("stop_sequence".to_string(), stop_sequence.clone());
        }
        self.send(UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason: self.finish_reason,
            usage: self.usage.snapshot(),
            output,
            extra_body,
        })
        .await;
    }

    /// A completed function call whose input is not a JSON object. Input truncated by
    /// `max_tokens` is accepted.
    fn has_invalid_tool_input(&self) -> bool {
        self.blocks.iter().any(|slot| {
            let BlockPhase::Done(node) = &slot.phase else {
                return false;
            };
            let Node::ToolCall {
                tool_type: ToolCallType::Function,
                arguments,
                ..
            } = node.as_ref()
            else {
                return false;
            };
            match serde_json::from_str::<Value>(arguments) {
                Ok(value) => !value.is_object(),
                Err(error) => !(self.finish_reason == Some(FinishReason::Length) && error.is_eof()),
            }
        })
    }
}

fn delta_event(
    node_index: u32,
    delta: NodeDelta,
    extra_body: HashMap<String, Value>,
) -> UrpStreamEvent {
    UrpStreamEvent::NodeDelta {
        node_index,
        delta,
        usage: None,
        extra_body,
    }
}

fn estimated_output_chars(output: &[TerminalNode]) -> u64 {
    output
        .iter()
        .map(|entry| match &entry.node {
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

fn finish_reason_name(reason: &FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other => "other",
        FinishReason::ContextLimit => "context_limit",
        FinishReason::Paused => "paused",
        FinishReason::Compaction => "compaction",
    }
}

fn messages_stream_error_parts(
    data: &Value,
) -> (
    Option<String>,
    String,
    HashMap<String, Value>,
    StreamTerminalError,
) {
    let error = data.get("error").unwrap_or(data);
    let member = |key: &str| {
        error
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| data.get(key).and_then(Value::as_str))
    };
    let code = error
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| member("code"))
        .map(str::to_string);
    let message = member("message")
        .unwrap_or_else(|| data.as_str().unwrap_or("upstream error"))
        .to_string();
    let param = member("param").map(str::to_string);
    let http_status = error
        .get("status")
        .and_then(Value::as_u64)
        .or_else(|| data.get("status").and_then(Value::as_u64))
        .filter(|status| (400..=599).contains(status))
        .and_then(|status| u16::try_from(status).ok())
        .unwrap_or(StatusCode::BAD_REQUEST.as_u16());
    let mut extra_body = object_without_keys(error, &["type", "code", "message", "param"]);
    extra_body.extend(object_without_keys(
        data,
        &["type", "error", "code", "message", "param"],
    ));
    if let Some(value) = error.get("param").or_else(|| data.get("param")) {
        extra_body.insert("param".into(), value.clone());
    }
    let terminal_error = StreamTerminalError {
        code: code
            .clone()
            .unwrap_or_else(|| "upstream_stream_error".to_string()),
        message: message.clone(),
        http_status,
        error_type: code.clone(),
        param,
    };
    (code, message, extra_body, terminal_error)
}
