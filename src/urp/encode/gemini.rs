use crate::urp::encode::{merge_extra, usage_input_details, usage_output_details};
use crate::urp::{
    AudioSource, FileSource, FinishReason, FunctionDefinition, ImageSource, Node, OrdinaryRole,
    ToolChoice, ToolDefinition, ToolResultContent, UrpRequest, UrpResponse,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut contents = Vec::new();
    let mut system_parts = Vec::new();
    let mut tool_names_by_call_id: Map<String, Value> = Map::new();
    let request_nodes = &req.input;

    for node in request_nodes {
        if let Node::ToolCall { call_id, name, .. } = node {
            tool_names_by_call_id
                .entry(call_id.clone())
                .or_insert_with(|| Value::String(name.clone()));
        }
    }

    let mut pending_content: Option<GeminiMessageEnvelope> = None;
    for node in request_nodes {
        match node {
            Node::Text {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                content,
                ..
            } => {
                flush_pending_gemini_message(&mut pending_content, &mut contents);
                if !content.is_empty() {
                    system_parts.push(json!({ "text": content }));
                }
            }
            Node::Text {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Image {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::File {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Audio {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Reasoning { .. }
            | Node::ToolCall { .. } => {
                append_node_to_pending_gemini_message(&mut pending_content, &mut contents, node);
            }
            Node::ToolResult {
                id: _,
                call_id,
                content,
                is_error,
                extra_body,
            } => {
                flush_pending_gemini_message(&mut pending_content, &mut contents);
                let result = content
                    .iter()
                    .filter_map(|entry| match entry {
                        ToolResultContent::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let function_name = extra_body
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| tool_names_by_call_id.get(call_id).and_then(|v| v.as_str()))
                    .unwrap_or(call_id);
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": function_name,
                            "response": {
                                "result": result,
                                "is_error": is_error
                            }
                        }
                    }]
                }));
            }
            Node::NextDownstreamEnvelopeExtra { .. }
            | Node::Image {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::File {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::Audio {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::Refusal { .. } => {
                flush_pending_gemini_message(&mut pending_content, &mut contents);
            }
        }
    }
    flush_pending_gemini_message(&mut pending_content, &mut contents);

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
    let response_nodes = &resp.output;
    let mut parts = Vec::new();
    for node in response_nodes {
        match node {
            Node::Text {
                role: OrdinaryRole::Assistant,
                content,
                ..
            } => parts.push(json!({ "text": content })),
            Node::Reasoning {
                content: Some(content),
                ..
            } => parts.push(json!({ "text": content, "thought": true })),
            Node::Reasoning {
                encrypted: Some(data),
                ..
            } => parts.push(json!({ "thoughtSignature": data })),
            Node::ToolCall {
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
            Node::Image {
                role: OrdinaryRole::Assistant,
                source,
                ..
            } => parts.push(encode_image_part(source)),
            Node::File {
                role: OrdinaryRole::Assistant,
                source,
                ..
            } => parts.push(encode_file_part(source)),
            Node::Audio {
                role: OrdinaryRole::Assistant,
                source,
                ..
            } => parts.push(encode_audio_part(source)),
            Node::Refusal { content, .. } => parts.push(json!({ "text": content })),
            Node::ProviderItem {
                role: OrdinaryRole::Assistant,
                body,
                ..
            } => parts.push(body.clone()),
            Node::Reasoning { .. }
            | Node::Text { .. }
            | Node::Image { .. }
            | Node::File { .. }
            | Node::Audio { .. }
            | Node::ProviderItem { .. }
            | Node::ToolResult { .. }
            | Node::NextDownstreamEnvelopeExtra { .. } => continue,
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

#[derive(Clone)]
struct GeminiMessageEnvelope {
    role: OrdinaryRole,
    parts: Vec<Value>,
    extra_body: HashMap<String, Value>,
}

fn flush_pending_gemini_message(pending: &mut Option<GeminiMessageEnvelope>, out: &mut Vec<Value>) {
    let Some(message) = pending.take() else {
        return;
    };
    if message.parts.is_empty() {
        return;
    }
    let role = if message.role == OrdinaryRole::Assistant {
        "model"
    } else {
        "user"
    };
    let mut obj = Map::new();
    obj.insert("role".to_string(), Value::String(role.to_string()));
    obj.insert("parts".to_string(), Value::Array(message.parts));
    merge_extra(&mut obj, &message.extra_body);
    out.push(Value::Object(obj));
}

fn append_node_to_pending_gemini_message(
    pending: &mut Option<GeminiMessageEnvelope>,
    out: &mut Vec<Value>,
    node: &Node,
) {
    let Some((role, part, extra_body)) = encode_request_node_part(node) else {
        return;
    };
    let should_flush = pending
        .as_ref()
        .is_some_and(|existing| existing.role != role || existing.extra_body != extra_body);
    if should_flush {
        flush_pending_gemini_message(pending, out);
    }
    let entry = pending.get_or_insert_with(|| GeminiMessageEnvelope {
        role,
        parts: Vec::new(),
        extra_body,
    });
    entry.parts.push(part);
}

fn encode_request_node_part(node: &Node) -> Option<(OrdinaryRole, Value, HashMap<String, Value>)> {
    match node {
        Node::Text {
            role,
            content,
            extra_body,
            ..
        } => Some((*role, json!({ "text": content }), extra_body.clone())),
        Node::Image {
            role,
            source,
            extra_body,
            ..
        } => Some((*role, encode_image_part(source), extra_body.clone())),
        Node::File {
            role,
            source,
            extra_body,
            ..
        } => Some((*role, encode_file_part(source), extra_body.clone())),
        Node::Audio {
            role,
            source,
            extra_body,
            ..
        } => Some((*role, encode_audio_part(source), extra_body.clone())),
        Node::Refusal {
            content,
            extra_body,
            ..
        } => Some((
            OrdinaryRole::Assistant,
            json!({ "text": content }),
            extra_body.clone(),
        )),
        Node::Reasoning {
            content: Some(content),
            extra_body,
            ..
        } => Some((
            OrdinaryRole::Assistant,
            json!({ "text": content, "thought": true }),
            extra_body.clone(),
        )),
        Node::Reasoning {
            encrypted: Some(data),
            extra_body,
            ..
        } => Some((
            OrdinaryRole::Assistant,
            json!({ "thoughtSignature": data }),
            extra_body.clone(),
        )),
        Node::ToolCall {
            id: _,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let args = serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}));
            Some((
                OrdinaryRole::Assistant,
                json!({
                    "functionCall": {
                        "id": call_id,
                        "name": name,
                        "args": args
                    }
                }),
                extra_body.clone(),
            ))
        }
        Node::Reasoning { .. } => None,
        Node::ProviderItem {
            role,
            body,
            extra_body,
            ..
        } => Some((*role, body.clone(), extra_body.clone())),
        Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::gemini as decode_gemini;
    use crate::urp::{
        items_to_nodes, InputDetails, Item, OutputDetails, Part, Role, UrpRequest, UrpResponse,
        Usage,
    };
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn gemini_usage_round_trips_extension_fields_without_extra_leakage() {
        let mut usage_extra = HashMap::new();
        usage_extra.insert("providerCounter".to_string(), json!(9));
        let response = UrpResponse { id: "gem_resp".to_string(), model: "gemini-2.5-pro".to_string(), created_at: None, output: items_to_nodes(vec![Item::new_message(Role::Assistant)]), finish_reason: Some(FinishReason::Stop), usage: Some(Usage {
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
        }), extra_body: empty_map() };

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
        assert!(!decoded_usage
            .extra_body
            .contains_key("cachedContentTokenCount"));
        assert!(!decoded_usage.extra_body.contains_key("thoughtsTokenCount"));
        assert_eq!(
            decoded_usage.extra_body.get("providerCounter"),
            Some(&json!(9))
        );
    }

    #[test]
    fn encode_request_uses_function_name_for_function_response() {
        let req = UrpRequest { model: "gemini-2.5-pro".to_string(), input: items_to_nodes(vec![
            Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::ToolCall {
                    id: None,
                    call_id: "call_1".to_string(),
                    name: "lookup".to_string(),
                    arguments: "{\"q\":1}".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            },
            Item::ToolResult {
                id: None,
                call_id: "call_1".to_string(),
                is_error: false,
                content: vec![ToolResultContent::Text {
                    text: "ok".to_string(),
                }],
                extra_body: empty_map(),
            },
        ]), stream: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        reasoning: None,
        tools: None,
        tool_choice: None,
        response_format: None,
        user: None,
        extra_body: empty_map(), };

        let encoded = encode_request(&req, "gemini-2.5-pro");
        let contents = encoded["contents"].as_array().expect("contents array");
        let function_response = contents[1]["parts"][0]["functionResponse"]
            .as_object()
            .expect("function response object");

        assert_eq!(
            function_response.get("name"),
            Some(&json!("lookup")),
            "Gemini functionResponse.name must use the function name, not the call_id"
        );
    }
}
