fn encode_reasoning_item(part: &Part) -> Option<Value> {
    encode_reasoning_item_inner(part, false)
}

fn encode_reasoning_request_item(part: &Part) -> Option<Value> {
    encode_reasoning_item_inner(part, true)
}

fn encode_reasoning_item_inner(part: &Part, require_stable_id_for_encrypted: bool) -> Option<Value> {
    match part {
        Part::Reasoning {
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => {
            if require_stable_id_for_encrypted
                && extra_body
                    .get(REASONING_DOWNSTREAM_ONLY_PRESENTATION_EXTRA_KEY)
                    .and_then(Value::as_bool)
                    == Some(true)
            {
                return None;
            }
            if !require_stable_id_for_encrypted
                && !reasoning_payload_is_meaningful(content, summary, encrypted)
            {
                return None;
            }
            let mut obj = Map::new();
            let stable_id = id.clone().or_else(|| {
                extra_body
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            });
            let encrypted_len = encrypted
                .as_ref()
                .map(|value| match value {
                    Value::String(s) => s.len(),
                    other => other.to_string().len(),
                })
                .unwrap_or(0);
            if require_stable_id_for_encrypted && encrypted_len > 0 && stable_id.is_none() {
                return None;
            }
            if let Some(id) = stable_id {
                obj.insert("id".to_string(), Value::String(id));
            } else if !require_stable_id_for_encrypted {
                obj.insert(
                    "id".to_string(),
                    Value::String(format!("rs_{}", uuid::Uuid::new_v4().simple())),
                );
            }
            obj.insert("type".to_string(), Value::String("reasoning".to_string()));
            let summary_value = if let Some(raw_summary) =
                extra_body.get(RESPONSES_REASONING_SUMMARY_EXTRA_KEY)
            {
                raw_summary.clone()
            } else if let Some(summary) = summary.as_ref() {
                Value::Array(vec![json!({ "type": "summary_text", "text": summary })])
            } else {
                Value::Array(Vec::new())
            };
            obj.insert("summary".to_string(), summary_value);
            if let Some(raw_content) = extra_body.get(RESPONSES_REASONING_CONTENT_EXTRA_KEY) {
                obj.insert("content".to_string(), raw_content.clone());
            } else if let Some(content) = content {
                obj.insert(
                    "content".to_string(),
                    Value::Array(vec![json!({
                        "type": "reasoning_text",
                        "text": content
                    })]),
                );
            } else if !require_stable_id_for_encrypted {
                obj.insert("content".to_string(), Value::Array(Vec::new()));
            }
            if let Some(encrypted) = encrypted {
                obj.insert("encrypted_content".to_string(), encrypted.clone());
            }
            if !require_stable_id_for_encrypted
                && let Some(source) = source.as_ref().filter(|source| !source.is_empty())
            {
                obj.insert("source".to_string(), Value::String(source.clone()));
            }
            for (key, value) in extra_body {
                if !key.starts_with("_monoize_") {
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
    obj.retain(|key, _| !key.starts_with("_monoize_"));
    if !obj.contains_key("summary") {
        obj.insert("summary".to_string(), Value::Array(Vec::new()));
    }
}

fn ensure_responses_encrypted_reasoning_include(obj: &mut Map<String, Value>) {
    const INCLUDE_REASONING_ENCRYPTED_CONTENT: &str = "reasoning.encrypted_content";

    match obj.get_mut("include") {
        Some(Value::Array(include)) => {
            if !include
                .iter()
                .any(|value| value.as_str() == Some(INCLUDE_REASONING_ENCRYPTED_CONTENT))
            {
                include.push(Value::String(
                    INCLUDE_REASONING_ENCRYPTED_CONTENT.to_string(),
                ));
            }
        }
        _ => {
            obj.insert(
                "include".to_string(),
                Value::Array(vec![Value::String(
                    INCLUDE_REASONING_ENCRYPTED_CONTENT.to_string(),
                )]),
            );
        }
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
                    if matches!(
                        part_obj.get("type").and_then(Value::as_str),
                        Some("output_text" | "input_text" | "text")
                    ) {
                        part_obj.remove("annotations");
                        part_obj.remove("logprobs");
                        if part_obj.get("phase").and_then(Value::as_str) == Some("analysis") {
                            part_obj.remove("phase");
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn sanitize_request_input_items(input_items: &mut [Value]) {
    for item in input_items {
        sanitize_request_input_item(item);
    }
}

fn encode_tool_call_item(part: &Part) -> Option<Value> {
    match part {
        Part::ToolCall {
            id,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String("function_call".to_string()),
            );
            obj.insert(
                "id".to_string(),
                Value::String(normalize_openai_function_call_item_id(
                    id.as_deref()
                        .or_else(|| extra_body.get("id").and_then(Value::as_str)),
                )),
            );
            obj.insert("status".to_string(), Value::String("completed".to_string()));
            obj.insert("call_id".to_string(), Value::String(call_id.clone()));
            obj.insert("name".to_string(), Value::String(name.clone()));
            obj.insert("arguments".to_string(), Value::String(arguments.clone()));
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        _ => None,
    }
}
