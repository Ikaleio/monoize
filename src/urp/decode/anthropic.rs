use crate::urp::decode::{
    deserialize_u64ish_default, parse_file_part_from_obj, parse_image_part_from_obj,
    parse_tool_definition, split_extra, value_to_text,
};
use crate::urp::{
    unwrap_reasoning_signature_sigil, FinishReason, InputDetails, Node, OrdinaryRole,
    OutputDetails, Part, ReasoningConfig, ToolChoice, ToolResultContent, UrpRequest, UrpResponse,
    Usage, REASONING_KIND_EXTRA_KEY, REASONING_KIND_REDACTED_THINKING,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

fn decode_anthropic_thinking_block(bobj: &Map<String, Value>) -> Option<Node> {
    let thinking = bobj
        .get("thinking")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let raw_signature = bobj
        .get("signature")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    if thinking.is_none() && raw_signature.is_none() {
        return None;
    }
    let (id, encrypted) = match raw_signature {
        Some(sig) => match unwrap_reasoning_signature_sigil(sig) {
            Some((id, original)) => (Some(id), Some(Value::String(original))),
            None => (None, Some(Value::String(sig.to_string()))),
        },
        None => (None, None),
    };
    Some(Node::Reasoning {
        id,
        content: thinking,
        encrypted,
        summary: None,
        source: None,
        extra_body: split_extra(bobj, &["type", "thinking", "signature"]),
    })
}

fn decode_anthropic_redacted_thinking_block(bobj: &Map<String, Value>) -> Option<Node> {
    let raw_data = bobj.get("data").cloned().filter(|v| match v {
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        _ => true,
    })?;
    let (id, encrypted) = match raw_data.as_str() {
        Some(sig) => match unwrap_reasoning_signature_sigil(sig) {
            Some((id, original)) => (Some(id), Value::String(original)),
            None => (None, raw_data.clone()),
        },
        None => (None, raw_data.clone()),
    };
    let mut extra_body = split_extra(bobj, &["type", "data"]);
    extra_body.insert(
        REASONING_KIND_EXTRA_KEY.to_string(),
        Value::String(REASONING_KIND_REDACTED_THINKING.to_string()),
    );
    Some(Node::Reasoning {
        id,
        content: None,
        encrypted: Some(encrypted),
        summary: None,
        source: None,
        extra_body,
    })
}

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

        // Normalize Anthropic's disjoint-bucket usage semantics to the internal
        // aggregate/inclusive invariant (spec: user-billing-and-model-metadata.spec.md § 5 C3-ii):
        // wire `input_tokens` excludes cache buckets; internal `input_tokens` MUST include them.
        // The stream/non-stream Anthropic encoders invert this by subtracting cache buckets back out.
        let normalized_input_tokens = value
            .input_tokens
            .saturating_add(value.cache_read_input_tokens)
            .saturating_add(value.cache_creation_input_tokens);

        Usage {
            input_tokens: normalized_input_tokens,
            output_tokens: value.output_tokens,
            input_details,
            output_details,
            extra_body: value.extra,
        }
    }
}

fn text_node_with_phase(
    role: OrdinaryRole,
    content: impl Into<String>,
    phase: Option<&str>,
    mut extra_body: HashMap<String, Value>,
) -> Node {
    if let Some(phase) = phase {
        extra_body.insert("phase".to_string(), Value::String(phase.to_string()));
    }
    Node::Text {
        id: None,
        role,
        content: content.into(),
        phase: phase.map(str::to_string),
        extra_body,
    }
}

fn attach_message_extra(node: &mut Node, extra_body: &HashMap<String, Value>) {
    if extra_body.is_empty() {
        return;
    }
    node.extra_body_mut().extend(extra_body.clone());
}

fn ordinary_role_from_messages_role(role: &str) -> OrdinaryRole {
    match role {
        "assistant" => OrdinaryRole::Assistant,
        "system" => OrdinaryRole::System,
        "developer" => OrdinaryRole::Developer,
        _ => OrdinaryRole::User,
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

    let mut input_nodes = Vec::new();

    if let Some(system) = obj.get("system") {
        if let Some(text) = system.as_str() {
            if !text.is_empty() {
                input_nodes.push(Node::Text {
                    id: None,
                    role: OrdinaryRole::System,
                    content: text.to_string(),
                    phase: None,
                    extra_body: HashMap::new(),
                });
            }
        } else if let Some(blocks) = system.as_array() {
            for block in blocks {
                let Some(bobj) = block.as_object() else {
                    continue;
                };
                let btype = bobj.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                match btype {
                    "text" => {
                        if let Some(text) = bobj.get("text").and_then(|v| v.as_str()) {
                            input_nodes.push(text_node_with_phase(
                                OrdinaryRole::System,
                                text,
                                bobj.get("phase").and_then(|v| v.as_str()),
                                split_extra(bobj, &["type", "text", "phase"]),
                            ));
                        }
                    }
                    _ => {
                        input_nodes.push(text_node_with_phase(
                            OrdinaryRole::System,
                            serde_json::to_string(block).unwrap_or_default(),
                            None,
                            HashMap::new(),
                        ));
                    }
                }
            }
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
        let base_role = ordinary_role_from_messages_role(
            msg_obj
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user"),
        );

        let msg_extra_body = split_extra(msg_obj, &["role", "content"]);
        let mut message_nodes = Vec::new();
        let content = msg_obj.get("content").cloned().unwrap_or(Value::Null);
        if let Some(s) = content.as_str() {
            if !s.is_empty() {
                message_nodes.push(Node::Text {
                    id: None,
                    role: base_role,
                    content: s.to_string(),
                    phase: None,
                    extra_body: HashMap::new(),
                });
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
                            message_nodes.push(text_node_with_phase(
                                base_role,
                                text,
                                bobj.get("phase").and_then(|v| v.as_str()),
                                split_extra(bobj, &["type", "text", "phase"]),
                            ));
                        }
                    }
                    "thinking" => {
                        if let Some(node) = decode_anthropic_thinking_block(bobj) {
                            message_nodes.push(node);
                        }
                    }
                    "redacted_thinking" => {
                        if let Some(node) = decode_anthropic_redacted_thinking_block(bobj) {
                            message_nodes.push(node);
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
                        message_nodes.push(Node::ToolCall {
                            id: bobj
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
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
                        message_nodes.push(Node::ToolResult {
                            id: None,
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
                        message_nodes.push(text_node_with_phase(
                            base_role,
                            serde_json::to_string(block).unwrap_or_default(),
                            None,
                            HashMap::new(),
                        ));
                    }
                }
            }
        }

        if let Some(first_node) = message_nodes.first_mut() {
            attach_message_extra(first_node, &msg_extra_body);
        }
        input_nodes.extend(message_nodes);
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
        input: input_nodes,
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

    let mut output_nodes = Vec::new();
    if let Some(content) = obj.get("content").and_then(|v| v.as_array()) {
        for block in content {
            let Some(bobj) = block.as_object() else {
                continue;
            };
            let btype = bobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let decoded_nodes = match btype {
                "text" => {
                    if let Some(text) = bobj.get("text").and_then(|v| v.as_str()) {
                        vec![text_node_with_phase(
                            OrdinaryRole::Assistant,
                            text,
                            bobj.get("phase").and_then(|v| v.as_str()),
                            split_extra(bobj, &["type", "text", "phase"]),
                        )]
                    } else {
                        Vec::new()
                    }
                }
                "thinking" => decode_anthropic_thinking_block(bobj)
                    .map(|node| vec![node])
                    .unwrap_or_default(),
                "redacted_thinking" => decode_anthropic_redacted_thinking_block(bobj)
                    .map(|node| vec![node])
                    .unwrap_or_default(),
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
                    vec![Node::ToolCall {
                        id: bobj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(bobj, &["type", "id", "name", "input"]),
                    }]
                }
                "image" => parse_image_part_from_obj(bobj)
                    .into_iter()
                    .map(|part| match part {
                        crate::urp::Part::Image { source, extra_body } => Node::Image {
                            id: None,
                            role: OrdinaryRole::Assistant,
                            source,
                            extra_body,
                        },
                        _ => unreachable!(),
                    })
                    .collect(),
                "document" | "file" => parse_file_part_from_obj(bobj)
                    .into_iter()
                    .map(|part| match part {
                        crate::urp::Part::File { source, extra_body } => Node::File {
                            id: None,
                            role: OrdinaryRole::Assistant,
                            source,
                            extra_body,
                        },
                        _ => unreachable!(),
                    })
                    .collect(),
                _ => {
                    vec![text_node_with_phase(
                        OrdinaryRole::Assistant,
                        serde_json::to_string(block).unwrap_or_default(),
                        None,
                        HashMap::new(),
                    )]
                }
            };

            output_nodes.extend(decoded_nodes);
        }
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
        created_at: None,
        output: output_nodes,
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
