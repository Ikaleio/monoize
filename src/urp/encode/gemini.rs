use crate::urp::encode::{merge_extra, text_parts};
use crate::urp::{
    AudioSource, FileSource, FinishReason, FunctionDefinition, ImageSource, Part, Role, ToolChoice,
    ToolDefinition, UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut contents = Vec::new();
    let mut system_parts = Vec::new();

    for message in &req.messages {
        match message.role {
            Role::System | Role::Developer => {
                let text = text_parts(&message.parts);
                if !text.is_empty() {
                    system_parts.push(json!({ "text": text }));
                }
            }
            _ => {
                let role = if message.role == Role::Assistant {
                    "model"
                } else {
                    "user"
                };
                let parts = encode_message_parts(message);
                if !parts.is_empty() {
                    contents.push(json!({ "role": role, "parts": parts }));
                }
            }
        }
    }

    let mut body = json!({
        "contents": contents,
    });
    let obj = body.as_object_mut().expect("gemini request object");

    if !system_parts.is_empty() {
        obj.insert(
            "systemInstruction".to_string(),
            json!({ "parts": system_parts }),
        );
    }

    let mut generation_config = Map::new();
    if let Some(temp) = req.temperature {
        generation_config.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        generation_config.insert("topP".to_string(), Value::from(top_p));
    }
    if let Some(max_tokens) = req.max_output_tokens {
        generation_config.insert("maxOutputTokens".to_string(), Value::from(max_tokens));
    }
    if let Some(reasoning) = &req.reasoning {
        if let Some(effort) = &reasoning.effort {
            generation_config.insert(
                "thinkingConfig".to_string(),
                json!({ "thinkingBudget": effort_to_budget(effort) }),
            );
        }
    }
    if !generation_config.is_empty() {
        obj.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }

    if let Some(tools) = &req.tools {
        let declarations = encode_function_declarations(tools);
        if !declarations.is_empty() {
            obj.insert(
                "tools".to_string(),
                Value::Array(vec![json!({ "functionDeclarations": declarations })]),
            );
        }
    }

    if let Some(tc) = &req.tool_choice {
        if let Some(cfg) = encode_tool_choice(tc) {
            obj.insert(
                "toolConfig".to_string(),
                json!({ "functionCallingConfig": cfg }),
            );
        }
    }

    merge_extra(obj, &req.extra_body);

    if !upstream_model.is_empty() {
        obj.remove("model");
    }

    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut parts = Vec::new();
    for part in &resp.message.parts {
        match part {
            Part::Text { content, .. } => parts.push(json!({ "text": content })),
            Part::Reasoning { content, .. } => {
                parts.push(json!({ "text": content, "thought": true }));
            }
            Part::ReasoningEncrypted { data, .. } => {
                parts.push(json!({ "thoughtSignature": data }));
            }
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                let args = serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}));
                parts.push(json!({
                    "functionCall": {
                        "id": call_id,
                        "name": name,
                        "args": args
                    }
                }));
            }
            Part::Image { source, .. } => parts.push(encode_image_part(source)),
            Part::File { source, .. } => parts.push(encode_file_part(source)),
            Part::Audio { source, .. } => parts.push(encode_audio_part(source)),
            Part::Refusal { content, .. } => parts.push(json!({ "text": content })),
            Part::ToolResult { .. } => {}
        }
    }

    let mut body = json!({
        "candidates": [{
            "index": 0,
            "content": {
                "role": "model",
                "parts": parts,
            },
            "finishReason": finish_reason_to_gemini(resp.finish_reason),
        }],
        "usageMetadata": {
            "promptTokenCount": resp.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0),
            "candidatesTokenCount": resp.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
            "totalTokenCount": resp
                .usage
                .as_ref()
                .map(|u| u.prompt_tokens + u.completion_tokens)
                .unwrap_or(0),
            "thoughtsTokenCount": resp.usage.as_ref().and_then(|u| u.reasoning_tokens).unwrap_or(0),
            "cachedContentTokenCount": resp.usage.as_ref().and_then(|u| u.cached_tokens).unwrap_or(0),
        },
        "modelVersion": logical_model,
    });

    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &resp.extra_body);
    }
    body
}

fn encode_function_declarations(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type != "function" {
            continue;
        }
        let Some(function) = &tool.function else {
            continue;
        };
        out.push(encode_function_declaration(function));
    }
    out
}

fn encode_function_declaration(function: &FunctionDefinition) -> Value {
    let mut obj = Map::new();
    obj.insert("name".to_string(), Value::String(function.name.clone()));
    if let Some(desc) = &function.description {
        obj.insert("description".to_string(), Value::String(desc.clone()));
    }
    if let Some(params) = &function.parameters {
        obj.insert("parameters".to_string(), params.clone());
    }
    merge_extra(&mut obj, &function.extra_body);
    Value::Object(obj)
}

fn encode_tool_choice(tc: &ToolChoice) -> Option<Value> {
    match tc {
        ToolChoice::Mode(mode) => match mode.as_str() {
            "none" => Some(json!({ "mode": "NONE" })),
            "required" => Some(json!({ "mode": "ANY" })),
            _ => Some(json!({ "mode": "AUTO" })),
        },
        ToolChoice::Specific(v) => {
            let name = v
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());
            name.map(|n| json!({ "mode": "ANY", "allowedFunctionNames": [n] }))
        }
    }
}

fn encode_message_parts(message: &crate::urp::Message) -> Vec<Value> {
    let mut out = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text { content, .. } => out.push(json!({ "text": content })),
            Part::Image { source, .. } => out.push(encode_image_part(source)),
            Part::File { source, .. } => out.push(encode_file_part(source)),
            Part::Audio { source, .. } => out.push(encode_audio_part(source)),
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                let args = serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}));
                out.push(json!({
                    "functionCall": {
                        "id": call_id,
                        "name": name,
                        "args": args
                    }
                }));
            }
            Part::ToolResult {
                call_id, is_error, ..
            } => {
                out.push(json!({
                    "functionResponse": {
                        "name": call_id,
                        "response": {
                            "result": text_parts(&message.parts),
                            "is_error": is_error
                        }
                    }
                }));
            }
            Part::Reasoning { content, .. } => {
                out.push(json!({ "text": content, "thought": true }));
            }
            Part::ReasoningEncrypted { data, .. } => {
                out.push(json!({ "thoughtSignature": data }));
            }
            Part::Refusal { content, .. } => out.push(json!({ "text": content })),
        }
    }
    out
}

fn encode_image_part(source: &ImageSource) -> Value {
    match source {
        ImageSource::Url { url, .. } => {
            json!({ "fileData": { "mimeType": "image/*", "fileUri": url } })
        }
        ImageSource::Base64 { media_type, data } => {
            json!({ "inlineData": { "mimeType": media_type, "data": data } })
        }
    }
}

fn encode_file_part(source: &FileSource) -> Value {
    match source {
        FileSource::Url { url } => {
            json!({ "fileData": { "mimeType": "application/octet-stream", "fileUri": url } })
        }
        FileSource::Base64 {
            media_type, data, ..
        } => {
            json!({ "inlineData": { "mimeType": media_type, "data": data } })
        }
    }
}

fn encode_audio_part(source: &AudioSource) -> Value {
    match source {
        AudioSource::Url { url } => {
            json!({ "fileData": { "mimeType": "audio/*", "fileUri": url } })
        }
        AudioSource::Base64 { media_type, data } => {
            json!({ "inlineData": { "mimeType": media_type, "data": data } })
        }
    }
}

fn effort_to_budget(effort: &str) -> u32 {
    match effort {
        "low" => 512,
        "high" => 2048,
        _ => 1024,
    }
}

fn finish_reason_to_gemini(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(FinishReason::Length) => "MAX_TOKENS",
        Some(FinishReason::ToolCalls) => "STOP",
        Some(FinishReason::ContentFilter) => "SAFETY",
        Some(FinishReason::Stop) => "STOP",
        _ => "OTHER",
    }
}
