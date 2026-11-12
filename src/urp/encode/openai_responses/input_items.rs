//! Request `input[]` items and the request `instructions` carried by input nodes.

use super::items::{ItemTarget, encode_envelope};
use super::reasoning::sanitize_reasoning_request_item;
use crate::urp::encode::text_from_nodes;
use crate::urp::grouping::{Envelope, envelopes};
use crate::urp::{Node, OrdinaryRole, RESPONSES_INSTRUCTION_NODE_EXTRA_KEY};
use serde_json::Value;
use std::collections::HashMap;

/// Whether `node` was decoded from a top-level Responses `instructions` member.
pub(super) fn is_retained_instruction_node(node: &Node) -> bool {
    let extra_body = match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. }
        | Node::ToolResult { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body,
    };
    extra_body
        .get(RESPONSES_INSTRUCTION_NODE_EXTRA_KEY)
        .and_then(Value::as_bool)
        == Some(true)
}

/// Encodes the retained instruction nodes of `input` as structured `instructions` items.
pub(super) fn encode_instruction_items(input: &[Node]) -> Vec<Value> {
    let mut items = Vec::new();
    // Each contiguous instruction run is grouped on its own, without copying nodes.
    for run in input.split(|node| !is_retained_instruction_node(node)) {
        for envelope in envelopes(run) {
            encode_envelope(envelope, ItemTarget::Input, &mut items);
        }
    }
    items
}

/// Encodes request `input[]`. `skip_instruction_nodes` drops retained instruction nodes, which
/// then own `instructions`. While `instructions` is `None`, the first message it can carry
/// becomes `instructions` instead of an input item.
pub(super) fn encode_input_items(
    input: &[Node],
    skip_instruction_nodes: bool,
    instructions: &mut Option<Value>,
) -> Vec<Value> {
    let mut items = Vec::new();
    for run in input.split(|node| skip_instruction_nodes && is_retained_instruction_node(node)) {
        for envelope in envelopes(run) {
            if instructions.is_none()
                && let Envelope::Message {
                    role, nodes, extra, ..
                } = &envelope
                && can_use_responses_instructions(*role, nodes, extra)
            {
                let text = text_from_nodes(nodes);
                if !text.is_empty() {
                    *instructions = Some(Value::String(text));
                    continue;
                }
            }
            encode_envelope(envelope, ItemTarget::Input, &mut items);
        }
    }
    for item in &mut items {
        sanitize_request_input_item(item);
    }
    items
}

fn can_use_responses_instructions(
    role: OrdinaryRole,
    nodes: &[Node],
    extra: &HashMap<String, Value>,
) -> bool {
    matches!(role, OrdinaryRole::System | OrdinaryRole::Developer)
        && extra.is_empty()
        && nodes.iter().all(|node| {
            // A stray `phase` extra never reaches the wire, so it does not block the lift.
            matches!(
                node,
                Node::Text { phase: None, extra_body, .. }
                    if extra_body.keys().all(|key| key == "phase")
            )
        })
}

/// Removes output-only members that a Responses request rejects on replayed items.
fn sanitize_request_input_item(item: &mut Value) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    match obj.get("type").and_then(Value::as_str) {
        Some("reasoning") => sanitize_reasoning_request_item(obj),
        Some("message") => {
            obj.remove("status");
            obj.remove("annotations");
            obj.remove("logprobs");
            if obj.get("phase").and_then(Value::as_str) == Some("analysis") {
                obj.remove("phase");
            }
            if let Some(Value::Array(content)) = obj.get_mut("content") {
                for part in content {
                    let Some(part_obj) = part.as_object_mut() else {
                        continue;
                    };
                    let part_type = part_obj.get("type").and_then(Value::as_str);
                    if matches!(part_type, Some("input_text" | "text")) {
                        part_obj.remove("annotations");
                        part_obj.remove("logprobs");
                    }
                    if part_obj.get("phase").and_then(Value::as_str) == Some("analysis") {
                        part_obj.remove("phase");
                    }
                }
            }
        }
        Some("custom_tool_call") => {
            obj.remove("status");
        }
        _ => {}
    }
}
