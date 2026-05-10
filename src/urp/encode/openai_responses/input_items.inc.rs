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
            obj.insert(
                "text".to_string(),
                json!({
                    "format": {
                        "type": "json_schema",
                        "name": json_schema.name,
                        "description": json_schema.description,
                        "strict": json_schema.strict,
                        "schema": json_schema.schema
                    }
                }),
            );
        }
    }
}

fn finish_reason_to_status(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(FinishReason::Length) => "incomplete",
        Some(FinishReason::Other) => "failed",
        _ => "completed",
    }
}

