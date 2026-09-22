use crate::urp::decode::{
    deserialize_u64ish_default, normalize_reasoning_effort, parse_compatible_media_part,
    parse_tool_definition, remove_untrusted_internal_keys, retain_wire_extra_fields, split_extra,
    value_to_text,
};
use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{
    FinishReason, InputDetails, Node, OrdinaryRole, OutputDetails, ProviderProtocol,
    RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, RESPONSES_INSTRUCTION_NODE_EXTRA_KEY,
    RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, ReasoningConfig, ToolCallType, ToolChoice,
    ToolResultContent, UrpRequest, UrpResponse, Usage,
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
    extra_body.retain(|key, _| !crate::urp::ImageGenerationMetadata::KEYS.contains(&key.as_str()));
    extra_body.insert(
        RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
        Value::Object(Map::new()),
    );
    Some(Node::Image {
        metadata: crate::urp::MediaMetadata {
            image_generation: crate::urp::ImageGenerationMetadata::from_object(item_obj),
            ..Default::default()
        },
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
    input_tokens_details: Option<OpenAiResponsesInputDetails>,
    #[serde(default)]
    output_tokens_details: Option<OpenAiResponsesOutputDetails>,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiResponsesInputDetails>,
    #[serde(default)]
    completion_tokens_details: Option<OpenAiResponsesOutputDetails>,
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
            mut input_tokens_details,
            mut output_tokens_details,
            mut prompt_tokens_details,
            mut completion_tokens_details,
            mut extra,
        } = value;

        retain_wire_extra_fields(&mut extra);
        for details in [&mut input_tokens_details, &mut prompt_tokens_details]
            .into_iter()
            .flatten()
        {
            retain_wire_extra_fields(&mut details.extra);
        }
        for details in [&mut output_tokens_details, &mut completion_tokens_details]
            .into_iter()
            .flatten()
        {
            retain_wire_extra_fields(&mut details.extra);
        }

        let input_details = input_tokens_details
            .as_ref()
            .or(prompt_tokens_details.as_ref())
            .and_then(|details| {
                let cache_creation_tokens = details
                    .cache_creation_tokens
                    .max(details.cache_write_tokens);
                if details.cached_tokens > 0
                    || cache_creation_tokens > 0
                    || details.tool_prompt_tokens > 0
                {
                    Some(InputDetails {
                        tool_prompt_modality_breakdown: None,
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

        let output_details = output_tokens_details
            .as_ref()
            .or(completion_tokens_details.as_ref())
            .and_then(|details| {
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

        for (key, details) in [
            ("input_tokens_details", input_tokens_details),
            ("prompt_tokens_details", prompt_tokens_details),
        ] {
            if let Some(details) = details
                && !details.extra.is_empty()
            {
                extra.insert(
                    key.to_string(),
                    Value::Object(details.extra.into_iter().collect()),
                );
            }
        }
        for (key, details) in [
            ("output_tokens_details", output_tokens_details),
            ("completion_tokens_details", completion_tokens_details),
        ] {
            if let Some(details) = details
                && !details.extra.is_empty()
            {
                extra.insert(
                    key.to_string(),
                    Value::Object(details.extra.into_iter().collect()),
                );
            }
        }

        Usage {
            iterations: None,
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
    let citations = extra_body
        .remove("annotations")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    Part::Text {
        logprobs: crate::urp::logprobs::decode(extra_body.remove("logprobs").as_ref()),
        signature: None,
        citations: crate::urp::citations::decode(
            citations,
            crate::urp::ProviderProtocol::Responses,
        ),
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

fn mark_responses_instruction_node(node: &mut Node) {
    node.extra_body_mut().insert(
        RESPONSES_INSTRUCTION_NODE_EXTRA_KEY.to_string(),
        Value::Bool(true),
    );
}

fn decode_structured_instruction_item(item: &Value, out: &mut Vec<Node>) {
    if let Some(text) = item.as_str() {
        if !text.is_empty() {
            let mut node = Node::text(OrdinaryRole::Developer, text);
            mark_responses_instruction_node(&mut node);
            out.push(node);
        }
        return;
    }

    let Some(source_obj) = item.as_object() else {
        return;
    };
    let item_type = source_obj.get("type").and_then(Value::as_str).unwrap_or("");
    let is_message = matches!(item_type, "" | "message") && source_obj.contains_key("content");
    let is_content_part = matches!(
        item_type,
        "input_text"
            | "output_text"
            | "text"
            | "image"
            | "image_url"
            | "input_image"
            | "output_image"
            | "file"
            | "document"
            | "input_file"
            | "output_file"
            | "audio"
            | "input_audio"
            | "output_audio"
    );
    if !is_message && !is_content_part {
        return;
    }

    let target_role = if is_message {
        match source_obj.get("role").and_then(Value::as_str) {
            Some(role @ ("system" | "developer" | "user" | "assistant")) => role,
            _ => "developer",
        }
    } else {
        "developer"
    };
    let mut message = if is_message {
        source_obj.clone()
    } else {
        let mut message = Map::new();
        message.insert("type".to_string(), Value::String("message".to_string()));
        message.insert(
            "content".to_string(),
            Value::Array(vec![Value::Object(source_obj.clone())]),
        );
        message
    };
    message.insert("role".to_string(), Value::String(target_role.to_string()));

    let mut decoded = Vec::new();
    decode_input_item_nodes(&message, &mut decoded);
    for mut node in decoded {
        if matches!(node, Node::NextDownstreamEnvelopeExtra { .. }) {
            continue;
        }
        mark_responses_instruction_node(&mut node);
        out.push(node);
    }
}

fn decode_instructions_nodes(instructions: &Value, out: &mut Vec<Node>) {
    if let Some(text) = instructions.as_str() {
        if !text.is_empty() {
            let mut node = Node::text(OrdinaryRole::Developer, text);
            mark_responses_instruction_node(&mut node);
            out.push(node);
        }
        return;
    }

    if let Some(items) = instructions.as_array() {
        for item in items {
            decode_structured_instruction_item(item, out);
        }
    }
}

fn mark_responses_tool_origin(tool: &mut crate::urp::ToolDefinition) {
    if let Some(children) = &mut tool.tools {
        for child in children {
            mark_responses_tool_origin(child);
        }
    }
    if tool.function.is_none() && tool.custom.is_none() && tool.tools.is_none() {
        tool.origin_protocol = Some(ProviderProtocol::Responses);
    }
}

pub(crate) const RESPONSES_CONTENT_PART_SHAPE_KEY: &str = "_monoize_responses_content_part";

pub(crate) fn validate_compatible_content(value: &Value) -> Result<(), String> {
    let parts = match value {
        Value::Array(parts) => parts.as_slice(),
        _ => std::slice::from_ref(value),
    };
    for part in parts {
        if let Some(obj) = part.as_object() {
            parse_compatible_media_part(obj)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_responses_items(value: &Value) -> Result<(), String> {
    let items = match value {
        Value::Array(items) => items.as_slice(),
        _ => std::slice::from_ref(value),
    };
    for item in items {
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message");
        let content = match kind {
            "message" => item.get("content"),
            "function_call_output" | "custom_tool_call_output" => item.get("output"),
            "input_text" | "output_text" | "text" | "image" | "image_url" | "input_image"
            | "output_image" | "file" | "document" | "input_file" | "output_file" | "audio"
            | "input_audio" | "output_audio" => Some(item),
            _ => None,
        };
        if let Some(content) = content {
            validate_compatible_content(content)?;
        }
    }
    Ok(())
}

fn compatible_content_parts(content: &Value, phase: Option<&str>) -> Vec<Part> {
    let values = match content {
        Value::Null => return Vec::new(),
        Value::Array(values) => values.as_slice(),
        _ => std::slice::from_ref(content),
    };
    let mut parts = Vec::new();
    for value in values {
        if let Some(text) = value.as_str() {
            parts.push(text_part_with_phase(text, phase, HashMap::new()));
            continue;
        }
        let Some(obj) = value.as_object() else {
            continue;
        };
        let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(kind, "text" | "input_text" | "output_text")
            && let Some(text) = obj
                .get("text")
                .or_else(|| obj.get("content"))
                .and_then(Value::as_str)
        {
            parts.push(text_part_with_phase(
                text,
                phase,
                split_extra(obj, &["type", "text", "content"]),
            ));
        } else if kind == "refusal"
            && let Some(text) = obj.get("refusal").and_then(Value::as_str)
        {
            parts.push(Part::Refusal {
                logprobs: None,
                content: text.into(),
                extra_body: split_extra(obj, &["type", "refusal"]),
            });
        } else if let Ok(Some(part)) = parse_compatible_media_part(obj) {
            parts.push(part);
        } else {
            parts.push(Part::ProviderItem {
                id: obj.get("id").and_then(Value::as_str).map(str::to_owned),
                origin_protocol: ProviderProtocol::Responses,
                item_type: kind.into(),
                body: value.clone(),
                extra_body: HashMap::from([(
                    RESPONSES_CONTENT_PART_SHAPE_KEY.into(),
                    Value::Bool(true),
                )]),
            });
        }
    }
    parts
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    for key in ["input", "instructions"] {
        if let Some(items) = value.get(key) {
            validate_responses_items(items)?;
        }
    }
    let obj = value
        .as_object()
        .ok_or_else(|| "responses request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let mut input_nodes = Vec::new();

    if let Some(instructions) = obj.get("instructions") {
        decode_instructions_nodes(instructions, &mut input_nodes);
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
                summary: reasoning_obj
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                extra_body: split_extra(reasoning_obj, &["effort", "summary"]),
                ..Default::default()
            })
        });

    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(parse_tool_definition)
            .map(|mut tool| {
                mark_responses_tool_origin(&mut tool);
                tool
            })
            .collect::<Vec<_>>()
    });

    let mut extra_body = split_extra(
        obj,
        &[
            "model",
            "input",
            "instructions",
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
            "text",
        ],
    );
    if let Some(text) = obj.get("text").and_then(Value::as_object) {
        let unknown = split_extra(text, &["format", "verbosity"]);
        if !unknown.is_empty() {
            extra_body.insert(
                "text".to_string(),
                Value::Object(unknown.into_iter().collect()),
            );
        }
    }

    crate::urp::tool_signature::restore_request_call_signatures(&mut input_nodes);
    crate::urp::logprobs::strip_request_extras(&mut extra_body);
    Ok(UrpRequest {
        image_generation: None,
        sampling: None,
        logprobs: crate::urp::logprobs::request_config(
            obj,
            crate::urp::ProviderProtocol::Responses,
        ),
        context: Default::default(),
        instructions_format: obj.get("instructions").map(|v| {
            if v.is_null() {
                crate::urp::InstructionsFormat::Null
            } else if v.is_array() {
                crate::urp::InstructionsFormat::Items
            } else {
                crate::urp::InstructionsFormat::Text
            }
        }),
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
        stop: None,
        verbosity: obj
            .get("text")
            .and_then(|value| value.get("verbosity"))
            .and_then(Value::as_str)
            .map(str::to_string),
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
        extra_body,
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
        "function_call" | "custom_tool_call" => {
            let tool_type = if item_type == "custom_tool_call" {
                ToolCallType::Custom
            } else {
                ToolCallType::Function
            };
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
                .get(if tool_type == ToolCallType::Custom {
                    "input"
                } else {
                    "arguments"
                })
                .cloned()
                .unwrap_or(Value::String("{}".to_string()));
            let arguments = if let Some(s) = arguments.as_str() {
                s.to_string()
            } else {
                serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
            };
            out.push(Node::ToolCall {
                namespace: obj
                    .get("namespace")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                signature: None,
                id: obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                tool_type,
                call_id,
                name,
                arguments,
                extra_body: split_extra(
                    obj,
                    &[
                        "type",
                        "call_id",
                        "id",
                        "name",
                        "namespace",
                        "arguments",
                        "input",
                    ],
                ),
            });
        }
        "function_call_output" | "custom_tool_call_output" => {
            let tool_type = if item_type == "custom_tool_call_output" {
                ToolCallType::Custom
            } else {
                ToolCallType::Function
            };
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
                signature: None,
                namespace: obj
                    .get("namespace")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                name: obj.get("name").and_then(Value::as_str).map(str::to_string),
                id: obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                tool_type,
                call_id,
                is_error: false,
                content,
                extra_body: split_extra(
                    obj,
                    &["type", "id", "call_id", "namespace", "name", "output"],
                ),
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
            let parts = obj
                .get("content")
                .map(|content| compatible_content_parts(content, message_phase))
                .unwrap_or_default();

            push_message_nodes_with_envelope_control(
                out,
                role,
                obj.get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                parts,
                split_extra(obj, &["type", "id", "role", "content", "phase"]),
            );
        }
        "input_text" | "output_text" | "text" | "image" | "image_url" | "input_image"
        | "output_image" | "file" | "document" | "input_file" | "output_file" | "audio"
        | "input_audio" | "output_audio" => {
            let role = match obj.get("role").and_then(Value::as_str) {
                Some("system") => OrdinaryRole::System,
                Some("developer") => OrdinaryRole::Developer,
                Some("assistant") => OrdinaryRole::Assistant,
                _ => OrdinaryRole::User,
            };
            out.extend(
                compatible_content_parts(&Value::Object(obj.clone()), None)
                    .into_iter()
                    .map(|part| part.into_node(role)),
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
            if let Ok(Some(part)) = parse_compatible_media_part(obj) {
                content.push(match part {
                    Part::Image {
                        metadata,
                        source,
                        extra_body,
                    } => ToolResultContent::Image {
                        metadata,
                        source,
                        extra_body,
                    },
                    Part::File {
                        metadata,
                        source,
                        extra_body,
                    } => ToolResultContent::File {
                        metadata,
                        source,
                        extra_body,
                    },
                    Part::Audio {
                        metadata,
                        source,
                        extra_body,
                    } => ToolResultContent::File {
                        metadata,
                        source: match source {
                            crate::urp::AudioSource::Base64 { media_type, data } => {
                                crate::urp::FileSource::Base64 { media_type, data }
                            }
                            crate::urp::AudioSource::Url { url } => {
                                crate::urp::FileSource::Url { url }
                            }
                        },
                        extra_body,
                    },
                    _ => unreachable!(),
                });
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
    content: Option<&Value>,
) -> Vec<Node> {
    let parts = content
        .map(|content| compatible_content_parts(content, message_phase))
        .unwrap_or_default();

    let mut nodes = Vec::new();
    push_message_nodes_with_envelope_control(&mut nodes, role, message_id, parts, extra_body);
    nodes
}

fn decode_reasoning_node(
    item_obj: &Map<String, Value>,
    synthesize_missing_id: bool,
) -> Option<Node> {
    let shared_extra = split_extra(
        item_obj,
        &[
            "type",
            "id",
            "content",
            "encrypted_content",
            "summary",
            "text",
            "source",
        ],
    );
    let metadata = crate::urp::ReasoningMetadata {
        summary_parts: item_obj
            .get("summary")
            .and_then(crate::urp::reasoning::text_part_shapes),
        content_parts: item_obj
            .get("content")
            .and_then(crate::urp::reasoning::text_part_shapes),
        ..Default::default()
    };
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
            metadata,
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
                    let extra_body =
                        split_extra(item_obj, &["type", "id", "role", "content", "phase"]);
                    nodes.extend(decode_response_message_nodes(
                        role,
                        item_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        message_phase,
                        extra_body,
                        item_obj.get("content"),
                    ));
                }
                "function_call" | "custom_tool_call" => {
                    let tool_type = if item_type == "custom_tool_call" {
                        ToolCallType::Custom
                    } else {
                        ToolCallType::Function
                    };
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
                        .get(if tool_type == ToolCallType::Custom {
                            "input"
                        } else {
                            "arguments"
                        })
                        .cloned()
                        .unwrap_or(Value::String("{}".to_string()));
                    let arguments = if let Some(s) = arguments.as_str() {
                        s.to_string()
                    } else {
                        serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
                    };
                    nodes.push(Node::ToolCall {
                        namespace: item_obj
                            .get("namespace")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        signature: None,
                        id: item_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        tool_type,
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(
                            item_obj,
                            &[
                                "type",
                                "id",
                                "call_id",
                                "name",
                                "namespace",
                                "arguments",
                                "input",
                            ],
                        ),
                    });
                }
                "function_call_output" | "custom_tool_call_output" => {
                    let tool_type = if item_type == "custom_tool_call_output" {
                        ToolCallType::Custom
                    } else {
                        ToolCallType::Function
                    };
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
                        signature: None,
                        namespace: item_obj
                            .get("namespace")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        name: item_obj
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        id: item_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        tool_type,
                        call_id,
                        is_error: false,
                        content,
                        extra_body: split_extra(
                            item_obj,
                            &["type", "call_id", "id", "namespace", "name", "output"],
                        ),
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
                "input_text" | "output_text" | "text" | "image" | "image_url" | "input_image"
                | "output_image" | "file" | "document" | "input_file" | "output_file" | "audio"
                | "input_audio" | "output_audio" => {
                    nodes.extend(
                        compatible_content_parts(item, None)
                            .into_iter()
                            .map(|part| part.into_node(responses_item_role(item_obj))),
                    );
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
    if let Some(items) = value.get("output") {
        validate_responses_items(items)?;
    }
    let obj = value
        .as_object()
        .ok_or_else(|| "responses response must be object".to_string())?;

    let outcome = crate::urp::ResponseOutcome::from_responses(obj);
    if let Some(error) = obj.get("error").filter(|error| !error.is_null()) {
        if outcome.is_none() {
            return Err(error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("upstream Responses error")
                .to_string());
        }
    }
    if outcome.is_none() && !obj.get("output").is_some_and(Value::is_array) {
        return Err("Responses response requires a valid status or an output array".to_string());
    }

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
        Some("incomplete") => Some(incomplete_finish_reason(obj)),
        Some("failed" | "cancelled") => Some(FinishReason::Other),
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
            "status",
            "error",
            "incomplete_details",
        ],
    );
    extra_body.insert(
        RESPONSES_RESPONSE_SOURCE_EXTRA_KEY.to_string(),
        Value::Object(Map::new()),
    );

    Ok(UrpResponse {
        outcome,
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

pub(crate) fn parse_usage_from_responses(obj: &Map<String, Value>) -> Usage {
    serde_json::from_value::<OpenAiResponsesUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| Usage {
            iterations: None,
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: split_extra(obj, &[]),
        })
}

fn responses_selector_to_urp(value: Value) -> Value {
    let Value::Object(mut obj) = value else {
        return value;
    };
    match obj.get("type").and_then(Value::as_str) {
        Some(kind @ ("function" | "custom")) => {
            let kind = kind.to_string();
            let flat_name = obj.remove("name");
            let mut nested = obj
                .remove(&kind)
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            if let Some(name) = flat_name {
                nested.insert("name".to_string(), name);
            }
            obj.remove(if kind == "function" {
                "custom"
            } else {
                "function"
            });
            obj.insert("type".to_string(), Value::String(kind.clone()));
            obj.insert(kind, Value::Object(nested));
        }
        Some("allowed_tools") => {
            let flat_mode = obj.remove("mode");
            let flat_tools = obj.remove("tools");
            let mut allowed = obj
                .remove("allowed_tools")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            if let Some(mode) = flat_mode {
                allowed.insert("mode".to_string(), mode);
            }
            if let Some(mut tools) = flat_tools {
                if let Some(tools) = tools.as_array_mut() {
                    for selector in tools {
                        *selector = responses_selector_to_urp(selector.take());
                    }
                }
                allowed.insert("tools".to_string(), tools);
            } else if let Some(tools) = allowed.get_mut("tools").and_then(Value::as_array_mut) {
                for selector in tools {
                    *selector = responses_selector_to_urp(selector.take());
                }
            }
            obj.insert("allowed_tools".to_string(), Value::Object(allowed));
        }
        _ => {}
    }
    Value::Object(obj)
}

fn tool_choice_from_value(mut v: Value) -> ToolChoice {
    remove_untrusted_internal_keys(&mut v);
    if let Some(s) = v.as_str() {
        ToolChoice::Mode(s.to_string())
    } else {
        ToolChoice::Specific(responses_selector_to_urp(v))
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

pub(crate) fn incomplete_finish_reason(obj: &Map<String, Value>) -> FinishReason {
    match obj
        .get("incomplete_details")
        .and_then(|value| value.get("reason"))
        .and_then(Value::as_str)
    {
        Some("content_filter") => FinishReason::ContentFilter,
        Some("model_context_window_exceeded" | "context_length_exceeded") => {
            FinishReason::ContextLimit
        }
        Some("pause_turn") => FinishReason::Paused,
        Some("compaction") => FinishReason::Compaction,
        Some("max_output_tokens" | "max_messages") => FinishReason::Length,
        _ => FinishReason::Other,
    }
}
