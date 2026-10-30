use super::{CHAT_REASONING_DETAIL_EXTRA_KEY, Node, ReasoningMetadata, ReasoningTextPart};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn text_part_shapes(value: &Value) -> Option<Vec<ReasoningTextPart>> {
    Some(
        value
            .as_array()?
            .iter()
            .map(|part| ReasoningTextPart {
                byte_length: part
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .len(),
                extra_body: part
                    .as_object()
                    .map(|obj| {
                        obj.iter()
                            .filter(|(key, _)| {
                                key.as_str() != "text" && !key.starts_with("_monoize_")
                            })
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect(),
    )
}

pub fn encode_text_parts(
    text: Option<&str>,
    shapes: Option<&[ReasoningTextPart]>,
    kind: &str,
) -> Value {
    let Some(text) = text else {
        return json!([]);
    };
    if let Some(shapes) = shapes {
        let total = shapes
            .iter()
            .try_fold(0usize, |total, shape| total.checked_add(shape.byte_length));
        let mut end = 0usize;
        let boundaries_valid = shapes.iter().all(|shape| {
            let Some(next) = end.checked_add(shape.byte_length) else {
                return false;
            };
            end = next;
            text.is_char_boundary(end)
        });
        if !shapes.is_empty() && total == Some(text.len()) && boundaries_valid {
            let mut offset = 0;
            return Value::Array(
                shapes
                    .iter()
                    .map(|shape| {
                        let mut part: Map<String, Value> =
                            shape.extra_body.clone().into_iter().collect();
                        part.entry("type").or_insert_with(|| json!(kind));
                        part.insert(
                            "text".into(),
                            json!(&text[offset..offset + shape.byte_length]),
                        );
                        offset += shape.byte_length;
                        Value::Object(part)
                    })
                    .collect(),
            );
        }
    }
    json!([{ "type": kind, "text": text }])
}

fn promoted_text_signature(detail: &Map<String, Value>) -> Option<&str> {
    (detail.get("type").and_then(Value::as_str) == Some("reasoning.text"))
        .then(|| detail.get("signature").and_then(Value::as_str))
        .flatten()
        .filter(|signature| !signature.is_empty())
}

/// Native entry shape of one Chat reasoning detail without the payloads owned by typed fields
/// (MAP-20). A non-empty `reasoning.text.signature` is owned by typed `encrypted`.
pub fn detail_metadata(detail: &Map<String, Value>) -> Map<String, Value> {
    let signature_is_typed = promoted_text_signature(detail).is_some();
    detail
        .iter()
        .filter(|(key, _)| {
            !matches!(key.as_str(), "text" | "summary" | "data" | "id" | "format")
                && !(signature_is_typed && key.as_str() == "signature")
                && !key.starts_with("_monoize_")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Decodes one Chat `reasoning_details[]` entry into one reasoning node (PC8, MAP-20).
pub fn chat_reasoning_node_from_detail(detail: &Map<String, Value>) -> Option<Node> {
    let detail_type = detail.get("type").and_then(Value::as_str)?;
    if !detail_type.starts_with("reasoning.") {
        return None;
    }
    let text = |key: &str| {
        detail
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let encrypted = match detail_type {
        "reasoning.encrypted" => detail.get("data").filter(|value| !value.is_null()).cloned(),
        "reasoning.text" => promoted_text_signature(detail).map(|sig| Value::String(sig.into())),
        _ => None,
    };
    Some(Node::Reasoning {
        metadata: Default::default(),
        id: text("id").filter(|id| !id.is_empty()),
        content: (detail_type == "reasoning.text").then(|| text("text")).flatten(),
        encrypted,
        summary: (detail_type == "reasoning.summary")
            .then(|| text("summary"))
            .flatten(),
        source: text("format").filter(|format| !format.is_empty()),
        extra_body: HashMap::from([(
            CHAT_REASONING_DETAIL_EXTRA_KEY.to_string(),
            Value::Object(detail_metadata(detail)),
        )]),
    })
}

/// Rebuilds the native Chat details of a node decoded from `reasoning_details[]`.
pub fn chat_details(
    content: Option<&str>,
    summary: Option<&str>,
    encrypted: Option<&Value>,
    id: Option<&str>,
    source: Option<&str>,
    native: Option<&Map<String, Value>>,
) -> Vec<Value> {
    let base = native.map(detail_metadata).unwrap_or_default();
    let signed_text = base.get("type").and_then(Value::as_str) == Some("reasoning.text");
    build_details(
        base, content, summary, encrypted, id, source, signed_text, false,
    )
    .into_iter()
    .map(Value::Object)
    .collect()
}

/// Chat details for one reasoning node or delta, native or cross-protocol (DC5).
/// Entries are in summary, text, encrypted order. Cross-protocol entries carry no `index`;
/// callers assign one.
pub fn chat_node_details(
    metadata: &ReasoningMetadata,
    id: Option<&str>,
    content: Option<&str>,
    summary: Option<&str>,
    encrypted: Option<&Value>,
    source: Option<&str>,
    extra_body: &HashMap<String, Value>,
) -> Vec<Map<String, Value>> {
    if let Some(native) = extra_body
        .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
        .and_then(Value::as_object)
    {
        return chat_details(content, summary, encrypted, id, source, Some(native))
            .into_iter()
            .filter_map(|detail| match detail {
                Value::Object(detail) => Some(detail),
                _ => None,
            })
            .collect();
    }
    fn non_empty(value: Option<&str>) -> Option<&str> {
        value.filter(|value| !value.is_empty())
    }
    let encrypted = encrypted.filter(|value| match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        _ => true,
    });
    let id = id.filter(|id| !id.is_empty() && !id.starts_with("rs_urp_"));
    // Anthropic thinking reaches URP as summary text marked `summary_as_thinking`; it is raw
    // thinking and its signature belongs on the same `reasoning.text` entry (OpenRouter shape).
    let thinking = metadata.summary_as_thinking;
    let (content, summary) = match (non_empty(content), non_empty(summary)) {
        (None, Some(summary)) if thinking => (Some(summary), None),
        other => other,
    };
    build_details(
        Map::new(),
        content,
        summary,
        encrypted,
        id,
        source.filter(|source| !source.is_empty()),
        thinking,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_details(
    base: Map<String, Value>,
    content: Option<&str>,
    summary: Option<&str>,
    encrypted: Option<&Value>,
    id: Option<&str>,
    source: Option<&str>,
    signed_text: bool,
    skip_empty: bool,
) -> Vec<Map<String, Value>> {
    let signature = encrypted
        .filter(|_| signed_text)
        .and_then(Value::as_str)
        .filter(|signature| !signature.is_empty());
    let content = content
        .filter(|content| !skip_empty || !content.is_empty())
        .or(signature.map(|_| ""));
    let mut result = Vec::new();
    for (kind, field, payload) in [
        ("reasoning.summary", "summary", summary.map(|v| json!(v))),
        ("reasoning.text", "text", content.map(|v| json!(v))),
        (
            "reasoning.encrypted",
            "data",
            encrypted.filter(|_| signature.is_none()).cloned(),
        ),
    ] {
        let Some(payload) = payload else {
            continue;
        };
        let mut detail = base.clone();
        detail.insert("type".into(), json!(kind));
        detail.insert(field.into(), payload);
        if kind == "reasoning.text"
            && let Some(signature) = signature
        {
            detail.insert("signature".into(), json!(signature));
        }
        if let Some(id) = id {
            detail.insert("id".into(), json!(id));
        }
        if let Some(source) = source {
            detail.insert("format".into(), json!(source));
        }
        result.push(detail);
    }
    if result.is_empty()
        && base.get("type").and_then(Value::as_str) == Some("reasoning.server_tool_call")
    {
        let mut detail = base;
        if let Some(id) = id {
            detail.insert("id".into(), json!(id));
        }
        if let Some(source) = source {
            detail.insert("format".into(), json!(source));
        }
        result.push(detail);
    }
    result
}

impl ReasoningMetadata {
    pub fn merge(&mut self, other: &Self) {
        self.redacted |= other.redacted;
        self.downstream_only |= other.downstream_only;
        self.chat_content |= other.chat_content;
        self.summary_as_thinking |= other.summary_as_thinking;
        self.native_empty |= other.native_empty;
        if other.item_id.is_some() {
            self.item_id.clone_from(&other.item_id);
        }
        if other.summary_parts.is_some() {
            self.summary_parts.clone_from(&other.summary_parts);
        }
        if other.content_parts.is_some() {
            self.content_parts.clone_from(&other.content_parts);
        }
    }
}
