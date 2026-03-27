use crate::urp::encode::{
    merge_extra, tool_choice_to_value, usage_input_details, usage_output_details,
};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Item, Part, Role, ToolDefinition, ToolResultContent,
    UrpRequest, UrpResponse,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    for item in &req.inputs {
        match item {
            Item::Message {
                role,
                parts,
                extra_body: _,
            } => match role {
                Role::System | Role::Developer => {
                    for part in parts {
                        if let Part::Text {
                            content: text,
                            extra_body,
                            ..
                        } = part
                        {
                            if !text.is_empty() {
                                let mut block = json!({ "type": "text", "text": text });
                                if let Some(obj) = block.as_object_mut() {
                                    if let Some(phase) =
                                        extra_body.get("phase").and_then(|v| v.as_str())
                                    {
                                        obj.insert(
                                            "phase".to_string(),
                                            Value::String(phase.to_string()),
                                        );
                                    }
                                    merge_extra(obj, extra_body);
                                }
                                system_blocks.push(block);
                            }
                        }
                    }
                }
                Role::User | Role::Assistant | Role::Tool => {
                    messages.push(encode_regular_message(item))
                }
            },
            Item::ToolResult {
                call_id,
                content,
                is_error,
                extra_body,
            } => messages.push(encode_tool_result_message(
                call_id, content, *is_error, extra_body,
            )),
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
    merge_extra(obj, &req.extra_body);
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut content = Vec::new();
    let mut encrypted = None;
    for item in &resp.outputs {
        match item {
            Item::Message {
                role: Role::Assistant,
                parts,
                ..
            } => {
                if encrypted.is_none() {
                    encrypted = parts.iter().find_map(|part| match part {
                        Part::Reasoning {
                            encrypted: Some(data),
                            ..
                        } => Some(data.clone()),
                        _ => None,
                    });
                }
            }
            Item::ToolResult { .. } | Item::Message { .. } => continue,
        }
    }

    for item in &resp.outputs {
        match item {
            Item::Message {
                role: Role::Assistant,
                parts,
                ..
            } => {
                for part in parts {
                    match part {
                        Part::Reasoning {
                            content: Some(text),
                            extra_body,
                            ..
                        } => {
                            let mut thinking = Map::new();
                            thinking
                                .insert("type".to_string(), Value::String("thinking".to_string()));
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
                            ..
                        } => {
                            let mut block = json!({ "type": "text", "text": text });
                            if let Some(obj) = block.as_object_mut() {
                                if let Some(phase) =
                                    extra_body.get("phase").and_then(|v| v.as_str())
                                {
                                    obj.insert(
                                        "phase".to_string(),
                                        Value::String(phase.to_string()),
                                    );
                                }
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
                        Part::ProviderItem {
                            body, extra_body, ..
                        } => {
                            let mut block = body.clone();
                            if let Some(obj) = block.as_object_mut() {
                                merge_extra(obj, extra_body);
                            }
                            content.push(block);
                        }
                        Part::Audio { .. } | Part::Refusal { .. } => {}
                        Part::Reasoning {
                            encrypted: Some(_), ..
                        } => {
                            // Handled below: emitted as standalone block only when no
                            // Part::Reasoning exists to carry it as a `signature`.
                        }
                        Part::Reasoning { .. } => {}
                    }
                }
            }
            Item::ToolResult { .. } | Item::Message { .. } => continue,
        }
    }

    // When the response contains encrypted reasoning but no plaintext reasoning
    // (e.g. Gemini returns only thoughtSignature), emit a standalone thinking
    // block so the downstream receives the encrypted data.
    let has_reasoning_part = resp.outputs.iter().any(|item| {
        matches!(
            item,
            Item::Message {
                role: Role::Assistant,
                parts,
                ..
            } if parts.iter().any(|p| matches!(
                p,
                Part::Reasoning {
                    content: Some(_),
                    ..
                }
            ))
        )
    });
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

    let mut usage_value = json!({
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_input_tokens": 0,
        "cache_creation_input_tokens": 0,
        "tool_prompt_input_tokens": 0,
        "reasoning_output_tokens": 0,
        "accepted_prediction_output_tokens": 0,
        "rejected_prediction_output_tokens": 0
    });
    if let Some(usage) = &resp.usage {
        if let Some(obj) = usage_value.as_object_mut() {
            let input_details = usage_input_details(usage);
            let output_details = usage_output_details(usage);
            obj.insert("input_tokens".to_string(), Value::from(usage.input_tokens));
            obj.insert(
                "output_tokens".to_string(),
                Value::from(usage.output_tokens),
            );
            obj.insert(
                "cache_read_input_tokens".to_string(),
                Value::from(input_details.cache_read_tokens),
            );
            obj.insert(
                "cache_creation_input_tokens".to_string(),
                Value::from(input_details.cache_creation_tokens),
            );
            obj.insert(
                "tool_prompt_input_tokens".to_string(),
                Value::from(input_details.tool_prompt_tokens),
            );
            obj.insert(
                "reasoning_output_tokens".to_string(),
                Value::from(output_details.reasoning_tokens),
            );
            obj.insert(
                "accepted_prediction_output_tokens".to_string(),
                Value::from(output_details.accepted_prediction_tokens),
            );
            obj.insert(
                "rejected_prediction_output_tokens".to_string(),
                Value::from(output_details.rejected_prediction_tokens),
            );
            for (k, v) in &usage.extra_body {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    body["usage"] = usage_value;
    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &resp.extra_body);
    }
    body
}

fn encode_regular_message(message: &Item) -> Value {
    let Item::Message {
        role,
        parts,
        extra_body,
    } = message
    else {
        unreachable!("encode_regular_message requires Item::Message")
    };
    let role = match role {
        Role::Assistant => "assistant",
        _ => "user",
    };
    let mut content = Vec::new();
    let has_encrypted = parts.iter().any(|part| {
        matches!(
            part,
            Part::Reasoning {
                encrypted: Some(_),
                ..
            }
        )
    });

    for part in parts {
        match part {
            Part::Text {
                content: text,
                extra_body,
                ..
            } => {
                let mut block = json!({ "type": "text", "text": text });
                if let Some(obj) = block.as_object_mut() {
                    if let Some(phase) = extra_body.get("phase").and_then(|v| v.as_str()) {
                        obj.insert("phase".to_string(), Value::String(phase.to_string()));
                    }
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
                content: Some(text),
                extra_body,
                ..
            } if !has_encrypted => {
                let mut block = json!({ "type": "thinking", "thinking": text });
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::Reasoning {
                encrypted: Some(data),
                extra_body,
                ..
            } => {
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
            Part::ProviderItem {
                body, extra_body, ..
            } => {
                let mut block = body.clone();
                if let Some(obj) = block.as_object_mut() {
                    merge_extra(obj, extra_body);
                }
                content.push(block);
            }
            Part::Audio { .. } | Part::Refusal { .. } => {}
            Part::Reasoning { .. } => {}
        }
    }
    let mut msg = json!({ "role": role, "content": content });
    if let Some(obj) = msg.as_object_mut() {
        merge_extra(obj, extra_body);
    }
    msg
}

fn encode_tool_result_message(
    call_id: &str,
    content: &[ToolResultContent],
    is_error: bool,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut content: Vec<Value> = content
        .iter()
        .map(|item| match item {
            ToolResultContent::Text { text } => json!({ "type": "text", "text": text }),
            ToolResultContent::Image { source } => encode_anthropic_image(source),
            ToolResultContent::File { source } => encode_anthropic_file(source),
        })
        .collect();
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "" }));
    }
    let mut tool_result_block = json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "is_error": is_error,
        "content": content
    });
    if let Some(obj) = tool_result_block.as_object_mut() {
        merge_extra(obj, extra_body);
    }
    json!({
        "role": "user",
        "content": [tool_result_block]
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::anthropic as decode_anthropic;
    use crate::urp::{Item, OutputDetails, ResponseFormat, Role, UrpRequest, UrpResponse, Usage};
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn encode_request_does_not_emit_fake_response_format() {
        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
            inputs: vec![Item::Message {
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            response_format: Some(ResponseFormat::JsonObject),
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        assert!(
            encoded.get("response_format").is_none(),
            "Anthropic requests must omit unsupported response_format"
        );
    }

    #[test]
    fn anthropic_text_block_phase_round_trips_to_responses_compatible_urp() {
        let source = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude",
            "content": [
                { "type": "text", "text": "prep", "phase": "commentary" },
                { "type": "tool_use", "id": "call_1", "name": "tool", "input": {} },
                { "type": "text", "text": "done", "phase": "final_answer" }
            ],
            "stop_reason": "tool_use"
        });

        let decoded = decode_anthropic::decode_response(&source).expect("decode response");
        let encoded = encode_response(&decoded, "claude");
        let content = encoded["content"].as_array().expect("content array");

        assert_eq!(content[0]["phase"], json!("commentary"));
        assert_eq!(content[1]["type"], json!("tool_use"));
        assert_eq!(content[2]["phase"], json!("final_answer"));
    }

    #[test]
    fn anthropic_usage_round_trips_extension_fields_without_leaking_nested_aliases() {
        let mut usage_extra = HashMap::new();
        usage_extra.insert("native_counter".to_string(), json!(7));
        let response = UrpResponse {
            id: "msg_usage".to_string(),
            model: "claude".to_string(),
            outputs: vec![Item::new_message(Role::Assistant)],
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 11,
                output_tokens: 5,
                input_details: Some(crate::urp::InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: 2,
                    cache_creation_tokens: 3,
                    tool_prompt_tokens: 4,
                    modality_breakdown: None,
                }),
                output_details: Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: 6,
                    accepted_prediction_tokens: 7,
                    rejected_prediction_tokens: 8,
                    modality_breakdown: None,
                }),
                extra_body: usage_extra,
            }),
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "claude");
        let usage = encoded["usage"].as_object().expect("usage object");
        assert_eq!(usage.get("tool_prompt_input_tokens"), Some(&json!(4)));
        assert_eq!(usage.get("reasoning_output_tokens"), Some(&json!(6)));
        assert_eq!(
            usage.get("accepted_prediction_output_tokens"),
            Some(&json!(7))
        );
        assert_eq!(
            usage.get("rejected_prediction_output_tokens"),
            Some(&json!(8))
        );
        assert_eq!(usage.get("native_counter"), Some(&json!(7)));

        let decoded = decode_anthropic::decode_response(&encoded).expect("decode response");
        let decoded_usage = decoded.usage.expect("usage should decode");
        assert_eq!(
            decoded_usage
                .input_details
                .expect("input details")
                .tool_prompt_tokens,
            4
        );
        let decoded_output = decoded_usage.output_details.expect("output details");
        assert_eq!(decoded_output.reasoning_tokens, 6);
        assert_eq!(decoded_output.accepted_prediction_tokens, 7);
        assert_eq!(decoded_output.rejected_prediction_tokens, 8);
        assert!(!decoded_usage
            .extra_body
            .contains_key("tool_prompt_input_tokens"));
        assert!(!decoded_usage
            .extra_body
            .contains_key("reasoning_output_tokens"));
        assert_eq!(
            decoded_usage.extra_body.get("native_counter"),
            Some(&json!(7))
        );
    }

    #[test]
    fn anthropic_response_round_trip_preserves_combined_thinking_block_shape() {
        let response = UrpResponse {
            id: "msg_roundtrip_reasoning".to_string(),
            model: "claude".to_string(),
            outputs: vec![Item::Message {
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    content: Some("full reasoning".to_string()),
                    encrypted: Some(json!("sig_1")),
                    summary: None,
                    source: None,
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }],
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "claude");
        let decoded = decode_anthropic::decode_response(&encoded).expect("decode response");
        let Item::Message { parts, .. } = &decoded.outputs[0] else {
            panic!("expected assistant output");
        };

        assert_eq!(
            parts.len(),
            1,
            "thinking block should decode to one reasoning part"
        );
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                encrypted: Some(Value::String(sig)),
                summary: None,
                ..
            } if content == "full reasoning" && sig == "sig_1"
        ));
    }
}
