//! Downstream Anthropic Messages SSE encoder (MSG-1 through MSG-8, PM9 through PM12a, DM5a
//! through DM11).
//!
//! `MessagesStreamEncoder` owns every cross-event state of one downstream stream: the message
//! envelope, the wire state of each announced node that has a block, and the cursor that keeps
//! block order equal to node order.

mod blocks;
mod envelope;
mod lifecycle;
mod nodes;
mod wire;

use crate::error::AppResult;
use crate::payload::sse::WireEvent as Event;
use crate::urp::encode::anthropic::{
    input_only_media_type, messages_response_id, validate_complete_tool_inputs,
    validate_response_node, validate_response_nodes,
};
use crate::urp::stream_encode::{StreamEncodeOptions, StreamEncoder};
use crate::urp::tool_signature::SignatureProjection;
use crate::urp::{NodeDelta, NodeHeader, ProviderProtocol, UrpResponse, UrpStreamEvent, Usage};
use nodes::NodeState;
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc;
use wire::Wire;

pub(crate) struct MessagesStreamEncoder {
    wire: Wire,
    options: StreamEncodeOptions,
    /// RECON-8 replay option: a block opens only at its `NodeDone`, from the complete node,
    /// because a replayed `NodeStart` carries no wire members (STR-14). A replayed response
    /// without content still forms a message.
    replay: bool,
    /// `message_stop` or an `error` frame ended the stream; later events are dropped.
    terminated: bool,
    message_id: Option<String>,
    /// The latest usage snapshot (PM12a).
    usage: Option<Usage>,
    /// Wire members for `message_start` (CTL-5).
    envelope_extra: HashMap<String, Value>,
    message_started: bool,
    saw_tool_use: bool,
    saw_refusal: bool,
    /// The wire state of each node that has a Messages block.
    nodes: HashMap<u32, NodeState>,
    /// The node whose block comes next (DM9a). An open block always belongs to this node.
    next_node: u32,
    /// The wire index of the next block (PM9a).
    next_block_index: u32,
}

impl MessagesStreamEncoder {
    pub(crate) fn new(tx: mpsc::Sender<Event>, options: StreamEncodeOptions) -> Self {
        Self {
            wire: Wire::new(tx, options.sse_max_frame_length),
            options,
            replay: false,
            terminated: false,
            message_id: None,
            usage: None,
            envelope_extra: HashMap::new(),
            message_started: false,
            saw_tool_use: false,
            saw_refusal: false,
            nodes: HashMap::new(),
            next_node: 0,
            next_block_index: 0,
        }
    }
}

impl StreamEncoder for MessagesStreamEncoder {
    fn signature_projection() -> Option<SignatureProjection> {
        Some(SignatureProjection::for_messages())
    }

    /// RECON-8: a failed response replays as its native error alone. Output that a Messages
    /// response cannot represent ends the replay with the native error before `message_start`.
    async fn begin_replay(&mut self, response: &UrpResponse) -> AppResult<()> {
        self.replay = true;
        if let Some(body) = response
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.failure_body(true))
        {
            return self.emit_failure(body).await;
        }
        if let Err(message) = validate_response_nodes(&response.output)
            .and_then(|()| validate_complete_tool_inputs(&response.output))
        {
            return self.wire.media_error(message).await;
        }
        // A replayed `ResponseStart` has no usage, yet `message_start` reports it.
        self.usage.clone_from(&response.usage);
        Ok(())
    }

    async fn on_event(&mut self, event: UrpStreamEvent) -> AppResult<()> {
        if self.terminated {
            return Ok(());
        }
        if let Err(message) = validate_media_event(&event) {
            return self.wire.media_error(message).await;
        }
        match event {
            UrpStreamEvent::ResponseStart {
                usage,
                id,
                extra_body,
                ..
            } => {
                self.message_id = Some(messages_response_id(&id));
                if usage.is_some() {
                    self.usage = usage;
                }
                self.add_envelope_extra(&extra_body);
                Ok(())
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header,
                extra_body,
            } => self.on_node_start(node_index, header, extra_body).await,
            UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                usage,
                extra_body,
            } => {
                if usage.is_some() {
                    self.usage = usage;
                }
                self.on_node_delta(node_index, delta, extra_body).await
            }
            UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage,
                ..
            } => {
                if usage.is_some() {
                    self.usage = usage;
                }
                self.on_node_done(node_index, node).await
            }
            UrpStreamEvent::ResponseDone {
                outcome,
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                self.on_response_done(outcome, finish_reason, usage, output, extra_body)
                    .await
            }
            UrpStreamEvent::ProviderControl {
                protocol,
                event_name,
                ..
            } => {
                if protocol == "messages" && event_name != "ping" {
                    tracing::debug!(
                        protocol = %protocol,
                        event_name = %event_name,
                        "dropping unsupported messages provider-control stream event"
                    );
                }
                Ok(())
            }
            UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            } => self.on_error(code, message, extra_body).await,
        }
    }

    /// An input that ends without `ResponseDone` gets no synthesized terminal.
    async fn finish(&mut self) -> AppResult<()> {
        Ok(())
    }
}

/// Top-level media and tool results have no Messages response block; they end the stream
/// with the native error (MSG-8).
fn validate_media_event(event: &UrpStreamEvent) -> Result<(), String> {
    let kind = match event {
        UrpStreamEvent::NodeStart { header, .. } => match header {
            NodeHeader::Image { .. } => "image",
            NodeHeader::File { .. } => "document",
            NodeHeader::Audio { .. } => "audio",
            NodeHeader::ToolResult { .. } => "tool_result",
            NodeHeader::ProviderItem {
                origin_protocol: ProviderProtocol::Messages,
                item_type,
                ..
            } if input_only_media_type(item_type) => item_type.as_str(),
            _ => return Ok(()),
        },
        UrpStreamEvent::NodeDelta { delta, .. } => match delta {
            NodeDelta::Image { .. } => "image",
            NodeDelta::File { .. } => "document",
            NodeDelta::Audio { .. } => "audio",
            _ => return Ok(()),
        },
        UrpStreamEvent::NodeDone { node, .. } => return validate_response_node(node),
        UrpStreamEvent::ResponseDone { output, .. } => {
            return output
                .iter()
                .try_for_each(|entry| validate_response_node(&entry.node));
        }
        _ => return Ok(()),
    };
    Err(format!(
        "Messages responses cannot represent top-level {kind} content"
    ))
}
