use crate::urp::decode::{
    deserialize_u64ish_default, parse_file_part_from_obj, parse_image_part_from_obj,
    parse_tool_definition, split_extra, value_to_text,
};
use crate::urp::greedy::{Action, GreedyMerger};
use crate::urp::{
    FinishReason, InputDetails, Item, OutputDetails, Part, ReasoningConfig, Role, ToolChoice,
    ToolResultContent, UrpRequest, UrpResponse, Usage,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "prompt_tokens"
    )]
    input_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "completion_tokens"
    )]
    output_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_read_tokens",
        alias = "cached_tokens"
    )]
    cache_read_input_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_creation_tokens",
        alias = "cache_write_tokens"
    )]
    cache_creation_input_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "tool_prompt_input_tokens"
    )]
    tool_prompt_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "reasoning_output_tokens"
    )]
    reasoning_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "accepted_prediction_output_tokens"
    )]
    accepted_prediction_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "rejected_prediction_output_tokens"
    )]
    rejected_prediction_tokens: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

impl From<AnthropicUsage> for Usage {
    fn from(value: AnthropicUsage) -> Self {
        let input_details = if value.cache_read_input_tokens > 0
            || value.cache_creation_input_tokens > 0
            || value.tool_prompt_tokens > 0
        {
            Some(InputDetails {
                standard_tokens: 0,
                cache_read_tokens: value.cache_read_input_tokens,
                cache_creation_tokens: value.cache_creation_input_tokens,
                tool_prompt_tokens: value.tool_prompt_tokens,
                modality_breakdown: None,
            })
        } else {
            None
        };

        let output_details = if value.reasoning_tokens > 0
            || value.accepted_prediction_tokens > 0
            || value.rejected_prediction_tokens > 0
        {
            Some(OutputDetails {
                standard_tokens: 0,
                reasoning_tokens: value.reasoning_tokens,
                accepted_prediction_tokens: value.accepted_prediction_tokens,
                rejected_prediction_tokens: value.rejected_prediction_tokens,
                modality_breakdown: None,
            })
        } else {
            None
        };

        Usage {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            input_details,
            output_details,
            extra_body: value.extra,
        }
    }
}

fn text_part_with_phase(
    content: impl Into<String>,
    phase: Option<&str>,
    mut extra_body: HashMap<String, Value>,
) -> Part {
    if let Some(phase) = phase {
        extra_body.insert("phase".to_string(), Value::String(phase.to_string()));
    }
    Part::Text {
        content: content.into(),
        extra_body,
    }
}

fn make_item_message(role: Role, parts: Vec<Part>, extra_body: HashMap<String, Value>) -> Item {
    let mut body = serde_json::Map::new();
    body.insert("type".to_string(), Value::String("message".to_string()));
    body.insert(
        "role".to_string(),
        serde_json::to_value(role).expect("role serialization must succeed"),
    );
    body.insert(
        "parts".to_string(),
        serde_json::to_value(parts).expect("parts serialization must succeed"),
    );
    body.extend(extra_body);

    serde_json::from_value(Value::Object(body)).expect("message item serialization must succeed")
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

    let mut inputs = Vec::new();

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
            inputs.push(Item::text(Role::System, system_text));
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

        let mut parts = Vec::new();
        let msg_extra_body = split_extra(msg_obj, &["role", "content"]);

        let mut tool_messages: Vec<Item> = Vec::new();
        let content = msg_obj.get("content").cloned().unwrap_or(Value::Null);
        if let Some(s) = content.as_str() {
            if !s.is_empty() {
                parts.push(text_part_with_phase(s, None, HashMap::new()));
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
                            parts.push(text_part_with_phase(
                                text,
                                bobj.get("phase").and_then(|v| v.as_str()),
                                split_extra(bobj, &["type", "text", "phase"]),
                            ));
                        }
                    }
                    "thinking" => {
                        if let Some(thinking) = bobj.get("thinking").and_then(|v| v.as_str()) {
                            parts.push(Part::Reasoning {
                                content: Some(thinking.to_string()),
                                encrypted: None,
                                summary: None,
                                source: None,
                                extra_body: split_extra(bobj, &["type", "thinking", "signature"]),
                            });
                        }
                        if let Some(signature) = bobj.get("signature").and_then(|v| v.as_str()) {
                            if !signature.is_empty() {
                                parts.push(Part::Reasoning {
                                    content: None,
                                    encrypted: Some(Value::String(signature.to_string())),
                                    summary: None,
                                    source: None,
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
                        parts.push(Part::ToolCall {
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
                        tool_messages.push(Item::ToolResult {
                            call_id,
                            is_error,
                            content: decode_tool_result_content(bobj.get("content")),
                            extra_body: split_extra(
                                bobj,
                                &["type", "tool_use_id", "is_error", "content"],
                            ),
                        });
                    }
                    _ => {
                        parts.push(text_part_with_phase(
                            serde_json::to_string(block).unwrap_or_default(),
                            None,
                            HashMap::new(),
                        ));
                    }
                }
            }
        }

        if !parts.is_empty() {
            inputs.push(make_item_message(base_role, parts, msg_extra_body));
        }
        inputs.extend(tool_messages);
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
        inputs,
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

    let mut outputs = Vec::new();
    let mut merger = GreedyMerger::new();
    if let Some(content) = obj.get("content").and_then(|v| v.as_array()) {
        for block in content {
            let Some(bobj) = block.as_object() else {
                continue;
            };
            let btype = bobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let decoded_parts = match btype {
                "text" => {
                    if let Some(text) = bobj.get("text").and_then(|v| v.as_str()) {
                        vec![text_part_with_phase(
                            text,
                            bobj.get("phase").and_then(|v| v.as_str()),
                            split_extra(bobj, &["type", "text", "phase"]),
                        )]
                    } else {
                        Vec::new()
                    }
                }
                "thinking" => {
                    let mut parts = Vec::new();
                    if let Some(thinking) = bobj.get("thinking").and_then(|v| v.as_str()) {
                        parts.push(Part::Reasoning {
                            content: Some(thinking.to_string()),
                            encrypted: None,
                            summary: None,
                            source: None,
                            extra_body: split_extra(bobj, &["type", "thinking", "signature"]),
                        });
                    }
                    if let Some(signature) = bobj.get("signature").and_then(|v| v.as_str()) {
                        if !signature.is_empty() {
                            parts.push(Part::Reasoning {
                                content: None,
                                encrypted: Some(Value::String(signature.to_string())),
                                summary: None,
                                source: None,
                                extra_body: HashMap::new(),
                            });
                        }
                    }
                    parts
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
                    vec![Part::ToolCall {
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(bobj, &["type", "id", "name", "input"]),
                    }]
                }
                "image" => parse_image_part_from_obj(bobj).into_iter().collect(),
                "document" | "file" => parse_file_part_from_obj(bobj).into_iter().collect(),
                _ => {
                    vec![text_part_with_phase(
                        serde_json::to_string(block).unwrap_or_default(),
                        None,
                        HashMap::new(),
                    )]
                }
            };

            for part in decoded_parts {
                match merger.feed(part, Role::Assistant) {
                    Action::Append => {}
                    Action::FlushAndNew(flushed_parts) => {
                        outputs.push(make_item_message(
                            Role::Assistant,
                            flushed_parts,
                            HashMap::new(),
                        ));
                    }
                }
            }
        }
    }

    if let Some(flushed_parts) = merger.finish() {
        outputs.push(make_item_message(
            Role::Assistant,
            flushed_parts,
            HashMap::new(),
        ));
    }

    let finish_reason = match obj.get("stop_reason").and_then(|v| v.as_str()) {
        Some("end_turn") => Some(FinishReason::Stop),
        Some("max_tokens") => Some(FinishReason::Length),
        Some("tool_use") => Some(FinishReason::ToolCalls),
        _ => Some(FinishReason::Other),
    };

    let usage = obj
        .get("usage")
        .cloned()
        .and_then(|v| serde_json::from_value::<AnthropicUsage>(v).ok())
        .map(Usage::from);

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
        outputs,
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

fn decode_tool_result_content(content: Option<&Value>) -> Vec<ToolResultContent> {
    let mut blocks = Vec::new();
    let Some(content) = content else {
        return blocks;
    };
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            blocks.push(ToolResultContent::Text {
                text: text.to_string(),
            });
        }
        return blocks;
    }

    if let Some(blocks) = content.as_array() {
        let mut decoded = Vec::new();
        for block in blocks {
            decode_tool_result_content_block(block, &mut decoded);
        }
        return decoded;
    }

    if let Some(obj) = content.as_object() {
        decode_tool_result_content_block(&Value::Object(obj.clone()), &mut blocks);
        return blocks;
    }

    let text = value_to_text(content);
    if !text.is_empty() {
        blocks.push(ToolResultContent::Text { text });
    }
    blocks
}

fn decode_tool_result_content_block(block: &Value, content: &mut Vec<ToolResultContent>) {
    if let Some(text) = block.as_str() {
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text: text.to_string(),
            });
        }
        return;
    }
    let Some(obj) = block.as_object() else {
        let text = value_to_text(block);
        if !text.is_empty() {
            content.push(ToolResultContent::Text { text });
        }
        return;
    };

    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "text" => {
            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                content.push(ToolResultContent::Text {
                    text: text.to_string(),
                });
            }
        }
        _ => {
            if let Some(image) = parse_image_part_from_obj(obj) {
                let Part::Image { source, .. } = image else {
                    unreachable!();
                };
                content.push(ToolResultContent::Image { source });
                return;
            }
            if let Some(file) = parse_file_part_from_obj(obj) {
                let Part::File { source, .. } = file else {
                    unreachable!();
                };
                content.push(ToolResultContent::File { source });
                return;
            }
            let text = value_to_text(block);
            if !text.is_empty() {
                content.push(ToolResultContent::Text { text });
            }
        }
    }
}
