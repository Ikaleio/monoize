use crate::urp::encode::{merge_extra, text_parts, usage_input_details, usage_output_details};
use crate::urp::{
    merged_output_items, AudioSource, FileSource, FinishReason, FunctionDefinition, ImageSource,
    Item, Part, Role, ToolChoice, ToolDefinition, ToolResultContent, UrpRequest, UrpResponse,
};
use serde_json::{json, Map, Value};

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut contents = Vec::new();
    let mut system_parts = Vec::new();

    for item in &req.inputs {
        match item {
            Item::Message { role, parts, .. } => match role {
                Role::System | Role::Developer => {
                    let text = text_parts(parts);
                    if !text.is_empty() {
                        system_parts.push(json!({ "text": text }));
                    }
                }
                _ => {
                    let role = if *role == Role::Assistant {
                        "model"
                    } else {
                        "user"
                    };
                    let parts = encode_message_parts(item);
                    if !parts.is_empty() {
                        contents.push(json!({ "role": role, "parts": parts }));
                    }
                }
            },
            Item::ToolResult {
                call_id,
                content,
                is_error,
                ..
            } => {
                let result = content
                    .iter()
                    .filter_map(|entry| match entry {
                        ToolResultContent::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": call_id,
                            "response": {
                                "result": result,
                                "is_error": is_error
                            }
                        }
                    }]
                }));
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
    let merged = merged_output_items(&resp.outputs);
    let mut parts = Vec::new();
    let merged_parts: &[Part] = match &merged {
        Item::Message { parts, .. } => parts,
        Item::ToolResult { .. } => &[],
    };
    for part in merged_parts {
        match part {
            Part::Text { content, .. } => parts.push(json!({ "text": content })),
            Part::Reasoning {
                content: Some(content),
                ..
            } => {
                parts.push(json!({ "text": content, "thought": true }));
            }
            Part::Reasoning {
                encrypted: Some(data),
                ..
            } => {
                parts.push(json!({ "thoughtSignature": data }));
            }
            Part::Reasoning { .. } => {}
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                let args = serde_json::from_str::<Value>(&arguments).unwrap_or_else(|_| json!({}));
                parts.push(json!({
                    "functionCall": {
                        "id": call_id,
                        "name": name,
                        "args": args
                    }
                }));
            }
            Part::Image { source, .. } => parts.push(encode_image_part(&source)),
            Part::File { source, .. } => parts.push(encode_file_part(&source)),
            Part::Audio { source, .. } => parts.push(encode_audio_part(&source)),
            Part::Refusal { content, .. } => parts.push(json!({ "text": content })),
            Part::ProviderItem { body, .. } => parts.push(body.clone()),
        }
    }

    let mut usage_metadata = json!({
        "promptTokenCount": 0,
        "candidatesTokenCount": 0,
        "totalTokenCount": 0,
        "thoughtsTokenCount": 0,
        "cachedContentTokenCount": 0,
        "cacheCreationTokenCount": 0,
        "toolPromptInputTokenCount": 0,
        "acceptedPredictionOutputTokenCount": 0,
        "rejectedPredictionOutputTokenCount": 0
    });
    if let Some(usage) = &resp.usage {
        if let Some(obj) = usage_metadata.as_object_mut() {
            let input_details = usage_input_details(usage);
            let output_details = usage_output_details(usage);
            obj.insert(
                "promptTokenCount".to_string(),
                Value::from(usage.input_tokens),
            );
            obj.insert(
                "candidatesTokenCount".to_string(),
                Value::from(usage.output_tokens),
            );
            obj.insert(
                "totalTokenCount".to_string(),
                Value::from(usage.total_tokens()),
            );
            obj.insert(
                "thoughtsTokenCount".to_string(),
                Value::from(output_details.reasoning_tokens),
            );
            obj.insert(
                "cachedContentTokenCount".to_string(),
                Value::from(input_details.cache_read_tokens),
            );
            obj.insert(
                "cacheCreationTokenCount".to_string(),
                Value::from(input_details.cache_creation_tokens),
            );
            obj.insert(
                "toolPromptInputTokenCount".to_string(),
                Value::from(input_details.tool_prompt_tokens),
            );
            obj.insert(
                "acceptedPredictionOutputTokenCount".to_string(),
                Value::from(output_details.accepted_prediction_tokens),
            );
            obj.insert(
                "rejectedPredictionOutputTokenCount".to_string(),
                Value::from(output_details.rejected_prediction_tokens),
            );
            for (k, v) in &usage.extra_body {
                obj.insert(k.clone(), v.clone());
            }
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
        "usageMetadata": usage_metadata,
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

fn encode_message_parts(item: &Item) -> Vec<Value> {
    let mut out = Vec::new();
    let parts = match item {
        Item::Message { parts, .. } => parts,
        Item::ToolResult { .. } => return out,
    };
    for part in parts {
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
            Part::Reasoning {
                content: Some(content),
                ..
            } => {
                out.push(json!({ "text": content, "thought": true }));
            }
            Part::Reasoning {
                encrypted: Some(data),
                ..
            } => {
                out.push(json!({ "thoughtSignature": data }));
            }
            Part::Reasoning { .. } => {}
            Part::Refusal { content, .. } => out.push(json!({ "text": content })),
            Part::ProviderItem { body, .. } => out.push(body.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::gemini as decode_gemini;
    use crate::urp::{InputDetails, Item, OutputDetails, Role, UrpResponse, Usage};
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn gemini_usage_round_trips_extension_fields_without_extra_leakage() {
        let mut usage_extra = HashMap::new();
        usage_extra.insert("providerCounter".to_string(), json!(9));
        let response = UrpResponse {
            id: "gem_resp".to_string(),
            model: "gemini-2.5-pro".to_string(),
            outputs: vec![Item::new_message(Role::Assistant)],
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 14,
                output_tokens: 9,
                input_details: Some(InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: 2,
                    cache_creation_tokens: 3,
                    tool_prompt_tokens: 4,
                    modality_breakdown: None,
                }),
                output_details: Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: 5,
                    accepted_prediction_tokens: 6,
                    rejected_prediction_tokens: 7,
                    modality_breakdown: None,
                }),
                extra_body: usage_extra,
            }),
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gemini-2.5-pro");
        assert_eq!(
            encoded["usageMetadata"]["cachedContentTokenCount"],
            json!(2)
        );
        assert_eq!(
            encoded["usageMetadata"]["cacheCreationTokenCount"],
            json!(3)
        );
        assert_eq!(
            encoded["usageMetadata"]["toolPromptInputTokenCount"],
            json!(4)
        );
        assert_eq!(encoded["usageMetadata"]["thoughtsTokenCount"], json!(5));
        assert_eq!(
            encoded["usageMetadata"]["acceptedPredictionOutputTokenCount"],
            json!(6)
        );
        assert_eq!(
            encoded["usageMetadata"]["rejectedPredictionOutputTokenCount"],
            json!(7)
        );

        let decoded = decode_gemini::decode_response(&encoded).expect("decode response");
        let decoded_usage = decoded.usage.expect("usage should decode");
        let input = decoded_usage.input_details.expect("input details");
        let output = decoded_usage.output_details.expect("output details");
        assert_eq!(input.cache_read_tokens, 2);
        assert_eq!(input.cache_creation_tokens, 3);
        assert_eq!(input.tool_prompt_tokens, 4);
        assert_eq!(output.reasoning_tokens, 5);
        assert_eq!(output.accepted_prediction_tokens, 6);
        assert_eq!(output.rejected_prediction_tokens, 7);
        assert!(decoded_usage
            .extra_body
            .get("cachedContentTokenCount")
            .is_none());
        assert!(decoded_usage.extra_body.get("thoughtsTokenCount").is_none());
        assert_eq!(
            decoded_usage.extra_body.get("providerCounter"),
            Some(&json!(9))
        );
    }
}
