pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_image;
pub mod openai_responses;
pub mod openrouter_image;
pub mod replicate;

use crate::urp::decode::remove_untrusted_internal_keys;
use crate::urp::{
    InputDetails, Node, OrdinaryRole, OutputDetails, ToolChoice, Usage, is_internal_key,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub fn merge_extra(obj: &mut Map<String, Value>, extra: &HashMap<String, Value>) {
    for (k, v) in extra {
        if !is_internal_key(k) && !obj.contains_key(k) {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// Returns a wire-only clone of an opaque ProviderItem body with internal adapter keys removed.
pub(crate) fn sanitize_provider_item_wire_body(body: &Value) -> Value {
    let mut sanitized = body.clone();
    remove_untrusted_internal_keys(&mut sanitized);
    sanitized
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
