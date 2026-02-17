use crate::urp::encode::{
    has_encrypted_reasoning, merge_extra, role_to_str, text_parts, tool_choice_to_value,
};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Message, Part, ResponseFormat, Role, ToolDefinition,
    UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut input_items = Vec::new();
    let mut instructions: Option<String> = None;
    let mut consumed_instructions = false;

    for message in &req.messages {
        if !consumed_instructions && matches!(message.role, Role::System | Role::Developer) {
            let text = text_parts(&message.parts);
            if !text.is_empty() {
                instructions = Some(text);
                consumed_instructions = true;
                continue;
            }
        }
        encode_message_to_input_items(message, &mut input_items);
    }

    let mut body = json!({
        "model": upstream_model,
        "input": Value::Array(input_items),
    });
    let obj = body.as_object_mut().expect("responses request object");

    if let Some(text) = instructions {
        obj.insert("instructions".to_string(), Value::String(text));
    }
    if let Some(stream) = req.stream {
        obj.insert("stream".to_string(), Value::Bool(stream));
    }
    if let Some(temp) = req.temperature {
        obj.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        obj.insert("top_p".to_string(), Value::from(top_p));
    }
    if let Some(max) = req.max_output_tokens {
        obj.insert("max_output_tokens".to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning {
        let mut reasoning_obj = Map::new();
        if let Some(effort) = &reasoning.effort {
            reasoning_obj.insert("effort".to_string(), Value::String(effort.clone()));
        }
        merge_extra(&mut reasoning_obj, &reasoning.extra_body);
        if !reasoning_obj.is_empty() {
            obj.insert("reasoning".to_string(), Value::Object(reasoning_obj));
        }
    }
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(choice) = &req.tool_choice {
        obj.insert("tool_choice".to_string(), tool_choice_to_value(choice));
    }
    if let Some(user) = &req.user {
        obj.insert("user".to_string(), Value::String(user.clone()));
    }
    if let Some(format) = &req.response_format {
        apply_response_format(obj, format);
    }
    merge_extra(obj, &req.extra_body);
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut output = Vec::new();
    let mut message_content = Vec::new();

    for part in &resp.message.parts {
        match part {
            Part::Text { content, .. } => {
                message_content.push(json!({ "type": "output_text", "text": content }));
            }
            Part::Refusal { content, .. } => {
                message_content.push(json!({ "type": "refusal", "refusal": content }));
            }
            Part::Image { source, .. } => message_content.push(encode_output_image(source)),
            Part::File { source, .. } => message_content.push(encode_output_file(source)),
            Part::Reasoning { content, .. } => {
                output.push(json!({
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": content }],
                    "text": content
                }));
            }
            Part::ReasoningEncrypted { data, .. } => {
                output.push(json!({
                    "type": "reasoning",
                    "encrypted_content": data
                }));
            }
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                output.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments
                }));
            }
            Part::Audio { .. } | Part::ToolResult { .. } => {}
        }
    }

    if !message_content.is_empty() {
        output.push(json!({
            "type": "message",
            "role": "assistant",
            "content": message_content
        }));
    }

    let mut body = json!({
        "id": resp.id,
        "object": "response",
        "created": chrono::Utc::now().timestamp(),
        "model": logical_model,
        "status": finish_reason_to_status(resp.finish_reason),
        "output": output
    });

    if let Some(usage) = &resp.usage {
        body["usage"] = json!({
            "input_tokens": usage.prompt_tokens,
            "output_tokens": usage.completion_tokens,
            "total_tokens": usage.prompt_tokens + usage.completion_tokens,
            "output_tokens_details": {
                "reasoning_tokens": usage.reasoning_tokens.unwrap_or(0)
            },
            "input_tokens_details": {
                "cached_tokens": usage.cached_tokens.unwrap_or(0)
            }
        });
    }

    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &resp.extra_body);
    }
    body
}

fn encode_message_to_input_items(message: &Message, out: &mut Vec<Value>) {
    if message.role == Role::Tool {
        encode_tool_result_item(message, out);
        return;
    }

    for part in &message.parts {
        match part {
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                out.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments
                }));
            }
            Part::ReasoningEncrypted { data, .. } => {
                out.push(json!({
                    "type": "reasoning",
                    "encrypted_content": data
                }));
            }
            Part::Reasoning { content, .. } if !has_encrypted_reasoning(&message.parts) => {
                out.push(json!({
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": content }],
                    "text": content
                }));
            }
            _ => {}
        }
    }

    let content = encode_message_content(message);
    if !content.is_empty() {
        out.push(json!({
            "type": "message",
            "role": role_to_str(message.role),
            "content": content
        }));
    }
}

fn encode_tool_result_item(message: &Message, out: &mut Vec<Value>) {
    let call_id = message.parts.iter().find_map(|part| match part {
        Part::ToolResult { call_id, .. } => Some(call_id.clone()),
        _ => None,
    });
    let Some(call_id) = call_id else {
        return;
    };

    let mut tool_content = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text { content, .. } => tool_content.push(json!({
                "type": "input_text",
                "text": content
            })),
            Part::Image { source, extra_body } => {
                tool_content.push(encode_input_image(source, extra_body))
            }
            Part::File { source, extra_body } => {
                tool_content.push(encode_input_file(source, extra_body))
            }
            _ => {}
        }
    }
    if tool_content.is_empty() {
        out.push(json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": ""
        }));
    } else if tool_content.len() == 1
        && tool_content[0].get("type").and_then(|v| v.as_str()) == Some("input_text")
    {
        out.push(json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": tool_content[0].get("text").cloned().unwrap_or(Value::String(String::new()))
        }));
    } else {
        out.push(json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": Value::Array(tool_content)
        }));
    }
}

fn encode_message_content(message: &Message) -> Vec<Value> {
    let mut out = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text { content, .. } => {
                out.push(json!({ "type": "input_text", "text": content }));
            }
            Part::Image { source, extra_body } => out.push(encode_input_image(source, extra_body)),
            Part::File { source, extra_body } => out.push(encode_input_file(source, extra_body)),
            Part::Refusal { content, .. } => out.push(json!({
                "type": "refusal",
                "refusal": content
            })),
            Part::Audio { .. }
            | Part::Reasoning { .. }
            | Part::ReasoningEncrypted { .. }
            | Part::ToolCall { .. }
            | Part::ToolResult { .. } => {}
        }
    }
    out
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
            obj.insert("image_base64".to_string(), Value::String(data.clone()));
            obj.insert("media_type".to_string(), Value::String(media_type.clone()));
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

fn encode_output_image(source: &ImageSource) -> Value {
    match source {
        ImageSource::Url { url, detail } => json!({
            "type": "output_image",
            "url": url,
            "detail": detail
        }),
        ImageSource::Base64 { media_type, data } => json!({
            "type": "output_image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data
            }
        }),
    }
}

fn encode_output_file(source: &FileSource) -> Value {
    match source {
        FileSource::Url { url } => json!({
            "type": "output_file",
            "url": url
        }),
        FileSource::Base64 {
            filename,
            media_type,
            data,
        } => json!({
            "type": "output_file",
            "source": {
                "type": "base64",
                "filename": filename,
                "media_type": media_type,
                "data": data
            }
        }),
    }
}

fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                let mut item = json!({
                    "type": "function",
                    "name": function.name,
                    "parameters": function.parameters.clone().unwrap_or(json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": true
                    }))
                });
                if let Some(description) = &function.description {
                    item["description"] = Value::String(description.clone());
                }
                if let Some(strict) = function.strict {
                    item["strict"] = Value::Bool(strict);
                }
                out.push(item);
            }
        } else {
            let mut item = Map::new();
            item.insert("type".to_string(), Value::String(tool.tool_type.clone()));
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
                json!({"format": { "type": "json_schema", "name": "response", "schema": { "type": "object" } }}),
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
