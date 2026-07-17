use crate::urp::decode::{
    deserialize_u64ish_default, normalize_reasoning_effort, parse_file_part_from_obj,
    parse_image_part_from_obj, parse_tool_definition, split_extra, value_to_text,
};
use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{
    FinishReason, InputDetails, Node, OrdinaryRole, OutputDetails, ProviderProtocol,
    RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, RESPONSES_REASONING_CONTENT_EXTRA_KEY,
    RESPONSES_REASONING_SUMMARY_EXTRA_KEY, RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, ReasoningConfig,
    ToolChoice, ToolResultContent, UrpRequest, UrpResponse, Usage,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

fn image_media_type_from_output_format(output_format: Option<&str>) -> &'static str {
    match output_format.unwrap_or("png") {
        "webp" => "image/webp",
        "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

fn decode_image_generation_call_node(item_obj: &Map<String, Value>) -> Option<Node> {
    let result = item_obj.get("result")?.as_str()?.trim();
    if result.is_empty() {
        return None;
    }
    let mut extra_body = split_extra(item_obj, &["type", "id", "result", "output_format"]);
    extra_body.insert(
        RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
        Value::Object(item_obj.clone()),
    );
    Some(Node::Image {
        id: item_obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        role: OrdinaryRole::Assistant,
        source: crate::urp::ImageSource::Base64 {
            media_type: image_media_type_from_output_format(
                item_obj.get("output_format").and_then(|v| v.as_str()),
            )
            .to_string(),
            data: result.to_string(),
        },
        extra_body,
    })
}

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
                    cache_read_modality_breakdown: None,
                    cache_creation_tokens,
                    cache_creation_5m_tokens: 0,
                    cache_creation_1h_tokens: 0,
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
        decode_input_items_nodes(input, &mut input_nodes);
    }

    let reasoning = obj
        .get("reasoning")
        .and_then(|v| v.as_object())
        .and_then(|reasoning_obj| {
            let effort = reasoning_obj
                .get("effort")
                .and_then(|v| v.as_str())
                .map(normalize_reasoning_effort);
            (!reasoning_obj.is_empty()).then(|| ReasoningConfig {
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
        parallel_tool_calls: obj.get("parallel_tool_calls").and_then(|v| v.as_bool()),
        response_format: obj
            .get("text")
            .and_then(|value| value.get("format"))
            .or_else(|| obj.get("response_format"))
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
                "input",
                "stream",
                "temperature",
                "top_p",
                "max_output_tokens",
                "reasoning",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "response_format",
                "user",
            ],
        ),
    })
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
        "reasoning" => {
            if let Some(node) = decode_reasoning_node(obj, false) {
                out.push(node);
            }
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
            out.push(Node::ProviderItem {
                id: obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                origin_protocol: ProviderProtocol::Responses,
                role: responses_item_role(obj),
                item_type: item_type.to_string(),
                body: Value::Object(obj.clone()),
                extra_body: HashMap::new(),
            });
        }
    }
}

fn responses_item_role(obj: &Map<String, Value>) -> OrdinaryRole {
    match obj.get("role").and_then(|v| v.as_str()) {
        Some("system") => OrdinaryRole::System,
        Some("developer") => OrdinaryRole::Developer,
        Some("user") => OrdinaryRole::User,
        _ => OrdinaryRole::Assistant,
    }
}

fn decode_tool_result_content(output: &Value, content: &mut Vec<ToolResultContent>) {
    match output {
        Value::String(text) => {
            if !text.is_empty() {
                content.push(ToolResultContent::Text {
                    text: text.clone(),
                    extra_body: HashMap::new(),
                });
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
                content.push(ToolResultContent::Text {
                    text,
                    extra_body: HashMap::new(),
                });
            }
        }
    }
}

fn decode_tool_result_item(value: &Value, content: &mut Vec<ToolResultContent>) {
    if let Some(text) = value.as_str() {
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text: text.to_string(),
                extra_body: HashMap::new(),
            });
        }
        return;
    }
    let Some(obj) = value.as_object() else {
        let text = value_to_text(value);
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text,
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
                content.push(ToolResultContent::Text {
                    text: text.to_string(),
                    extra_body: split_extra(obj, &["type", "text", "content"]),
                });
            }
        }
        _ => {
            if let Some(image) = parse_image_part_from_obj(obj) {
                let Part::Image { source, extra_body } = image else {
                    unreachable!();
                };
                content.push(ToolResultContent::Image { source, extra_body });
                return;
            }
            if let Some(file) = parse_file_part_from_obj(obj) {
                let Part::File { source, extra_body } = file else {
                    unreachable!();
                };
                content.push(ToolResultContent::File { source, extra_body });
                return;
            }
            content.push(ToolResultContent::ProviderItem {
                origin_protocol: ProviderProtocol::Responses,
                item_type: ptype.to_string(),
                body: value.clone(),
                extra_body: HashMap::new(),
            });
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
    push_message_nodes_with_envelope_control(&mut nodes, role, message_id, parts, extra_body);
    nodes
}

fn decode_reasoning_node(
    item_obj: &Map<String, Value>,
    synthesize_missing_id: bool,
) -> Option<Node> {
    let mut shared_extra = split_extra(
        item_obj,
        &[
            "type",
            "content",
            "encrypted_content",
            "summary",
            "text",
            "source",
        ],
    );
    if let Some(summary) = item_obj.get("summary") {
        shared_extra.insert(
            RESPONSES_REASONING_SUMMARY_EXTRA_KEY.to_string(),
            summary.clone(),
        );
    }
    if let Some(content) = item_obj.get("content") {
        shared_extra.insert(
            RESPONSES_REASONING_CONTENT_EXTRA_KEY.to_string(),
            content.clone(),
        );
    }
    let encrypted = item_obj.get("encrypted_content").map(|value| match value {
        Value::String(text) => Value::String(text.clone()),
        _ => value.clone(),
    });
    let summary = item_obj
        .get("summary")
        .and_then(|value| value.as_array())
        .and_then(|_| summary_to_text(item_obj));
    let text = reasoning_content_to_text(item_obj).or_else(|| {
        item_obj
            .get("text")
            .and_then(|v| v.as_str())
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    });
    let source = item_obj
        .get("source")
        .and_then(|v| v.as_str())
        .filter(|source| !source.is_empty())
        .map(|s| s.to_string());
    (text.is_some()
        || summary.is_some()
        || encrypted.is_some()
        || (!synthesize_missing_id
            && (item_obj.contains_key("summary") || item_obj.contains_key("content"))))
    .then(|| {
        let id = item_obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Node::Reasoning {
            id: id.or_else(|| synthesize_missing_id.then(crate::urp::synthetic_reasoning_id)),
            content: text,
            encrypted,
            summary,
            source,
            extra_body: shared_extra,
        }
    })
}

fn reasoning_content_to_text(item_obj: &Map<String, Value>) -> Option<String> {
    let text = item_obj
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("reasoning_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty()).then_some(text)
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
                    if let Some(node) = decode_reasoning_node(item_obj, true) {
                        nodes.push(node);
                    }
                }
                "image_generation_call" => {
                    if let Some(node) = decode_image_generation_call_node(item_obj) {
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
                        origin_protocol: ProviderProtocol::Responses,
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

    let mut extra_body = split_extra(
        obj,
        &[
            "id",
            "object",
            "created",
            "created_at",
            "model",
            "output",
            "usage",
        ],
    );
    extra_body.insert(
        RESPONSES_RESPONSE_SOURCE_EXTRA_KEY.to_string(),
        Value::Object(obj.clone()),
    );

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
        extra_body,
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
    if out.is_empty() { None } else { Some(out) }
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
            let schema_obj = obj
                .get("json_schema")
                .and_then(Value::as_object)
                .unwrap_or(obj);
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
                        &["type", "name", "description", "schema", "strict"],
                    ),
                },
            });
        }
        if obj.get("type").and_then(|x| x.as_str()) == Some("json_object") {
            return Some(crate::urp::ResponseFormat::JsonObject);
        }
        if obj.get("type").and_then(|x| x.as_str()) == Some("text") {
            return Some(crate::urp::ResponseFormat::Text);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::internal_legacy_bridge::nodes_to_items;
    use serde_json::json;

    #[test]
    fn reasoning_source_omits_empty_and_preserves_non_empty_values() {
        let empty_source = json!({
            "type": "reasoning",
            "content": [{ "type": "reasoning_text", "text": "thinking" }],
            "source": ""
        });
        let explicit_source = json!({
            "type": "reasoning",
            "content": [{ "type": "reasoning_text", "text": "thinking" }],
            "source": "openrouter"
        });

        let Node::Reasoning { source, .. } =
            decode_reasoning_node(empty_source.as_object().expect("reasoning object"), true)
                .expect("reasoning node")
        else {
            panic!("expected reasoning node");
        };
        assert!(source.is_none());

        let Node::Reasoning { source, .. } =
            decode_reasoning_node(explicit_source.as_object().expect("reasoning object"), true)
                .expect("reasoning node")
        else {
            panic!("expected reasoning node");
        };
        assert_eq!(source.as_deref(), Some("openrouter"));
    }

    #[test]
    fn official_text_format_and_structured_instructions_round_trip() {
        let source = json!({
            "model": "gpt-5.4",
            "instructions": [{ "type": "input_text", "text": "be exact" }],
            "input": "answer",
            "text": {
                "verbosity": "low",
                "format": {
                    "type": "json_schema",
                    "name": "answer",
                    "schema": { "type": "object", "properties": { "ok": { "type": "boolean" } } },
                    "strict": true,
                    "future_format_field": 7
                }
            }
        });

        let decoded = decode_request(&source).expect("decode Responses request");
        assert!(matches!(
            decoded.response_format,
            Some(crate::urp::ResponseFormat::JsonSchema { .. })
        ));
        let encoded = crate::urp::encode::openai_responses::encode_request(&decoded, "gpt-5.4");
        assert_eq!(encoded["text"], source["text"]);
        assert_eq!(encoded["instructions"], source["instructions"]);
    }

    #[test]
    fn typed_response_format_overrides_preserved_text_format() {
        let source = json!({
            "model": "gpt-5.4",
            "input": "answer",
            "text": {
                "verbosity": "low",
                "future_text_field": { "enabled": true },
                "format": {
                    "type": "json_schema",
                    "name": "answer",
                    "schema": { "type": "object" },
                    "strict": true,
                    "future_format_field": 7
                }
            }
        });

        let mut decoded = decode_request(&source).expect("decode Responses request");
        decoded.response_format = Some(crate::urp::ResponseFormat::JsonObject);

        let encoded = crate::urp::encode::openai_responses::encode_request(&decoded, "gpt-5.4");
        assert_eq!(encoded["text"]["format"], json!({ "type": "json_object" }));
        assert_eq!(encoded["text"]["verbosity"], json!("low"));
        assert_eq!(
            encoded["text"]["future_text_field"],
            json!({ "enabled": true })
        );
    }

    #[test]
    fn failed_response_status_and_error_round_trip() {
        let source = json!({
            "id": "resp_failed",
            "object": "response",
            "created_at": 123,
            "model": "gpt-5.4",
            "status": "failed",
            "output": [],
            "error": {
                "type": "server_error",
                "code": "capacity",
                "message": "try later",
                "param": null
            }
        });

        let decoded = decode_response(&source).expect("decode failed response");
        let encoded = crate::urp::encode::openai_responses::encode_response(&decoded, "gpt-5.4");
        assert_eq!(encoded["status"], json!("failed"));
        assert_eq!(encoded["error"], source["error"]);
        assert!(encoded.get("presence_penalty").is_none());
        assert!(encoded.get("frequency_penalty").is_none());
        assert!(encoded.get("truncation").is_none());
        assert!(encoded.get("incomplete_details").is_none());
        assert!(encoded.get("completed_at").is_none());
        assert!(encoded.get(RESPONSES_RESPONSE_SOURCE_EXTRA_KEY).is_none());
    }

    #[test]
    fn native_image_generation_call_round_trips_as_top_level_response_and_request_item() {
        let native_item = json!({
            "type": "image_generation_call",
            "id": "ig_1",
            "status": "completed",
            "result": "QUJD",
            "output_format": "webp",
            "future_field": { "keep": true }
        });
        let source = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 123,
            "model": "gpt-5.4",
            "status": "completed",
            "output": [native_item.clone()]
        });

        let decoded = decode_response(&source).expect("decode image-generation response");
        assert!(matches!(
            &decoded.output[0],
            Node::Image { extra_body, .. }
                if extra_body.get(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY)
                    == Some(&native_item)
        ));

        let encoded_response =
            crate::urp::encode::openai_responses::encode_response(&decoded, "gpt-5.4");
        assert_eq!(encoded_response["output"][0], native_item);

        let request = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: decoded.output,
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };
        let encoded_request =
            crate::urp::encode::openai_responses::encode_request(&request, "gpt-5.4");
        assert_eq!(encoded_request["input"][0], native_item);
    }

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
                {
                    "type": "reasoning",
                    "content": [{ "type": "reasoning_text", "text": "hmm" }]
                },
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
                    && matches!(&content[0], ToolResultContent::Text { text, .. } if text == "ok")
        ));
    }

    #[test]
    fn responses_file_ids_round_trip_as_typed_sources_without_synthetic_urls() {
        let value = json!({
            "model": "gpt-5-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_image",
                        "file_id": "file_img_1",
                        "detail": "high",
                        "image_trace": "keep"
                    },
                    {
                        "type": "input_file",
                        "file_id": "file_doc_1",
                        "file_trace": "keep"
                    }
                ]
            }]
        });

        let decoded = decode_request(&value).expect("decode_request should succeed");
        assert!(matches!(
            &decoded.input[0],
            Node::Image {
                source: crate::urp::ImageSource::FileId { file_id, detail },
                extra_body,
                ..
            } if file_id == "file_img_1"
                && detail.as_deref() == Some("high")
                && extra_body.get("image_trace") == Some(&json!("keep"))
        ));
        assert!(matches!(
            &decoded.input[1],
            Node::File {
                source: crate::urp::FileSource::FileId { file_id },
                extra_body,
                ..
            } if file_id == "file_doc_1"
                && extra_body.get("file_trace") == Some(&json!("keep"))
        ));

        let encoded = crate::urp::encode::openai_responses::encode_request(&decoded, "gpt-5-mini");
        let wire = serde_json::to_string(&encoded).expect("responses request json");
        assert!(!wire.contains("file_id://"));
        let content = encoded["input"][0]["content"]
            .as_array()
            .expect("message content array");
        assert_eq!(content[0]["file_id"], json!("file_img_1"));
        assert_eq!(content[0]["detail"], json!("high"));
        assert_eq!(content[0]["image_trace"], json!("keep"));
        assert_eq!(content[1]["file_id"], json!("file_doc_1"));
        assert_eq!(content[1]["file_trace"], json!("keep"));
    }

    #[test]
    fn responses_tool_result_provider_content_replays_only_to_responses() {
        let value = json!({
            "model": "gpt-5-mini",
            "input": [{
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [
                    { "type": "input_text", "text": "ok", "text_trace": 1 },
                    { "type": "computer_screenshot", "image_url": "https://example.test/shot.png" }
                ]
            }]
        });

        let decoded = decode_request(&value).expect("decode_request should succeed");
        let Node::ToolResult { content, .. } = &decoded.input[0] else {
            panic!("expected tool result");
        };
        assert!(matches!(
            &content[0],
            ToolResultContent::Text { text, extra_body }
                if text == "ok" && extra_body.get("text_trace") == Some(&json!(1))
        ));
        assert!(matches!(
            &content[1],
            ToolResultContent::ProviderItem {
                origin_protocol: ProviderProtocol::Responses,
                item_type,
                body,
                ..
            } if item_type == "computer_screenshot" && body == &value["input"][0]["output"][1]
        ));

        let same = crate::urp::encode::openai_responses::encode_request(&decoded, "gpt-5-mini");
        assert_eq!(same["input"][0]["output"], value["input"][0]["output"]);

        let mut collision = decoded.clone();
        let Node::ToolResult { content, .. } = &mut collision.input[0] else {
            panic!("expected tool result");
        };
        let ToolResultContent::Text { extra_body, .. } = &mut content[0] else {
            panic!("expected text tool result content");
        };
        extra_body.insert("type".to_string(), json!("wrong"));
        extra_body.insert("text".to_string(), json!("wrong"));
        let collision_wire =
            crate::urp::encode::openai_responses::encode_request(&collision, "gpt-5-mini");
        assert_eq!(
            collision_wire["input"][0]["output"][0]["type"],
            json!("input_text")
        );
        assert_eq!(collision_wire["input"][0]["output"][0]["text"], json!("ok"));

        let mut cross = decoded.clone();
        crate::urp::retain_provider_items_for_protocol(
            &mut cross.input,
            ProviderProtocol::Messages,
        );
        let Node::ToolResult { content, .. } = &cross.input[0] else {
            panic!("expected tool result");
        };
        assert_eq!(content.len(), 1);
        assert!(matches!(content[0], ToolResultContent::Text { .. }));
    }

    #[test]
    fn decode_request_reasoning_input_without_id_preserves_missing_id() {
        let value = json!({
            "model": "gpt-5-mini",
            "input": [
                {
                    "type": "reasoning",
                    "summary": [],
                    "encrypted_content": "opaque_without_item_id"
                }
            ]
        });

        let decoded = decode_request(&value).expect("decode_request should succeed");
        assert!(matches!(
            &decoded.input[0],
            Node::Reasoning { id: None, encrypted: Some(value), .. }
                if value.as_str() == Some("opaque_without_item_id")
        ));
    }

    #[test]
    fn decode_request_preserves_reasoning_summary_without_effort() {
        let value = json!({
            "model": "gpt-5-mini",
            "input": "hello",
            "reasoning": { "summary": "auto" }
        });

        let decoded = decode_request(&value).expect("decode_request should succeed");
        let reasoning = decoded.reasoning.expect("reasoning should decode");
        assert!(reasoning.effort.is_none());
        assert_eq!(reasoning.extra_body.get("summary"), Some(&json!("auto")));
    }

    #[test]
    fn responses_request_normalizes_legacy_minimum_effort_before_reencoding() {
        let value = json!({
            "model": "gpt-5-mini",
            "input": "hello",
            "reasoning": { "effort": "minimum", "summary": "auto" }
        });

        let decoded = decode_request(&value).expect("decode_request should succeed");
        assert_eq!(
            decoded
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.effort.as_deref()),
            Some("minimal")
        );
        let encoded = crate::urp::encode::openai_responses::encode_request(&decoded, "gpt-5-mini");
        assert_eq!(encoded["reasoning"]["effort"], json!("minimal"));
        assert_eq!(encoded["reasoning"]["summary"], json!("auto"));
    }

    #[test]
    fn responses_compaction_input_is_same_protocol_provider_item_only() {
        let value = json!({
            "model": "gpt-5-mini",
            "input": [{
                "type": "compaction",
                "id": "cmp_1",
                "encrypted_content": "opaque_compaction",
                "metadata": { "turn": 3 }
            }]
        });

        let decoded = decode_request(&value).expect("decode_request should succeed");
        assert_eq!(decoded.input.len(), 1);
        assert!(matches!(
            &decoded.input[0],
            Node::ProviderItem {
                id: Some(id),
                origin_protocol: ProviderProtocol::Responses,
                item_type,
                body,
                ..
            } if id == "cmp_1"
                && item_type == "compaction"
                && body == &value["input"][0]
        ));

        let responses_encoded =
            crate::urp::encode::openai_responses::encode_request(&decoded, "gpt-5-mini");
        assert_eq!(responses_encoded["input"][0], value["input"][0]);

        let mut chat_attempt = decoded.clone();
        crate::urp::retain_provider_items_for_protocol(
            &mut chat_attempt.input,
            ProviderProtocol::ChatCompletion,
        );
        let chat_encoded =
            crate::urp::encode::openai_chat::encode_request(&chat_attempt, "gpt-5-mini");
        let chat_wire = serde_json::to_string(&chat_encoded).expect("chat json");
        assert!(!chat_wire.contains("compaction"));
        assert!(!chat_wire.contains("opaque_compaction"));
        assert_eq!(chat_encoded["messages"], json!([]));
    }

    #[test]
    fn responses_provider_output_item_encodes_without_message_wrapper() {
        let body = json!({
            "type": "compaction",
            "id": "cmp_out_1",
            "encrypted_content": "terminal_opaque"
        });
        let response = UrpResponse {
            id: "resp_provider".to_string(),
            model: "gpt-5-mini".to_string(),
            created_at: Some(1770000000),
            output: vec![Node::ProviderItem {
                id: Some("cmp_out_1".to_string()),
                origin_protocol: ProviderProtocol::Responses,
                role: OrdinaryRole::Assistant,
                item_type: "compaction".to_string(),
                body: body.clone(),
                extra_body: HashMap::new(),
            }],
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: HashMap::new(),
        };

        let encoded =
            crate::urp::encode::openai_responses::encode_response(&response, "gpt-5-mini");
        assert_eq!(encoded["output"], json!([body]));
    }

    #[test]
    fn decodes_responses_custom_and_builtin_tools_locally() {
        let value = json!({
            "model": "gpt-5-mini",
            "input": "use tools",
            "tools": [
                {
                    "type": "custom",
                    "name": "freeform_lookup",
                    "description": "Freeform lookup",
                    "format": {
                        "type": "grammar",
                        "syntax": "lark",
                        "definition": "start: /[a-z]+/"
                    },
                    "defer_loading": true
                },
                {
                    "type": "file_search",
                    "vector_store_ids": ["vs_1", "vs_2"]
                },
                {
                    "type": "code_interpreter",
                    "container": { "type": "auto", "file_ids": ["file_1"] }
                },
                {
                    "type": "web_search",
                    "search_context_size": "medium",
                    "user_location": { "type": "approximate", "country": "US" }
                },
                {
                    "type": "mcp",
                    "server_label": "docs",
                    "server_url": "https://mcp.example.test",
                    "allowed_tools": ["search"],
                    "defer_loading": true
                },
                {
                    "type": "namespace",
                    "name": "app_tools",
                    "description": "Application tools",
                    "tools": [{ "name": "fetch_docs", "description": "Fetch docs" }]
                },
                {
                    "type": "tool_search",
                    "description": "Discover tools",
                    "execution": "server",
                    "parameters": { "type": "object", "properties": {} }
                }
            ]
        });

        let decoded = decode_request(&value).expect("decode_request should succeed");
        let tools = decoded.tools.expect("tools");
        let custom_tool = tools
            .iter()
            .find(|tool| tool.tool_type == "custom")
            .expect("custom tool");
        let custom = custom_tool.custom.as_ref().expect("custom IR");
        assert_eq!(custom.name, "freeform_lookup");
        assert_eq!(custom.description.as_deref(), Some("Freeform lookup"));
        assert_eq!(
            custom.format.as_ref().expect("format")["type"],
            json!("grammar")
        );
        assert_eq!(custom.extra_body.get("defer_loading"), Some(&json!(true)));
        assert!(custom_tool.function.is_none());
        assert!(custom_tool.extra_body.is_empty());

        let file_search = tools
            .iter()
            .find(|tool| tool.tool_type == "file_search")
            .expect("file_search tool");
        assert!(file_search.function.is_none());
        assert!(file_search.custom.is_none());
        assert_eq!(
            file_search.extra_body.get("vector_store_ids"),
            Some(&json!(["vs_1", "vs_2"]))
        );

        let code_interpreter = tools
            .iter()
            .find(|tool| tool.tool_type == "code_interpreter")
            .expect("code_interpreter tool");
        assert_eq!(
            code_interpreter.extra_body.get("container"),
            Some(&json!({ "type": "auto", "file_ids": ["file_1"] }))
        );

        let web_search = tools
            .iter()
            .find(|tool| tool.tool_type == "web_search")
            .expect("web_search tool");
        assert_eq!(
            web_search.extra_body.get("search_context_size"),
            Some(&json!("medium"))
        );
        assert_eq!(
            web_search.extra_body.get("user_location"),
            Some(&json!({ "type": "approximate", "country": "US" }))
        );

        let mcp = tools
            .iter()
            .find(|tool| tool.tool_type == "mcp")
            .expect("mcp tool");
        assert_eq!(mcp.extra_body.get("server_label"), Some(&json!("docs")));
        assert_eq!(
            mcp.extra_body.get("allowed_tools"),
            Some(&json!(["search"]))
        );
        assert_eq!(mcp.extra_body.get("defer_loading"), Some(&json!(true)));

        let namespace = tools
            .iter()
            .find(|tool| tool.tool_type == "namespace")
            .expect("namespace tool");
        assert_eq!(namespace.name.as_deref(), Some("app_tools"));
        assert_eq!(namespace.description.as_deref(), Some("Application tools"));
        assert_eq!(
            namespace.extra_body.get("tools"),
            Some(&json!([{ "name": "fetch_docs", "description": "Fetch docs" }]))
        );

        let tool_search = tools
            .iter()
            .find(|tool| tool.tool_type == "tool_search")
            .expect("tool_search tool");
        assert_eq!(tool_search.description.as_deref(), Some("Discover tools"));
        assert_eq!(
            tool_search.extra_body.get("execution"),
            Some(&json!("server"))
        );
        assert_eq!(
            tool_search.extra_body.get("parameters"),
            Some(&json!({ "type": "object", "properties": {} }))
        );
    }
}
