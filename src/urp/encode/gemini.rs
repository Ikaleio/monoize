use crate::urp::decode::gemini::{
    GEMINI_PART_EXTRA_KEY, GEMINI_SYNTHETIC_CALL_PREFIX, signature_call_id,
};
use crate::urp::encode::{
    merge_extra, sanitize_provider_item_wire_body, usage_input_details, usage_output_details,
};
use crate::urp::{
    AudioSource, FileSource, FinishReason, FunctionDefinition, ImageSource, Node, OrdinaryRole,
    ProviderProtocol, ResponseFormat, StopControl, ToolChoice, ToolDefinition, ToolResultContent,
    UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut contents = Vec::new();
    let mut system_parts = Vec::new();
    let mut tool_names_by_call_id: Map<String, Value> = Map::new();
    let request_nodes = &req.input;
    let call_signatures = collect_call_signatures(request_nodes);

    for node in request_nodes {
        if let Node::ToolCall { call_id, name, .. } = node {
            tool_names_by_call_id
                .entry(call_id.clone())
                .or_insert_with(|| Value::String(name.clone()));
        }
    }

    let mut pending_content: Option<GeminiMessageEnvelope> = None;
    for node in request_nodes {
        if is_call_signature(node) {
            continue;
        }
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
                append_node_to_pending_gemini_message(
                    &mut pending_content,
                    &mut contents,
                    node,
                    &call_signatures,
                );
            }
            Node::ToolResult {
                id: _,
                tool_type,
                call_id,
                content,
                is_error,
                extra_body,
            } => {
                if *tool_type == crate::urp::ToolCallType::Custom {
                    continue;
                }
                flush_pending_gemini_message(&mut pending_content, &mut contents);
                let result = content
                    .iter()
                    .filter_map(|entry| match entry {
                        ToolResultContent::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let function_name = extra_body
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| tool_names_by_call_id.get(call_id).and_then(|v| v.as_str()))
                    .unwrap_or(call_id);
                let response = if extra_body
                    .get("_monoize_gemini_function_response")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    serde_json::from_str::<Value>(&result)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({"result": result}))
                } else {
                    json!({"result": result, "is_error": is_error})
                };
                let mut function_response = json!({"name": function_name, "response": response});
                if !call_id.is_empty() && !call_id.starts_with(GEMINI_SYNTHETIC_CALL_PREFIX) {
                    function_response["id"] = json!(call_id);
                }
                let part = json!({"functionResponse": function_response});
                if let Some(parts) = contents
                    .last_mut()
                    .filter(|content| content.get("role").and_then(Value::as_str) == Some("user"))
                    .and_then(|content| content.get_mut("parts"))
                    .and_then(Value::as_array_mut)
                    .filter(|parts| {
                        parts
                            .iter()
                            .all(|part| part.get("functionResponse").is_some())
                    })
                {
                    parts.push(part);
                } else {
                    contents.push(json!({"role": "user", "parts": [part]}));
                }
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

    let mut generation_config = req
        .extra_body
        .get("generationConfig")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(temp) = req.temperature {
        generation_config.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        generation_config.insert("topP".to_string(), Value::from(top_p));
    }
    if let Some(max_tokens) = req.max_output_tokens {
        generation_config.insert("maxOutputTokens".to_string(), Value::from(max_tokens));
    }
    if let Some(stop) = &req.stop {
        generation_config.insert(
            "stopSequences".to_string(),
            match stop {
                StopControl::Single(stop) => json!([stop]),
                StopControl::Multiple(stops) => json!(stops),
            },
        );
    }
    if let Some(format) = &req.response_format {
        generation_config.remove("responseSchema");
        generation_config.remove("responseJsonSchema");
        generation_config.insert(
            "responseMimeType".to_string(),
            json!(if matches!(format, ResponseFormat::Text) {
                "text/plain"
            } else {
                "application/json"
            }),
        );
        if let ResponseFormat::JsonSchema { json_schema } = format {
            generation_config.insert("responseJsonSchema".to_string(), json_schema.schema.clone());
        }
    }
    if let Some(effort) = req
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.effort.as_ref())
    {
        let mut thinking = generation_config
            .remove("thinkingConfig")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        thinking.remove("thinkingLevel");
        thinking.insert(
            "thinkingBudget".to_string(),
            json!(effort_to_budget(effort)),
        );
        generation_config.insert("thinkingConfig".to_string(), Value::Object(thinking));
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
    let call_signatures = collect_call_signatures(&resp.output);
    let parts: Vec<Value> = resp
        .output
        .iter()
        .filter(|node| !is_call_signature(node))
        .filter_map(|node| {
            encode_request_node_part(node)
                .filter(|(role, _, _)| *role == OrdinaryRole::Assistant)
                .map(|(_, mut part, _)| {
                    attach_call_signature(node, &mut part, &call_signatures);
                    part
                })
        })
        .collect();

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
                if !k.starts_with("_monoize_") {
                    obj.insert(k.clone(), v.clone());
                }
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

fn encode_image_part(source: &ImageSource) -> Option<Value> {
    match source {
        ImageSource::Url { url, .. } => {
            Some(json!({ "fileData": { "mimeType": "image/*", "fileUri": url } }))
        }
        ImageSource::Base64 { media_type, data } => {
            Some(json!({ "inlineData": { "mimeType": media_type, "data": data } }))
        }
        ImageSource::FileId { .. } => None,
    }
}

fn encode_file_part(source: &FileSource) -> Option<Value> {
    match source {
        FileSource::Url { url } => {
            Some(json!({ "fileData": { "mimeType": "application/octet-stream", "fileUri": url } }))
        }
        FileSource::Base64 {
            media_type, data, ..
        } => Some(json!({ "inlineData": { "mimeType": media_type, "data": data } })),
        FileSource::FileId { .. } | FileSource::Text { .. } | FileSource::Content { .. } => None,
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
        "none" => 0,
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
    call_signatures: &HashMap<String, Value>,
) {
    let Some((role, mut part, extra_body)) = encode_request_node_part(node) else {
        return;
    };
    attach_call_signature(node, &mut part, call_signatures);
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
    let (role, mut part, mut extra) = match node {
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
        } => Some((*role, encode_image_part(source)?, extra_body.clone())),
        Node::File {
            role,
            source,
            extra_body,
            ..
        } => Some((*role, encode_file_part(source)?, extra_body.clone())),
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
            content,
            encrypted,
            summary,
            extra_body,
            ..
        } if content.is_some() || summary.is_some() || encrypted.is_some() => {
            let mut part = json!({"text": content.as_deref().or(summary.as_deref()).unwrap_or(""), "thought": true});
            if let Some(signature) = encrypted {
                part["thoughtSignature"] = signature.clone();
            }
            Some((OrdinaryRole::Assistant, part, extra_body.clone()))
        }
        Node::ToolCall {
            id: _,
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            if *tool_type == crate::urp::ToolCallType::Custom {
                return None;
            }
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
            origin_protocol: ProviderProtocol::Gemini,
            body,
            extra_body,
            ..
        } => Some((
            *role,
            sanitize_provider_item_wire_body(body),
            extra_body.clone(),
        )),
        Node::ProviderItem { .. } => None,
        Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }?;
    if let Some(Value::Object(native)) = extra.remove(GEMINI_PART_EXTRA_KEY) {
        if let Some(obj) = part.as_object_mut() {
            for (key, value) in native {
                obj.entry(key).or_insert(value);
            }
        }
    }
    if let Node::ToolCall { call_id, .. } = node {
        if call_id.starts_with(GEMINI_SYNTHETIC_CALL_PREFIX) {
            if let Some(fc) = part.get_mut("functionCall").and_then(Value::as_object_mut) {
                fc.remove("id");
            }
        }
    }
    Some((role, part, extra))
}

fn bound_signature_call_id(node: &Node) -> Option<String> {
    let Node::Reasoning { id, extra_body, .. } = node else {
        return None;
    };
    id.as_deref().and_then(signature_call_id).or_else(|| {
        extra_body
            .get(crate::urp::REASONING_ENVELOPE_ITEM_ID_EXTRA_KEY)
            .and_then(Value::as_str)
            .and_then(signature_call_id)
    })
}

fn is_call_signature(node: &Node) -> bool {
    bound_signature_call_id(node).is_some()
}

fn collect_call_signatures(nodes: &[Node]) -> HashMap<String, Value> {
    nodes
        .iter()
        .filter_map(|node| {
            let Node::Reasoning {
                encrypted: Some(signature),
                ..
            } = node
            else {
                return None;
            };
            bound_signature_call_id(node).map(|call_id| (call_id, signature.clone()))
        })
        .collect()
}

fn attach_call_signature(node: &Node, part: &mut Value, signatures: &HashMap<String, Value>) {
    if let Node::ToolCall { call_id, .. } = node {
        if let Some(signature) = signatures.get(call_id) {
            part["thoughtSignature"] = signature.clone();
        }
    }
}
