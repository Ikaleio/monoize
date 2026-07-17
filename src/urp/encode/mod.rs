pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_image;
pub mod openai_responses;
pub mod replicate;

use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{
    FILE_ID_ORIGIN_EXTRA_KEY, InputDetails, Node, OrdinaryRole, OutputDetails, ToolChoice, Usage,
};
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

pub(crate) fn file_id_origin_matches(
    extra_body: &HashMap<String, Value>,
    target_origin: &str,
) -> bool {
    extra_body
        .get(FILE_ID_ORIGIN_EXTRA_KEY)
        .and_then(Value::as_str)
        == Some(target_origin)
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
        ToolChoice::Specific(v) => sanitize_provider_item_wire_body(v),
    }
}

fn selector_name(obj: &Map<String, Value>, nested_key: &str) -> Option<Value> {
    obj.get(nested_key)
        .and_then(Value::as_object)
        .and_then(|nested| nested.get("name"))
        .cloned()
        .or_else(|| obj.get("name").cloned())
}

fn specific_tool_choice_to_chat_value(obj: &Map<String, Value>) -> Value {
    let Value::Object(mut out) = sanitize_provider_item_wire_body(&Value::Object(obj.clone()))
    else {
        unreachable!("tool choice sanitizer preserves object shape")
    };
    out.remove("disable_parallel_tool_use");
    match out.get("type").and_then(Value::as_str) {
        Some(mode @ ("auto" | "required" | "none")) => Value::String(mode.to_string()),
        Some(kind @ ("function" | "custom")) => {
            let kind = kind.to_string();
            let name = selector_name(&out, &kind);
            let mut nested = out
                .remove(&kind)
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            out.remove("name");
            if let Some(name) = name {
                nested.insert("name".to_string(), name);
            }
            out.insert("type".to_string(), Value::String(kind.clone()));
            out.insert(kind, Value::Object(nested));
            Value::Object(out)
        }
        Some("allowed_tools") => {
            let mut allowed = out
                .remove("allowed_tools")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            if !allowed.contains_key("mode")
                && let Some(mode) = out.remove("mode")
            {
                allowed.insert("mode".to_string(), mode);
            } else {
                out.remove("mode");
            }
            if !allowed.contains_key("tools")
                && let Some(tools) = out.remove("tools")
            {
                allowed.insert("tools".to_string(), tools);
            } else {
                out.remove("tools");
            }
            if let Some(tools) = allowed.get_mut("tools").and_then(Value::as_array_mut) {
                for selector in tools {
                    if let Value::Object(selector_obj) = selector {
                        *selector = specific_tool_choice_to_chat_value(selector_obj);
                    }
                }
            }
            out.insert(
                "type".to_string(),
                Value::String("allowed_tools".to_string()),
            );
            out.insert("allowed_tools".to_string(), Value::Object(allowed));
            Value::Object(out)
        }
        _ => Value::Object(out),
    }
}

fn specific_tool_choice_to_responses_value(obj: &Map<String, Value>) -> Value {
    let Value::Object(mut out) = sanitize_provider_item_wire_body(&Value::Object(obj.clone()))
    else {
        unreachable!("tool choice sanitizer preserves object shape")
    };
    out.remove("disable_parallel_tool_use");
    match out.get("type").and_then(Value::as_str) {
        Some(mode @ ("auto" | "required" | "none")) => Value::String(mode.to_string()),
        Some(kind @ ("function" | "custom")) => {
            let kind = kind.to_string();
            let name = selector_name(&out, &kind);
            out.remove("function");
            out.remove("custom");
            out.remove("name");
            out.insert("type".to_string(), Value::String(kind));
            if let Some(name) = name {
                out.insert("name".to_string(), name);
            }
            Value::Object(out)
        }
        Some("allowed_tools") => {
            let allowed = out
                .remove("allowed_tools")
                .and_then(|value| value.as_object().cloned());
            let canonical_mode = allowed
                .as_ref()
                .and_then(|allowed| allowed.get("mode"))
                .cloned()
                .or_else(|| out.get("mode").cloned());
            let canonical_tools = allowed
                .as_ref()
                .and_then(|allowed| allowed.get("tools"))
                .cloned()
                .or_else(|| out.get("tools").cloned());
            out.remove("mode");
            out.remove("tools");
            if let Some(allowed) = allowed {
                for (key, value) in allowed {
                    if !matches!(key.as_str(), "mode" | "tools") {
                        out.entry(key).or_insert(value);
                    }
                }
            }
            out.insert(
                "type".to_string(),
                Value::String("allowed_tools".to_string()),
            );
            if let Some(mode) = canonical_mode {
                out.insert("mode".to_string(), mode);
            }
            if let Some(mut tools) = canonical_tools {
                if let Some(tools) = tools.as_array_mut() {
                    for selector in tools {
                        if let Value::Object(selector_obj) = selector {
                            *selector = specific_tool_choice_to_responses_value(selector_obj);
                        }
                    }
                }
                out.insert("tools".to_string(), tools);
            }
            Value::Object(out)
        }
        _ => Value::Object(out),
    }
}

pub fn tool_choice_to_chat_value(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Mode(s) => Value::String(s.clone()),
        ToolChoice::Specific(Value::Object(obj)) => specific_tool_choice_to_chat_value(obj),
        ToolChoice::Specific(v) => sanitize_provider_item_wire_body(v),
    }
}

pub fn tool_choice_to_responses_value(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Mode(s) => Value::String(s.clone()),
        ToolChoice::Specific(Value::Object(obj)) => specific_tool_choice_to_responses_value(obj),
        ToolChoice::Specific(v) => sanitize_provider_item_wire_body(v),
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

    #[test]
    fn target_specific_tool_choice_shapes_preserve_semantics() {
        let function = ToolChoice::Specific(json!({
            "type": "function",
            "name": "stale-flat-name",
            "function": { "name": "lookup", "_monoize_nested_spoof": true },
            "future_selector_field": 7,
            "disable_parallel_tool_use": true,
            "_monoize_outer_spoof": true
        }));
        assert_eq!(
            tool_choice_to_chat_value(&function),
            json!({
                "type": "function",
                "function": { "name": "lookup" },
                "future_selector_field": 7
            })
        );
        assert_eq!(
            tool_choice_to_responses_value(&function),
            json!({
                "type": "function",
                "name": "lookup",
                "future_selector_field": 7
            })
        );

        let custom = ToolChoice::Specific(json!({
            "type": "custom",
            "custom": { "name": "grammar" }
        }));
        assert_eq!(
            tool_choice_to_responses_value(&custom),
            json!({ "type": "custom", "name": "grammar" })
        );

        let allowed = ToolChoice::Specific(json!({
            "type": "allowed_tools",
            "mode": "stale",
            "tools": [{ "type": "file_search" }],
            "allowed_tools": {
                "mode": "required",
                "_monoize_wrapper_spoof": true,
                "tools": [
                    {
                        "type": "function",
                        "function": { "name": "lookup", "_monoize_inner_spoof": true }
                    },
                    { "type": "custom", "custom": { "name": "grammar" } },
                    { "type": "mcp", "server_label": "docs", "name": "search" },
                    { "type": "image_generation" }
                ]
            }
        }));
        assert_eq!(
            tool_choice_to_chat_value(&allowed),
            json!({
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        { "type": "function", "function": { "name": "lookup" } },
                        { "type": "custom", "custom": { "name": "grammar" } },
                        { "type": "mcp", "server_label": "docs", "name": "search" },
                        { "type": "image_generation" }
                    ]
                }
            })
        );
        assert_eq!(
            tool_choice_to_responses_value(&allowed),
            json!({
                "type": "allowed_tools",
                "mode": "required",
                "tools": [
                    { "type": "function", "name": "lookup" },
                    { "type": "custom", "name": "grammar" },
                    { "type": "mcp", "server_label": "docs", "name": "search" },
                    { "type": "image_generation" }
                ]
            })
        );
    }

    #[test]
    fn target_tool_choice_fallbacks_reject_recursive_internal_keys() {
        let fallback = ToolChoice::Specific(json!([
            {
                "type": "vendor_selector",
                "vendor_keep": true,
                "_monoize_outer_spoof": true,
                "nested": {
                    "vendor_nested_keep": 7,
                    "_monoize_nested_spoof": true
                }
            }
        ]));
        let expected = json!([{
            "type": "vendor_selector",
            "vendor_keep": true,
            "nested": { "vendor_nested_keep": 7 }
        }]);

        assert_eq!(tool_choice_to_value(&fallback), expected);
        assert_eq!(tool_choice_to_chat_value(&fallback), expected);
        assert_eq!(tool_choice_to_responses_value(&fallback), expected);
    }
}
