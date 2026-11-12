use super::{ChatDialect, ChatEncodeError, flag_set, merge_chat_wire_extra};
use crate::urp::encode::sanitize_provider_item_wire_body;
use crate::urp::{
    CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY, ProviderProtocol, ToolChoice, ToolDefinition,
    is_internal_key,
};
use serde_json::{Map, Value, json};

pub(crate) fn is_web_search_builtin(tool_type: &str) -> bool {
    matches!(tool_type, "web_search" | "web_search_preview")
        || tool_type
            .strip_prefix("web_search_")
            .is_some_and(|date| date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()))
}

fn is_legacy_function_definition(tool: &ToolDefinition) -> bool {
    flag_set(&tool.extra_body, CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY)
}

/// PC3. Returns the Chat tool descriptors and the names of degraded (dropped) tools.
pub(super) fn encode_tools(
    tools: &[ToolDefinition],
    dialect: ChatDialect,
) -> Result<(Vec<Value>, Vec<String>), ChatEncodeError> {
    let mut out = Vec::new();
    let mut dropped = Vec::new();
    for tool in tools {
        if is_legacy_function_definition(tool) {
            continue;
        }
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                let mut fn_obj = Map::new();
                fn_obj.insert("name".to_string(), Value::String(function.name.clone()));
                if let Some(desc) = &function.description {
                    fn_obj.insert("description".to_string(), Value::String(desc.clone()));
                }
                if let Some(parameters) = &function.parameters {
                    fn_obj.insert("parameters".to_string(), parameters.clone());
                }
                if let Some(strict) = function.strict {
                    fn_obj.insert("strict".to_string(), Value::Bool(strict));
                }
                merge_chat_wire_extra(&mut fn_obj, &function.extra_body);

                let mut obj = Map::new();
                obj.insert("type".to_string(), Value::String("function".to_string()));
                obj.insert("function".to_string(), Value::Object(fn_obj));
                merge_chat_wire_extra(&mut obj, &tool.extra_body);
                out.push(Value::Object(obj));
            }
        } else if tool.tool_type == "custom"
            && let Some(custom) = &tool.custom
        {
            let mut custom_obj = Map::new();
            custom_obj.insert("name".to_string(), Value::String(custom.name.clone()));
            if let Some(desc) = &custom.description {
                custom_obj.insert("description".to_string(), Value::String(desc.clone()));
            }
            if let Some(format) = &custom.format {
                custom_obj.insert("format".to_string(), format.clone());
            }
            merge_chat_wire_extra(&mut custom_obj, &custom.extra_body);

            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("custom".to_string()));
            obj.insert("custom".to_string(), Value::Object(custom_obj));
            merge_chat_wire_extra(&mut obj, &tool.extra_body);
            out.push(Value::Object(obj));
        } else if tool.origin_protocol == Some(ProviderProtocol::ChatCompletion)
            && tool.tool_type.starts_with("openrouter:")
        {
            if !matches!(dialect, ChatDialect::OpenRouter | ChatDialect::Generic) {
                return Err(ChatEncodeError {
                    code: "unsupported_tool",
                    message: format!(
                        "tool type `{}` is not available on this Chat Completions upstream",
                        tool.tool_type
                    ),
                });
            }
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!(tool.tool_type));
            if let Some(name) = &tool.name {
                obj.insert("name".to_string(), json!(name));
            }
            if let Some(description) = &tool.description {
                obj.insert("description".to_string(), json!(description));
            }
            if let Some(Value::Object(config)) = &tool.config {
                for (key, value) in config {
                    if !is_internal_key(key) {
                        obj.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
            merge_chat_wire_extra(&mut obj, &tool.extra_body);
            out.push(Value::Object(obj));
        } else if is_web_search_builtin(&tool.tool_type) {
            if dialect == ChatDialect::OpenRouter {
                out.push(json!({ "type": "openrouter:web_search" }));
            }
            dropped.push(tool.name.clone().unwrap_or_else(|| tool.tool_type.clone()));
            dropped.push(tool.tool_type.clone());
        } else {
            return Err(ChatEncodeError {
                code: "unsupported_tool",
                message: format!(
                    "tool type `{}` cannot be encoded for a Chat Completions upstream",
                    tool.tool_type
                ),
            });
        }
    }
    Ok((out, dropped))
}

/// True when a specific tool choice selects one of `names` (by `type` or nested/flat `name`).
pub(super) fn tool_choice_names_any(choice: &ToolChoice, names: &[String]) -> bool {
    let ToolChoice::Specific(Value::Object(selector)) = choice else {
        return false;
    };
    let selected = [
        selector.get("type").and_then(Value::as_str),
        selector.get("name").and_then(Value::as_str),
        selector
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str),
    ];
    selected
        .into_iter()
        .flatten()
        .any(|selected| names.iter().any(|name| name == selected))
}

/// PC2.9: the deprecated top-level `functions[]` array.
pub(super) fn encode_legacy_functions(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type != "function" || !is_legacy_function_definition(tool) {
            continue;
        }
        let Some(function) = &tool.function else {
            continue;
        };

        let mut function_obj = Map::new();
        function_obj.insert("name".to_string(), Value::String(function.name.clone()));
        if let Some(description) = &function.description {
            function_obj.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        if let Some(parameters) = &function.parameters {
            function_obj.insert("parameters".to_string(), parameters.clone());
        }
        if let Some(strict) = function.strict {
            function_obj.insert("strict".to_string(), Value::Bool(strict));
        }
        merge_chat_wire_extra(&mut function_obj, &function.extra_body);
        merge_chat_wire_extra(&mut function_obj, &tool.extra_body);
        out.push(Value::Object(function_obj));
    }
    out
}

/// PC2.9: the deprecated top-level `function_call` control.
pub(super) fn encode_legacy_function_choice(
    choice: &ToolChoice,
    raw_choice: &Value,
) -> Option<Value> {
    match choice {
        ToolChoice::Mode(mode) => Some(Value::String(mode.clone())),
        ToolChoice::Specific(Value::Object(choice_obj)) => {
            let name = choice_obj
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .or_else(|| choice_obj.get("name"))
                .and_then(Value::as_str)?;
            let mut legacy_choice = match sanitize_provider_item_wire_body(raw_choice) {
                Value::Object(legacy_choice) => legacy_choice,
                _ => Map::new(),
            };
            legacy_choice.insert("name".to_string(), Value::String(name.to_string()));
            Some(Value::Object(legacy_choice))
        }
        ToolChoice::Specific(_) => None,
    }
}
