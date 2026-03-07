use crate::urp::decode::{
    parse_file_part_from_obj, parse_image_part_from_obj, parse_tool_definition, split_extra,
    value_to_text,
};
use crate::urp::{
    FinishReason, InputDetails, Message, OutputDetails, Part, ReasoningConfig, Role, ToolChoice,
    UrpRequest, UrpResponse, Usage,
};
use serde_json::Value;
use std::collections::HashMap;

fn text_part_with_phase(
    content: impl Into<String>,
    phase: Option<&str>,
    extra_body: HashMap<String, Value>,
) -> Part {
    Part::Text {
        content: content.into(),
        phase: phase.map(str::to_string),
        extra_body,
    }
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "messages request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let mut messages = Vec::new();

    if let Some(system) = obj.get("system") {
        let system_text = if let Some(s) = system.as_str() {
            s.to_string()
        } else if let Some(arr) = system.as_array() {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };
        if !system_text.is_empty() {
            messages.push(Message::text(Role::System, system_text));
        }
    }

    for raw_msg in obj
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing messages".to_string())?
    {
        let Some(msg_obj) = raw_msg.as_object() else {
            continue;
        };
        let base_role = match msg_obj
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
        {
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "developer" => Role::Developer,
            _ => Role::User,
        };

        let mut msg = Message {
            role: base_role,
            parts: Vec::new(),
            extra_body: split_extra(msg_obj, &["role", "content"]),
        };

        let mut tool_messages: Vec<Message> = Vec::new();
        let content = msg_obj.get("content").cloned().unwrap_or(Value::Null);
        if let Some(s) = content.as_str() {
            if !s.is_empty() {
                msg.parts
                    .push(text_part_with_phase(s, None, HashMap::new()));
            }
        } else if let Some(blocks) = content.as_array() {
            for block in blocks {
                let Some(bobj) = block.as_object() else {
                    continue;
                };
                let btype = bobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match btype {
                    "text" => {
                        if let Some(text) = bobj.get("text").and_then(|v| v.as_str()) {
                            msg.parts.push(text_part_with_phase(
                                text,
                                bobj.get("phase").and_then(|v| v.as_str()),
                                split_extra(bobj, &["type", "text", "phase"]),
                            ));
                        }
                    }
                    "thinking" => {
                        if let Some(thinking) = bobj.get("thinking").and_then(|v| v.as_str()) {
                            msg.parts.push(Part::Reasoning {
                                content: thinking.to_string(),
                                extra_body: split_extra(bobj, &["type", "thinking", "signature"]),
                            });
                        }
                        if let Some(signature) = bobj.get("signature").and_then(|v| v.as_str()) {
                            if !signature.is_empty() {
                                msg.parts.push(Part::ReasoningEncrypted {
                                    data: Value::String(signature.to_string()),
                                    extra_body: HashMap::new(),
                                });
                            }
                        }
                    }
                    "tool_use" => {
                        let call_id = bobj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = bobj
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = bobj.get("input").cloned().unwrap_or(Value::Null);
                        let arguments =
                            serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string());
                        msg.parts.push(Part::ToolCall {
                            call_id,
                            name,
                            arguments,
                            extra_body: split_extra(bobj, &["type", "id", "name", "input"]),
                        });
                    }
                    "tool_result" => {
                        let call_id = bobj
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let is_error = bobj
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let mut tool_msg = Message::new(Role::Tool);
                        tool_msg.parts.push(Part::ToolResult {
                            call_id,
                            is_error,
                            extra_body: split_extra(
                                bobj,
                                &["type", "tool_use_id", "is_error", "content"],
                            ),
                        });
                        decode_tool_result_content(bobj.get("content"), &mut tool_msg.parts);
                        tool_messages.push(tool_msg);
                    }
                    _ => {
                        msg.parts.push(text_part_with_phase(
                            serde_json::to_string(block).unwrap_or_default(),
                            None,
                            HashMap::new(),
                        ));
                    }
                }
            }
        }

        if !msg.parts.is_empty() {
            messages.push(msg);
        }
        messages.extend(tool_messages);
    }

    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(parse_tool_definition)
            .collect::<Vec<_>>()
    });

    let reasoning = {
        let thinking = obj.get("thinking").and_then(|v| v.as_object());
        let thinking_type = thinking
            .and_then(|t| t.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match thinking_type {
            "adaptive" => {
                let effort = obj
                    .get("output_config")
                    .and_then(|v| v.get("effort"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let mut extra = thinking
                    .map(|t| split_extra(t, &["type"]))
                    .unwrap_or_default();
                if let Some(oc_extra) = obj.get("output_config").and_then(|v| v.as_object()) {
                    for (k, v) in split_extra(oc_extra, &["effort"]) {
                        extra.insert(format!("output_config.{k}"), v);
                    }
                }
                Some(ReasoningConfig {
                    effort,
                    extra_body: extra,
                })
            }
            "enabled" => {
                let budget = thinking
                    .and_then(|t| t.get("budget_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let effort = if budget == 0 {
                    None
                } else if budget <= 512 {
                    Some("low".to_string())
                } else if budget >= 2048 {
                    Some("high".to_string())
                } else {
                    Some("medium".to_string())
                };
                Some(ReasoningConfig {
                    effort,
                    extra_body: thinking
                        .map(|t| split_extra(t, &["type", "budget_tokens"]))
                        .unwrap_or_default(),
                })
            }
            _ => None,
        }
    };

    Ok(UrpRequest {
        model,
        messages,
        stream: obj.get("stream").and_then(|v| v.as_bool()),
        temperature: obj.get("temperature").and_then(|v| v.as_f64()),
        top_p: obj.get("top_p").and_then(|v| v.as_f64()),
        max_output_tokens: obj.get("max_tokens").and_then(|v| v.as_u64()),
        reasoning,
        tools,
        tool_choice: obj
            .get("tool_choice")
            .cloned()
            .map(tool_choice_from_messages_value),
        response_format: None,
        user: obj
            .get("metadata")
            .and_then(|v| v.get("user_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        extra_body: split_extra(
            obj,
            &[
                "model",
                "messages",
                "system",
                "stream",
                "temperature",
                "top_p",
                "max_tokens",
                "thinking",
                "output_config",
                "tools",
                "tool_choice",
                "metadata",
            ],
        ),
    })
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "messages response must be object".to_string())?;

    let mut message = Message::new(Role::Assistant);
    if let Some(content) = obj.get("content").and_then(|v| v.as_array()) {
        for block in content {
            let Some(bobj) = block.as_object() else {
                continue;
            };
            let btype = bobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match btype {
                "text" => {
                    if let Some(text) = bobj.get("text").and_then(|v| v.as_str()) {
                        message.parts.push(text_part_with_phase(
                            text,
                            bobj.get("phase").and_then(|v| v.as_str()),
                            split_extra(bobj, &["type", "text", "phase"]),
                        ));
                    }
                }
                "thinking" => {
                    if let Some(thinking) = bobj.get("thinking").and_then(|v| v.as_str()) {
                        message.parts.push(Part::Reasoning {
                            content: thinking.to_string(),
                            extra_body: split_extra(bobj, &["type", "thinking", "signature"]),
                        });
                    }
                    if let Some(signature) = bobj.get("signature").and_then(|v| v.as_str()) {
                        if !signature.is_empty() {
                            message.parts.push(Part::ReasoningEncrypted {
                                data: Value::String(signature.to_string()),
                                extra_body: HashMap::new(),
                            });
                        }
                    }
                }
                "tool_use" => {
                    let call_id = bobj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = bobj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments =
                        serde_json::to_string(&bobj.get("input").cloned().unwrap_or(Value::Null))
                            .unwrap_or_else(|_| "{}".to_string());
                    message.parts.push(Part::ToolCall {
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(bobj, &["type", "id", "name", "input"]),
                    });
                }
                "image" => {
                    if let Some(image) = parse_image_part_from_obj(bobj) {
                        message.parts.push(image);
                    }
                }
                "document" | "file" => {
                    if let Some(file) = parse_file_part_from_obj(bobj) {
                        message.parts.push(file);
                    }
                }
                _ => {
                    message.parts.push(text_part_with_phase(
                        serde_json::to_string(block).unwrap_or_default(),
                        None,
                        HashMap::new(),
                    ));
                }
            }
        }
    }

    let finish_reason = match obj.get("stop_reason").and_then(|v| v.as_str()) {
        Some("end_turn") => Some(FinishReason::Stop),
        Some("max_tokens") => Some(FinishReason::Length),
        Some("tool_use") => Some(FinishReason::ToolCalls),
        _ => Some(FinishReason::Other),
    };

    let usage = obj.get("usage").and_then(|v| v.as_object()).map(|u| {
        let input_tokens = u
            .get("input_tokens")
            .or_else(|| u.get("prompt_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_tokens = u
            .get("output_tokens")
            .or_else(|| u.get("completion_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_read_tokens = u
            .get("cache_read_input_tokens")
            .or_else(|| u.get("cache_read_tokens"))
            .or_else(|| u.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_creation_tokens = u
            .get("cache_creation_input_tokens")
            .or_else(|| u.get("cache_creation_tokens"))
            .or_else(|| u.get("cache_write_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let tool_prompt_tokens = u
            .get("tool_prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let reasoning_tokens = u
            .get("reasoning_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let accepted_prediction_tokens = u
            .get("accepted_prediction_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let rejected_prediction_tokens = u
            .get("rejected_prediction_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let input_details =
            if cache_read_tokens > 0 || cache_creation_tokens > 0 || tool_prompt_tokens > 0 {
                Some(InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens,
                    cache_creation_tokens,
                    tool_prompt_tokens,
                    modality_breakdown: None,
                })
            } else {
                None
            };

        let output_details = if reasoning_tokens > 0
            || accepted_prediction_tokens > 0
            || rejected_prediction_tokens > 0
        {
            Some(OutputDetails {
                standard_tokens: 0,
                reasoning_tokens,
                accepted_prediction_tokens,
                rejected_prediction_tokens,
                modality_breakdown: None,
            })
        } else {
            None
        };

        Usage {
            input_tokens,
            output_tokens,
            input_details,
            output_details,
            extra_body: split_extra(u, &["input_tokens", "output_tokens"]),
        }
    });

    Ok(UrpResponse {
        id: obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("msg")
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
            &[
                "id",
                "type",
                "role",
                "model",
                "content",
                "stop_reason",
                "usage",
            ],
        ),
    })
}

fn tool_choice_from_messages_value(v: Value) -> ToolChoice {
    if let Some(obj) = v.as_object() {
        match obj.get("type").and_then(|x| x.as_str()) {
            Some("auto") => return ToolChoice::Mode("auto".to_string()),
            Some("any") => return ToolChoice::Mode("required".to_string()),
            Some("none") => return ToolChoice::Mode("none".to_string()),
            Some("tool") => {
                if let Some(name) = obj.get("name").and_then(|x| x.as_str()) {
                    return ToolChoice::Specific(serde_json::json!({
                        "type": "function",
                        "function": { "name": name }
                    }));
                }
            }
            _ => {}
        }
    }
    if let Some(s) = v.as_str() {
        return ToolChoice::Mode(s.to_string());
    }
    ToolChoice::Specific(v)
}

fn decode_tool_result_content(content: Option<&Value>, parts: &mut Vec<Part>) {
    let Some(content) = content else {
        return;
    };
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            parts.push(text_part_with_phase(text, None, HashMap::new()));
        }
        return;
    }

    if let Some(blocks) = content.as_array() {
        for block in blocks {
            decode_tool_result_content_block(block, parts);
        }
        return;
    }

    if let Some(obj) = content.as_object() {
        decode_tool_result_content_block(&Value::Object(obj.clone()), parts);
        return;
    }

    let text = value_to_text(content);
    if !text.is_empty() {
        parts.push(text_part_with_phase(text, None, HashMap::new()));
    }
}

fn decode_tool_result_content_block(block: &Value, parts: &mut Vec<Part>) {
    if let Some(text) = block.as_str() {
        if !text.is_empty() {
            parts.push(text_part_with_phase(text, None, HashMap::new()));
        }
        return;
    }
    let Some(obj) = block.as_object() else {
        let text = value_to_text(block);
        if !text.is_empty() {
            parts.push(text_part_with_phase(text, None, HashMap::new()));
        }
        return;
    };

    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "text" => {
            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                parts.push(text_part_with_phase(
                    text,
                    obj.get("phase").and_then(|v| v.as_str()),
                    split_extra(obj, &["type", "text", "phase"]),
                ));
            }
        }
        _ => {
            if let Some(image) = parse_image_part_from_obj(obj) {
                parts.push(image);
                return;
            }
            if let Some(file) = parse_file_part_from_obj(obj) {
                parts.push(file);
                return;
            }
            let text = value_to_text(block);
            if !text.is_empty() {
                parts.push(text_part_with_phase(text, None, HashMap::new()));
            }
        }
    }
}
