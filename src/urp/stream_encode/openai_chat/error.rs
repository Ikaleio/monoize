//! Chat stream failure payloads (DC5b, SE2a, SAN-11).

use crate::error_sanitize::maybe_mask_sensitive_text;
use crate::urp::{ResponseOutcome, is_internal_key};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// The native body of a failed or cancelled outcome, with its message masked.
pub(super) fn failure_body(outcome: Option<&ResponseOutcome>, mask: bool) -> Option<Value> {
    let mut body = outcome?.failure_body(false)?;
    if let Some(message) = body["error"]["message"].as_str() {
        body["error"]["message"] = Value::String(maybe_mask_sensitive_text(message, mask));
    }
    Some(body)
}

/// The chunk of a terminal `Error` event. A same-Chat error keeps its native placement and
/// unknown members; code and message always come from the typed event (DC5b).
pub(super) fn error_payload(
    original: Option<&Value>,
    code: Option<&str>,
    message: &str,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut payload = original.cloned().unwrap_or_else(|| json!({}));
    sanitize_owners(&mut payload);
    let Some(root) = payload.as_object_mut() else {
        return error_payload(None, code, message, extra_body);
    };
    if let Some(error) = root.get_mut("error").and_then(Value::as_object_mut) {
        materialize_error_fields(error, code, message, extra_body);
        return payload;
    }
    if let Some(error) = root
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|choices| choices.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("error"))
        .and_then(Value::as_object_mut)
    {
        materialize_error_fields(error, code, message, extra_body);
        return payload;
    }
    let mut error = extra_body
        .get("error")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    sanitize_error_object(&mut error);
    materialize_error_fields(&mut error, code, message, extra_body);
    root.insert("error".to_string(), Value::Object(error));
    payload
}

fn retain_wire_members(owner: &mut Map<String, Value>) {
    owner.retain(|key, _| !is_internal_key(key));
}

fn sanitize_error_object(error: &mut Map<String, Value>) {
    retain_wire_members(error);
    if let Some(metadata) = error.get_mut("metadata").and_then(Value::as_object_mut) {
        retain_wire_members(metadata);
    }
}

/// Removes internal members at the chunk, choice, delta, message, and error owner layers.
fn sanitize_owners(payload: &mut Value) {
    let Some(root) = payload.as_object_mut() else {
        return;
    };
    retain_wire_members(root);
    if let Some(error) = root.get_mut("error").and_then(Value::as_object_mut) {
        sanitize_error_object(error);
    }
    let Some(choices) = root.get_mut("choices").and_then(Value::as_array_mut) else {
        return;
    };
    for choice in choices.iter_mut().filter_map(Value::as_object_mut) {
        retain_wire_members(choice);
        for key in ["delta", "message"] {
            if let Some(owner) = choice.get_mut(key).and_then(Value::as_object_mut) {
                retain_wire_members(owner);
            }
        }
        if let Some(error) = choice.get_mut("error").and_then(Value::as_object_mut) {
            sanitize_error_object(error);
        }
    }
}

fn nonempty_scalar(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Number(_)))
        || matches!(value, Some(Value::String(value)) if !value.is_empty())
}

fn materialize_error_fields(
    error: &mut Map<String, Value>,
    code: Option<&str>,
    message: &str,
    extra_body: &HashMap<String, Value>,
) {
    error.insert("message".to_string(), Value::String(message.to_string()));
    error.remove("code");
    if let Some(code) = code {
        error.insert("code".to_string(), Value::String(code.to_string()));
    }
    if !nonempty_scalar(error.get("type")) {
        let error_type = extra_body
            .get("type")
            .filter(|value| nonempty_scalar(Some(value)))
            .cloned()
            .unwrap_or_else(|| Value::String("server_error".to_string()));
        error.insert("type".to_string(), error_type);
    }
    if !error.contains_key("param")
        && let Some(param) = extra_body.get("param")
    {
        error.insert("param".to_string(), param.clone());
    }
}
