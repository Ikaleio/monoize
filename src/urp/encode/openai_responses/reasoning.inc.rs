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
                tracing::info!(
                    target: "monoize::urp::reasoning_trace",
                    encrypted_len,
                    has_content = content.as_ref().is_some_and(|v| !v.is_empty()),
                    has_summary = summary.as_ref().is_some_and(|v| !v.is_empty()),
                    "dropping responses reasoning request item without stable item id"
                );
                return None;
            }
            let id = stable_id.unwrap_or_else(|| format!("rs_{}", uuid::Uuid::new_v4().simple()));
            tracing::info!(
                target: "monoize::urp::reasoning_trace",
                item_id = %id,
                encrypted_len,
                has_content = content.as_ref().is_some_and(|v| !v.is_empty()),
                has_summary = summary.as_ref().is_some_and(|v| !v.is_empty()),
                "encoding responses reasoning request item"
            );
            obj.insert("id".to_string(), Value::String(id));
            obj.insert("type".to_string(), Value::String("reasoning".to_string()));
            obj.insert(
                "started_at".to_string(),
                Value::Number(serde_json::Number::from(chrono::Utc::now().timestamp())),
            );
            let summary_arr = if let Some(summary) = summary.as_ref() {
                vec![json!({ "type": "summary_text", "text": summary })]
            } else {
                Vec::new()
            };
            obj.insert("summary".to_string(), Value::Array(summary_arr));
            if let Some(content) = content {
                obj.insert("text".to_string(), Value::String(content.clone()));
            }
            if let Some(encrypted) = encrypted {
                let enc_str = match encrypted {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                obj.insert("encrypted_content".to_string(), Value::String(enc_str));
            }
            if let Some(source) = source {
                obj.insert("source".to_string(), Value::String(source.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        _ => None,
    }
}

fn sanitize_reasoning_request_item(item: &mut Value) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    if obj.get("type").and_then(Value::as_str) != Some("reasoning") {
        return;
    }
    let summary_present = obj
        .get("summary")
        .and_then(Value::as_array)
        .is_some_and(|arr| !arr.is_empty());
    let text_value = obj.remove("text");
    obj.remove("source");
    obj.remove("started_at");
    obj.remove("status");
    if !summary_present {
        let summary = text_value
            .and_then(|value| value.as_str().map(|text| text.to_string()))
            .filter(|text| !text.is_empty())
            .map(|text| Value::Array(vec![json!({ "type": "summary_text", "text": text })]))
            .unwrap_or_else(|| Value::Array(Vec::new()));
        obj.insert("summary".to_string(), summary);
    }
}

fn ensure_default_responses_reasoning_summary(obj: &mut Map<String, Value>) {
    let Some(existing) = obj.remove("reasoning") else {
        obj.insert("reasoning".to_string(), json!({ "summary": "detailed" }));
        return;
    };

    let Value::Object(mut reasoning_obj) = existing else {
        obj.insert("reasoning".to_string(), existing);
        return;
    };

    reasoning_obj
        .entry("summary".to_string())
        .or_insert_with(|| Value::String("detailed".to_string()));
    obj.insert("reasoning".to_string(), Value::Object(reasoning_obj));
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
