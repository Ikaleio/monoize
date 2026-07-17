pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_image;
pub mod openai_responses;
pub mod replicate;

use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{InputDetails, Node, OrdinaryRole, OutputDetails, ToolChoice, Usage};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn merge_extra(obj: &mut Map<String, Value>, extra: &HashMap<String, Value>) {
    for (k, v) in extra {
        if k.starts_with("_monoize_") {
            continue;
        }
        if !obj.contains_key(k) {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// Returns a wire-only clone of an opaque ProviderItem body with internal adapter keys removed.
pub(crate) fn sanitize_provider_item_wire_body(body: &Value) -> Value {
    fn sanitize(value: &mut Value) {
        match value {
            Value::Object(obj) => {
                obj.retain(|key, _| !key.starts_with("_monoize_"));
                for value in obj.values_mut() {
                    sanitize(value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    sanitize(value);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    let mut sanitized = body.clone();
    sanitize(&mut sanitized);
    sanitized
}

pub fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::Developer => "developer",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

pub fn tool_choice_to_value(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Mode(s) => Value::String(s.clone()),
        ToolChoice::Specific(v) => v.clone(),
    }
}

pub fn tool_choice_to_openai_value(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Mode(s) => Value::String(s.clone()),
        ToolChoice::Specific(Value::Object(obj)) => match obj.get("type").and_then(Value::as_str) {
            Some("auto" | "required" | "none") => Value::String(
                obj.get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            _ => {
                let mut out = obj.clone();
                out.remove("disable_parallel_tool_use");
                Value::Object(out)
            }
        },
        ToolChoice::Specific(v) => v.clone(),
    }
}

pub fn text_parts(parts: &[Part]) -> String {
    let mut out = String::new();
    for p in parts {
        if let Part::Text { content, .. } = p {
            out.push_str(content);
        }
    }
    out
}

pub fn has_encrypted_reasoning(parts: &[Part]) -> bool {
    parts.iter().any(|p| {
        matches!(
            p,
            Part::Reasoning {
                encrypted: Some(_),
                ..
            }
        )
    })
}

pub fn extract_reasoning_plain(parts: &[Part]) -> String {
    let mut out = String::new();
    for p in parts {
        if let Part::Reasoning {
            content, summary, ..
        } = p
        {
            if let Some(content) = content {
                out.push_str(content);
            } else if let Some(summary) = summary {
                out.push_str(summary);
            }
        }
    }
    out
}

pub fn extract_reasoning_encrypted(parts: &[Part]) -> Option<Value> {
    parts.iter().find_map(|p| match p {
        Part::Reasoning {
            encrypted: Some(data),
            ..
        } => Some(data.clone()),
        _ => None,
    })
}

pub fn extract_tool_calls(parts: &[Part]) -> Vec<Value> {
    let mut out = Vec::new();
    for p in parts {
        if let Part::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } = p
        {
            out.push(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments
                }
            }));
        }
    }
    out
}

pub fn usage_input_details(usage: &Usage) -> InputDetails {
    usage.input_details.clone().unwrap_or_default()
}

pub fn usage_output_details(usage: &Usage) -> OutputDetails {
    usage.output_details.clone().unwrap_or_default()
}

pub fn ordinary_role_to_str(role: OrdinaryRole) -> &'static str {
    match role {
        OrdinaryRole::System => "system",
        OrdinaryRole::Developer => "developer",
        OrdinaryRole::User => "user",
        OrdinaryRole::Assistant => "assistant",
    }
}

pub fn text_from_nodes(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        if let Node::Text { content, .. } = n {
            out.push_str(content);
        }
    }
    out
}

pub fn extract_tool_calls_from_nodes(nodes: &[Node]) -> Vec<Value> {
    let mut out = Vec::new();
    for n in nodes {
        if let Node::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } = n
        {
            out.push(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments
                }
            }));
        }
    }
    out
}

pub fn has_encrypted_reasoning_in_nodes(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| {
        matches!(
            n,
            Node::Reasoning {
                encrypted: Some(_),
                ..
            }
        )
    })
}

pub fn extract_reasoning_plain_from_nodes(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        if let Node::Reasoning {
            content, summary, ..
        } = n
        {
            if let Some(content) = content {
                out.push_str(content);
            } else if let Some(summary) = summary {
                out.push_str(summary);
            }
        }
    }
    out
}

pub fn extract_reasoning_encrypted_from_nodes(nodes: &[Node]) -> Option<Value> {
    nodes.iter().find_map(|n| match n {
        Node::Reasoning {
            encrypted: Some(data),
            ..
        } => Some(data.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_item_wire_body_sanitizer_is_recursive_and_non_mutating() {
        let body = json!({
            "type": "opaque_item",
            "vendor_unknown": { "keep": true, "_monoize_nested": "drop" },
            "items": [
                { "keep": 1, "_monoize_array_member": "drop" },
                [
                    { "deep_keep": "yes", "_monoize_deep": "drop" }
                ]
            ],
            "_monoize_top": "drop"
        });

        let sanitized = sanitize_provider_item_wire_body(&body);

        assert_eq!(
            sanitized,
            json!({
                "type": "opaque_item",
                "vendor_unknown": { "keep": true },
                "items": [
                    { "keep": 1 },
                    [
                        { "deep_keep": "yes" }
                    ]
                ]
            })
        );
        assert_eq!(body["_monoize_top"], json!("drop"));
        assert_eq!(body["vendor_unknown"]["_monoize_nested"], json!("drop"));
    }
}
