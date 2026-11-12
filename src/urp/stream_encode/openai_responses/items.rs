//! Wire shapes of stream output items and message content parts.

use super::nodes::NodeKind;
use crate::urp::encode::openai_responses::{
    encode_image_generation_call_item, encode_provider_item_for_responses,
    encode_tool_result_output, merge_reasoning_extra,
};
use crate::urp::encode::{ordinary_role_to_str, sanitize_provider_item_wire_body};
use crate::urp::{
    Node, NodeHeader, ProviderProtocol, RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, ToolCallType,
    is_internal_key, tool_call_arguments_for_wire,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub(super) fn merge_json_extra(obj: &mut Map<String, Value>, extra: &HashMap<String, Value>) {
    for (key, value) in extra {
        if !is_internal_key(key) {
            obj.insert(key.clone(), value.clone());
        }
    }
}

/// Adds wire members of `extra` that `obj` does not own yet.
pub(super) fn merge_json_extra_preserving(
    obj: &mut Map<String, Value>,
    extra: &HashMap<String, Value>,
) {
    for (key, value) in extra {
        if !is_internal_key(key) && !obj.contains_key(key) {
            obj.insert(key.clone(), value.clone());
        }
    }
}

pub(super) fn merge_extra_preserving(
    dst: &mut HashMap<String, Value>,
    extra: &HashMap<String, Value>,
) {
    for (key, value) in extra {
        if !is_internal_key(key) && !dst.contains_key(key) {
            dst.insert(key.clone(), value.clone());
        }
    }
}

fn header_id(header: &NodeHeader) -> Option<&str> {
    match header {
        NodeHeader::Text { id, .. }
        | NodeHeader::Image { id, .. }
        | NodeHeader::Audio { id, .. }
        | NodeHeader::File { id, .. }
        | NodeHeader::Refusal { id }
        | NodeHeader::Reasoning { id, .. }
        | NodeHeader::ToolCall { id, .. }
        | NodeHeader::ProviderItem { id, .. }
        | NodeHeader::ToolResult { id, .. } => id.as_deref(),
        NodeHeader::NextDownstreamEnvelopeExtra => None,
    }
}

/// The item id of a stub: the header id, then a wire `id` member, then a fresh id.
fn stub_id(
    header: &NodeHeader,
    extra_body: &HashMap<String, Value>,
    envelope_extra: &HashMap<String, Value>,
    prefix: &str,
) -> String {
    header_id(header)
        .or_else(|| extra_body.get("id").and_then(Value::as_str))
        .or_else(|| envelope_extra.get("id").and_then(Value::as_str))
        .map_or_else(
            || format!("{prefix}_{}", uuid::Uuid::new_v4()),
            str::to_string,
        )
}

/// The `response.output_item.added` item of a node of `kind`.
pub(super) fn item_stub(
    kind: NodeKind,
    header: &NodeHeader,
    extra_body: &HashMap<String, Value>,
    envelope_extra: &HashMap<String, Value>,
) -> Value {
    let mut obj = Map::new();
    match kind {
        NodeKind::Part => {
            let role = match header {
                NodeHeader::Text { role, .. }
                | NodeHeader::Image { role, .. }
                | NodeHeader::Audio { role, .. }
                | NodeHeader::File { role, .. }
                | NodeHeader::ProviderItem { role, .. } => ordinary_role_to_str(*role),
                _ => "assistant",
            };
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!(role));
            obj.insert("content".to_string(), json!([]));
            let id = stub_id(header, extra_body, envelope_extra, "msg");
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("in_progress"));
            if let NodeHeader::Text {
                phase: Some(phase), ..
            } = header
            {
                obj.insert("phase".to_string(), json!(phase));
            }
        }
        NodeKind::Reasoning => {
            obj.insert("type".to_string(), json!("reasoning"));
            obj.insert("content".to_string(), json!([]));
            obj.insert("summary".to_string(), json!([]));
            let id = stub_id(header, extra_body, envelope_extra, "rs");
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("in_progress"));
            obj.insert(
                "started_at".to_string(),
                json!(chrono::Utc::now().timestamp()),
            );
        }
        NodeKind::Image => return image_generation_call_stub(header, extra_body, envelope_extra),
        NodeKind::ToolResult => return tool_result_stub(header, extra_body, envelope_extra),
        NodeKind::ToolCall => tool_call_stub(&mut obj, header, extra_body, envelope_extra),
        NodeKind::Provider => return provider_item_stub(header, extra_body, envelope_extra),
        NodeKind::Silent => return Value::Null,
    }
    merge_json_extra_preserving(&mut obj, envelope_extra);
    merge_json_extra(&mut obj, extra_body);
    Value::Object(obj)
}

fn image_generation_call_stub(
    header: &NodeHeader,
    extra_body: &HashMap<String, Value>,
    envelope_extra: &HashMap<String, Value>,
) -> Value {
    let mut obj = extra_body
        .get(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    obj.insert("type".to_string(), json!("image_generation_call"));
    obj.remove("result");
    let id = header_id(header)
        .or_else(|| obj.get("id").and_then(Value::as_str))
        .map_or_else(|| format!("ig_{}", uuid::Uuid::new_v4()), str::to_string);
    obj.insert("id".to_string(), json!(id));
    obj.insert("status".to_string(), json!("in_progress"));
    merge_json_extra_preserving(&mut obj, envelope_extra);
    for (key, value) in extra_body {
        if !is_internal_key(key) && key != "result" {
            obj.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if let NodeHeader::Image { metadata, .. } = header {
        metadata.image_generation.apply_to(&mut obj);
    }
    obj.retain(|key, _| !is_internal_key(key));
    Value::Object(obj)
}

fn tool_result_stub(
    header: &NodeHeader,
    extra_body: &HashMap<String, Value>,
    envelope_extra: &HashMap<String, Value>,
) -> Value {
    let NodeHeader::ToolResult {
        id,
        tool_type,
        call_id,
        name,
        namespace,
        ..
    } = header
    else {
        return Value::Null;
    };
    let mut obj = Map::new();
    merge_json_extra_preserving(&mut obj, envelope_extra);
    merge_json_extra(&mut obj, extra_body);
    let kind = match tool_type {
        ToolCallType::Custom => "custom_tool_call_output",
        ToolCallType::Function => "function_call_output",
    };
    obj.insert("type".into(), json!(kind));
    let id = id
        .clone()
        .unwrap_or_else(|| format!("fco_{}", uuid::Uuid::new_v4()));
    obj.insert("id".into(), json!(id));
    obj.insert("call_id".into(), json!(call_id));
    obj.insert("output".into(), json!(""));
    obj.insert("status".into(), json!("in_progress"));
    obj.remove("name");
    obj.remove("namespace");
    if let Some(name) = name {
        obj.insert("name".into(), json!(name));
    }
    if let Some(namespace) = namespace {
        obj.insert("namespace".into(), json!(namespace));
    }
    Value::Object(obj)
}

fn tool_call_stub(
    obj: &mut Map<String, Value>,
    header: &NodeHeader,
    extra_body: &HashMap<String, Value>,
    envelope_extra: &HashMap<String, Value>,
) {
    let NodeHeader::ToolCall {
        tool_type,
        call_id,
        name,
        namespace,
        ..
    } = header
    else {
        return;
    };
    let custom = *tool_type == ToolCallType::Custom;
    obj.insert(
        "type".to_string(),
        json!(if custom {
            "custom_tool_call"
        } else {
            "function_call"
        }),
    );
    obj.insert("call_id".to_string(), json!(call_id));
    obj.insert("name".to_string(), json!(name));
    if let Some(namespace) = namespace {
        obj.insert("namespace".to_string(), json!(namespace));
    }
    obj.insert(
        if custom { "input" } else { "arguments" }.to_string(),
        json!(""),
    );
    let id = stub_id(header, extra_body, envelope_extra, "fc");
    obj.insert("id".to_string(), json!(id));
    obj.insert("status".to_string(), json!("in_progress"));
}

fn provider_item_stub(
    header: &NodeHeader,
    extra_body: &HashMap<String, Value>,
    envelope_extra: &HashMap<String, Value>,
) -> Value {
    let NodeHeader::ProviderItem {
        id,
        item_type,
        body,
        ..
    } = header
    else {
        return Value::Null;
    };
    let mut obj = body
        .as_ref()
        .map(sanitize_provider_item_wire_body)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let native_had_id = obj.remove("id").is_some();
    merge_json_extra_preserving(&mut obj, envelope_extra);
    if let Value::Object(extra) =
        sanitize_provider_item_wire_body(&Value::Object(extra_body.clone().into_iter().collect()))
    {
        obj.extend(extra);
    }
    obj.insert("type".to_string(), json!(item_type));
    obj.remove("id");
    if (body.is_none() || native_had_id)
        && let Some(id) = id
    {
        obj.insert("id".to_string(), json!(id));
    }
    Value::Object(obj)
}

/// The `response.content_part.added` part of a message content node.
pub(super) fn start_content_part(header: &NodeHeader) -> Value {
    match header {
        NodeHeader::Text { .. } => {
            json!({ "type": "output_text", "text": "", "annotations": [], "logprobs": [] })
        }
        NodeHeader::Refusal { .. } => json!({ "type": "refusal", "refusal": "" }),
        NodeHeader::ProviderItem {
            id,
            origin_protocol,
            item_type,
            body: Some(body),
            ..
        } => encode_provider_item_for_responses(
            *origin_protocol,
            item_type,
            body,
            &HashMap::new(),
            Some(id),
        )
        .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// The `response.content_part.done` part of a message content node.
pub(super) fn done_content_part(node: &Node) -> Option<Value> {
    match node {
        Node::Text {
            logprobs,
            citations,
            content,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("output_text"));
            obj.insert("text".to_string(), json!(content));
            obj.insert(
                "annotations".to_string(),
                json!(crate::urp::citations::encode(
                    citations,
                    ProviderProtocol::Responses,
                    0
                )),
            );
            obj.insert(
                "logprobs".to_string(),
                crate::urp::logprobs::encode_openai(
                    crate::urp::logprobs::valid(logprobs, content).unwrap_or_default(),
                ),
            );
            merge_json_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        Node::Refusal {
            content,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("refusal"));
            obj.insert("refusal".to_string(), json!(content));
            merge_json_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
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
    }
}

/// Places the done `part` at `content_index` of a message item.
pub(super) fn set_content_part(item: &mut Value, content_index: usize, part: Value) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    let content = obj
        .entry("content".to_string())
        .or_insert_with(|| json!([]));
    if !content.is_array() {
        *content = json!([]);
    }
    let Some(content) = content.as_array_mut() else {
        return;
    };
    match content.get_mut(content_index) {
        Some(slot) => *slot = part,
        None => content.push(part),
    }
}

/// Merges a `ProviderItem` delta into an opaque item.
pub(super) fn merge_provider_delta(item: &mut Value, data: &Value) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    match sanitize_provider_item_wire_body(data) {
        Value::Object(fields) => {
            obj.extend(fields.into_iter().filter(|(key, _)| !is_internal_key(key)))
        }
        Value::Null => {}
        other => {
            obj.insert("data".to_string(), other);
        }
    }
}

/// STR3p: a known item type without a native final status completes.
pub(super) fn complete_item(mut item: Value) -> Value {
    if let Some(obj) = item.as_object_mut()
        && matches!(
            obj.get("type").and_then(Value::as_str),
            Some(
                "message"
                    | "image_generation_call"
                    | "reasoning"
                    | "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
            )
        )
        && obj
            .get("status")
            .and_then(Value::as_str)
            .is_none_or(|status| status == "in_progress")
    {
        obj.insert("status".to_string(), json!("completed"));
    }
    item
}

/// The `response.output_item.done` item of an own-item node. `current` is the streamed item,
/// whose id the done item keeps (STR3l); an image-generation or opaque item keeps an id of its
/// own.
pub(super) fn done_item(kind: NodeKind, current: &Value, node: &Node) -> Value {
    let mut item = encode_output_item(node);
    if kind != NodeKind::Provider {
        item = complete_item(item);
    }
    let keep_id = match kind {
        NodeKind::Image | NodeKind::Provider => item.get("id").is_none(),
        _ => true,
    };
    if keep_id
        && let Some(id) = current.get("id")
        && let Some(obj) = item.as_object_mut()
    {
        obj.insert("id".to_string(), id.clone());
    }
    item
}

fn id_or_fresh(id: &Option<String>, extra_body: &HashMap<String, Value>, prefix: &str) -> String {
    id.clone()
        .or_else(|| {
            extra_body
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("{prefix}_{}", uuid::Uuid::new_v4()))
}

fn encode_output_item(node: &Node) -> Value {
    match node {
        Node::Reasoning {
            metadata,
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("reasoning"));
            obj.insert("id".to_string(), json!(id_or_fresh(id, extra_body, "rs")));
            obj.insert(
                "summary".to_string(),
                crate::urp::reasoning::encode_text_parts(
                    summary.as_deref(),
                    metadata.summary_parts.as_deref(),
                    "summary_text",
                ),
            );
            obj.insert(
                "content".to_string(),
                crate::urp::reasoning::encode_text_parts(
                    content.as_deref(),
                    metadata.content_parts.as_deref(),
                    "reasoning_text",
                ),
            );
            if let Some(encrypted) = encrypted.as_ref().filter(|encrypted| !encrypted.is_null()) {
                obj.insert("encrypted_content".to_string(), encrypted.clone());
            }
            if let Some(source) = source.as_ref().filter(|source| !source.is_empty()) {
                obj.insert("source".to_string(), json!(source));
            }
            obj.insert("status".to_string(), json!("completed"));
            merge_reasoning_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        Node::ToolCall {
            namespace,
            id,
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
            ..
        } => {
            let custom = *tool_type == ToolCallType::Custom;
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                json!(if custom {
                    "custom_tool_call"
                } else {
                    "function_call"
                }),
            );
            obj.insert("call_id".to_string(), json!(call_id));
            obj.insert("name".to_string(), json!(name));
            if let Some(namespace) = namespace {
                obj.insert("namespace".to_string(), json!(namespace));
            }
            let arguments = if custom {
                arguments.clone()
            } else {
                tool_call_arguments_for_wire(arguments)
            };
            obj.insert(
                if custom { "input" } else { "arguments" }.to_string(),
                json!(arguments),
            );
            obj.insert("id".to_string(), json!(id_or_fresh(id, extra_body, "fc")));
            obj.insert("status".to_string(), json!("completed"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        Node::Image {
            id,
            source,
            metadata,
            extra_body,
            ..
        } => encode_image_generation_call_item(id.as_deref(), source, metadata, extra_body)
            .map(complete_item)
            .unwrap_or(Value::Null),
        Node::ProviderItem {
            id,
            origin_protocol: ProviderProtocol::Responses,
            item_type,
            body,
            extra_body,
            ..
        } => provider_output_item(item_type, body, extra_body, id.as_deref()),
        Node::ToolResult { .. } => tool_result_item(node),
        _ => Value::Null,
    }
}

fn provider_output_item(
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
    id: Option<&str>,
) -> Value {
    let mut obj = match sanitize_provider_item_wire_body(body) {
        Value::Object(map) => map,
        other => Map::from_iter([("body".to_string(), other)]),
    };
    obj.insert("type".to_string(), Value::String(item_type.to_string()));
    let had_id = obj.remove("id").is_some();
    if had_id && let Some(id) = id.filter(|id| !id.is_empty()) {
        obj.insert("id".to_string(), json!(id));
    }
    merge_json_extra(&mut obj, extra_body);
    Value::Object(obj)
}

fn tool_result_item(node: &Node) -> Value {
    let Node::ToolResult {
        namespace,
        name,
        id,
        tool_type,
        call_id,
        is_error,
        content,
        extra_body,
        ..
    } = node
    else {
        return Value::Null;
    };
    let mut obj = Map::new();
    merge_json_extra(&mut obj, extra_body);
    for key in ["id", "name", "namespace", "is_error"] {
        obj.remove(key);
    }
    if let Some(name) = name {
        obj.insert("name".into(), json!(name));
    }
    if let Some(namespace) = namespace {
        obj.insert("namespace".into(), json!(namespace));
    }
    let kind = match tool_type {
        ToolCallType::Custom => "custom_tool_call_output",
        ToolCallType::Function => "function_call_output",
    };
    obj.insert("type".to_string(), json!(kind));
    obj.insert("call_id".to_string(), json!(call_id));
    let id = id
        .clone()
        .unwrap_or_else(|| format!("fco_{}", uuid::Uuid::new_v4()));
    obj.insert("id".to_string(), json!(id));
    obj.insert("status".to_string(), json!("completed"));
    obj.insert("output".to_string(), encode_tool_result_output(content));
    if *is_error {
        obj.insert("is_error".to_string(), Value::Bool(true));
    }
    Value::Object(obj)
}
