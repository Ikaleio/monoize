use crate::urp::encode::{
    extract_reasoning_plain, extract_tool_calls, merge_extra, role_to_str, text_parts,
    tool_choice_to_value,
};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Message, Part, ResponseFormat, Role, ToolDefinition,
    UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut body = json!({
        "model": upstream_model,
        "messages": encode_messages(&req.messages),
    });

    let obj = body.as_object_mut().expect("chat request object");
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
        obj.insert("max_completion_tokens".to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning {
        if let Some(effort) = &reasoning.effort {
            obj.insert(
                "reasoning_effort".to_string(),
                Value::String(effort.clone()),
            );
        }
    }
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(tc) = &req.tool_choice {
        obj.insert("tool_choice".to_string(), tool_choice_to_value(tc));
    }
    if let Some(format) = &req.response_format {
        obj.insert(
            "response_format".to_string(),
            encode_response_format(format),
        );
    }
    if let Some(user) = &req.user {
        obj.insert("user".to_string(), Value::String(user.clone()));
    }

    merge_extra(obj, &req.extra_body);
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));

    let text = text_parts(&resp.message.parts);
    if !text.is_empty() {
        message.insert("content".to_string(), Value::String(text));
    } else {
        message.insert("content".to_string(), Value::String(String::new()));
    }

    insert_openrouter_reasoning_fields(&mut message, &resp.message.parts);

    let tool_calls = extract_tool_calls(&resp.message.parts);
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }

    merge_extra(&mut message, &resp.message.extra_body);

    let finish_reason = resp
        .finish_reason
        .map(finish_reason_to_chat)
        .unwrap_or_else(|| {
            if has_tool_calls(&resp.message) {
                "tool_calls"
            } else {
                "stop"
            }
        });

    let mut result = json!({
        "id": resp.id,
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": logical_model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason,
        }],
    });

    if let Some(usage) = &resp.usage {
        result["usage"] = json!({
            "prompt_tokens": usage.prompt_tokens,
            "completion_tokens": usage.completion_tokens,
            "total_tokens": usage.prompt_tokens + usage.completion_tokens,
            "completion_tokens_details": {
                "reasoning_tokens": usage.reasoning_tokens.unwrap_or(0)
            },
            "prompt_tokens_details": {
                "cached_tokens": usage.cached_tokens.unwrap_or(0)
            }
        });
    }

    let obj = result.as_object_mut().expect("chat response object");
    merge_extra(obj, &resp.extra_body);
    result
}

fn encode_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for msg in messages {
        if msg.role == Role::Tool {
            let call_id = msg.parts.iter().find_map(|p| match p {
                Part::ToolResult { call_id, .. } => Some(call_id.clone()),
                _ => None,
            });
            let mut m = Map::new();
            m.insert("role".to_string(), Value::String("tool".to_string()));
            m.insert("content".to_string(), Value::String(text_parts(&msg.parts)));
            if let Some(call_id) = call_id {
                m.insert("tool_call_id".to_string(), Value::String(call_id));
            }
            merge_extra(&mut m, &msg.extra_body);
            out.push(Value::Object(m));
            continue;
        }

        let mut m = Map::new();
        m.insert(
            "role".to_string(),
            Value::String(role_to_str(msg.role).to_string()),
        );

        let mut content_parts = Vec::new();
        for part in &msg.parts {
            match part {
                Part::Text { content, .. } => {
                    content_parts.push(json!({ "type": "text", "text": content }));
                }
                Part::Image { source, .. } => {
                    let image = match source {
                        ImageSource::Url { url, detail } => {
                            json!({ "type": "image_url", "image_url": { "url": url, "detail": detail } })
                        }
                        ImageSource::Base64 { media_type, data } => json!({
                            "type": "image_url",
                            "image_url": { "url": format!("data:{};base64,{}", media_type, data) }
                        }),
                    };
                    content_parts.push(image);
                }
                Part::File { source, .. } => {
                    let text = match source {
                        FileSource::Url { url } => format!("[file:{}]", url),
                        FileSource::Base64 {
                            filename,
                            media_type,
                            ..
                        } => {
                            format!(
                                "[file:{}:{}]",
                                filename.clone().unwrap_or_else(|| "file".to_string()),
                                media_type
                            )
                        }
                    };
                    content_parts.push(json!({ "type": "text", "text": text }));
                }
                Part::Reasoning { .. } | Part::ReasoningEncrypted { .. } => {}
                Part::ToolCall { .. } => {}
                Part::Refusal { content, .. } => {
                    m.insert("refusal".to_string(), Value::String(content.clone()));
                }
                _ => {}
            }
        }

        let tool_calls = extract_tool_calls(&msg.parts);
        if !tool_calls.is_empty() {
            m.insert("tool_calls".to_string(), Value::Array(tool_calls));
        }

        if !content_parts.is_empty() {
            if content_parts.len() == 1
                && content_parts[0].get("type").and_then(|v| v.as_str()) == Some("text")
            {
                if let Some(text) = content_parts[0].get("text").and_then(|v| v.as_str()) {
                    m.insert("content".to_string(), Value::String(text.to_string()));
                }
            } else {
                m.insert("content".to_string(), Value::Array(content_parts));
            }
        } else {
            m.insert("content".to_string(), Value::String(String::new()));
        }

        insert_openrouter_reasoning_fields(&mut m, &msg.parts);
        merge_extra(&mut m, &msg.extra_body);
        out.push(Value::Object(m));
    }
    out
}

fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                let mut fn_obj = Map::new();
                fn_obj.insert("name".to_string(), Value::String(function.name.clone()));
                if let Some(desc) = &function.description {
                    fn_obj.insert("description".to_string(), Value::String(desc.clone()));
                }
                if let Some(parameters) = &function.parameters {
                    fn_obj.insert("parameters".to_string(), parameters.clone());
                }
                if let Some(strict) = function.strict {
                    fn_obj.insert("strict".to_string(), Value::Bool(strict));
                }
                super::merge_extra(&mut fn_obj, &function.extra_body);

                let mut obj = Map::new();
                obj.insert("type".to_string(), Value::String("function".to_string()));
                obj.insert("function".to_string(), Value::Object(fn_obj));
                super::merge_extra(&mut obj, &tool.extra_body);
                out.push(Value::Object(obj));
            }
        } else {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String(tool.tool_type.clone()));
            super::merge_extra(&mut obj, &tool.extra_body);
            out.push(Value::Object(obj));
        }
    }
    out
}

fn encode_response_format(format: &ResponseFormat) -> Value {
    match format {
        ResponseFormat::Text => json!({ "type": "text" }),
        ResponseFormat::JsonObject => json!({ "type": "json_object" }),
        ResponseFormat::JsonSchema { json_schema } => {
            let mut schema_obj = Map::new();
            schema_obj.insert("name".to_string(), Value::String(json_schema.name.clone()));
            schema_obj.insert("schema".to_string(), json_schema.schema.clone());
            if let Some(desc) = &json_schema.description {
                schema_obj.insert("description".to_string(), Value::String(desc.clone()));
            }
            if let Some(strict) = json_schema.strict {
                schema_obj.insert("strict".to_string(), Value::Bool(strict));
            }
            super::merge_extra(&mut schema_obj, &json_schema.extra_body);
            json!({
                "type": "json_schema",
                "json_schema": Value::Object(schema_obj),
            })
        }
    }
}

fn has_tool_calls(message: &Message) -> bool {
    message
        .parts
        .iter()
        .any(|p| matches!(p, Part::ToolCall { .. }))
}

fn insert_openrouter_reasoning_fields(message: &mut Map<String, Value>, parts: &[Part]) {
    let reasoning_text = extract_reasoning_plain(parts);
    let encrypted = super::extract_reasoning_encrypted(parts);
    let mut details = Vec::new();

    if !reasoning_text.is_empty() {
        message.insert(
            "reasoning".to_string(),
            Value::String(reasoning_text.clone()),
        );

        let signature = encrypted.as_ref().and_then(|v| v.as_str());
        details.push(json!({
            "type": "reasoning.text",
            "text": reasoning_text,
            "signature": signature,
            "format": "unknown"
        }));

        if let Some(enc) = encrypted {
            if !enc.is_string() && !matches!(enc, Value::Null) {
                details.push(json!({
                    "type": "reasoning.encrypted",
                    "data": enc,
                    "format": "unknown"
                }));
            }
        }
    } else if let Some(enc) = encrypted {
        if !matches!(enc, Value::Null) {
            if let Some(s) = enc.as_str() {
                if s.is_empty() {
                    return;
                }
            }
            details.push(json!({
                "type": "reasoning.encrypted",
                "data": enc,
                "format": "unknown"
            }));
        }
    }

    if !details.is_empty() {
        message.insert("reasoning_details".to_string(), Value::Array(details));
    }
}

fn finish_reason_to_chat(finish_reason: FinishReason) -> &'static str {
    match finish_reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other => "stop",
    }
}
