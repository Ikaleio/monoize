use crate::urp::decode::{normalize_reasoning_effort, remove_untrusted_internal_keys, split_extra};
use crate::urp::{
    JsonSchemaDefinition, ProviderProtocol, ReasoningConfig, ResponseFormat, ToolChoice,
    ToolDefinition,
};
use serde_json::{Map, Value};

pub(super) fn reasoning_config(reasoning_obj: &Map<String, Value>) -> Option<ReasoningConfig> {
    (!reasoning_obj.is_empty()).then(|| ReasoningConfig {
        effort: reasoning_obj
            .get("effort")
            .and_then(Value::as_str)
            .map(normalize_reasoning_effort),
        summary: reasoning_obj
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string),
        extra_body: split_extra(reasoning_obj, &["effort", "summary"]),
        ..Default::default()
    })
}

pub(super) fn mark_responses_tool_origin(tool: &mut ToolDefinition) {
    if let Some(children) = &mut tool.tools {
        for child in children {
            mark_responses_tool_origin(child);
        }
    }
    if tool.function.is_some() || (tool.custom.is_none() && tool.tools.is_none()) {
        tool.origin_protocol = Some(ProviderProtocol::Responses);
    }
}

/// Converts the flat Responses selector shape (`{"type":"function","name":..}`) to the nested
/// URP shape (`{"type":"function","function":{"name":..}}`), recursively for `allowed_tools`.
fn responses_selector_to_urp(value: Value) -> Value {
    let Value::Object(mut obj) = value else {
        return value;
    };
    match obj.get("type").and_then(Value::as_str) {
        Some(kind @ ("function" | "custom")) => {
            let kind = kind.to_string();
            let flat_name = obj.remove("name");
            let mut nested = obj
                .remove(&kind)
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            if let Some(name) = flat_name {
                nested.insert("name".to_string(), name);
            }
            obj.remove(if kind == "function" {
                "custom"
            } else {
                "function"
            });
            obj.insert("type".to_string(), Value::String(kind.clone()));
            obj.insert(kind, Value::Object(nested));
        }
        Some("allowed_tools") => {
            let flat_mode = obj.remove("mode");
            let flat_tools = obj.remove("tools");
            let mut allowed = obj
                .remove("allowed_tools")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            if let Some(mode) = flat_mode {
                allowed.insert("mode".to_string(), mode);
            }
            if let Some(tools) = flat_tools {
                allowed.insert("tools".to_string(), tools);
            }
            if let Some(tools) = allowed.get_mut("tools").and_then(Value::as_array_mut) {
                for selector in tools {
                    *selector = responses_selector_to_urp(selector.take());
                }
            }
            obj.insert("allowed_tools".to_string(), Value::Object(allowed));
        }
        _ => {}
    }
    Value::Object(obj)
}

pub(super) fn tool_choice_from_value(mut value: Value) -> ToolChoice {
    remove_untrusted_internal_keys(&mut value);
    match value {
        Value::String(mode) => ToolChoice::Mode(mode),
        other => ToolChoice::Specific(responses_selector_to_urp(other)),
    }
}

pub(super) fn parse_response_format(value: &Value) -> Option<ResponseFormat> {
    let obj = value.as_object()?;
    match obj.get("type").and_then(Value::as_str)? {
        "json_schema" => {
            let schema_obj = obj
                .get("json_schema")
                .and_then(Value::as_object)
                .unwrap_or(obj);
            let name = schema_obj.get("name")?.as_str()?.to_string();
            Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchemaDefinition {
                    name,
                    description: schema_obj
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    schema: schema_obj.get("schema").cloned().unwrap_or(Value::Null),
                    strict: schema_obj.get("strict").and_then(Value::as_bool),
                    extra_body: split_extra(
                        schema_obj,
                        &["type", "name", "description", "schema", "strict"],
                    ),
                },
            })
        }
        "json_object" => Some(ResponseFormat::JsonObject),
        "text" => Some(ResponseFormat::Text),
        _ => None,
    }
}
