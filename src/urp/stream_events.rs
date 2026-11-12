//! STR-14 canonical replay of a complete response as stream events.

use super::{Node, NodeHeader, TerminalNode, UrpResponse, UrpStreamEvent, is_internal_key};
use serde_json::Value;
use std::collections::HashMap;

impl From<&Node> for NodeHeader {
    /// The identity members of `node`. Payload members (text, citations, call signatures,
    /// arguments, reasoning payload, media sources) arrive only with `NodeDone`.
    fn from(node: &Node) -> Self {
        match node {
            Node::Text {
                id, role, phase, ..
            } => NodeHeader::Text {
                citations: Vec::new(),
                signature: None,
                id: id.clone(),
                role: *role,
                phase: phase.clone(),
            },
            Node::Image {
                metadata, id, role, ..
            } => NodeHeader::Image {
                metadata: metadata.clone(),
                id: id.clone(),
                role: *role,
            },
            Node::Audio {
                metadata, id, role, ..
            } => NodeHeader::Audio {
                metadata: metadata.clone(),
                id: id.clone(),
                role: *role,
            },
            Node::File {
                metadata, id, role, ..
            } => NodeHeader::File {
                metadata: metadata.clone(),
                id: id.clone(),
                role: *role,
            },
            Node::Refusal { id, .. } => NodeHeader::Refusal { id: id.clone() },
            Node::Reasoning { metadata, id, .. } => NodeHeader::Reasoning {
                metadata: metadata.clone(),
                id: id.clone(),
            },
            Node::ToolCall {
                namespace,
                id,
                tool_type,
                call_id,
                name,
                ..
            } => NodeHeader::ToolCall {
                namespace: namespace.clone(),
                signature: None,
                id: id.clone(),
                tool_type: *tool_type,
                call_id: call_id.clone(),
                name: name.clone(),
            },
            Node::ProviderItem {
                id,
                origin_protocol,
                role,
                item_type,
                body,
                ..
            } => NodeHeader::ProviderItem {
                body: Some(body.clone()),
                id: id.clone(),
                origin_protocol: *origin_protocol,
                role: *role,
                item_type: item_type.clone(),
            },
            Node::ToolResult {
                signature,
                namespace,
                name,
                id,
                tool_type,
                call_id,
                ..
            } => NodeHeader::ToolResult {
                signature: signature.clone(),
                namespace: namespace.clone(),
                name: name.clone(),
                id: id.clone(),
                tool_type: *tool_type,
                call_id: call_id.clone(),
            },
            Node::NextDownstreamEnvelopeExtra { .. } => NodeHeader::NextDownstreamEnvelopeExtra,
        }
    }
}

/// The STR-14 replay of `response`. `ResponseStart` and `ResponseDone` both carry the
/// response top-level `extra_body`, because each opens or closes its own wire envelope.
pub fn events_from_response(response: UrpResponse) -> Vec<UrpStreamEvent> {
    let UrpResponse {
        outcome,
        id,
        model,
        created_at,
        output,
        finish_reason,
        usage,
        extra_body,
    } = response;
    let mut events = Vec::with_capacity(output.len() * 2 + 2);
    events.push(UrpStreamEvent::ResponseStart {
        usage: None,
        id,
        model,
        created_at,
        extra_body: extra_body.clone(),
    });
    let mut terminal = Vec::with_capacity(output.len());
    for (node_index, node) in (0u32..).zip(output) {
        events.push(UrpStreamEvent::NodeStart {
            node_index,
            header: replay_header(&node),
            extra_body: internal_members(&node),
        });
        events.push(UrpStreamEvent::NodeDone {
            node_index,
            node: node.clone(),
            usage: None,
            extra_body: HashMap::new(),
        });
        terminal.push(TerminalNode::streamed(node_index, node));
    }
    events.push(UrpStreamEvent::ResponseDone {
        outcome,
        finish_reason,
        usage,
        output: terminal,
        extra_body,
    });
    events
}

/// `header(n)` of STR-14. A replay knows a call signature before the call starts, as a live
/// Gemini call does, so the signature transport precedes the call (PG5a1).
fn replay_header(node: &Node) -> NodeHeader {
    let mut header = NodeHeader::from(node);
    if let (
        NodeHeader::ToolCall { signature, .. },
        Node::ToolCall {
            signature: Some(value),
            ..
        },
    ) = (&mut header, node)
    {
        *signature = Some(value.clone());
    }
    header
}

/// The internal adapter members of a node, which live decoders also place on `NodeStart` so an
/// encoder can choose the native shape before the node completes (legacy calls, image
/// generation items, generated audio).
fn internal_members(node: &Node) -> HashMap<String, Value> {
    let (Node::Text { extra_body, .. }
    | Node::Image { extra_body, .. }
    | Node::Audio { extra_body, .. }
    | Node::File { extra_body, .. }
    | Node::Refusal { extra_body, .. }
    | Node::Reasoning { extra_body, .. }
    | Node::ToolCall { extra_body, .. }
    | Node::ProviderItem { extra_body, .. }
    | Node::ToolResult { extra_body, .. }
    | Node::NextDownstreamEnvelopeExtra { extra_body }) = node;
    extra_body
        .iter()
        .filter(|(key, _)| is_internal_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
