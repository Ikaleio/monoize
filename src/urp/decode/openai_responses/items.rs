use super::content::{
    content_object_node, decode_tool_result_content, is_content_part_type, push_content_nodes,
};
use crate::urp::decode::split_extra;
use crate::urp::{
    ImageGenerationMetadata, ImageSource, MediaMetadata, Node, OrdinaryRole, ProviderProtocol,
    RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, RESPONSES_INSTRUCTION_NODE_EXTRA_KEY,
    ReasoningMetadata, ToolCallType,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Which side of the protocol an item comes from. Request input and response output share item
/// shapes but differ in role defaults, `call_id` fallbacks, and synthesized identities.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ItemSource {
    RequestInput,
    ResponseOutput,
}

fn string_member(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_string)
}

fn role_member(obj: &Map<String, Value>) -> Option<&str> {
    obj.get("role").and_then(Value::as_str)
}

/// Role of a message item: unknown request roles (including `tool`) default to user; response
/// messages default to assistant unless they name user or tool.
fn message_role(obj: &Map<String, Value>, source: ItemSource) -> OrdinaryRole {
    match (role_member(obj), source) {
        (Some("system"), _) => OrdinaryRole::System,
        (Some("developer"), _) => OrdinaryRole::Developer,
        (Some("assistant"), _) => OrdinaryRole::Assistant,
        (_, ItemSource::RequestInput) => OrdinaryRole::User,
        (Some("user" | "tool"), ItemSource::ResponseOutput) => OrdinaryRole::User,
        (_, ItemSource::ResponseOutput) => OrdinaryRole::Assistant,
    }
}

/// Role of a bare content-part item or an unmodeled provider item.
fn standalone_item_role(obj: &Map<String, Value>, default: OrdinaryRole) -> OrdinaryRole {
    match role_member(obj) {
        Some("system") => OrdinaryRole::System,
        Some("developer") => OrdinaryRole::Developer,
        Some("user") => OrdinaryRole::User,
        Some("assistant") => OrdinaryRole::Assistant,
        _ => default,
    }
}

/// Decodes the content of a message item with the item id on its first node.
fn message_content_nodes(obj: &Map<String, Value>, role: OrdinaryRole) -> Vec<Node> {
    let mut nodes = Vec::new();
    if let Some(content) = obj.get("content") {
        push_content_nodes(
            &mut nodes,
            content,
            role,
            obj.get("phase").and_then(Value::as_str),
        );
    }
    if let (Some(first), Some(id)) = (nodes.first_mut(), string_member(obj, "id")) {
        first.set_id(Some(id));
    }
    nodes
}

/// Unknown message members travel on a control node before the message's first node.
fn push_message_item(out: &mut Vec<Node>, obj: &Map<String, Value>, role: OrdinaryRole) {
    let nodes = message_content_nodes(obj, role);
    let extra_body = split_extra(obj, &["type", "id", "role", "content", "phase"]);
    if !extra_body.is_empty() && !nodes.is_empty() {
        out.push(Node::NextDownstreamEnvelopeExtra { extra_body });
    }
    out.extend(nodes);
}

fn decode_tool_call(obj: &Map<String, Value>, item_type: &str, source: ItemSource) -> Node {
    let tool_type = if item_type == "custom_tool_call" {
        ToolCallType::Custom
    } else {
        ToolCallType::Function
    };
    let call_id = match source {
        ItemSource::RequestInput => obj.get("call_id").or_else(|| obj.get("id")),
        ItemSource::ResponseOutput => obj.get("call_id"),
    }
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string();
    let arguments = match obj.get(if tool_type == ToolCallType::Custom {
        "input"
    } else {
        "arguments"
    }) {
        None => "{}".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(value) => value.to_string(),
    };
    Node::ToolCall {
        namespace: string_member(obj, "namespace"),
        signature: None,
        id: string_member(obj, "id"),
        tool_type,
        call_id,
        name: string_member(obj, "name").unwrap_or_default(),
        arguments,
        extra_body: split_extra(
            obj,
            &[
                "type",
                "id",
                "call_id",
                "name",
                "namespace",
                "arguments",
                "input",
            ],
        ),
        cache_breakpoint: None,
    }
}

fn decode_tool_result(obj: &Map<String, Value>, item_type: &str, source: ItemSource) -> Node {
    let tool_type = if item_type == "custom_tool_call_output" {
        ToolCallType::Custom
    } else {
        ToolCallType::Function
    };
    let call_id = match source {
        ItemSource::RequestInput => obj.get("call_id"),
        ItemSource::ResponseOutput => obj.get("call_id").or_else(|| obj.get("id")),
    }
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string();
    Node::ToolResult {
        signature: None,
        namespace: string_member(obj, "namespace"),
        name: string_member(obj, "name"),
        id: string_member(obj, "id"),
        tool_type,
        call_id,
        is_error: false,
        content: obj
            .get("output")
            .map(decode_tool_result_content)
            .unwrap_or_default(),
        extra_body: split_extra(
            obj,
            &["type", "id", "call_id", "namespace", "name", "output"],
        ),
        cache_breakpoint: None,
    }
}

/// Concatenated `summary_text` parts; explicit parts with empty text decode to explicit empty
/// text (RTYPE-NEW-4a).
fn summary_to_text(summary: &[Value]) -> Option<String> {
    let texts = summary
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("summary_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>();
    (!texts.is_empty()).then(|| texts.concat())
}

/// Concatenated `reasoning_text` parts, with the same explicit-empty rule as summaries.
fn reasoning_content_to_text(obj: &Map<String, Value>) -> Option<String> {
    let texts = obj
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("reasoning_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>();
    (!texts.is_empty()).then(|| texts.concat())
}

/// Request reasoning items keep explicit empty `summary`/`content` members. Response reasoning
/// items get a synthetic id when absent, and an empty item with a native id is kept as
/// `native_empty` so its identity survives.
fn decode_reasoning(obj: &Map<String, Value>, source: ItemSource) -> Option<Node> {
    let from_response = source == ItemSource::ResponseOutput;
    let summary = obj
        .get("summary")
        .and_then(Value::as_array)
        .and_then(|parts| summary_to_text(parts));
    let content = reasoning_content_to_text(obj).or_else(|| {
        obj.get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    });
    let encrypted = obj.get("encrypted_content").cloned();
    let id = string_member(obj, "id");
    let native_empty = from_response
        && content.as_deref().is_none_or(str::is_empty)
        && summary.as_deref().is_none_or(str::is_empty)
        && encrypted
            .as_ref()
            .is_none_or(|value| value.is_null() || value.as_str().is_some_and(str::is_empty))
        && id.as_deref().is_some_and(|id| !id.is_empty());
    let keep = content.is_some()
        || summary.is_some()
        || encrypted.is_some()
        || native_empty
        || (!from_response && (obj.contains_key("summary") || obj.contains_key("content")));
    if !keep {
        return None;
    }
    Some(Node::Reasoning {
        metadata: ReasoningMetadata {
            summary_parts: obj
                .get("summary")
                .and_then(crate::urp::reasoning::text_part_shapes),
            content_parts: obj
                .get("content")
                .and_then(crate::urp::reasoning::text_part_shapes),
            native_empty,
            ..Default::default()
        },
        id: id.or_else(|| from_response.then(crate::urp::synthetic_reasoning_id)),
        content,
        encrypted,
        summary,
        source: obj
            .get("source")
            .and_then(Value::as_str)
            .filter(|source| !source.is_empty())
            .map(str::to_string),
        extra_body: split_extra(
            obj,
            &[
                "type",
                "id",
                "content",
                "encrypted_content",
                "summary",
                "text",
                "source",
            ],
        ),
    })
}

fn image_media_type_from_output_format(output_format: Option<&str>) -> &'static str {
    match output_format.unwrap_or("png") {
        "webp" => "image/webp",
        "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => "image/png",
    }
}

/// A completed `image_generation_call` with a non-empty `result` becomes an assistant image.
fn decode_image_generation_call(obj: &Map<String, Value>) -> Option<Node> {
    let result = obj.get("result")?.as_str()?.trim();
    if result.is_empty() {
        return None;
    }
    let mut extra_body = split_extra(obj, &["type", "id", "result", "output_format"]);
    extra_body.retain(|key, _| !ImageGenerationMetadata::KEYS.contains(&key.as_str()));
    extra_body.insert(
        RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
        Value::Object(Map::new()),
    );
    Some(Node::Image {
        metadata: MediaMetadata {
            image_generation: ImageGenerationMetadata::from_object(obj),
            ..Default::default()
        },
        id: string_member(obj, "id").or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        role: OrdinaryRole::Assistant,
        source: ImageSource::Base64 {
            media_type: image_media_type_from_output_format(
                obj.get("output_format").and_then(Value::as_str),
            )
            .to_string(),
            data: result.to_string(),
        },
        extra_body,
    })
}

fn provider_item(obj: &Map<String, Value>, item_type: &str, role: OrdinaryRole) -> Node {
    Node::ProviderItem {
        id: string_member(obj, "id").or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        origin_protocol: ProviderProtocol::Responses,
        role,
        item_type: item_type.to_string(),
        body: Value::Object(obj.clone()),
        extra_body: HashMap::new(),
    }
}

/// Decodes one Responses item object from request `input` or response `output`.
pub(super) fn push_item_nodes(out: &mut Vec<Node>, obj: &Map<String, Value>, source: ItemSource) {
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
    let is_request = source == ItemSource::RequestInput;
    match item_type {
        "function_call" | "custom_tool_call" => out.push(decode_tool_call(obj, item_type, source)),
        "function_call_output" | "custom_tool_call_output" => {
            out.push(decode_tool_result(obj, item_type, source))
        }
        "reasoning" => out.extend(decode_reasoning(obj, source)),
        "message" => push_message_item(out, obj, message_role(obj, source)),
        "" if is_request => push_message_item(out, obj, message_role(obj, source)),
        "image_generation_call" if !is_request => out.push(
            decode_image_generation_call(obj)
                .unwrap_or_else(|| provider_item(obj, item_type, OrdinaryRole::Assistant)),
        ),
        kind if is_content_part_type(kind) => {
            // Request content-part items never default to assistant; response ones do.
            let role = match source {
                ItemSource::RequestInput => match role_member(obj) {
                    Some("system") => OrdinaryRole::System,
                    Some("developer") => OrdinaryRole::Developer,
                    Some("assistant") => OrdinaryRole::Assistant,
                    _ => OrdinaryRole::User,
                },
                ItemSource::ResponseOutput => standalone_item_role(obj, OrdinaryRole::Assistant),
            };
            out.push(content_object_node(obj, role, None));
        }
        _ => {
            let role = match source {
                ItemSource::RequestInput => standalone_item_role(obj, OrdinaryRole::Assistant),
                ItemSource::ResponseOutput => OrdinaryRole::Assistant,
            };
            out.push(provider_item(obj, item_type, role));
        }
    }
}

pub(super) fn decode_input_nodes(input: &Value, out: &mut Vec<Node>) {
    let items = match input {
        Value::Array(items) => items.as_slice(),
        _ => std::slice::from_ref(input),
    };
    for item in items {
        match item {
            Value::String(text) => out.push(Node::text(OrdinaryRole::User, text.as_str())),
            Value::Object(obj) => push_item_nodes(out, obj, ItemSource::RequestInput),
            _ => {}
        }
    }
}

fn mark_instruction(mut node: Node) -> Node {
    node.extra_body_mut().insert(
        RESPONSES_INSTRUCTION_NODE_EXTRA_KEY.to_string(),
        Value::Bool(true),
    );
    node
}

/// Decodes top-level `instructions`: a string, or an array of strings, messages, and bare
/// content parts. Instruction messages default to developer and drop their envelope members.
pub(super) fn decode_instruction_nodes(instructions: &Value, out: &mut Vec<Node>) {
    let items = match instructions {
        Value::String(_) => std::slice::from_ref(instructions),
        Value::Array(items) => items.as_slice(),
        _ => return,
    };
    for item in items {
        match item {
            Value::String(text) if !text.is_empty() => {
                out.push(mark_instruction(Node::text(
                    OrdinaryRole::Developer,
                    text.as_str(),
                )));
            }
            Value::Object(obj) => {
                let item_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(item_type, "" | "message") && obj.contains_key("content") {
                    let role = match role_member(obj) {
                        Some("system") => OrdinaryRole::System,
                        Some("user") => OrdinaryRole::User,
                        Some("assistant") => OrdinaryRole::Assistant,
                        _ => OrdinaryRole::Developer,
                    };
                    out.extend(
                        message_content_nodes(obj, role)
                            .into_iter()
                            .map(mark_instruction),
                    );
                } else if is_content_part_type(item_type) {
                    out.push(mark_instruction(content_object_node(
                        obj,
                        OrdinaryRole::Developer,
                        None,
                    )));
                }
            }
            _ => {}
        }
    }
}
