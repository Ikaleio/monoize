//! Responses tool call and tool output items.

use super::items::encode_provider_item_for_responses;
use super::media::{encode_input_file, encode_input_image};
use crate::urp::encode::merge_extra;
use crate::urp::{Node, ToolCallType, ToolResultContent, tool_call_arguments_for_wire};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub(super) fn encode_tool_call_item(node: &Node, output_item: bool) -> Option<Value> {
    let Node::ToolCall {
        namespace,
        id,
        tool_type,
        call_id,
        name,
        arguments,
        extra_body,
        ..
    } = node
    else {
        return None;
    };
    let item_type = match tool_type {
        ToolCallType::Function => "function_call",
        ToolCallType::Custom => "custom_tool_call",
    };
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!(item_type));
    if output_item {
        let id = match tool_type {
            ToolCallType::Function => normalize_function_call_id(id.as_deref()),
            ToolCallType::Custom => id
                .clone()
                .unwrap_or_else(|| format!("ctc_urp_{}", uuid::Uuid::new_v4().simple())),
        };
        obj.insert("id".to_string(), Value::String(id));
    } else if let Some(id) = id {
        obj.insert("id".to_string(), Value::String(id.clone()));
    }
    obj.insert("call_id".to_string(), Value::String(call_id.clone()));
    obj.insert("name".to_string(), Value::String(name.clone()));
    if let Some(namespace) = namespace {
        obj.insert("namespace".to_string(), json!(namespace));
    }
    let (arguments_key, arguments) = match tool_type {
        ToolCallType::Function => ("arguments", tool_call_arguments_for_wire(arguments)),
        ToolCallType::Custom => ("input", arguments.clone()),
    };
    obj.insert(arguments_key.to_string(), Value::String(arguments));
    merge_extra(&mut obj, extra_body);
    if !output_item {
        // PR4.1: foreign call identifiers are rejected by Responses as item ids.
        let prefix = match tool_type {
            ToolCallType::Function => "fc_",
            ToolCallType::Custom => "ctc_",
        };
        if !obj
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with(prefix))
        {
            obj.remove("id");
        }
    }
    Some(Value::Object(obj))
}

/// Encodes a `ToolResult` envelope. `control_extra` holds the members of preceding control
/// nodes; the node's own extras win over them.
pub(super) fn encode_tool_result_item(
    node: &Node,
    control_extra: &HashMap<String, Value>,
    output_item: bool,
    out: &mut Vec<Value>,
) {
    let Node::ToolResult {
        namespace,
        name,
        id,
        tool_type,
        call_id,
        content,
        extra_body,
        ..
    } = node
    else {
        return;
    };
    let item_type = match tool_type {
        ToolCallType::Function => "function_call_output",
        ToolCallType::Custom => "custom_tool_call_output",
    };
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!(item_type));
    if output_item {
        let id = match tool_type {
            ToolCallType::Function => normalize_function_output_id(id.as_deref()),
            ToolCallType::Custom => id
                .clone()
                .unwrap_or_else(|| format!("ctco_urp_{}", uuid::Uuid::new_v4().simple())),
        };
        obj.insert("id".to_string(), Value::String(id));
    } else if let Some(id) = id {
        obj.insert("id".to_string(), Value::String(id.clone()));
    }
    obj.insert("call_id".to_string(), Value::String(call_id.clone()));
    obj.insert("output".to_string(), encode_tool_result_output(content));
    merge_extra(&mut obj, extra_body);
    merge_extra(&mut obj, control_extra);
    obj.remove("namespace");
    obj.remove("name");
    if let Some(namespace) = namespace {
        obj.insert("namespace".into(), json!(namespace));
    }
    if let Some(name) = name {
        obj.insert("name".into(), json!(name));
    }
    if id.is_none() && !output_item {
        obj.remove("id");
    }
    out.push(Value::Object(obj));
}

/// The `output` member of a tool output item: a plain string for one bare text block,
/// otherwise an array of input content parts.
pub(crate) fn encode_tool_result_output(content: &[ToolResultContent]) -> Value {
    let mut tool_content = Vec::new();
    for item in content {
        let block = match item {
            ToolResultContent::Text {
                text, extra_body, ..
            } => {
                let mut block = Map::new();
                block.insert("type".to_string(), json!("input_text"));
                block.insert("text".to_string(), json!(text));
                merge_extra(&mut block, extra_body);
                Some(Value::Object(block))
            }
            ToolResultContent::Image {
                metadata,
                source,
                extra_body,
            } => encode_input_image(source, metadata, extra_body),
            ToolResultContent::File {
                metadata,
                source,
                extra_body,
            } => encode_input_file(source, metadata, extra_body),
            ToolResultContent::ProviderItem {
                origin_protocol,
                item_type,
                body,
                extra_body,
            } => encode_provider_item_for_responses(
                *origin_protocol,
                item_type,
                body,
                extra_body,
                None,
            ),
        };
        tool_content.extend(block);
    }

    let bare_text = tool_content.len() == 1
        && tool_content[0].get("type").and_then(Value::as_str) == Some("input_text")
        && tool_content[0]
            .as_object()
            .is_some_and(|obj| obj.keys().all(|key| key == "type" || key == "text"));
    if tool_content.is_empty() {
        Value::String(String::new())
    } else if bare_text {
        tool_content[0]
            .get("text")
            .cloned()
            .unwrap_or_else(|| json!(""))
    } else {
        Value::Array(tool_content)
    }
}

fn normalize_function_call_id(id: Option<&str>) -> String {
    match id {
        Some(existing) if existing.starts_with("fc_") => existing.to_string(),
        _ => format!("fc_{}", uuid::Uuid::new_v4().simple()),
    }
}

fn normalize_function_output_id(id: Option<&str>) -> String {
    match id {
        Some(existing) if existing.starts_with("fco_") => existing.to_string(),
        Some(existing) if existing.starts_with("fc_") => existing.replacen("fc_", "fco_", 1),
        _ => format!("fco_{}", uuid::Uuid::new_v4().simple()),
    }
}
