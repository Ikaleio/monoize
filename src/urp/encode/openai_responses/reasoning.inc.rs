fn encode_reasoning_item(part: &Part) -> Option<Value> {
    encode_reasoning_item_inner(part, false)
}

fn encode_reasoning_request_item(part: &Part) -> Option<Value> {
    encode_reasoning_item_inner(part, true)
}

fn encode_reasoning_item_inner(part: &Part, request_item: bool) -> Option<Value> {
    match part {
        Part::Reasoning {
            metadata,
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => {
            if request_item && metadata.downstream_only {
                return None;
            }
            if !request_item && !reasoning_payload_is_meaningful(content, summary, encrypted) {
                return None;
            }
            let mut obj = Map::new();
            let stable_id = id.clone();
            if let Some(id) = stable_id {
                obj.insert("id".to_string(), Value::String(id));
            } else if !request_item {
                obj.insert(
                    "id".to_string(),
                    Value::String(format!("rs_{}", uuid::Uuid::new_v4().simple())),
                );
            }
            obj.insert("type".to_string(), Value::String("reasoning".to_string()));
            obj.insert(
                "summary".into(),
                crate::urp::reasoning::encode_text_parts(
                    summary.as_deref(),
                    metadata.summary_parts.as_deref(),
                    "summary_text",
                ),
            );
            if content.is_some() || !request_item {
                obj.insert(
                    "content".into(),
                    crate::urp::reasoning::encode_text_parts(
                        content.as_deref(),
                        metadata.content_parts.as_deref(),
                        "reasoning_text",
                    ),
                );
            }
            if let Some(encrypted) = encrypted {
                obj.insert("encrypted_content".to_string(), encrypted.clone());
            }
            if !request_item
                && let Some(source) = source.as_ref().filter(|source| !source.is_empty())
            {
                obj.insert("source".to_string(), Value::String(source.clone()));
            }
            for (key, value) in extra_body {
                if !key.starts_with("_monoize_")
                    && !matches!(
                        key.as_str(),
                        "id" | "type"
                            | "text"
                            | "content"
                            | "summary"
                            | "encrypted_content"
                            | "source"
                    )
                {
                    obj.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            Some(Value::Object(obj))
        }
        _ => None,
    }
}

fn reasoning_payload_is_meaningful(
    content: &Option<String>,
    summary: &Option<String>,
    encrypted: &Option<Value>,
) -> bool {
    content.as_ref().is_some_and(|value| !value.is_empty())
        || summary.as_ref().is_some_and(|value| !value.is_empty())
        || encrypted.as_ref().is_some_and(non_empty_json_value)
}

fn non_empty_json_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

fn sanitize_reasoning_request_item(item: &mut Value) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    if obj.get("type").and_then(Value::as_str) != Some("reasoning") {
        return;
    }
    obj.remove("text");
    obj.remove("source");
    obj.remove("started_at");
    obj.remove("status");
    obj.remove("duration");
    obj.retain(|key, _| !key.starts_with("_monoize_"));
    if !obj.contains_key("summary") {
        obj.insert("summary".to_string(), Value::Array(Vec::new()));
    }
}

fn sanitize_request_input_item(item: &mut Value) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };

    match obj.get("type").and_then(Value::as_str) {
        Some("reasoning") => sanitize_reasoning_request_item(item),
        Some("message") => {
            obj.remove("status");
            obj.remove("annotations");
            obj.remove("logprobs");
            if obj.get("phase").and_then(Value::as_str) == Some("analysis") {
                obj.remove("phase");
            }
            if let Some(Value::Array(content)) = obj.get_mut("content") {
                for part in content {
                    let Some(part_obj) = part.as_object_mut() else {
                        continue;
                    };
                    let part_type = part_obj.get("type").and_then(Value::as_str);
                    if matches!(part_type, Some("input_text" | "text")) {
                        part_obj.remove("annotations");
                        part_obj.remove("logprobs");
                    }
                    if part_obj.get("phase").and_then(Value::as_str) == Some("analysis") {
                        part_obj.remove("phase");
                    }
                }
            }
        }
        Some("custom_tool_call") => {
            obj.remove("status");
        }
        _ => {}
    }
}

fn sanitize_request_input_items(input_items: &mut [Value]) {
    for item in input_items {
        sanitize_request_input_item(item);
    }
}

fn encode_tool_call_item(part: &Part, output_item: bool) -> Option<Value> {
    match part {
        Part::ToolCall {
            namespace,
            signature: _,
            id,
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String(
                    match tool_type {
                        ToolCallType::Function => "function_call",
                        ToolCallType::Custom => "custom_tool_call",
                    }
                    .to_string(),
                ),
            );
            let item_id = id.as_deref();
            if let Some(item_id) = item_id {
                obj.insert(
                    "id".to_string(),
                    Value::String(if output_item {
                        match tool_type {
                            ToolCallType::Function => {
                                normalize_openai_function_call_item_id(Some(item_id))
                            }
                            ToolCallType::Custom => item_id.to_string(),
                        }
                    } else {
                        item_id.to_string()
                    }),
                );
            } else if output_item {
                obj.insert(
                    "id".to_string(),
                    Value::String(match tool_type {
                        ToolCallType::Function => normalize_openai_function_call_item_id(None),
                        ToolCallType::Custom => {
                            format!("ctc_urp_{}", uuid::Uuid::new_v4().simple())
                        }
                    }),
                );
            }
            if output_item {
                obj.insert("status".to_string(), Value::String("completed".to_string()));
            }
            obj.insert("call_id".to_string(), Value::String(call_id.clone()));
            obj.insert("name".to_string(), Value::String(name.clone()));
            if let Some(namespace) = namespace { obj.insert("namespace".to_string(), json!(namespace)); }
            obj.insert(
                match tool_type {
                    ToolCallType::Function => "arguments",
                    ToolCallType::Custom => "input",
                }
                .to_string(),
                Value::String(if *tool_type == ToolCallType::Custom { arguments.clone() } else { crate::urp::tool_call_arguments_for_wire(arguments) }),
            );
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        _ => None,
    }
}
