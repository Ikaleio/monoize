//! Node producers and wire-member helpers for Responses stream values.

use crate::urp::decode::openai_responses::decode_response;
use crate::urp::decode::{parse_compatible_media_node, split_extra};
use crate::urp::stream_helpers::extract_reasoning_parts;
use crate::urp::{
    ImageGenerationMetadata, ImageSource, MediaMetadata, Node, NodeHeader, OrdinaryRole,
    ProviderProtocol, RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, ReasoningMetadata,
};
use serde_json::{Value, json};
use std::collections::HashMap;

/// The wire members of a JSON object except `known` and internal keys.
pub(super) fn wire_extra(value: &Value, known: &[&str]) -> HashMap<String, Value> {
    value
        .as_object()
        .map_or_else(HashMap::new, |obj| split_extra(obj, known))
}

pub(super) fn string_member(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(super) fn node_extra(node: &Node) -> &HashMap<String, Value> {
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
}

/// Every node of one output item, envelope control nodes included.
pub(super) fn decode_item(item: &Value) -> Vec<Node> {
    decode_response(&json!({ "output": [item] }))
        .map(|response| response.output)
        .unwrap_or_default()
}

/// The typed nodes of one output item.
pub(super) fn item_nodes(item: &Value) -> Vec<Node> {
    let mut nodes = decode_item(item);
    nodes.retain(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }));
    nodes
}

pub(super) fn first_item_node(item: &Value) -> Option<Node> {
    item_nodes(item).into_iter().next()
}

/// The content indices of a message item's decoded nodes: every string or object entry of
/// `content` decodes to one node in order; other entries decode to nothing.
pub(super) fn content_indices(item: &Value) -> Vec<u64> {
    match item.get("content") {
        Some(Value::Array(parts)) => (0u64..)
            .zip(parts)
            .filter(|(_, part)| part.is_string() || part.is_object())
            .map(|(index, _)| index)
            .collect(),
        Some(Value::String(_) | Value::Object(_)) => vec![0],
        _ => Vec::new(),
    }
}

/// Unknown item-level members of an item kind whose envelope the stream re-encodes.
pub(super) fn item_extra(item: &Value) -> HashMap<String, Value> {
    if !matches!(
        item.get("type").and_then(Value::as_str),
        Some(
            "message"
                | "reasoning"
                | "function_call"
                | "custom_tool_call"
                | "function_call_output"
                | "custom_tool_call_output"
                | "image_generation_call"
        )
    ) {
        return HashMap::new();
    }
    let mut extra = wire_extra(
        item,
        &[
            "type",
            "role",
            "content",
            "call_id",
            "id",
            "phase",
            "output",
            "name",
            "arguments",
            "namespace",
            "input",
            "result",
            "output_format",
        ],
    );
    mark_image_generation_call(item, &mut extra);
    extra
}

/// Unknown members of a part or node-level item value.
pub(super) fn part_extra(part: &Value) -> HashMap<String, Value> {
    if let Some(obj) = part.as_object()
        && let Ok(Some(mut media)) = parse_compatible_media_node(obj, OrdinaryRole::Assistant)
    {
        return std::mem::take(media.extra_body_mut());
    }
    if !matches!(
        part.get("type").and_then(Value::as_str),
        Some(
            "input_text"
                | "output_text"
                | "text"
                | "refusal"
                | "reasoning_text"
                | "reasoning"
                | "function_call"
                | "custom_tool_call"
                | "function_call_output"
                | "custom_tool_call_output"
                | "image_generation_call"
                | "input_image"
                | "output_image"
                | "input_file"
                | "output_file"
        )
    ) {
        return HashMap::new();
    }
    let mut extra = wire_extra(
        part,
        &[
            "type",
            "id",
            "content",
            "text",
            "summary",
            "refusal",
            "annotations",
            "logprobs",
            "call_id",
            "name",
            "arguments",
            "namespace",
            "input",
            "result",
            "output_format",
            "source",
            "encrypted_content",
        ],
    );
    mark_image_generation_call(part, &mut extra);
    extra
}

fn mark_image_generation_call(value: &Value, extra: &mut HashMap<String, Value>) {
    if matches!(
        value.get("type").and_then(Value::as_str),
        Some(
            "image_generation_call"
                | "image_generation.completed"
                | "response.image_generation.completed"
        )
    ) {
        extra.insert(
            RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
            json!({}),
        );
    }
}

fn image_media_type(output_format: Option<&str>) -> &'static str {
    match output_format.unwrap_or("png") {
        "webp" => "image/webp",
        "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => "image/png",
    }
}

/// The assistant image of an image-generation item or event payload with non-empty bytes.
pub(super) fn image_node(payload: &Value) -> Option<Node> {
    let data = payload
        .get("b64_json")
        .or_else(|| payload.get("result"))
        .or_else(|| payload.get("partial_image_b64"))
        .and_then(Value::as_str)?
        .trim();
    if data.is_empty() {
        return None;
    }
    let mut extra_body = wire_extra(
        payload,
        &[
            "type",
            "id",
            "b64_json",
            "result",
            "partial_image_b64",
            "output_format",
            "partial_image_index",
            "sequence_number",
        ],
    );
    mark_image_generation_call(payload, &mut extra_body);
    extra_body.retain(|key, _| !ImageGenerationMetadata::KEYS.contains(&key.as_str()));
    Some(Node::Image {
        metadata: MediaMetadata {
            image_generation: ImageGenerationMetadata::from_object(payload.as_object()?),
            ..Default::default()
        },
        id: string_member(payload, "id").or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        role: OrdinaryRole::Assistant,
        source: ImageSource::Base64 {
            media_type: image_media_type(payload.get("output_format").and_then(Value::as_str))
                .to_string(),
            data: data.to_string(),
        },
        extra_body,
    })
}

/// One message or reasoning content part as a node of `role` carrying `id`.
pub(super) fn part_node(part: &Value, role: OrdinaryRole, id: Option<String>) -> Node {
    let mut node = part_node_without_id(part, role);
    node.set_id(id);
    node
}

fn part_node_without_id(part: &Value, role: OrdinaryRole) -> Node {
    if let Some(obj) = part.as_object()
        && let Ok(Some(node)) = parse_compatible_media_node(obj, role)
    {
        return node;
    }
    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
    match part_type {
        "input_text" | "output_text" | "text" => {
            let mut extra_body = part_extra(part);
            let phase = extra_body
                .remove("phase")
                .and_then(|value| value.as_str().map(str::to_string));
            Node::Text {
                logprobs: crate::urp::logprobs::decode(part.get("logprobs")),
                citations: crate::urp::citations::decode(
                    part.get("annotations")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    ProviderProtocol::Responses,
                ),
                signature: None,
                id: None,
                role,
                content: string_member(part, "text").unwrap_or_default(),
                phase,
                cache_breakpoint: None,
                extra_body,
            }
        }
        "reasoning" => {
            let (content, _, _) = extract_reasoning_parts(part);
            let summary = part
                .get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("summary_text"))
                .filter_map(|entry| entry.get("text").and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            Node::Reasoning {
                metadata: ReasoningMetadata::default(),
                id: None,
                content: (!content.is_empty()).then_some(content),
                encrypted: part.get("encrypted_content").cloned(),
                summary: (!summary.is_empty()).then_some(summary),
                source: string_member(part, "source"),
                extra_body: part_extra(part),
            }
        }
        "reasoning_text" => Node::Reasoning {
            metadata: ReasoningMetadata::default(),
            id: None,
            content: string_member(part, "text").filter(|text| !text.is_empty()),
            encrypted: None,
            summary: None,
            source: string_member(part, "source").filter(|source| !source.is_empty()),
            extra_body: part_extra(part),
        },
        "refusal" => Node::Refusal {
            logprobs: None,
            id: None,
            content: string_member(part, "refusal")
                .or_else(|| string_member(part, "text"))
                .unwrap_or_default(),
            extra_body: part_extra(part),
        },
        "image_generation_call" => match image_node(part) {
            Some(Node::Image {
                metadata,
                source,
                extra_body,
                ..
            }) => Node::Image {
                metadata,
                id: None,
                role,
                source,
                extra_body,
            },
            _ => provider_part(part, role, part_type),
        },
        other => provider_part(part, role, other),
    }
}

fn provider_part(part: &Value, role: OrdinaryRole, item_type: &str) -> Node {
    Node::ProviderItem {
        id: None,
        origin_protocol: ProviderProtocol::Responses,
        role,
        item_type: item_type.to_string(),
        body: part.clone(),
        extra_body: HashMap::new(),
    }
}

/// The `NodeStart` header of a live node: its identity members plus the citations and
/// signatures that the native start event already carried.
pub(super) fn start_header(node: &Node) -> NodeHeader {
    let mut header = NodeHeader::from(node);
    match (&mut header, node) {
        (
            NodeHeader::Text {
                citations,
                signature,
                ..
            },
            Node::Text {
                citations: node_citations,
                signature: node_signature,
                ..
            },
        ) => {
            citations.clone_from(node_citations);
            signature.clone_from(node_signature);
        }
        (
            NodeHeader::ToolCall { signature, .. },
            Node::ToolCall {
                signature: node_signature,
                ..
            },
        ) => signature.clone_from(node_signature),
        _ => {}
    }
    header
}
