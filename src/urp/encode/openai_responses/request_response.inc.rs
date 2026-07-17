fn encode_tool_result_item(
    id: Option<&str>,
    call_id: &str,
    content: &[ToolResultContent],
    _is_error: bool,
    extra_body: &HashMap<String, Value>,
    out: &mut Vec<Value>,
) {
    let mut tool_content = Vec::new();
    for item in content {
        match item {
            ToolResultContent::Text { text, extra_body } => {
                let mut block = json!({
                    "type": "input_text",
                    "text": text,
                });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                tool_content.push(block);
            }
            ToolResultContent::Image { source, extra_body } => {
                if let Some(block) = encode_input_image(source, extra_body) {
                    tool_content.push(block);
                }
            }
            ToolResultContent::File { source, extra_body } => {
                if let Some(block) = encode_input_file(source, extra_body) {
                    tool_content.push(block);
                }
            }
            ToolResultContent::ProviderItem {
                origin_protocol,
                item_type,
                body,
                extra_body,
            } => {
                if let Some(block) = encode_provider_item_for_responses(
                    *origin_protocol,
                    item_type,
                    body,
                    extra_body,
                ) {
                    tool_content.push(block);
                }
            }
        }
    }

    let mut obj = Map::new();
    obj.insert(
        "type".to_string(),
        Value::String("function_call_output".to_string()),
    );
    if let Some(id) = id {
        obj.insert(
            "id".to_string(),
            Value::String(normalize_openai_function_output_id(Some(id))),
        );
    } else {
        obj.insert(
            "id".to_string(),
            Value::String(normalize_openai_function_output_id(None)),
        );
    }
    obj.insert("call_id".to_string(), Value::String(call_id.to_string()));

    if tool_content.is_empty() {
        obj.insert("output".to_string(), Value::String(String::new()));
    } else if tool_content.len() == 1
        && tool_content[0].get("type").and_then(|v| v.as_str()) == Some("input_text")
        && tool_content[0]
            .as_object()
            .is_some_and(|obj| obj.keys().all(|key| key == "type" || key == "text"))
    {
        obj.insert(
            "output".to_string(),
            tool_content[0]
                .get("text")
                .cloned()
                .unwrap_or(Value::String(String::new())),
        );
    } else {
        obj.insert("output".to_string(), Value::Array(tool_content));
    }

    merge_extra(&mut obj, extra_body);
    out.push(Value::Object(obj));
}

fn encode_provider_item_for_responses(
    origin_protocol: ProviderProtocol,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    if origin_protocol != ProviderProtocol::Responses {
        return None;
    }
    let sanitized_body = sanitize_provider_item_wire_body(body);
    let mut item = match sanitized_body {
        Value::Object(obj) => obj,
        other => {
            let mut obj = Map::new();
            obj.insert("body".to_string(), other);
            obj
        }
    };
    item.entry("type".to_string())
        .or_insert_with(|| Value::String(item_type.to_string()));
    merge_extra(&mut item, extra_body);
    Some(Value::Object(item))
}

pub(crate) fn encode_image_generation_call_item(
    id: Option<&str>,
    source: &ImageSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let mut item = extra_body
        .get(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY)?
        .as_object()?
        .clone();
    let ImageSource::Base64 { data, .. } = source else {
        return None;
    };

    item.insert("type".to_string(), json!("image_generation_call"));
    item.insert("result".to_string(), Value::String(data.clone()));
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        item.insert("id".to_string(), Value::String(id.to_string()));
    }
    for (key, value) in extra_body {
        if key != RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY && !key.starts_with("_monoize_") {
            item.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    item.retain(|key, _| !key.starts_with("_monoize_"));
    Some(Value::Object(item))
}

fn encode_image_generation_call_part(part: &Part) -> Option<Value> {
    let Part::Image {
        source, extra_body, ..
    } = part
    else {
        return None;
    };
    encode_image_generation_call_item(None, source, extra_body)
}

fn encode_input_image(
    source: &ImageSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Option<Value> {
    match source {
        ImageSource::Url { url, detail } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_image".to_string()));
            obj.insert("image_url".to_string(), Value::String(url.clone()));
            if let Some(detail) = detail {
                obj.insert("detail".to_string(), Value::String(detail.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        ImageSource::Base64 { media_type, data } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_image".to_string()));
            obj.insert(
                "image_url".to_string(),
                Value::String(format!("data:{media_type};base64,{data}")),
            );
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        ImageSource::FileId { file_id, detail } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_image".to_string()));
            obj.insert("file_id".to_string(), Value::String(file_id.clone()));
            if let Some(detail) = detail {
                obj.insert("detail".to_string(), Value::String(detail.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
    }
}

fn encode_input_file(
    source: &FileSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Option<Value> {
    match source {
        FileSource::Url { url } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_file".to_string()));
            obj.insert("file_url".to_string(), Value::String(url.clone()));
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        FileSource::FileId { file_id } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_file".to_string()));
            obj.insert("file_id".to_string(), Value::String(file_id.clone()));
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        FileSource::Base64 {
            filename,
            media_type,
            data,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_file".to_string()));
            obj.insert("file_data".to_string(), Value::String(data.clone()));
            obj.insert("media_type".to_string(), Value::String(media_type.clone()));
            if let Some(name) = filename {
                obj.insert("filename".to_string(), Value::String(name.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        FileSource::Text { .. } | FileSource::Content { .. } => None,
    }
}

fn encode_output_image(
    source: &ImageSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Option<Value> {
    match source {
        ImageSource::Url { url, detail } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String("output_image".to_string()),
            );
            obj.insert("url".to_string(), Value::String(url.clone()));
            if let Some(detail) = detail {
                obj.insert("detail".to_string(), Value::String(detail.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        ImageSource::Base64 { media_type, data } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String("output_image".to_string()),
            );
            obj.insert(
                "source".to_string(),
                json!({
                    "type": "base64",
                    "media_type": media_type,
                    "data": data
                }),
            );
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        ImageSource::FileId { file_id, detail } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String("output_image".to_string()),
            );
            obj.insert("file_id".to_string(), Value::String(file_id.clone()));
            if let Some(detail) = detail {
                obj.insert("detail".to_string(), Value::String(detail.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
    }
}

fn encode_output_file(
    source: &FileSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Option<Value> {
    match source {
        FileSource::Url { url } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("output_file".to_string()));
            obj.insert("url".to_string(), Value::String(url.clone()));
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        FileSource::FileId { file_id } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("output_file".to_string()));
            obj.insert("file_id".to_string(), Value::String(file_id.clone()));
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        FileSource::Base64 {
            filename,
            media_type,
            data,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("output_file".to_string()));
            obj.insert(
                "source".to_string(),
                json!({
                    "type": "base64",
                    "filename": filename,
                    "media_type": media_type,
                    "data": data
                }),
            );
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        FileSource::Text { .. } | FileSource::Content { .. } => None,
    }
}
