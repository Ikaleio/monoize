//! Responses `reasoning` items.

use super::items::ItemTarget;
use crate::urp::{Node, is_internal_key};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Encodes a `Reasoning` node. A request item omits downstream-only reasoning and output-only
/// members; an output item omits a reasoning node with no meaningful payload unless the
/// upstream item was natively empty.
pub(super) fn encode_reasoning_item(node: &Node, target: ItemTarget) -> Option<Value> {
    let Node::Reasoning {
        metadata,
        id,
        content,
        encrypted,
        summary,
        source,
        extra_body,
    } = node
    else {
        return None;
    };
    let request_item = target == ItemTarget::Input;
    if request_item && metadata.downstream_only {
        return None;
    }
    if !request_item
        && !metadata.native_empty
        && !reasoning_payload_is_meaningful(content, summary, encrypted)
    {
        return None;
    }
    let mut obj = Map::new();
    if let Some(id) = id {
        obj.insert("id".to_string(), Value::String(id.clone()));
    } else if !request_item {
        obj.insert(
            "id".to_string(),
            Value::String(format!("rs_{}", uuid::Uuid::new_v4().simple())),
        );
    }
    obj.insert("type".to_string(), Value::String("reasoning".to_string()));
    obj.insert(
        "summary".into(),
        crate::urp::reasoning::encode_text_parts(
            summary.as_deref(),
            metadata.summary_parts.as_deref(),
            "summary_text",
        ),
    );
    if content.is_some() || !request_item {
        obj.insert(
            "content".into(),
            crate::urp::reasoning::encode_text_parts(
                content.as_deref(),
                metadata.content_parts.as_deref(),
                "reasoning_text",
            ),
        );
    }
    if let Some(encrypted) = encrypted {
        obj.insert("encrypted_content".to_string(), encrypted.clone());
    }
    if !request_item && let Some(source) = source.as_ref().filter(|source| !source.is_empty()) {
        obj.insert("source".to_string(), Value::String(source.clone()));
    }
    merge_reasoning_extra(&mut obj, extra_body);
    if request_item {
        sanitize_reasoning_request_item(&mut obj);
    }
    Some(Value::Object(obj))
}

/// Adds the wire extras of a reasoning item. An extra named like a typed member is a stale
/// native copy: it never reaches the wire, so a deleted typed field stays deleted.
pub(crate) fn merge_reasoning_extra(
    obj: &mut Map<String, Value>,
    extra_body: &HashMap<String, Value>,
) {
    const TYPED_MEMBERS: [&str; 7] = [
        "id",
        "type",
        "text",
        "content",
        "summary",
        "encrypted_content",
        "source",
    ];
    for (key, value) in extra_body {
        if !is_internal_key(key) && !TYPED_MEMBERS.contains(&key.as_str()) {
            obj.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

fn reasoning_payload_is_meaningful(
    content: &Option<String>,
    summary: &Option<String>,
    encrypted: &Option<Value>,
) -> bool {
    content.as_ref().is_some_and(|value| !value.is_empty())
        || summary.as_ref().is_some_and(|value| !value.is_empty())
        || encrypted.as_ref().is_some_and(non_empty_json_value)
}

fn non_empty_json_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

/// Removes output-only members a Responses request rejects on a `reasoning` input item.
pub(super) fn sanitize_reasoning_request_item(obj: &mut Map<String, Value>) {
    obj.remove("text");
    obj.remove("source");
    obj.remove("started_at");
    obj.remove("status");
    obj.remove("duration");
    obj.retain(|key, _| !is_internal_key(key));
    if !obj.contains_key("summary") {
        obj.insert("summary".to_string(), Value::Array(Vec::new()));
    }
}
