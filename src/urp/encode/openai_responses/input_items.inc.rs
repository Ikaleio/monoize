fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type == "function" {
            let Some(function) = &tool.function else {
                continue;
            };

            let mut item = Map::new();
            item.insert("type".to_string(), Value::String("function".to_string()));
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
            }
            merge_extra(&mut item, &function.extra_body);
            merge_extra(&mut item, &tool.extra_body);
            out.push(Value::Object(item));
        } else if tool.tool_type == "custom" {
            let Some(custom) = &tool.custom else {
                continue;
            };

            let mut item = Map::new();
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
            merge_extra(&mut item, &tool.extra_body);
            out.push(Value::Object(item));
        } else {
            let mut item = Map::new();
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
            merge_extra(&mut item, &tool.extra_body);
            out.push(Value::Object(item));
        }
    }
    out
}

fn apply_response_format(obj: &mut Map<String, Value>, format: &ResponseFormat) {
    match format {
        ResponseFormat::Text => {
            obj.insert("text".to_string(), json!({"format": { "type": "text" }}));
        }
        ResponseFormat::JsonObject => {
            obj.insert(
                "text".to_string(),
                json!({"format": { "type": "json_object" }}),
            );
        }
        ResponseFormat::JsonSchema { json_schema } => {
            let mut format = Map::new();
            format.insert("type".to_string(), json!("json_schema"));
            format.insert("name".to_string(), json!(json_schema.name));
            format.insert("schema".to_string(), json_schema.schema.clone());
            if let Some(description) = &json_schema.description {
                format.insert("description".to_string(), json!(description));
            }
            if let Some(strict) = json_schema.strict {
                format.insert("strict".to_string(), json!(strict));
            }
            merge_extra(&mut format, &json_schema.extra_body);
            obj.insert("text".to_string(), json!({ "format": format }));
        }
    }
}

fn merge_responses_text_config(obj: &mut Map<String, Value>, raw_text: Option<&Value>) {
    let Some(raw_text) = raw_text.and_then(Value::as_object) else {
        return;
    };
    let generated = obj.get("text").and_then(Value::as_object);
    let mut merged = raw_text.clone();
    merged.retain(|key, _| !key.starts_with("_monoize_"));
    if let Some(generated) = generated {
        for (key, value) in generated {
            merged.insert(key.clone(), value.clone());
        }
    }
    obj.insert("text".to_string(), Value::Object(merged));
}

fn finish_reason_to_status(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(FinishReason::Length) => "incomplete",
        Some(FinishReason::Other) => "failed",
        _ => "completed",
    }
}
