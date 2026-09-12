fn encode_tool_result_item(
    id: Option<&str>,
    tool_type: ToolCallType,
    call_id: &str,
    namespace: Option<&str>,
    name: Option<&str>,
    content: &[ToolResultContent],
    _is_error: bool,
    extra_body: &HashMap<String, Value>,
    output_item: bool,
    out: &mut Vec<Value>,
) {
    let mut obj = Map::new();
    obj.insert(
        "type".to_string(),
        Value::String(
            match tool_type {
                ToolCallType::Function => "function_call_output",
                ToolCallType::Custom => "custom_tool_call_output",
            }
            .to_string(),
        ),
    );
    if let Some(id) = id {
        obj.insert(
            "id".to_string(),
            Value::String(if output_item {
                match tool_type {
                    ToolCallType::Function => normalize_openai_function_output_id(Some(id)),
                    ToolCallType::Custom => id.to_string(),
                }
            } else {
                id.to_string()
            }),
        );
    } else if output_item {
        obj.insert(
            "id".to_string(),
            Value::String(match tool_type {
                ToolCallType::Function => normalize_openai_function_output_id(None),
                ToolCallType::Custom => {
                    format!("ctco_urp_{}", uuid::Uuid::new_v4().simple())
                }
            }),
        );
    }
    obj.insert("call_id".to_string(), Value::String(call_id.to_string()));

    obj.insert("output".to_string(), encode_tool_result_output(content));

    merge_extra(&mut obj, extra_body);
    obj.remove("namespace"); obj.remove("name");
    if let Some(namespace) = namespace { obj.insert("namespace".into(), json!(namespace)); }
    if let Some(name) = name { obj.insert("name".into(), json!(name)); }
    if id.is_none() && !output_item { obj.remove("id"); }
    out.push(Value::Object(obj));
}

pub(crate) fn encode_tool_result_output(content: &[ToolResultContent]) -> Value {
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
            ToolResultContent::Image { metadata, source, extra_body } => {
                if let Some(block) = encode_input_image(source, metadata, extra_body) {
                    tool_content.push(block);
                }
            }
            ToolResultContent::File { metadata, source, extra_body } => {
                if let Some(block) = encode_input_file(source, metadata, extra_body) {
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
                    None,
                ) {
                    tool_content.push(block);
                }
            }
        }
    }

    if tool_content.is_empty() {
        Value::String(String::new())
    } else if tool_content.len() == 1
        && tool_content[0].get("type").and_then(|v| v.as_str()) == Some("input_text")
        && tool_content[0]
            .as_object()
            .is_some_and(|obj| obj.keys().all(|key| key == "type" || key == "text"))
    {
        tool_content[0].get("text").cloned().unwrap_or_else(|| json!(""))
    } else {
        Value::Array(tool_content)
    }
}

pub(crate) fn encode_provider_item_for_responses(
    origin_protocol: ProviderProtocol,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
    id: Option<&Option<String>>,
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
    item.insert("type".to_string(), Value::String(item_type.to_string()));
    if let Some(id) = id {
        let had_id = item.remove("id").is_some();
        if had_id && let Some(id) = id { item.insert("id".to_string(), json!(id)); }
    }
    merge_extra(&mut item, extra_body);
    Some(Value::Object(item))
}

pub(crate) fn encode_image_generation_call_item(
    id: Option<&str>,
    source: &ImageSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    extra_body.get(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY)?;
    let mut item = Map::new();
    let ImageSource::Base64 { data, media_type } = source else {
        return None;
    };

    item.insert("type".to_string(), json!("image_generation_call"));
    item.insert("result".to_string(), Value::String(data.clone()));
    item.insert("output_format".to_string(), json!(match media_type.as_str() {
        "image/webp" => "webp",
        "image/jpeg" => "jpeg",
        _ => "png",
    }));
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        item.insert("id".to_string(), Value::String(id.to_string()));
    }
    for (key, value) in extra_body {
        if key != RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY && !key.starts_with("_monoize_")
            && !matches!(key.as_str(), "id" | "result" | "output_format" | "type") {
            item.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    item.retain(|key, _| !key.starts_with("_monoize_"));
    Some(Value::Object(item))
}

fn encode_image_generation_call_part(part: &Part, id: Option<&str>) -> Option<Value> {
    let Part::Image {
        source, extra_body, ..
    } = part
    else {
        return None;
    };
    encode_image_generation_call_item(id, source, extra_body)
}

pub(crate) fn encode_input_image(
    source: &ImageSource,
    metadata: &crate::urp::MediaMetadata,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let mut obj = Map::new();
    merge_extra(&mut obj, extra_body);
    for key in ["type", "source", "image_url", "url", "file_id", "detail", "filename", "media_type"] {
        obj.remove(key);
    }
    obj.insert("type".into(), json!("input_image"));
    let detail = match source {
        ImageSource::Url { url, detail } => {
            obj.insert("image_url".into(), json!(url));
            detail.as_ref()
        }
        ImageSource::Base64 { media_type, data } => {
            obj.insert("image_url".into(), json!(format!("data:{media_type};base64,{data}")));
            metadata.detail.as_ref()
        }
        ImageSource::FileId { file_id, detail }
            if crate::urp::media::resource_matches(metadata, ProviderProtocol::Responses) => {
            obj.insert("file_id".into(), json!(file_id));
            detail.as_ref()
        }
        ImageSource::FileId { .. } => return None,
    };
    if let Some(detail) = detail { obj.insert("detail".into(), json!(detail)); }
    Some(Value::Object(obj))
}

pub(crate) fn encode_input_file(
    source: &FileSource,
    metadata: &crate::urp::MediaMetadata,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let mut obj = Map::new();
    merge_extra(&mut obj, extra_body);
    for key in ["type", "source", "file_url", "url", "file_id", "file_data", "filename", "detail", "media_type"] {
        obj.remove(key);
    }
    obj.insert("type".into(), json!("input_file"));
    match source {
        FileSource::Url { url } => { obj.insert("file_url".into(), json!(url)); }
        FileSource::FileId { file_id }
            if crate::urp::media::resource_matches(metadata, ProviderProtocol::Responses) => {
            obj.insert("file_id".into(), json!(file_id));
        }
        FileSource::Base64 { media_type, data } => {
            obj.insert("file_data".into(), json!(format!("data:{media_type};base64,{data}")));
        }
        FileSource::FileId { .. } | FileSource::Text { .. } | FileSource::Content { .. } => return None,
    }
    if let Some(filename) = &metadata.filename { obj.insert("filename".into(), json!(filename)); }
    if let Some(detail) = &metadata.detail { obj.insert("detail".into(), json!(detail)); }
    Some(Value::Object(obj))
}
