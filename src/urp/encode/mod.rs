pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_image;
pub mod openai_responses;

use crate::urp::{InputDetails, OutputDetails, Part, Role, ToolChoice, Usage};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub fn merge_extra(obj: &mut Map<String, Value>, extra: &HashMap<String, Value>) {
    for (k, v) in extra {
        if !obj.contains_key(k) {
            obj.insert(k.clone(), v.clone());
        }
    }
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
