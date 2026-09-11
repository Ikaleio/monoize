use super::{ReasoningMetadata, ReasoningTextPart};
use serde_json::{Map, Value, json};

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

pub fn detail_metadata(detail: &Map<String, Value>) -> Map<String, Value> {
    detail
        .iter()
        .filter(|(key, _)| {
            !matches!(key.as_str(), "text" | "summary" | "data" | "id" | "format")
                && !key.starts_with("_monoize_")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub fn chat_details(
    content: Option<&str>,
    summary: Option<&str>,
    encrypted: Option<&Value>,
    id: Option<&str>,
    source: Option<&str>,
    native: Option<&Map<String, Value>>,
) -> Vec<Value> {
    let base = native.map(detail_metadata).unwrap_or_default();
    let mut result = Vec::new();
    for (kind, field, payload) in [
        ("reasoning.summary", "summary", summary.map(|v| json!(v))),
        ("reasoning.text", "text", content.map(|v| json!(v))),
        ("reasoning.encrypted", "data", encrypted.cloned()),
    ] {
        if let Some(payload) = payload {
            let mut detail = base.clone();
            detail.insert("type".into(), json!(kind));
            detail.insert(field.into(), payload);
            if let Some(id) = id {
                detail.insert("id".into(), json!(id));
            }
            if let Some(source) = source {
                detail.insert("format".into(), json!(source));
            }
            result.push(Value::Object(detail));
        }
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
        result.push(Value::Object(detail));
    }
    result
}

impl ReasoningMetadata {
    pub fn merge(&mut self, other: &Self) {
        self.redacted |= other.redacted;
        self.downstream_only |= other.downstream_only;
        self.chat_content |= other.chat_content;
        self.summary_as_thinking |= other.summary_as_thinking;
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
