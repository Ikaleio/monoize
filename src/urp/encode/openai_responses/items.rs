//! Responses items of one RECON-6 envelope, shared by request `input[]` and response `output[]`.

use super::image_generation::encode_image_generation_call_item;
use super::media::{encode_input_file, encode_input_image};
use super::reasoning::encode_reasoning_item;
use super::tool_items::{encode_tool_call_item, encode_tool_result_item};
use crate::urp::encode::{merge_extra, ordinary_role_to_str, sanitize_provider_item_wire_body};
use crate::urp::grouping::Envelope;
use crate::urp::{Node, OrdinaryRole, ProviderProtocol, RESPONSES_CONTENT_PART_SHAPE_KEY};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Which Responses item list an envelope is encoded into.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ItemTarget {
    /// A request `input[]` item: keeps only stable ids and uses easy-input shapes.
    Input,
    /// A response `output[]` item: every item carries a normalized id.
    Output,
}

impl ItemTarget {
    pub(super) fn is_output(self) -> bool {
        self == ItemTarget::Output
    }
}

pub(super) fn encode_envelope(envelope: Envelope<'_>, target: ItemTarget, out: &mut Vec<Value>) {
    match envelope {
        Envelope::Message {
            role,
            id,
            nodes,
            extra,
        } => encode_message_envelope(role, id, nodes, extra, target, out),
        Envelope::ToolResult { node, extra } => {
            encode_tool_result_item(node, &extra, target.is_output(), out)
        }
    }
}

fn encode_message_envelope(
    role: OrdinaryRole,
    id: Option<&str>,
    nodes: &[Node],
    mut extra: HashMap<String, Value>,
    target: ItemTarget,
    out: &mut Vec<Value>,
) {
    if let Some(id) = id {
        extra
            .entry("id".to_string())
            .or_insert_with(|| Value::String(id.to_string()));
    }
    let output_text = match target {
        ItemTarget::Output => true,
        ItemTarget::Input => {
            let has_media = nodes
                .iter()
                .any(|node| matches!(node, Node::Image { .. } | Node::File { .. }));
            if has_media {
                // Easy-input messages have no native output item identity or status.
                extra.remove("id");
                extra.remove("status");
            }
            role == OrdinaryRole::Assistant && !has_media
        }
    };

    let mut message = MessageItem::new(role, &extra, target.is_output());
    for node in nodes {
        if let Node::Image {
            id,
            source,
            metadata,
            extra_body,
            ..
        } = node
            && let Some(item) =
                encode_image_generation_call_item(id.as_deref(), source, metadata, extra_body)
        {
            message.flush(out);
            out.push(item);
            continue;
        }
        if let Some(part) = encode_content_part(node, output_text) {
            let phase = match node {
                Node::Text { phase, .. } => phase.as_deref(),
                _ => None,
            };
            message.push(phase, part, out);
            continue;
        }

        message.flush(out);
        let item = match node {
            Node::Reasoning { .. } => encode_reasoning_item(node, target),
            Node::ToolCall { .. } => encode_tool_call_item(node, target.is_output()),
            Node::ProviderItem {
                id,
                origin_protocol,
                item_type,
                body,
                extra_body,
                ..
            } => encode_provider_item_for_responses(
                *origin_protocol,
                item_type,
                body,
                extra_body,
                Some(id),
            ),
            _ => None,
        };
        out.extend(item);
    }
    message.flush(out);
}

/// The open `message` item of an envelope. A phase change among its content parts starts a
/// new item with the same envelope members.
struct MessageItem<'a> {
    role: OrdinaryRole,
    extra: &'a HashMap<String, Value>,
    output_item: bool,
    phase: Option<&'a str>,
    content: Vec<Value>,
}

impl<'a> MessageItem<'a> {
    fn new(role: OrdinaryRole, extra: &'a HashMap<String, Value>, output_item: bool) -> Self {
        Self {
            role,
            extra,
            output_item,
            phase: None,
            content: Vec::new(),
        }
    }

    fn push(&mut self, phase: Option<&'a str>, part: Value, out: &mut Vec<Value>) {
        if !self.content.is_empty() && self.phase != phase {
            self.flush(out);
        }
        if self.content.is_empty() {
            self.phase = phase;
        }
        self.content.push(part);
    }

    fn flush(&mut self, out: &mut Vec<Value>) {
        if self.content.is_empty() {
            return;
        }
        let mut obj = Map::new();
        obj.insert("type".to_string(), json!("message"));
        let id = self.extra.get("id").and_then(Value::as_str);
        if self.output_item {
            obj.insert("id".to_string(), Value::String(normalize_message_id(id)));
        } else if let Some(id) = id {
            obj.insert("id".to_string(), Value::String(id.to_string()));
        }
        obj.insert("role".to_string(), json!(ordinary_role_to_str(self.role)));
        obj.insert(
            "content".to_string(),
            Value::Array(std::mem::take(&mut self.content)),
        );
        if let Some(phase) = self.phase {
            obj.insert("phase".to_string(), json!(phase));
        }
        merge_extra(&mut obj, self.extra);
        out.push(Value::Object(obj));
    }
}

fn normalize_message_id(id: Option<&str>) -> String {
    match id {
        Some(existing) if existing.starts_with("msg_") => existing.to_string(),
        _ => format!("msg_{}", uuid::Uuid::new_v4().simple()),
    }
}

fn encode_content_part(node: &Node, output_text: bool) -> Option<Value> {
    match node {
        Node::Text {
            logprobs,
            citations,
            content,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            let part_type = if output_text {
                "output_text"
            } else {
                "input_text"
            };
            obj.insert("type".to_string(), json!(part_type));
            obj.insert("text".to_string(), Value::String(content.clone()));
            if output_text {
                obj.insert(
                    "annotations".to_string(),
                    Value::Array(crate::urp::citations::encode(
                        citations,
                        ProviderProtocol::Responses,
                        0,
                    )),
                );
                obj.insert(
                    "logprobs".to_string(),
                    crate::urp::logprobs::encode_openai(
                        crate::urp::logprobs::valid(logprobs, content).unwrap_or_default(),
                    ),
                );
            }
            merge_extra(&mut obj, extra_body);
            // Phase is lifted onto the message item; OpenAI rejects it on content parts.
            obj.remove("phase");
            Some(Value::Object(obj))
        }
        Node::Image {
            metadata,
            source,
            extra_body,
            ..
        } if !output_text => encode_input_image(source, metadata, extra_body),
        Node::File {
            metadata,
            source,
            extra_body,
            ..
        } if !output_text => encode_input_file(source, metadata, extra_body),
        Node::Refusal {
            content,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("refusal"));
            obj.insert("refusal".to_string(), Value::String(content.clone()));
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        Node::ProviderItem {
            id,
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } if extra_body.contains_key(RESPONSES_CONTENT_PART_SHAPE_KEY) => {
            encode_provider_item_for_responses(
                *origin_protocol,
                item_type,
                body,
                extra_body,
                Some(id),
            )
        }
        _ => None,
    }
}

/// A Responses-origin opaque item or content part. `id` is `Some` for a node-level item,
/// whose typed id replaces the body id only when the body carried one.
pub(crate) fn encode_provider_item_for_responses(
    origin_protocol: ProviderProtocol,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
    id: Option<&Option<String>>,
) -> Option<Value> {
    if origin_protocol != ProviderProtocol::Responses {
        return None;
    }
    let mut item = match sanitize_provider_item_wire_body(body) {
        Value::Object(obj) => obj,
        other => {
            let mut obj = Map::new();
            obj.insert("body".to_string(), other);
            obj
        }
    };
    item.insert("type".to_string(), Value::String(item_type.to_string()));
    if let Some(id) = id
        && item.remove("id").is_some()
        && let Some(id) = id
    {
        item.insert("id".to_string(), json!(id));
    }
    merge_extra(&mut item, extra_body);
    Some(Value::Object(item))
}
