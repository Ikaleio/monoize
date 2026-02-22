use crate::urp::encode::{merge_extra, tool_choice_to_value};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Message, Part, ResponseFormat, Role, ToolDefinition,
    UrpRequest, UrpResponse,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    for message in &req.messages {
        match message.role {
            Role::System | Role::Developer => {
                for part in &message.parts {
                    if let Part::Text {
                        content: text,
                        extra_body,
                    } = part
                    {
                        if !text.is_empty() {
                            let mut block = json!({ "type": "text", "text": text });
                            if let Some(obj) = block.as_object_mut() {
                                merge_extra(obj, extra_body);
                            }
                            system_blocks.push(block);
                        }
                    }
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
            if model_supports_adaptive(upstream_model) {
                obj.insert("thinking".to_string(), json!({ "type": "adaptive" }));
                obj.insert("output_config".to_string(), json!({ "effort": effort }));
            } else {
                obj.insert(
                    "thinking".to_string(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": effort_to_budget(effort)
                    }),
                );
            }
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
            Part::Reasoning {
                content: text,
                extra_body,
            } => {
                let mut thinking = Map::new();
                thinking.insert("type".to_string(), Value::String("thinking".to_string()));
                thinking.insert("thinking".to_string(), Value::String(text.clone()));
                if let Some(sig) = encrypted.clone() {
                    thinking.insert("signature".to_string(), sig);
                }
                merge_extra(&mut thinking, extra_body);
                content.push(Value::Object(thinking));
            }
            Part::Text {
                content: text,
                extra_body,
            } => {
                let mut block = json!({ "type": "text", "text": text });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::ToolCall {
                call_id,
                name,
                arguments,
                extra_body,
            } => {
                let input = serde_json::from_str::<Value>(arguments)
                    .unwrap_or_else(|_| json!({ "_raw": arguments }));
                let mut block = json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": input
                });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::Image {
                source, extra_body, ..
            } => {
                let mut block = encode_anthropic_image(source);
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::File {
                source, extra_body, ..
            } => {
                let mut block = encode_anthropic_file(source);
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::Audio { .. } | Part::ToolResult { .. } | Part::Refusal { .. } => {}
            Part::ReasoningEncrypted { .. } => {
                // Handled below: emitted as standalone block only when no
                // Part::Reasoning exists to carry it as a `signature`.
            }
        }
    }

    // When the response contains encrypted reasoning but no plaintext reasoning
    // (e.g. Gemini returns only thoughtSignature), emit a standalone thinking
    // block so the downstream receives the encrypted data.
    let has_reasoning_part = resp
        .message
        .parts
        .iter()
        .any(|p| matches!(p, Part::Reasoning { .. }));
    if !has_reasoning_part {
        if let Some(enc) = &encrypted {
            let block = json!({ "type": "thinking", "thinking": "", "signature": enc });
            content.insert(0, block);
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
            Part::Text {
                content: text,
                extra_body,
            } => {
                let mut block = json!({ "type": "text", "text": text });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::Image {
                source, extra_body, ..
            } => {
                let mut block = encode_anthropic_image(source);
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::File {
                source, extra_body, ..
            } => {
                let mut block = encode_anthropic_file(source);
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::Reasoning {
                content: text,
                extra_body,
            } if !has_encrypted => {
                let mut block = json!({ "type": "thinking", "thinking": text });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::ReasoningEncrypted { data, extra_body } => {
                let mut block = json!({ "type": "thinking", "encrypted_thinking": data });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::ToolCall {
                call_id,
                name,
                arguments,
                extra_body,
            } => {
                let input = serde_json::from_str::<Value>(arguments)
                    .unwrap_or_else(|_| json!({ "_raw": arguments }));
                let mut block = json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": input
                });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::Audio { .. } | Part::ToolResult { .. } | Part::Refusal { .. } => {}
            Part::Reasoning { .. } => {}
        }
    }
    let mut msg = json!({ "role": role, "content": content });
    if let Some(obj) = msg.as_object_mut() {
        merge_extra(obj, &message.extra_body);
    }
    msg
}

fn encode_tool_result_message(message: &Message) -> Option<Value> {
    let tool_result = message.parts.iter().find_map(|part| match part {
        Part::ToolResult {
            call_id,
            is_error,
            extra_body,
        } => Some((call_id.clone(), *is_error, extra_body.clone())),
        _ => None,
    })?;

    let mut content = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text {
                content: text,
                extra_body,
            } => {
                let mut block = json!({ "type": "text", "text": text });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::Image {
                source, extra_body, ..
            } => {
                let mut block = encode_anthropic_image(source);
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::File {
                source, extra_body, ..
            } => {
                let mut block = encode_anthropic_file(source);
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            _ => {}
        }
    }
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "" }));
    }
    let mut tool_result_block = json!({
        "type": "tool_result",
        "tool_use_id": tool_result.0,
        "is_error": tool_result.1,
        "content": content
    });
    if let Some(obj) = tool_result_block.as_object_mut() {
        merge_extra(obj, &tool_result.2);
    }
    Some(json!({
        "role": "user",
        "content": [tool_result_block]
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

/// Claude 4.6+ models use `thinking: {type: "adaptive"}` + `output_config: {effort}`.
/// Older models require the deprecated `thinking: {type: "enabled", budget_tokens: N}`.
fn model_supports_adaptive(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.contains("opus-4-6")
        || m.contains("sonnet-4-6")
        || m.contains("opus-4.6")
        || m.contains("sonnet-4.6")
    {
        return true;
    }
    for prefix in ["opus-", "sonnet-"] {
        if let Some(pos) = m.find(prefix) {
            let after = &m[pos + prefix.len()..];
            let major_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(major) = major_str.parse::<u32>() {
                if major >= 5 {
                    return true;
                }
            }
        }
    }
    false
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
