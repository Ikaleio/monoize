use crate::urp::decode::{
    parse_file_part_from_obj, parse_image_part_from_obj, parse_tool_definition, split_extra,
    value_to_text,
};
use crate::urp::{
    FinishReason, Message, Part, ReasoningConfig, Role, ToolChoice, UrpRequest, UrpResponse, Usage,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let mut messages = Vec::new();
    for raw_msg in obj
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing messages".to_string())?
    {
        let msg_obj = match raw_msg.as_object() {
            Some(v) => v,
            None => continue,
        };
        let role = match msg_obj
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
        {
            "system" => Role::System,
            "developer" => Role::Developer,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            _ => Role::User,
        };

        if role == Role::Tool {
            let mut tool_msg = Message {
                role: Role::Tool,
                parts: Vec::new(),
                extra_body: split_extra(msg_obj, &["role", "tool_call_id", "content"]),
            };
            let call_id = msg_obj
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !call_id.is_empty() {
                tool_msg.parts.push(Part::ToolResult {
                    call_id,
                    is_error: false,
                    extra_body: HashMap::new(),
                });
            }
            let content = msg_obj.get("content").cloned().unwrap_or(Value::Null);
            let text = value_to_text(&content);
            if !text.is_empty() {
                tool_msg.parts.push(Part::Text {
                    content: text,
                    extra_body: HashMap::new(),
                });
            }
            messages.push(tool_msg);
            continue;
        }

        let mut msg = Message {
            role,
            parts: Vec::new(),
            extra_body: split_extra(
                msg_obj,
                &[
                    "role",
                    "content",
                    "tool_calls",
                    "reasoning",
                    "reasoning_details",
                    "reasoning_content",
                    "reasoning_opaque",
                    "refusal",
                ],
            ),
        };

        parse_chat_reasoning_fields(msg_obj, &mut msg.parts);

        if let Some(content) = msg_obj.get("content") {
            if let Some(s) = content.as_str() {
                if !s.is_empty() {
                    msg.parts.push(Part::Text {
                        content: s.to_string(),
                        extra_body: HashMap::new(),
                    });
                }
            } else if let Some(arr) = content.as_array() {
                for item in arr {
                    if let Some(item_obj) = item.as_object() {
                        if let Some(text) = item_obj.get("text").and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                msg.parts.push(Part::Text {
                                    content: text.to_string(),
                                    extra_body: split_extra(item_obj, &["type", "text"]),
                                });
                            }
                        }
                        if let Some(image_part) = parse_image_part_from_obj(item_obj) {
                            msg.parts.push(image_part);
                        }
                        if let Some(file_part) = parse_file_part_from_obj(item_obj) {
                            msg.parts.push(file_part);
                        }
                    }
                }
            }
        }

        if let Some(refusal) = msg_obj.get("refusal").and_then(|v| v.as_str()) {
            if !refusal.is_empty() {
                msg.parts.push(Part::Refusal {
                    content: refusal.to_string(),
                    extra_body: HashMap::new(),
                });
            }
        }

        if let Some(tool_calls) = msg_obj.get("tool_calls").and_then(|v| v.as_array()) {
            for tool_call in tool_calls {
                let tc_obj = match tool_call.as_object() {
                    Some(v) => v,
                    None => continue,
                };
                let call_id = tc_obj
                    .get("id")
                    .or_else(|| tc_obj.get("call_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = tc_obj
                    .get("function")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = tc_obj
                    .get("function")
                    .and_then(|v| v.get("arguments"))
                    .cloned()
                    .unwrap_or(Value::String("{}".to_string()));
                let arguments = if let Some(s) = arguments.as_str() {
                    s.to_string()
                } else {
                    serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
                };
                if !call_id.is_empty() && !name.is_empty() {
                    msg.parts.push(Part::ToolCall {
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(tc_obj, &["id", "type", "function", "call_id"]),
                    });
                }
            }
        }

        messages.push(msg);
    }

    let reasoning = extract_reasoning(obj);
    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(parse_tool_definition)
            .collect::<Vec<_>>()
    });

    Ok(UrpRequest {
        model,
        messages,
        stream: obj.get("stream").and_then(|v| v.as_bool()),
        temperature: obj.get("temperature").and_then(|v| v.as_f64()),
        top_p: obj.get("top_p").and_then(|v| v.as_f64()),
        max_output_tokens: obj
            .get("max_completion_tokens")
            .or_else(|| obj.get("max_tokens"))
            .and_then(|v| v.as_u64()),
        reasoning,
        tools,
        tool_choice: obj.get("tool_choice").cloned().map(tool_choice_from_value),
        response_format: obj
            .get("response_format")
            .cloned()
            .and_then(parse_response_format),
        user: obj
            .get("user")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        extra_body: split_extra(
            obj,
            &[
                "model",
                "messages",
                "stream",
                "temperature",
                "top_p",
                "max_completion_tokens",
                "max_tokens",
                "reasoning_effort",
                "reasoning",
                "tools",
                "tool_choice",
                "response_format",
                "user",
            ],
        ),
    })
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat response must be object".to_string())?;

    let choice = obj
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing choices[0]".to_string())?;
    let msg_obj = choice
        .get("message")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing choices[0].message".to_string())?;

    let mut message = Message {
        role: Role::Assistant,
        parts: Vec::new(),
        extra_body: split_extra(
            msg_obj,
            &[
                "role",
                "content",
                "reasoning",
                "reasoning_details",
                "reasoning_content",
                "reasoning_opaque",
                "tool_calls",
                "refusal",
            ],
        ),
    };

    parse_chat_reasoning_fields(msg_obj, &mut message.parts);

    if let Some(content) = msg_obj.get("content") {
        if let Some(s) = content.as_str() {
            if !s.is_empty() {
                message.parts.push(Part::Text {
                    content: s.to_string(),
                    extra_body: HashMap::new(),
                });
            }
        } else if let Some(arr) = content.as_array() {
            for item in arr {
                if let Some(item_obj) = item.as_object() {
                    if let Some(text) = item_obj.get("text").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            message.parts.push(Part::Text {
                                content: text.to_string(),
                                extra_body: split_extra(item_obj, &["type", "text"]),
                            });
                        }
                    }
                    if let Some(image) = parse_image_part_from_obj(item_obj) {
                        message.parts.push(image);
                    }
                    if let Some(file) = parse_file_part_from_obj(item_obj) {
                        message.parts.push(file);
                    }
                }
            }
        }
    }

    if let Some(tool_calls) = msg_obj.get("tool_calls").and_then(|v| v.as_array()) {
        for tool_call in tool_calls {
            let tc_obj = match tool_call.as_object() {
                Some(v) => v,
                None => continue,
            };
            let call_id = tc_obj
                .get("id")
                .or_else(|| tc_obj.get("call_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = tc_obj
                .get("function")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = tc_obj
                .get("function")
                .and_then(|v| v.get("arguments"))
                .cloned()
                .unwrap_or(Value::String("{}".to_string()));
            let arguments = if let Some(s) = arguments.as_str() {
                s.to_string()
            } else {
                serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
            };
            if !call_id.is_empty() && !name.is_empty() {
                message.parts.push(Part::ToolCall {
                    call_id,
                    name,
                    arguments,
                    extra_body: split_extra(tc_obj, &["id", "type", "function", "call_id"]),
                });
            }
        }
    }

    if let Some(refusal) = msg_obj.get("refusal").and_then(|v| v.as_str()) {
        if !refusal.is_empty() {
            message.parts.push(Part::Refusal {
                content: refusal.to_string(),
                extra_body: HashMap::new(),
            });
        }
    }

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .map(parse_finish_reason);

    let usage = obj
        .get("usage")
        .and_then(|v| v.as_object())
        .map(parse_usage_from_chat);

    Ok(UrpResponse {
        id: obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("chat_completion")
            .to_string(),
        model: obj
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        message,
        finish_reason,
        usage,
        extra_body: split_extra(
            obj,
            &["id", "object", "created", "model", "choices", "usage"],
        ),
    })
}

fn extract_reasoning(obj: &Map<String, Value>) -> Option<ReasoningConfig> {
    if let Some(effort) = obj
        .get("reasoning")
        .and_then(|v| v.as_object())
        .and_then(|v| v.get("effort"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    {
        return Some(ReasoningConfig {
            effort: Some(effort),
            extra_body: HashMap::new(),
        });
    }
    if let Some(effort) = obj
        .get("reasoning_effort")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    {
        return Some(ReasoningConfig {
            effort: Some(effort),
            extra_body: HashMap::new(),
        });
    }
    None
}

fn parse_chat_reasoning_fields(msg_obj: &Map<String, Value>, parts: &mut Vec<Part>) {
    let mut saw_plain = false;
    let mut saw_encrypted = false;

    if let Some(details) = msg_obj.get("reasoning_details").and_then(|v| v.as_array()) {
        for detail in details {
            let Some(detail_obj) = detail.as_object() else {
                continue;
            };
            match detail_obj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
            {
                "reasoning.text" => {
                    if let Some(text) = detail_obj.get("text").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            parts.push(Part::Reasoning {
                                content: text.to_string(),
                                extra_body: HashMap::new(),
                            });
                            saw_plain = true;
                        }
                    }
                    if let Some(sig) = detail_obj.get("signature").and_then(|v| v.as_str()) {
                        if !sig.is_empty() {
                            parts.push(Part::ReasoningEncrypted {
                                data: Value::String(sig.to_string()),
                                extra_body: HashMap::new(),
                            });
                            saw_encrypted = true;
                        }
                    }
                }
                "reasoning.encrypted" => {
                    if let Some(data) = detail_obj.get("data") {
                        if !matches!(data, Value::Null) {
                            if let Some(s) = data.as_str() {
                                if s.is_empty() {
                                    continue;
                                }
                            }
                            parts.push(Part::ReasoningEncrypted {
                                data: data.clone(),
                                extra_body: HashMap::new(),
                            });
                            saw_encrypted = true;
                        }
                    }
                }
                "reasoning.summary" => {
                    if let Some(summary) = detail_obj.get("summary").and_then(|v| v.as_str()) {
                        if !summary.is_empty() {
                            parts.push(Part::Reasoning {
                                content: summary.to_string(),
                                extra_body: HashMap::new(),
                            });
                            saw_plain = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if !saw_plain {
        if let Some(reasoning) = msg_obj.get("reasoning").and_then(|v| v.as_str()) {
            if !reasoning.is_empty() {
                parts.push(Part::Reasoning {
                    content: reasoning.to_string(),
                    extra_body: HashMap::new(),
                });
                saw_plain = true;
            }
        }
    }

    if !saw_plain {
        if let Some(reasoning) = msg_obj.get("reasoning_content").and_then(|v| v.as_str()) {
            if !reasoning.is_empty() {
                parts.push(Part::Reasoning {
                    content: reasoning.to_string(),
                    extra_body: HashMap::new(),
                });
            }
        }
    }

    if !saw_encrypted {
        if let Some(opaque) = msg_obj.get("reasoning_opaque").and_then(|v| v.as_str()) {
            if !opaque.is_empty() {
                parts.push(Part::ReasoningEncrypted {
                    data: Value::String(opaque.to_string()),
                    extra_body: HashMap::new(),
                });
            }
        }
    }
}

fn tool_choice_from_value(v: Value) -> ToolChoice {
    if let Some(s) = v.as_str() {
        ToolChoice::Mode(s.to_string())
    } else {
        ToolChoice::Specific(v)
    }
}

fn parse_response_format(v: Value) -> Option<crate::urp::ResponseFormat> {
    if let Some(obj) = v.as_object() {
        match obj.get("type").and_then(|x| x.as_str()) {
            Some("json_object") => return Some(crate::urp::ResponseFormat::JsonObject),
            Some("json_schema") => {
                let schema_obj = obj.get("json_schema")?.as_object()?;
                let name = schema_obj.get("name")?.as_str()?.to_string();
                let schema = schema_obj.get("schema").cloned().unwrap_or(Value::Null);
                let description = schema_obj
                    .get("description")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let strict = schema_obj.get("strict").and_then(|x| x.as_bool());
                let mut extra = HashMap::new();
                for (k, v) in schema_obj {
                    if !["name", "schema", "description", "strict"].contains(&k.as_str()) {
                        extra.insert(k.clone(), v.clone());
                    }
                }
                return Some(crate::urp::ResponseFormat::JsonSchema {
                    json_schema: crate::urp::JsonSchemaDefinition {
                        name,
                        description,
                        schema,
                        strict,
                        extra_body: extra,
                    },
                });
            }
            _ => {}
        }
    }
    if let Some(s) = v.as_str() {
        if s == "json_object" {
            return Some(crate::urp::ResponseFormat::JsonObject);
        }
        if s == "text" {
            return Some(crate::urp::ResponseFormat::Text);
        }
    }
    None
}

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

fn parse_usage_from_chat(obj: &Map<String, Value>) -> Usage {
    let prompt_tokens = obj
        .get("prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = obj
        .get("completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reasoning_tokens = obj
        .get("completion_tokens_details")
        .and_then(|v| v.get("reasoning_tokens"))
        .and_then(|v| v.as_u64());
    let cached_tokens = obj
        .get("prompt_tokens_details")
        .and_then(|v| v.get("cached_tokens"))
        .and_then(|v| v.as_u64());

    Usage {
        prompt_tokens,
        completion_tokens,
        reasoning_tokens,
        cached_tokens,
        extra_body: split_extra(obj, &["prompt_tokens", "completion_tokens", "total_tokens"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_response_reads_openrouter_reasoning_details() {
        let value = json!({
            "id": "chatcmpl_test",
            "model": "m",
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "ok",
                    "reasoning": "new_reasoning",
                    "reasoning_details": [{
                        "type": "reasoning.text",
                        "text": "new_reasoning",
                        "signature": "new_sig",
                        "format": "unknown"
                    }],
                    "reasoning_content": "legacy_reasoning",
                    "reasoning_opaque": "legacy_sig"
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let mut saw_reasoning = false;
        let mut saw_sig = false;
        for part in decoded.message.parts {
            match part {
                Part::Reasoning { content, .. } => {
                    assert_eq!(content, "new_reasoning");
                    saw_reasoning = true;
                }
                Part::ReasoningEncrypted { data, .. } => {
                    assert_eq!(data.as_str().unwrap_or(""), "new_sig");
                    saw_sig = true;
                }
                _ => {}
            }
        }
        assert!(saw_reasoning);
        assert!(saw_sig);
    }

    #[test]
    fn decode_response_accepts_legacy_reasoning_fields() {
        let value = json!({
            "id": "chatcmpl_test",
            "model": "m",
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "ok",
                    "reasoning_content": "legacy_reasoning",
                    "reasoning_opaque": "legacy_sig"
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let mut saw_reasoning = false;
        let mut saw_sig = false;
        for part in decoded.message.parts {
            match part {
                Part::Reasoning { content, .. } => {
                    assert_eq!(content, "legacy_reasoning");
                    saw_reasoning = true;
                }
                Part::ReasoningEncrypted { data, .. } => {
                    assert_eq!(data.as_str().unwrap_or(""), "legacy_sig");
                    saw_sig = true;
                }
                _ => {}
            }
        }
        assert!(saw_reasoning);
        assert!(saw_sig);
    }
}
