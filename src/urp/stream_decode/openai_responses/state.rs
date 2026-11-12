//! The decoder state and event delivery.

use super::errors::native_error;
use super::gate::OutputOrderGate;
use super::item::OutputItem;
use crate::handlers::usage::{record_stream_terminal_error, record_visible_stream_event_delta};
use crate::handlers::{StreamRuntimeMetrics, StreamTerminalError, UrpRequest as HandlerUrpRequest};
use crate::urp::{Node, NodeHeader, UrpStreamEvent};
use axum::http::StatusCode;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};

/// What the frame loop does after one frame.
pub(super) enum Flow {
    Continue,
    /// The stream ended: a terminal response went out, or the upstream sent `[DONE]`.
    End,
    /// A terminal error went out; nothing follows.
    Stop,
}

/// Cross-event state of one upstream Responses stream. Node indices allocated here are raw;
/// the output order gate turns them into canonical indices at release (PR5b).
pub(super) struct ResponsesStreamDecoder {
    tx: mpsc::Sender<UrpStreamEvent>,
    pub(super) metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    pub(super) started_at: Option<Instant>,
    /// Model of the downstream request, used when the upstream names none.
    pub(super) request_model: String,
    /// Envelope members of the downstream request for the first upstream message item.
    pub(super) pending_request_envelope: Option<HashMap<String, Value>>,
    pub(super) response_id: String,
    pub(super) start_sent: bool,
    /// A `discard_partial_items` interrupt stands for this turn (PR6c).
    pub(super) discard_requested: bool,
    pub(super) items: BTreeMap<u64, OutputItem>,
    /// Raw index and id of every image from an `image_generation.completed` event; such an
    /// image belongs to no output item.
    pub(super) standalone_images: Vec<(u32, Option<String>)>,
    next_node_index: u32,
    gate: OutputOrderGate,
    /// Name of the upstream terminal event whose `ResponseDone` went out.
    pub(super) terminal_event: Option<String>,
}

impl ResponsesStreamDecoder {
    pub(super) fn new(
        urp: &HandlerUrpRequest,
        pending_request_envelope: Option<HashMap<String, Value>>,
        tx: mpsc::Sender<UrpStreamEvent>,
        started_at: Option<Instant>,
        metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    ) -> Self {
        Self {
            tx,
            metrics,
            started_at,
            request_model: urp.model.clone(),
            pending_request_envelope,
            response_id: format!("resp_{}", uuid::Uuid::new_v4()),
            start_sent: false,
            discard_requested: false,
            items: BTreeMap::new(),
            standalone_images: Vec::new(),
            next_node_index: 0,
            gate: OutputOrderGate::default(),
            terminal_event: None,
        }
    }

    pub(super) fn item(&mut self, output_index: u64) -> &mut OutputItem {
        self.items.entry(output_index).or_default()
    }

    pub(super) fn allocate(&mut self) -> u32 {
        let node_index = self.next_node_index;
        self.next_node_index += 1;
        node_index
    }

    /// Announces the envelope control node of an item once, before the item's first node,
    /// when the item has envelope members.
    fn push_control(&mut self, output_index: u64, events: &mut Vec<UrpStreamEvent>) {
        let item = self.items.entry(output_index).or_default();
        if item.control.is_some() {
            return;
        }
        let mut extra_body = item.extra.clone();
        if item.is_reasoning()
            && !extra_body.contains_key("id")
            && let Some(id) = &item.id
        {
            extra_body.insert("id".to_string(), Value::String(id.clone()));
        }
        if extra_body.is_empty() {
            return;
        }
        let node_index = self.next_node_index;
        self.next_node_index += 1;
        item.control = Some(node_index);
        events.push(UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            extra_body: extra_body.clone(),
        });
        events.push(UrpStreamEvent::NodeDone {
            node_index,
            node: Node::NextDownstreamEnvelopeExtra {
                extra_body: extra_body.clone(),
            },
            usage: None,
            extra_body,
        });
    }

    /// The raw index of an item's own node, allocated on first use. Image-generation progress
    /// events may use it before the item starts, since encoders forward them from their own
    /// members.
    pub(super) fn item_node_index(&mut self, output_index: u64) -> u32 {
        if let Some(node_index) = self
            .items
            .get(&output_index)
            .and_then(|item| item.node_index)
        {
            return node_index;
        }
        let node_index = self.allocate();
        self.item(output_index).node_index = Some(node_index);
        node_index
    }

    /// Starts the node of a non-message item.
    pub(super) fn start_item_node(
        &mut self,
        output_index: u64,
        header: NodeHeader,
        extra_body: HashMap<String, Value>,
        events: &mut Vec<UrpStreamEvent>,
    ) -> u32 {
        self.push_control(output_index, events);
        let node_index = self.item_node_index(output_index);
        self.item(output_index).started = true;
        events.push(UrpStreamEvent::NodeStart {
            node_index,
            header,
            extra_body,
        });
        node_index
    }

    /// Starts the node of one message content part.
    pub(super) fn start_content_node(
        &mut self,
        output_index: u64,
        content_index: u64,
        header: NodeHeader,
        extra_body: HashMap<String, Value>,
        events: &mut Vec<UrpStreamEvent>,
    ) -> u32 {
        self.push_control(output_index, events);
        let node_index = self.allocate();
        self.item(output_index)
            .content
            .entry(content_index)
            .or_default()
            .node_index = Some(node_index);
        events.push(UrpStreamEvent::NodeStart {
            node_index,
            header,
            extra_body,
        });
        node_index
    }

    /// Sends an event outside the node lifecycle (start, controls, errors) as is.
    pub(super) async fn send(&self, event: UrpStreamEvent) {
        let _ = self.tx.send(event).await;
    }

    /// Sends released events with canonical node indices.
    pub(super) async fn send_all(&mut self, events: Vec<UrpStreamEvent>) {
        for mut event in events {
            self.gate.canonicalize(&mut event);
            record_visible_stream_event_delta(&self.metrics, &event).await;
            let _ = self.tx.send(event).await;
        }
    }

    pub(super) fn announce(&mut self, output_index: u64) {
        self.gate.announce(output_index);
    }

    /// Sends the events of one upstream event, or holds them behind a lower open item.
    pub(super) async fn route(&mut self, output_index: Option<u64>, events: Vec<UrpStreamEvent>) {
        match output_index {
            Some(output_index) if self.gate.blocked(output_index) => {
                self.gate.hold(output_index, events)
            }
            _ => self.send_all(events).await,
        }
    }

    pub(super) async fn complete(&mut self, output_index: u64) {
        let released = self.gate.complete(output_index);
        self.send_all(released).await;
    }

    /// Releases every held item in `output_index` order (PR5b). An item still open while a
    /// later item follows closes first. `terminal` holds the terminal `output` items at the
    /// terminal boundary and is `None` on a failure path.
    pub(super) async fn release_held(
        &mut self,
        terminal: Option<&[Value]>,
        retained: Option<&BTreeSet<u64>>,
    ) {
        for released in self.gate.drain(retained) {
            self.send_all(released.events).await;
            if released.close_before_next {
                let events = self.close_open_item(released.output_index, terminal);
                self.send_all(events).await;
            }
        }
    }

    /// Ends the stream with an upstream protocol violation.
    pub(super) async fn protocol_error(
        &mut self,
        code: &str,
        message: String,
        param: Option<&str>,
        extra_body: HashMap<String, Value>,
    ) -> Flow {
        self.send(UrpStreamEvent::Error {
            code: Some(code.to_string()),
            message: message.clone(),
            extra_body,
        })
        .await;
        let terminal = StreamTerminalError {
            code: code.to_string(),
            message,
            http_status: StatusCode::BAD_GATEWAY.as_u16(),
            error_type: Some("upstream_protocol_error".to_string()),
            param: param.map(str::to_string),
        };
        record_stream_terminal_error(&self.metrics, code, terminal).await;
        Flow::Stop
    }

    /// Ends the stream with a native error envelope; `label` names it in the metrics.
    pub(super) async fn fail_native(
        &mut self,
        label: &str,
        event_name: &str,
        data: &Value,
    ) -> Flow {
        self.release_held(None, None).await;
        let error = native_error(event_name, data);
        self.send(UrpStreamEvent::Error {
            code: error.code,
            message: error.message,
            extra_body: error.extra_body,
        })
        .await;
        record_stream_terminal_error(&self.metrics, label, error.terminal).await;
        Flow::Stop
    }
}
