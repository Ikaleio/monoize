use crate::urp::encode::{merge_extra, tool_choice_to_value};
use crate::urp::{
    FunctionDefinition, Node, ProviderProtocol, ToolCallType, ToolChoice, ToolDefinition,
    UrpRequest,
};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

pub(super) fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools.iter().filter_map(encode_tool).collect()
}

fn encode_tool(tool: &ToolDefinition) -> Option<Value> {
    let mut item = Map::new();
    match tool.tool_type.as_str() {
        "function" => {
            let function = tool.function.as_ref()?;
            item.insert("name".to_string(), Value::String(function.name.clone()));
            if let Some(description) = &function.description {
                item.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
            item.insert(
                "input_schema".to_string(),
                function.parameters.clone().unwrap_or_else(|| {
                    json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": true
                    })
                }),
            );
            if let Some(strict) = function.strict {
                item.insert("strict".to_string(), Value::Bool(strict));
            }
            merge_extra(&mut item, &function.extra_body);
        }
        "custom" => {
            let custom = tool.custom.as_ref()?;
            item.insert("type".to_string(), Value::String("custom".to_string()));
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
        }
        // A server tool from another protocol family has no Messages form.
        _ if tool
            .origin_protocol
            .is_some_and(|origin| origin != ProviderProtocol::Messages) =>
        {
            return None;
        }
        tool_type => {
            if let Some(config) = tool.config.as_ref().and_then(Value::as_object) {
                item.clone_from(config);
            }
            item.remove("name");
            item.remove("description");
            item.insert("type".to_string(), Value::String(tool_type.to_string()));
            if let Some(name) = &tool.name {
                item.insert("name".to_string(), Value::String(name.clone()));
            }
            if let Some(description) = &tool.description {
                item.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
        }
    }
    merge_extra(&mut item, &tool.extra_body);
    Some(Value::Object(item))
}

pub(super) fn encode_tool_choice(choice: &ToolChoice, parallel_tool_calls: Option<bool>) -> Value {
    match tool_choice_to_value(choice) {
        Value::String(mode) => match mode.as_str() {
            "auto" => tool_choice_object("auto", parallel_tool_calls),
            "required" => tool_choice_object("any", parallel_tool_calls),
            "none" => json!({ "type": "none" }),
            _ => Value::String(mode),
        },
        Value::Object(mut obj) => {
            obj.remove("disable_parallel_tool_use");
            if let Some(name) = obj
                .get("function")
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                obj.remove("function");
                obj.insert("type".into(), json!("tool"));
                obj.insert("name".into(), json!(name));
            } else if obj.get("type").and_then(Value::as_str) == Some("required") {
                obj.insert("type".into(), json!("any"));
            }
            if obj.get("type").and_then(Value::as_str) != Some("none") {
                insert_disable_parallel(&mut obj, parallel_tool_calls);
            }
            Value::Object(obj)
        }
        other => other,
    }
}

fn tool_choice_object(mode: &str, parallel_tool_calls: Option<bool>) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String(mode.to_string()));
    insert_disable_parallel(&mut obj, parallel_tool_calls);
    Value::Object(obj)
}

fn insert_disable_parallel(obj: &mut Map<String, Value>, parallel_tool_calls: Option<bool>) {
    if let Some(enabled) = parallel_tool_calls {
        obj.insert(
            "disable_parallel_tool_use".to_string(),
            Value::Bool(!enabled),
        );
    }
}

/// Rewrites custom tools that declare an object `input_schema` as function tools, with
/// their calls, results, and a forced choice, since Messages has no freeform tool form.
pub(super) fn prepare_schema_custom_tools(req: &mut UrpRequest) -> Result<(), String> {
    let mut identities = HashSet::new();
    for tool in req.tools.iter_mut().flatten() {
        if tool.tool_type != "custom" {
            continue;
        }
        let Some(custom) = &mut tool.custom else {
            continue;
        };
        let Some(schema) = custom
            .extra_body
            .remove("input_schema")
            .or_else(|| tool.extra_body.remove("input_schema"))
        else {
            continue;
        };
        tool.extra_body.remove("input_schema");
        if !schema.is_object() || schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(format!(
                "Messages custom tool {} requires an object input_schema",
                custom.name
            ));
        }
        if let Some(custom) = tool.custom.take() {
            identities.insert((tool.namespace.clone(), custom.name.clone()));
            tool.tool_type = "function".to_string();
            tool.function = Some(FunctionDefinition {
                response_schema: None,
                name: custom.name,
                description: custom.description,
                parameters: Some(schema),
                strict: None,
                extra_body: custom.extra_body,
            });
        }
    }
    if identities.is_empty() {
        return Ok(());
    }
    let mut call_ids = HashSet::new();
    for node in &mut req.input {
        if let Node::ToolCall {
            tool_type,
            namespace,
            name,
            call_id,
            arguments,
            ..
        } = node
            && *tool_type == ToolCallType::Custom
            && identities.contains(&(namespace.clone(), name.clone()))
        {
            if !serde_json::from_str::<Value>(arguments).is_ok_and(|value| value.is_object()) {
                return Err(format!(
                    "Messages custom tool {name} input must be a complete JSON object"
                ));
            }
            *tool_type = ToolCallType::Function;
            call_ids.insert(call_id.clone());
        }
    }
    for node in &mut req.input {
        if let Node::ToolResult {
            tool_type, call_id, ..
        } = node
            && *tool_type == ToolCallType::Custom
            && call_ids.contains(call_id)
        {
            *tool_type = ToolCallType::Function;
        }
    }
    if let Some(ToolChoice::Specific(Value::Object(choice))) = &mut req.tool_choice
        && choice.get("type").and_then(Value::as_str) == Some("custom")
    {
        let name = choice
            .get("name")
            .or_else(|| choice.get("custom").and_then(|custom| custom.get("name")))
            .and_then(Value::as_str);
        let namespace = choice
            .get("namespace")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(name) = name
            && identities.contains(&(namespace, name.to_string()))
        {
            *choice = Map::from_iter([
                ("type".to_string(), json!("function")),
                ("function".to_string(), json!({"name": name})),
            ]);
        }
    }
    Ok(())
}
