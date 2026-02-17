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
        .ok_or_else(|| "responses request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let mut messages = Vec::new();

    if let Some(instructions) = obj.get("instructions").and_then(|v| v.as_str()) {
        if !instructions.is_empty() {
            messages.push(Message::text(Role::Developer, instructions));
        }
    }

    if let Some(input) = obj.get("input") {
        decode_input_items(input, &mut messages);
    }

    let reasoning = obj
        .get("reasoning")
        .and_then(|v| v.as_object())
        .and_then(|reasoning_obj| {
            let effort = reasoning_obj
                .get("effort")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if effort.is_none() {
                return None;
            }
            Some(ReasoningConfig {
                effort,
                extra_body: split_extra(reasoning_obj, &["effort"]),
            })
        });

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
        max_output_tokens: obj.get("max_output_tokens").and_then(|v| v.as_u64()),
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
                "instructions",
                "input",
                "stream",
                "temperature",
                "top_p",
                "max_output_tokens",
                "reasoning",
                "tools",
                "tool_choice",
                "response_format",
                "text",
                "user",
            ],
        ),
    })
}

fn decode_input_items(input: &Value, out: &mut Vec<Message>) {
    if let Some(s) = input.as_str() {
        out.push(Message::text(Role::User, s));
        return;
    }

    if let Some(obj) = input.as_object() {
        decode_input_item(obj, out);
        return;
    }

    if let Some(arr) = input.as_array() {
        for item in arr {
            if let Some(obj) = item.as_object() {
                decode_input_item(obj, out);
            } else if let Some(s) = item.as_str() {
                out.push(Message::text(Role::User, s));
            }
        }
    }
}

fn decode_input_item(obj: &Map<String, Value>, out: &mut Vec<Message>) {
    let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match item_type {
        "function_call" => {
            let call_id = obj
                .get("call_id")
                .or_else(|| obj.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = obj
                .get("arguments")
                .cloned()
                .unwrap_or(Value::String("{}".to_string()));
            let arguments = if let Some(s) = arguments.as_str() {
                s.to_string()
            } else {
                serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
            };
            let mut msg = Message::new(Role::Assistant);
            msg.parts.push(Part::ToolCall {
                call_id,
                name,
                arguments,
                extra_body: split_extra(obj, &["type", "call_id", "id", "name", "arguments"]),
            });
            out.push(msg);
        }
        "function_call_output" => {
            let call_id = obj
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut msg = Message::new(Role::Tool);
            msg.parts.push(Part::ToolResult {
                call_id,
                is_error: false,
                extra_body: split_extra(obj, &["type", "call_id", "output"]),
            });
            if let Some(output) = obj.get("output") {
                decode_tool_output_parts(output, &mut msg.parts);
            }
            out.push(msg);
        }
        "message" | "" => {
            let role = match obj.get("role").and_then(|v| v.as_str()).unwrap_or("user") {
                "system" => Role::System,
                "developer" => Role::Developer,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            let mut msg = Message {
                role,
                parts: Vec::new(),
                extra_body: split_extra(obj, &["type", "role", "content"]),
            };

            if let Some(content) = obj.get("content") {
                if let Some(s) = content.as_str() {
                    if !s.is_empty() {
                        msg.parts.push(Part::Text {
                            content: s.to_string(),
                            extra_body: HashMap::new(),
                        });
                    }
                } else if let Some(content_arr) = content.as_array() {
                    for p in content_arr {
                        let Some(pobj) = p.as_object() else { continue };
                        let ptype = pobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match ptype {
                            "input_text" | "output_text" | "text" => {
                                if let Some(text) = pobj
                                    .get("text")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| pobj.get("content").and_then(|v| v.as_str()))
                                {
                                    msg.parts.push(Part::Text {
                                        content: text.to_string(),
                                        extra_body: split_extra(pobj, &["type", "text", "content"]),
                                    });
                                }
                            }
                            "refusal" => {
                                if let Some(text) = pobj.get("refusal").and_then(|v| v.as_str()) {
                                    msg.parts.push(Part::Refusal {
                                        content: text.to_string(),
                                        extra_body: split_extra(pobj, &["type", "refusal"]),
                                    });
                                }
                            }
                            _ => {
                                if let Some(image) = parse_image_part_from_obj(pobj) {
                                    msg.parts.push(image);
                                }
                                if let Some(file) = parse_file_part_from_obj(pobj) {
                                    msg.parts.push(file);
                                }
                            }
                        }
                    }
                }
            }

            out.push(msg);
        }
        _ => {
            let mut msg = Message::new(Role::User);
            msg.parts.push(Part::Text {
                content: serde_json::to_string(obj).unwrap_or_default(),
                extra_body: HashMap::new(),
            });
            out.push(msg);
        }
    }
}

fn decode_tool_output_parts(output: &Value, parts: &mut Vec<Part>) {
    match output {
        Value::String(text) => {
            if !text.is_empty() {
                parts.push(Part::Text {
                    content: text.to_string(),
                    extra_body: HashMap::new(),
                });
            }
        }
        Value::Array(items) => {
            for item in items {
                decode_tool_output_part(item, parts);
            }
        }
        Value::Object(_) => decode_tool_output_part(output, parts),
        other => {
            let text = value_to_text(other);
            if !text.is_empty() {
                parts.push(Part::Text {
                    content: text,
                    extra_body: HashMap::new(),
                });
            }
        }
    }
}

fn decode_tool_output_part(value: &Value, parts: &mut Vec<Part>) {
    if let Some(text) = value.as_str() {
        if !text.is_empty() {
            parts.push(Part::Text {
                content: text.to_string(),
                extra_body: HashMap::new(),
            });
        }
        return;
    }
    let Some(obj) = value.as_object() else {
        let text = value_to_text(value);
        if !text.is_empty() {
            parts.push(Part::Text {
                content: text,
                extra_body: HashMap::new(),
            });
        }
        return;
    };

    let ptype = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match ptype {
        "input_text" | "output_text" | "text" => {
            if let Some(text) = obj
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("content").and_then(|v| v.as_str()))
            {
                parts.push(Part::Text {
                    content: text.to_string(),
                    extra_body: split_extra(obj, &["type", "text", "content"]),
                });
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
            let text = value_to_text(value);
            if !text.is_empty() {
                parts.push(Part::Text {
                    content: text,
                    extra_body: HashMap::new(),
                });
            }
        }
    }
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "responses response must be object".to_string())?;

    let mut message = Message::new(Role::Assistant);

    if let Some(output) = obj.get("output").and_then(|v| v.as_array()) {
        for item in output {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            let item_type = item_obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match item_type {
                "message" => {
                    if let Some(content_arr) = item_obj.get("content").and_then(|v| v.as_array()) {
                        for p in content_arr {
                            let Some(pobj) = p.as_object() else { continue };
                            let ptype = pobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match ptype {
                                "output_text" | "text" => {
                                    if let Some(text) = pobj.get("text").and_then(|v| v.as_str()) {
                                        message.parts.push(Part::Text {
                                            content: text.to_string(),
                                            extra_body: split_extra(pobj, &["type", "text"]),
                                        });
                                    }
                                }
                                "refusal" => {
                                    if let Some(text) = pobj.get("refusal").and_then(|v| v.as_str())
                                    {
                                        message.parts.push(Part::Refusal {
                                            content: text.to_string(),
                                            extra_body: split_extra(pobj, &["type", "refusal"]),
                                        });
                                    }
                                }
                                _ => {
                                    if let Some(image) = parse_image_part_from_obj(pobj) {
                                        message.parts.push(image);
                                    }
                                    if let Some(file) = parse_file_part_from_obj(pobj) {
                                        message.parts.push(file);
                                    }
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    let call_id = item_obj
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item_obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = item_obj
                        .get("arguments")
                        .cloned()
                        .unwrap_or(Value::String("{}".to_string()));
                    let arguments = if let Some(s) = arguments.as_str() {
                        s.to_string()
                    } else {
                        serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
                    };
                    message.parts.push(Part::ToolCall {
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(
                            item_obj,
                            &["type", "call_id", "name", "arguments"],
                        ),
                    });
                }
                "reasoning" => {
                    if let Some(encrypted) = item_obj.get("encrypted_content") {
                        message.parts.push(Part::ReasoningEncrypted {
                            data: encrypted.clone(),
                            extra_body: split_extra(
                                item_obj,
                                &["type", "encrypted_content", "summary", "text"],
                            ),
                        });
                    }
                    if let Some(text) = summary_to_text(item_obj) {
                        if !text.is_empty() {
                            message.parts.push(Part::Reasoning {
                                content: text,
                                extra_body: split_extra(
                                    item_obj,
                                    &["type", "summary", "text", "encrypted_content"],
                                ),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let finish_reason = match obj.get("status").and_then(|v| v.as_str()) {
        Some("completed") => Some(
            if message
                .parts
                .iter()
                .any(|p| matches!(p, Part::ToolCall { .. }))
            {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            },
        ),
        Some("incomplete") => Some(FinishReason::Length),
        Some("failed") => Some(FinishReason::Other),
        _ => None,
    };

    let usage = obj
        .get("usage")
        .and_then(|v| v.as_object())
        .map(parse_usage_from_responses);

    Ok(UrpResponse {
        id: obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("resp")
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
                "id", "object", "created", "model", "status", "output", "usage", "error",
            ],
        ),
    })
}

fn summary_to_text(item_obj: &Map<String, Value>) -> Option<String> {
    if let Some(t) = item_obj.get("text").and_then(|v| v.as_str()) {
        return Some(t.to_string());
    }
    let mut out = String::new();
    if let Some(summary) = item_obj.get("summary").and_then(|v| v.as_array()) {
        for s in summary {
            if s.get("type").and_then(|v| v.as_str()) == Some("summary_text") {
                if let Some(t) = s.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                }
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn parse_usage_from_responses(obj: &Map<String, Value>) -> Usage {
    Usage {
        prompt_tokens: obj
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| obj.get("prompt_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0),
        completion_tokens: obj
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| obj.get("completion_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0),
        reasoning_tokens: obj
            .get("output_tokens_details")
            .and_then(|v| v.get("reasoning_tokens"))
            .and_then(|v| v.as_u64()),
        cached_tokens: obj
            .get("input_tokens_details")
            .and_then(|v| v.get("cached_tokens"))
            .and_then(|v| v.as_u64()),
        extra_body: split_extra(
            obj,
            &[
                "input_tokens",
                "output_tokens",
                "total_tokens",
                "prompt_tokens",
                "completion_tokens",
            ],
        ),
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
        if obj.get("type").and_then(|x| x.as_str()) == Some("json_schema") {
            let schema_obj = obj.get("json_schema")?.as_object()?;
            let name = schema_obj.get("name")?.as_str()?.to_string();
            let schema = schema_obj.get("schema").cloned().unwrap_or(Value::Null);
            return Some(crate::urp::ResponseFormat::JsonSchema {
                json_schema: crate::urp::JsonSchemaDefinition {
                    name,
                    description: schema_obj
                        .get("description")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    schema,
                    strict: schema_obj.get("strict").and_then(|x| x.as_bool()),
                    extra_body: split_extra(
                        schema_obj,
                        &["name", "description", "schema", "strict"],
                    ),
                },
            });
        }
        if obj.get("type").and_then(|x| x.as_str()) == Some("json_object") {
            return Some(crate::urp::ResponseFormat::JsonObject);
        }
    }
    None
}
