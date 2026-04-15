use crate::urp::decode::{
    deserialize_u64ish_default, parse_file_part_from_obj, parse_image_part_from_obj,
    parse_tool_definition, split_extra, value_to_text,
};
use crate::urp::{
    FinishReason, InputDetails, Node, OrdinaryRole, OutputDetails, Part, ReasoningConfig, Role,
    ToolChoice, ToolResultContent, UrpRequest, UrpResponse, Usage,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponsesUsage {
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
    #[serde(default)]
    #[serde(alias = "prompt_tokens_details")]
    input_tokens_details: Option<OpenAiResponsesInputDetails>,
    #[serde(default)]
    #[serde(alias = "completion_tokens_details")]
    output_tokens_details: Option<OpenAiResponsesOutputDetails>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponsesInputDetails {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_read_tokens"
    )]
    cached_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_creation_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_write_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "tool_prompt_input_tokens"
    )]
    tool_prompt_tokens: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponsesOutputDetails {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
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

impl From<OpenAiResponsesUsage> for Usage {
    fn from(value: OpenAiResponsesUsage) -> Self {
        let OpenAiResponsesUsage {
            input_tokens,
            output_tokens,
            input_tokens_details,
            output_tokens_details,
            mut extra,
        } = value;

        let input_details = input_tokens_details.clone().and_then(|details| {
            let cache_creation_tokens = details
                .cache_creation_tokens
                .max(details.cache_write_tokens);
            if details.cached_tokens > 0
                || cache_creation_tokens > 0
                || details.tool_prompt_tokens > 0
            {
                Some(InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: details.cached_tokens,
                    cache_creation_tokens,
                    tool_prompt_tokens: details.tool_prompt_tokens,
                    modality_breakdown: None,
                })
            } else {
                None
            }
        });

        let output_details = output_tokens_details.clone().and_then(|details| {
            if details.reasoning_tokens > 0
                || details.accepted_prediction_tokens > 0
                || details.rejected_prediction_tokens > 0
            {
                Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: details.reasoning_tokens,
                    accepted_prediction_tokens: details.accepted_prediction_tokens,
                    rejected_prediction_tokens: details.rejected_prediction_tokens,
                    modality_breakdown: None,
                })
            } else {
                None
            }
        });

        if let Some(details) = input_tokens_details {
            extra.extend(details.extra);
        }
        if let Some(details) = output_tokens_details {
            extra.extend(details.extra);
        }

        Usage {
            input_tokens,
            output_tokens,
            input_details,
            output_details,
            extra_body: extra,
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

fn push_message_nodes(
    out: &mut Vec<Node>,
    role: Role,
    id: Option<String>,
    parts: Vec<Part>,
    extra_body: HashMap<String, Value>,
) {
    let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
    for (index, part) in parts.into_iter().enumerate() {
        let mut node = part.into_node(ordinary_role);
        if index == 0 && !extra_body.is_empty() {
            node.extra_body_mut().extend(extra_body.clone());
        }
        if index == 0 {
            if id.is_some() {
                node.set_id(id.clone());
            }
        }
        out.push(node);
    }
}

fn push_message_nodes_with_envelope_control(
    out: &mut Vec<Node>,
    role: Role,
    id: Option<String>,
    parts: Vec<Part>,
    extra_body: HashMap<String, Value>,
) {
    if !extra_body.is_empty() && !parts.is_empty() {
        out.push(Node::NextDownstreamEnvelopeExtra { extra_body });
    }
    push_message_nodes(out, role, id, parts, HashMap::new());
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "responses request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let mut input_nodes = Vec::new();

    if let Some(instructions) = obj.get("instructions").and_then(|v| v.as_str()) {
        if !instructions.is_empty() {
            input_nodes.push(Node::text(OrdinaryRole::Developer, instructions));
        }
    }

    if let Some(input) = obj.get("input") {
        validate_stateless_responses_input(input)?;
        decode_input_items_nodes(input, &mut input_nodes);
    }

    let reasoning = obj
        .get("reasoning")
        .and_then(|v| v.as_object())
        .and_then(|reasoning_obj| {
            let effort = reasoning_obj
                .get("effort")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            effort.as_ref()?;
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
        input: input_nodes,
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

fn validate_stateless_responses_input(input: &Value) -> Result<(), String> {
    if input.is_string() {
        return Ok(());
    }

    if let Some(obj) = input.as_object() {
        return validate_stateless_responses_input_item(obj);
    }

    if let Some(arr) = input.as_array() {
        for item in arr {
            validate_stateless_responses_input(item)?;
        }
    }

    Ok(())
}

fn validate_stateless_responses_input_item(obj: &Map<String, Value>) -> Result<(), String> {
    if obj.get("type").and_then(|v| v.as_str()) == Some("item_reference") {
        return Err(
            "Monoize is stateless and does not support Responses item_reference continuations; replay the full prior assistant and function_call items instead."
                .to_string(),
        );
    }

    Ok(())
}

fn decode_input_items_nodes(input: &Value, out: &mut Vec<Node>) {
    if let Some(s) = input.as_str() {
        out.push(Node::text(OrdinaryRole::User, s));
        return;
    }

    if let Some(obj) = input.as_object() {
        decode_input_item_nodes(obj, out);
        return;
    }

    if let Some(arr) = input.as_array() {
        for item in arr {
            if let Some(obj) = item.as_object() {
                decode_input_item_nodes(obj, out);
            } else if let Some(s) = item.as_str() {
                out.push(Node::text(OrdinaryRole::User, s));
            }
        }
    }
}

fn decode_input_item_nodes(obj: &Map<String, Value>, out: &mut Vec<Node>) {
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
            out.push(Node::ToolCall {
                id: obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_tool_call_id())),
                call_id,
                name,
                arguments,
                extra_body: split_extra(obj, &["type", "call_id", "id", "name", "arguments"]),
            });
        }
        "function_call_output" => {
            let call_id = obj
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut content = Vec::new();
            if let Some(output) = obj.get("output") {
                decode_tool_result_content(output, &mut content);
            }
            out.push(Node::ToolResult {
                id: obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_tool_result_id())),
                call_id,
                is_error: false,
                content,
                extra_body: split_extra(obj, &["type", "call_id", "output"]),
            });
        }
        "message" | "" => {
            let role = match obj.get("role").and_then(|v| v.as_str()).unwrap_or("user") {
                "system" => Role::System,
                "developer" => Role::Developer,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            let message_phase = obj.get("phase").and_then(|v| v.as_str());
            let mut parts = Vec::new();

            if let Some(content) = obj.get("content") {
                if let Some(s) = content.as_str() {
                    if !s.is_empty() {
                        parts.push(text_part_with_phase(s, message_phase, HashMap::new()));
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
                                    parts.push(text_part_with_phase(
                                        text,
                                        message_phase,
                                        split_extra(pobj, &["type", "text", "content"]),
                                    ));
                                }
                            }
                            "refusal" => {
                                if let Some(text) = pobj.get("refusal").and_then(|v| v.as_str()) {
                                    parts.push(Part::Refusal {
                                        content: text.to_string(),
                                        extra_body: split_extra(pobj, &["type", "refusal"]),
                                    });
                                }
                            }
                            _ => {
                                if let Some(image) = parse_image_part_from_obj(pobj) {
                                    parts.push(image);
                                }
                                if let Some(file) = parse_file_part_from_obj(pobj) {
                                    parts.push(file);
                                }
                            }
                        }
                    }
                }
            }

            push_message_nodes_with_envelope_control(
                out,
                role,
                obj.get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                parts,
                split_extra(obj, &["type", "role", "content", "phase"]),
            );
        }
        _ => {
            push_message_nodes_with_envelope_control(
                out,
                Role::User,
                Some(crate::urp::synthetic_message_id()),
                vec![text_part_with_phase(
                    serde_json::to_string(obj).unwrap_or_default(),
                    None,
                    HashMap::new(),
                )],
                HashMap::new(),
            );
        }
    }
}

fn decode_tool_result_content(output: &Value, content: &mut Vec<ToolResultContent>) {
    match output {
        Value::String(text) => {
            if !text.is_empty() {
                content.push(ToolResultContent::Text { text: text.clone() });
            }
        }
        Value::Array(items) => {
            for item in items {
                decode_tool_result_item(item, content);
            }
        }
        Value::Object(_) => decode_tool_result_item(output, content),
        other => {
            let text = value_to_text(other);
            if !text.is_empty() {
                content.push(ToolResultContent::Text { text });
            }
        }
    }
}

fn decode_tool_result_item(value: &Value, content: &mut Vec<ToolResultContent>) {
    if let Some(text) = value.as_str() {
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text: text.to_string(),
            });
        }
        return;
    }
    let Some(obj) = value.as_object() else {
        let text = value_to_text(value);
        if !text.is_empty() {
            content.push(ToolResultContent::Text { text });
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
            let text = value_to_text(value);
            if !text.is_empty() {
                content.push(ToolResultContent::Text { text });
            }
        }
    }
}

fn decode_response_message_nodes(
    role: Role,
    message_id: Option<String>,
    message_phase: Option<&str>,
    extra_body: HashMap<String, Value>,
    content_arr: Option<&Vec<Value>>,
) -> Vec<Node> {
    let mut parts = Vec::new();
    if let Some(content_arr) = content_arr {
        for p in content_arr {
            let Some(pobj) = p.as_object() else { continue };
            let ptype = pobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match ptype {
                "output_text" | "text" => {
                    if let Some(text) = pobj.get("text").and_then(|v| v.as_str()) {
                        parts.push(text_part_with_phase(
                            text,
                            message_phase,
                            split_extra(pobj, &["type", "text"]),
                        ));
                    }
                }
                "refusal" => {
                    if let Some(text) = pobj.get("refusal").and_then(|v| v.as_str()) {
                        parts.push(Part::Refusal {
                            content: text.to_string(),
                            extra_body: split_extra(pobj, &["type", "refusal"]),
                        });
                    }
                }
                _ => {
                    if let Some(image) = parse_image_part_from_obj(pobj) {
                        parts.push(image);
                    }
                    if let Some(file) = parse_file_part_from_obj(pobj) {
                        parts.push(file);
                    }
                }
            }
        }
    }

    let mut nodes = Vec::new();
    push_message_nodes(&mut nodes, role, message_id, parts, extra_body);
    nodes
}

fn decode_reasoning_node(item_obj: &Map<String, Value>) -> Option<Node> {
    let shared_extra = split_extra(
        item_obj,
        &["type", "encrypted_content", "summary", "text", "source"],
    );
    let encrypted = item_obj.get("encrypted_content").map(|value| match value {
        Value::String(text) => Value::String(text.clone()),
        _ => value.clone(),
    });
    let summary = item_obj
        .get("summary")
        .and_then(|value| value.as_array())
        .and_then(|_| summary_to_text(item_obj));
    let text = item_obj
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let source = item_obj
        .get("source")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (text.is_some() || summary.is_some() || encrypted.is_some()).then(|| Node::Reasoning {
        id: item_obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some(crate::urp::synthetic_reasoning_id())),
        content: text.or_else(|| summary.clone()),
        encrypted,
        summary,
        source,
        extra_body: shared_extra,
    })
}

fn decode_response_nodes(obj: &Map<String, Value>) -> Vec<Node> {
    let mut nodes = Vec::new();

    if let Some(output) = obj.get("output").and_then(|v| v.as_array()) {
        for item in output {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            let item_type = item_obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match item_type {
                "message" => {
                    let message_phase = item_obj.get("phase").and_then(|v| v.as_str());
                    let role = match item_obj
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("assistant")
                    {
                        "system" => Role::System,
                        "developer" => Role::Developer,
                        "user" => Role::User,
                        "tool" => Role::Tool,
                        _ => Role::Assistant,
                    };
                    let extra_body = split_extra(item_obj, &["type", "role", "content", "phase"]);
                    nodes.extend(decode_response_message_nodes(
                        role,
                        item_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        message_phase,
                        extra_body,
                        item_obj.get("content").and_then(|v| v.as_array()),
                    ));
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
                    nodes.push(Node::ToolCall {
                        id: item_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(
                            item_obj,
                            &["type", "id", "call_id", "name", "arguments"],
                        ),
                    });
                }
                "function_call_output" => {
                    let call_id = item_obj
                        .get("call_id")
                        .or_else(|| item_obj.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut content = Vec::new();
                    if let Some(output) = item_obj.get("output") {
                        decode_tool_result_content(output, &mut content);
                    }
                    nodes.push(Node::ToolResult {
                        id: item_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        call_id,
                        is_error: false,
                        content,
                        extra_body: split_extra(item_obj, &["type", "call_id", "id", "output"]),
                    });
                }
                "reasoning" => {
                    if let Some(node) = decode_reasoning_node(item_obj) {
                        nodes.push(node);
                    }
                }
                _ => {
                    nodes.push(Node::ProviderItem {
                        id: item_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                        role: OrdinaryRole::Assistant,
                        item_type: item_type.to_string(),
                        body: Value::Object(item_obj.clone()),
                        extra_body: HashMap::new(),
                    });
                }
            }
        }
    }

    nodes
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "responses response must be object".to_string())?;

    let output_nodes = decode_response_nodes(obj);
    let has_tool_calls = output_nodes
        .iter()
        .any(|node| matches!(node, Node::ToolCall { .. }));

    let finish_reason = match obj.get("status").and_then(|v| v.as_str()) {
        Some("completed") => Some(if has_tool_calls {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        }),
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
        created_at: obj.get("created_at").and_then(|v| v.as_i64()),
        output: output_nodes,
        finish_reason,
        usage,
        extra_body: split_extra(
            obj,
            &[
                "id",
                "object",
                "created",
                "created_at",
                "model",
                "status",
                "output",
                "usage",
                "error",
            ],
        ),
    })
}

fn summary_to_text(item_obj: &Map<String, Value>) -> Option<String> {
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
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn parse_usage_from_responses(obj: &Map<String, Value>) -> Usage {
    serde_json::from_value::<OpenAiResponsesUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| Usage {
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: obj.clone().into_iter().collect(),
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::nodes_to_items;
    use serde_json::json;

    #[test]
    fn parse_usage_reads_cache_write_tokens_from_input_details() {
        let usage = parse_usage_from_responses(
            json!({
                "input_tokens": 120,
                "output_tokens": 35,
                "input_tokens_details": {
                    "cached_tokens": 10,
                    "cache_write_tokens": 64
                }
            })
            .as_object()
            .expect("usage json object expected"),
        );

        let details = usage
            .input_details
            .expect("input_details should be present");
        assert_eq!(details.cache_read_tokens, 10);
        assert_eq!(details.cache_creation_tokens, 64);
    }

    #[test]
    fn parse_usage_keeps_cache_creation_when_cached_tokens_are_zero() {
        let usage = parse_usage_from_responses(
            json!({
                "input_tokens": 98,
                "output_tokens": 2,
                "input_tokens_details": {
                    "cached_tokens": 0,
                    "cache_creation_tokens": 98
                }
            })
            .as_object()
            .expect("usage json object expected"),
        );

        let details = usage
            .input_details
            .expect("input_details should be present");
        assert_eq!(details.cache_read_tokens, 0);
        assert_eq!(details.cache_creation_tokens, 98);
    }

    #[test]
    fn decode_response_nodes_preserves_reasoning_commentary_final_boundary_shape() {
        let source = json!({
            "id": "resp_cc",
            "model": "gpt-5.4",
            "status": "completed",
            "output": [
                { "type": "reasoning", "text": "hmm" },
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "commentary",
                    "content": [{ "type": "output_text", "text": "phase A" }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{ "type": "output_text", "text": "phase B" }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_2",
                    "name": "tool_b",
                    "arguments": "{}"
                }
            ]
        });

        let obj = source.as_object().expect("response object");
        let output_nodes = decode_response_nodes(obj);
        assert_eq!(output_nodes.len(), 4, "expected four flat nodes");
        assert!(
            matches!(&output_nodes[0], Node::Reasoning { content: Some(text), .. } if text == "hmm")
        );
        assert!(
            matches!(&output_nodes[1], Node::Text { role: OrdinaryRole::Assistant, content, phase: Some(phase), .. } if content == "phase A" && phase == "commentary")
        );
        assert!(
            matches!(&output_nodes[2], Node::Text { role: OrdinaryRole::Assistant, content, phase: Some(phase), .. } if content == "phase B" && phase == "final_answer")
        );
        assert!(
            matches!(&output_nodes[3], Node::ToolCall { call_id, name, .. } if call_id == "call_2" && name == "tool_b")
        );
        let outputs = nodes_to_items(&output_nodes);

        assert_eq!(outputs.len(), 2, "decoded outputs should preserve 2 items");
    }

    #[test]
    fn decode_request_inputs_emit_control_and_nodes_without_item_bridge_shape() {
        let value = json!({
            "model": "gpt-5-mini",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "first_only": "A",
                    "content": [{ "type": "input_text", "text": "hello" }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": { "q": 1 }
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "ok"
                }
            ]
        });

        let decoded = decode_request(&value).expect("decode_request should succeed");
        assert!(matches!(
            &decoded.input[0],
            Node::NextDownstreamEnvelopeExtra { extra_body }
                if extra_body.get("first_only") == Some(&json!("A"))
        ));
        assert!(matches!(
            &decoded.input[1],
            Node::Text { role: OrdinaryRole::User, content, .. } if content == "hello"
        ));
        assert!(matches!(
            &decoded.input[2],
            Node::ToolCall { call_id, name, arguments, .. }
                if call_id == "call_1" && name == "lookup" && arguments == "{\"q\":1}"
        ));
        assert!(matches!(
            &decoded.input[3],
            Node::ToolResult { call_id, content, .. }
                if call_id == "call_1"
                    && matches!(&content[0], ToolResultContent::Text { text } if text == "ok")
        ));
    }
}
