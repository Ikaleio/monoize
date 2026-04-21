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
