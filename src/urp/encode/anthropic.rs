use crate::urp::encode::{merge_extra, text_parts, tool_choice_to_value};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Message, Part, ResponseFormat, Role, ToolDefinition,
    UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    for message in &req.messages {
        match message.role {
            Role::System | Role::Developer => {
                let text = text_parts(&message.parts);
                if !text.is_empty() {
                    system_blocks.push(json!({ "type": "text", "text": text }));
                }
            }
            Role::Tool => {
                if let Some(item) = encode_tool_result_message(message) {
                    messages.push(item);
                }
            }
            Role::User | Role::Assistant => messages.push(encode_regular_message(message)),
        }
    }

    let mut body = json!({
        "model": upstream_model,
        "messages": messages,
        "max_tokens": req.max_output_tokens.unwrap_or(1024),
    });
    let obj = body.as_object_mut().expect("anthropic request object");

    if !system_blocks.is_empty() {
        obj.insert("system".to_string(), Value::Array(system_blocks));
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
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(choice) = &req.tool_choice {
        obj.insert(
            "tool_choice".to_string(),
            encode_tool_choice_for_anthropic(choice),
        );
    }
    if let Some(reasoning) = &req.reasoning {
        if let Some(effort) = &reasoning.effort {
            obj.insert(
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "budget_tokens": effort_to_budget(effort)
                }),
            );
        }
    }
    if matches!(
        req.response_format,
        Some(ResponseFormat::JsonObject | ResponseFormat::JsonSchema { .. })
    ) {
        obj.insert(
            "response_format".to_string(),
            Value::String("unsupported".to_string()),
        );
    }
    merge_extra(obj, &req.extra_body);
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut content = Vec::new();
    let encrypted = resp.message.parts.iter().find_map(|part| match part {
        Part::ReasoningEncrypted { data, .. } => Some(data.clone()),
        _ => None,
    });

    for part in &resp.message.parts {
        match part {
            Part::Reasoning { content: text, .. } => {
                let mut thinking = Map::new();
                thinking.insert("type".to_string(), Value::String("thinking".to_string()));
                thinking.insert("thinking".to_string(), Value::String(text.clone()));
                if let Some(sig) = encrypted.clone() {
                    thinking.insert("signature".to_string(), sig);
                }
                content.push(Value::Object(thinking));
            }
            Part::Text { content: text, .. } => {
                content.push(json!({ "type": "text", "text": text }));
            }
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                let input = serde_json::from_str::<Value>(arguments)
                    .unwrap_or_else(|_| json!({ "_raw": arguments }));
                content.push(json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": input
                }));
            }
            Part::Image { source, .. } => content.push(encode_anthropic_image(source)),
            Part::File { source, .. } => content.push(encode_anthropic_file(source)),
            Part::Audio { .. }
            | Part::ReasoningEncrypted { .. }
            | Part::ToolResult { .. }
            | Part::Refusal { .. } => {}
        }
    }

    let mut body = json!({
        "id": resp.id,
        "type": "message",
        "role": "assistant",
        "model": logical_model,
        "content": content,
        "stop_reason": finish_reason_to_stop_reason(resp.finish_reason),
    });

    let (input_tokens, output_tokens, cache_read) = match &resp.usage {
        Some(u) => (
            u.prompt_tokens,
            u.completion_tokens,
            u.cached_tokens.unwrap_or(0),
        ),
        None => (0, 0, 0),
    };
    body["usage"] = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_read_input_tokens": cache_read
    });
    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &resp.extra_body);
    }
    body
}

fn encode_regular_message(message: &Message) -> Value {
    let role = match message.role {
        Role::Assistant => "assistant",
        _ => "user",
    };
    let mut content = Vec::new();
    let has_encrypted = message
        .parts
        .iter()
        .any(|part| matches!(part, Part::ReasoningEncrypted { .. }));

    for part in &message.parts {
        match part {
            Part::Text { content: text, .. } => {
                content.push(json!({ "type": "text", "text": text }))
            }
            Part::Image { source, .. } => content.push(encode_anthropic_image(source)),
            Part::File { source, .. } => content.push(encode_anthropic_file(source)),
            Part::Reasoning { content: text, .. } if !has_encrypted => {
                content.push(json!({ "type": "thinking", "thinking": text }));
            }
            Part::ReasoningEncrypted { data, .. } => {
                content.push(json!({ "type": "thinking", "encrypted_thinking": data }));
            }
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                let input = serde_json::from_str::<Value>(arguments)
                    .unwrap_or_else(|_| json!({ "_raw": arguments }));
                content.push(json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": input
                }));
            }
            Part::Audio { .. } | Part::ToolResult { .. } | Part::Refusal { .. } => {}
            Part::Reasoning { .. } => {}
        }
    }
    json!({ "role": role, "content": content })
}

fn encode_tool_result_message(message: &Message) -> Option<Value> {
    let tool_result = message.parts.iter().find_map(|part| match part {
        Part::ToolResult {
            call_id, is_error, ..
        } => Some((call_id.clone(), *is_error)),
        _ => None,
    })?;

    let mut content = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text { content: text, .. } => {
                content.push(json!({ "type": "text", "text": text }))
            }
            Part::Image { source, .. } => content.push(encode_anthropic_image(source)),
            Part::File { source, .. } => content.push(encode_anthropic_file(source)),
            _ => {}
        }
    }
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "" }));
    }
    Some(json!({
        "role": "user",
        "content": [{
            "type": "tool_result",
            "tool_use_id": tool_result.0,
            "is_error": tool_result.1,
            "content": content
        }]
    }))
}

fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                out.push(json!({
                    "name": function.name,
                    "description": function.description,
                    "input_schema": function.parameters.clone().unwrap_or(json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": true
                    }))
                }));
            }
        } else {
            let mut obj = HashMap::new();
            obj.insert("name".to_string(), Value::String(tool.tool_type.clone()));
            for (k, v) in &tool.extra_body {
                obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
            out.push(Value::Object(obj.into_iter().collect()));
        }
    }
    out
}

fn encode_tool_choice_for_anthropic(choice: &crate::urp::ToolChoice) -> Value {
    match tool_choice_to_value(choice) {
        Value::String(mode) => match mode.as_str() {
            "auto" => json!({ "type": "auto" }),
            "required" => json!({ "type": "any" }),
            "none" => json!({ "type": "none" }),
            _ => Value::String(mode),
        },
        Value::Object(obj) => {
            if let Some(name) = obj
                .get("function")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                json!({ "type": "tool", "name": name })
            } else {
                Value::Object(obj)
            }
        }
        other => other,
    }
}

fn encode_anthropic_image(source: &ImageSource) -> Value {
    match source {
        ImageSource::Url { url, .. } => json!({
            "type": "image",
            "source": { "type": "url", "url": url }
        }),
        ImageSource::Base64 { media_type, data } => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data }
        }),
    }
}

fn encode_anthropic_file(source: &FileSource) -> Value {
    match source {
        FileSource::Url { url } => json!({
            "type": "document",
            "source": { "type": "url", "url": url }
        }),
        FileSource::Base64 {
            filename,
            media_type,
            data,
        } => json!({
            "type": "document",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data,
                "filename": filename
            }
        }),
    }
}

fn effort_to_budget(effort: &str) -> u32 {
    match effort {
        "low" => 1024,
        "high" => 16384,
        _ => 4096,
    }
}

fn finish_reason_to_stop_reason(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(FinishReason::Length) => "max_tokens",
        Some(FinishReason::ToolCalls) => "tool_use",
        _ => "end_turn",
    }
}
