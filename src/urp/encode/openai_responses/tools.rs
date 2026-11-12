//! Responses tool definitions.

use crate::urp::encode::merge_extra;
use crate::urp::{ProviderProtocol, ToolDefinition};
use serde_json::{Map, Value, json};

pub(super) fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool
            .origin_protocol
            .is_some_and(|origin| origin != ProviderProtocol::Responses)
            && tool.function.is_none()
            && tool.custom.is_none()
            && tool.tools.is_none()
        {
            continue;
        }
        let mut item = Map::new();
        if tool.tool_type == "function" {
            let Some(function) = &tool.function else {
                continue;
            };
            item.insert("type".to_string(), json!("function"));
            item.insert("name".to_string(), Value::String(function.name.clone()));
            item.insert(
                "parameters".to_string(),
                function.parameters.clone().unwrap_or(json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": true
                })),
            );
            if let Some(description) = &function.description {
                item.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
            if let Some(strict) = function.strict {
                item.insert("strict".to_string(), Value::Bool(strict));
            } else if tool.origin_protocol != Some(ProviderProtocol::Responses) {
                item.insert("strict".to_string(), Value::Bool(false));
            }
            merge_extra(&mut item, &function.extra_body);
        } else if tool.tool_type == "custom" {
            let Some(custom) = &tool.custom else {
                continue;
            };
            item.insert("type".to_string(), json!("custom"));
            item.insert("name".to_string(), Value::String(custom.name.clone()));
            if let Some(description) = &custom.description {
                item.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
            if let Some(format) = &custom.format {
                item.insert("format".to_string(), format.clone());
            }
            merge_extra(&mut item, &custom.extra_body);
        } else {
            item.insert("type".to_string(), Value::String(tool.tool_type.clone()));
            if let Some(name) = &tool.name {
                item.insert("name".to_string(), Value::String(name.clone()));
            }
            if let Some(description) = &tool.description {
                item.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
            if let Some(tools) = &tool.tools {
                item.insert("tools".to_string(), Value::Array(encode_tools(tools)));
            }
            if let Some(config) = tool.config.as_ref().and_then(Value::as_object) {
                for (key, value) in config {
                    item.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        merge_extra(&mut item, &tool.extra_body);
        item.remove("namespace");
        if let Some(namespace) = &tool.namespace {
            item.insert("namespace".to_string(), json!(namespace));
        }
        out.push(Value::Object(item));
    }
    out
}
