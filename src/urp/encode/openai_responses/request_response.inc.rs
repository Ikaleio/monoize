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
            ToolResultContent::Text { text } => {
                tool_content.push(json!({
                    "type": "input_text",
                    "text": text,
                }));
            }
            ToolResultContent::Image { source } => {
                tool_content.push(encode_input_image(source, &HashMap::new()));
            }
            ToolResultContent::File { source } => {
                tool_content.push(encode_input_file(source, &HashMap::new()));
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

fn encode_input_image(
    source: &ImageSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Value {
    match source {
        ImageSource::Url { url, detail } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_image".to_string()));
            obj.insert("image_url".to_string(), Value::String(url.clone()));
            if let Some(detail) = detail {
                obj.insert("detail".to_string(), Value::String(detail.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        ImageSource::Base64 { media_type, data } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_image".to_string()));
            obj.insert(
                "image_url".to_string(),
                Value::String(format!("data:{media_type};base64,{data}")),
            );
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
    }
}

fn encode_input_file(
    source: &FileSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Value {
    if let Some(file_id) = extra_body.get("file_id").and_then(|v| v.as_str()) {
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("input_file".to_string()));
        obj.insert("file_id".to_string(), Value::String(file_id.to_string()));
        merge_extra(&mut obj, extra_body);
        return Value::Object(obj);
    }

    match source {
        FileSource::Url { url } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_file".to_string()));
            if let Some(file_id) = url.strip_prefix("file_id://") {
                obj.insert("file_id".to_string(), Value::String(file_id.to_string()));
            } else {
                obj.insert("file_url".to_string(), Value::String(url.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
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
            Value::Object(obj)
        }
    }
}

fn encode_output_image(
    source: &ImageSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Value {
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
            Value::Object(obj)
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
            Value::Object(obj)
        }
    }
}

fn encode_output_file(
    source: &FileSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Value {
    match source {
        FileSource::Url { url } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("output_file".to_string()));
            obj.insert("url".to_string(), Value::String(url.clone()));
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
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
            Value::Object(obj)
        }
    }
}

