use crate::urp::decode::{
    deserialize_u64ish_default, is_internal_extra_key, normalize_reasoning_effort,
    parse_audio_part_from_obj, parse_file_part_from_obj, parse_image_part_from_obj,
    parse_tool_call_part_from_obj, parse_tool_definition, remove_untrusted_internal_keys,
    retain_wire_extra_fields, split_extra, value_to_text,
};
use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{
    CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY, CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY,
    CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY, CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY,
    CHAT_MESSAGE_AUDIO_EXTRA_KEY, CHAT_REASONING_CONFIG_EXTRA_KEY, CHAT_REASONING_DETAIL_EXTRA_KEY,
    CHAT_REASONING_SURFACE_EXTRA_KEY, CHAT_REASONING_SURFACE_REASONING,
    CHAT_REASONING_SURFACE_REASONING_CONTENT, CHAT_THINKING_CONFIG_EXTRA_KEY, FinishReason,
    InputDetails, Node, OrdinaryRole, OutputDetails, ProviderProtocol, ReasoningConfig,
    StopControl, ToolCallType, ToolChoice, ToolDefinition, ToolResultContent, UrpRequest,
    UrpResponse, Usage,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

const CHAT_CHOICE_EXTRA_BODY_KEY: &str = "_monoize_chat_choice_extra";
const CHAT_NATIVE_FINISH_REASON_EXTRA_KEY: &str = "_monoize_chat_native_finish_reason";

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatUsage {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    prompt_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiChatInputDetails>,
    #[serde(default)]
    completion_tokens_details: Option<OpenAiChatOutputDetails>,
    #[serde(default)]
    input_tokens_details: Option<OpenAiChatInputDetails>,
    #[serde(default)]
    output_tokens_details: Option<OpenAiChatOutputDetails>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatInputDetails {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_read_tokens"
    )]
    cached_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_write_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_creation_tokens: u64,
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
struct OpenAiChatOutputDetails {
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

impl From<OpenAiChatUsage> for Usage {
    fn from(value: OpenAiChatUsage) -> Self {
        let OpenAiChatUsage {
            prompt_tokens,
            completion_tokens,
            mut prompt_tokens_details,
            mut completion_tokens_details,
            mut input_tokens_details,
            mut output_tokens_details,
            mut extra,
        } = value;

        retain_wire_extra_fields(&mut extra);
        for details in [&mut prompt_tokens_details, &mut input_tokens_details]
            .into_iter()
            .flatten()
        {
            retain_wire_extra_fields(&mut details.extra);
        }
        for details in [&mut completion_tokens_details, &mut output_tokens_details]
            .into_iter()
            .flatten()
        {
            retain_wire_extra_fields(&mut details.extra);
        }

        let input_details = prompt_tokens_details
            .as_ref()
            .or(input_tokens_details.as_ref())
            .and_then(|details| {
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

        let output_details = completion_tokens_details
            .as_ref()
            .or(output_tokens_details.as_ref())
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
            ("prompt_tokens_details", prompt_tokens_details),
            ("input_tokens_details", input_tokens_details),
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
            ("completion_tokens_details", completion_tokens_details),
            ("output_tokens_details", output_tokens_details),
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
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
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
    parts: Vec<Part>,
    extra_body: HashMap<String, Value>,
) {
    let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
    if !parts.is_empty() && !extra_body.is_empty() {
        out.push(Node::NextDownstreamEnvelopeExtra { extra_body });
    }
    for part in parts {
        out.push(part.into_node(ordinary_role));
    }
}

fn legacy_function_call_id(name: &str) -> String {
    format!("legacy_function:{name}")
}

fn parse_legacy_function_definition(value: &Value) -> Option<ToolDefinition> {
    let mut wrapper = Map::new();
    wrapper.insert("type".to_string(), Value::String("function".to_string()));
    wrapper.insert("function".to_string(), value.clone());
    let mut tool = parse_tool_definition(&Value::Object(wrapper))?;
    tool.extra_body.insert(
        CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY.to_string(),
        Value::Bool(true),
    );
    Some(tool)
}

fn legacy_function_choice_from_value(value: &Value) -> Option<ToolChoice> {
    if let Some(mode) = value.as_str() {
        return Some(ToolChoice::Mode(mode.to_string()));
    }

    let name = value.as_object()?.get("name")?.as_str()?.to_string();
    Some(ToolChoice::Specific(serde_json::json!({
        "type": "function",
        "function": { "name": name }
    })))
}

fn parse_legacy_function_call_part(value: &Value) -> Option<Part> {
    let function_call = value.as_object()?;
    let name = function_call.get("name")?.as_str()?.to_string();
    let arguments = function_call
        .get("arguments")
        .map(|arguments| {
            arguments
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| arguments.to_string())
        })
        .unwrap_or_default();
    let mut extra_body = split_extra(function_call, &["name", "arguments"]);
    extra_body.insert(
        CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY.to_string(),
        Value::Bool(true),
    );
    Some(Part::ToolCall {
        id: None,
        tool_type: ToolCallType::Function,
        call_id: legacy_function_call_id(&name),
        name,
        arguments,
        extra_body,
    })
}

fn parse_chat_message_audio_part(value: &Value) -> Option<Part> {
    let body = value.as_object()?.clone();
    if body.is_empty() {
        return None;
    }
    Some(Part::ProviderItem {
        id: body.get("id").and_then(Value::as_str).map(str::to_string),
        origin_protocol: ProviderProtocol::ChatCompletion,
        item_type: "audio".to_string(),
        body: Value::Object(body),
        extra_body: HashMap::from([(CHAT_MESSAGE_AUDIO_EXTRA_KEY.to_string(), Value::Bool(true))]),
    })
}

fn push_chat_content_parts(parts: &mut Vec<Part>, content: &Value, message_phase: Option<&str>) {
    if let Some(s) = content.as_str() {
        if !s.is_empty() {
            parts.push(text_part_with_phase(s, message_phase, HashMap::new()));
        }
        return;
    }

    let Some(arr) = content.as_array() else {
        return;
    };

    for item in arr {
        if let Some(s) = item.as_str() {
            if !s.is_empty() {
                parts.push(text_part_with_phase(s, message_phase, HashMap::new()));
            }
            continue;
        }
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        let mut recognized = false;
        if let Some(text) = item_obj.get("text").and_then(|v| v.as_str()) {
            let item_type = item_obj.get("type").and_then(|v| v.as_str());
            if !text.is_empty() && matches!(item_type, Some("text" | "output_text")) {
                parts.push(text_part_with_phase(
                    text,
                    message_phase,
                    split_extra(item_obj, &["type", "text"]),
                ));
                recognized = true;
            }
        }
        if let Some(image_part) = parse_image_part_from_obj(item_obj) {
            parts.push(image_part);
            recognized = true;
        }
        if let Some(file_part) = parse_file_part_from_obj(item_obj) {
            parts.push(file_part);
            recognized = true;
        }
        if let Some(audio_part) = parse_audio_part_from_obj(item_obj) {
            parts.push(audio_part);
            recognized = true;
        }
        if let Some(tool_call_part) = parse_tool_call_part_from_obj(item_obj) {
            parts.push(tool_call_part);
            recognized = true;
        }
        if !recognized {
            parts.push(Part::ProviderItem {
                id: item_obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                origin_protocol: ProviderProtocol::ChatCompletion,
                item_type: item_obj
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                body: Value::Object(item_obj.clone()),
                extra_body: HashMap::new(),
            });
        }
    }
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat request must be object".to_string())?;

    if let Some(n) = obj.get("n")
        && n.as_u64() != Some(1)
    {
        return Err("Chat Completions n must be the integer 1".to_string());
    }

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let mut input_nodes = Vec::new();
    let mut tool_call_types = HashMap::new();
    for raw_msg in obj
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing messages".to_string())?
    {
        let msg_obj = match raw_msg.as_object() {
            Some(v) => v,
            None => continue,
        };
        let role_name = msg_obj
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("user");
        if role_name == "function" {
            let name = msg_obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let content = msg_obj.get("content").cloned().unwrap_or(Value::Null);
            let text = value_to_text(&content);
            let mut result_extra = split_extra(msg_obj, &["role", "name", "content"]);
            result_extra.insert(
                CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY.to_string(),
                Value::String(name.clone()),
            );
            input_nodes.push(Node::ToolResult {
                id: msg_obj
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tool_type: ToolCallType::Function,
                call_id: legacy_function_call_id(&name),
                is_error: false,
                content: (!text.is_empty())
                    .then(|| ToolResultContent::Text {
                        text,
                        extra_body: HashMap::new(),
                    })
                    .into_iter()
                    .collect(),
                extra_body: result_extra,
            });
            continue;
        }
        let role = match role_name {
            "system" => Role::System,
            "developer" => Role::Developer,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            _ => Role::User,
        };

        if role == Role::Tool {
            let call_id = msg_obj
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let content = msg_obj.get("content").cloned().unwrap_or(Value::Null);
            let text = value_to_text(&content);
            let mut tool_result_content = Vec::new();
            if !text.is_empty() {
                tool_result_content.push(ToolResultContent::Text {
                    text,
                    extra_body: HashMap::new(),
                });
            }
            input_nodes.push(Node::ToolResult {
                id: msg_obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                tool_type: tool_call_types
                    .get(&call_id)
                    .copied()
                    .unwrap_or(ToolCallType::Function),
                call_id,
                is_error: false,
                content: tool_result_content,
                extra_body: split_extra(msg_obj, &["role", "tool_call_id", "content"]),
            });
            continue;
        }

        let message_phase = msg_obj.get("phase").and_then(|v| v.as_str());
        let mut parts = Vec::new();
        let extra_body = split_extra(
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
                "function_call",
                "audio",
                "phase",
            ],
        );

        parse_chat_reasoning_fields(msg_obj, &mut parts);

        if let Some(audio) = msg_obj
            .get("audio")
            .filter(|audio| !audio.is_null())
            .and_then(parse_chat_message_audio_part)
        {
            parts.push(audio);
        }

        if let Some(content) = msg_obj.get("content") {
            push_chat_content_parts(&mut parts, content, message_phase);
        }

        if let Some(refusal) = msg_obj.get("refusal").and_then(|v| v.as_str()) {
            if !refusal.is_empty() {
                parts.push(Part::Refusal {
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
                if let Some(part) = parse_tool_call_part_from_obj(tc_obj) {
                    if let Part::ToolCall {
                        call_id, tool_type, ..
                    } = &part
                    {
                        tool_call_types.insert(call_id.clone(), *tool_type);
                    }
                    parts.push(part);
                }
            }
        }

        if let Some(function_call) = msg_obj
            .get("function_call")
            .filter(|function_call| !function_call.is_null())
            .and_then(parse_legacy_function_call_part)
        {
            if let Part::ToolCall {
                call_id, tool_type, ..
            } = &function_call
            {
                tool_call_types.insert(call_id.clone(), *tool_type);
            }
            parts.push(function_call);
        }

        push_message_nodes(&mut input_nodes, role, parts, extra_body);
    }

    let reasoning = extract_reasoning(obj);
    let modern_tools = obj.get("tools").and_then(Value::as_array);
    let legacy_functions = obj.get("functions").and_then(Value::as_array);
    let tools = if modern_tools.is_some() || legacy_functions.is_some() {
        Some(
            modern_tools
                .into_iter()
                .flatten()
                .filter_map(parse_tool_definition)
                .chain(
                    legacy_functions
                        .into_iter()
                        .flatten()
                        .filter_map(parse_legacy_function_definition),
                )
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    let modern_tool_choice = obj
        .get("tool_choice")
        .filter(|choice| !choice.is_null())
        .cloned();
    let legacy_function_choice = if modern_tool_choice.is_none() {
        obj.get("function_call")
            .filter(|choice| !choice.is_null())
            .and_then(legacy_function_choice_from_value)
            .map(|choice| {
                let mut raw = obj["function_call"].clone();
                remove_untrusted_internal_keys(&mut raw);
                (choice, raw)
            })
    } else {
        None
    };
    let (tool_choice, legacy_function_choice_raw) = match modern_tool_choice {
        Some(choice) => (Some(tool_choice_from_value(choice)), None),
        None => {
            legacy_function_choice.map_or((None, None), |(choice, raw)| (Some(choice), Some(raw)))
        }
    };

    let mut extra_body = split_extra(
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
            "thinking",
            "tools",
            "functions",
            "tool_choice",
            "function_call",
            "parallel_tool_calls",
            "stop",
            "verbosity",
            "response_format",
            "user",
        ],
    );
    if let Some(raw_choice) = legacy_function_choice_raw {
        extra_body.insert(
            CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY.to_string(),
            raw_choice,
        );
    }

    Ok(UrpRequest {
        model,
        input: input_nodes,
        stream: obj.get("stream").and_then(|v| v.as_bool()),
        temperature: obj.get("temperature").and_then(|v| v.as_f64()),
        top_p: obj.get("top_p").and_then(|v| v.as_f64()),
        max_output_tokens: obj
            .get("max_completion_tokens")
            .or_else(|| obj.get("max_tokens"))
            .and_then(|v| v.as_u64()),
        reasoning,
        tools,
        tool_choice,
        parallel_tool_calls: obj.get("parallel_tool_calls").and_then(|v| v.as_bool()),
        stop: obj.get("stop").and_then(|value| match value {
            Value::String(stop) => Some(StopControl::Single(stop.clone())),
            Value::Array(stops) => stops
                .iter()
                .map(Value::as_str)
                .map(|stop| stop.map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .map(StopControl::Multiple),
            _ => None,
        }),
        verbosity: obj
            .get("verbosity")
            .and_then(Value::as_str)
            .map(str::to_string),
        response_format: obj
            .get("response_format")
            .cloned()
            .and_then(parse_response_format),
        user: obj
            .get("user")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        extra_body,
    })
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat response must be object".to_string())?;

    if let Some(error) = obj.get("error").filter(|error| !error.is_null()) {
        return Err(format_chat_completion_error(error));
    }

    let choice = obj
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing choices[0]".to_string())?;

    if let Some(error) = choice.get("error").filter(|error| !error.is_null()) {
        return Err(format_chat_completion_error(error));
    }

    let native_finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
        .map(str::to_string);
    if native_finish_reason.as_deref() == Some("error") {
        return Err("upstream chat completion terminated with finish_reason=error".to_string());
    }

    let msg_obj = choice
        .get("message")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing choices[0].message".to_string())?;

    let mut parts = Vec::new();
    let message_extra_body = split_extra(
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
            "function_call",
            "audio",
            "phase",
        ],
    );
    let message_phase = msg_obj.get("phase").and_then(|v| v.as_str());

    parse_chat_reasoning_fields(msg_obj, &mut parts);

    if let Some(audio) = msg_obj
        .get("audio")
        .filter(|audio| !audio.is_null())
        .and_then(parse_chat_message_audio_part)
    {
        parts.push(audio);
    }

    if let Some(content) = msg_obj.get("content") {
        push_chat_content_parts(&mut parts, content, message_phase);
    }

    if let Some(tool_calls) = msg_obj.get("tool_calls").and_then(|v| v.as_array()) {
        for tool_call in tool_calls {
            let tc_obj = match tool_call.as_object() {
                Some(v) => v,
                None => continue,
            };
            if let Some(part) = parse_tool_call_part_from_obj(tc_obj) {
                parts.push(part);
            }
        }
    }

    if let Some(function_call) = msg_obj
        .get("function_call")
        .filter(|function_call| !function_call.is_null())
        .and_then(parse_legacy_function_call_part)
    {
        parts.push(function_call);
    }

    if let Some(refusal) = msg_obj.get("refusal").and_then(|v| v.as_str()) {
        if !refusal.is_empty() {
            parts.push(Part::Refusal {
                content: refusal.to_string(),
                extra_body: HashMap::new(),
            });
        }
    }

    let mut output_nodes = Vec::new();
    push_message_nodes(
        &mut output_nodes,
        Role::Assistant,
        parts,
        message_extra_body.clone(),
    );

    let finish_reason = native_finish_reason.as_deref().map(parse_finish_reason);

    let usage = obj
        .get("usage")
        .and_then(|v| v.as_object())
        .map(parse_usage_from_chat);

    let mut extra_body = split_extra(
        obj,
        &["id", "object", "created", "model", "choices", "usage"],
    );
    let choice_extra = choice
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "index" | "message" | "finish_reason"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if !choice_extra.is_empty() {
        extra_body.insert(
            CHAT_CHOICE_EXTRA_BODY_KEY.to_string(),
            Value::Object(choice_extra),
        );
    }
    if let Some(native_finish_reason) = native_finish_reason {
        extra_body.insert(
            CHAT_NATIVE_FINISH_REASON_EXTRA_KEY.to_string(),
            Value::String(native_finish_reason),
        );
    }

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
        created_at: obj.get("created").and_then(|v| v.as_i64()),
        output: output_nodes,
        finish_reason,
        usage,
        extra_body,
    })
}

fn format_chat_completion_error(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.is_empty())
        .unwrap_or("upstream chat completion error");
    let code = error.get("code").and_then(value_as_non_empty_string);
    match code {
        Some(code) => format!("{message} (code: {code})"),
        None => message.to_string(),
    }
}

fn value_as_non_empty_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn extract_reasoning(obj: &Map<String, Value>) -> Option<ReasoningConfig> {
    let reasoning_obj = obj.get("reasoning").and_then(Value::as_object);
    let thinking_obj = obj.get("thinking").and_then(Value::as_object);
    let effort = obj
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .or_else(|| {
            reasoning_obj
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(Value::as_str)
        })
        .map(normalize_reasoning_effort);

    if effort.is_none() && reasoning_obj.is_none() && thinking_obj.is_none() {
        return None;
    }

    let mut extra_body = HashMap::new();
    if let Some(reasoning) = reasoning_obj {
        let mut reasoning = reasoning.clone();
        reasoning.retain(|key, _| !is_internal_extra_key(key));
        extra_body.insert(
            CHAT_REASONING_CONFIG_EXTRA_KEY.to_string(),
            Value::Object(reasoning),
        );
    }
    if let Some(thinking) = thinking_obj {
        let mut thinking = thinking.clone();
        thinking.retain(|key, _| !is_internal_extra_key(key));
        extra_body.insert(
            CHAT_THINKING_CONFIG_EXTRA_KEY.to_string(),
            Value::Object(thinking),
        );
    }
    Some(ReasoningConfig { effort, extra_body })
}

fn parse_chat_reasoning_fields(msg_obj: &Map<String, Value>, parts: &mut Vec<Part>) {
    if let Some(details) = msg_obj.get("reasoning_details").and_then(|v| v.as_array()) {
        for detail in details {
            let Some(detail_obj) = detail.as_object() else {
                continue;
            };
            let detail_type = detail_obj.get("type").and_then(Value::as_str).unwrap_or("");
            if !detail_type.starts_with("reasoning.") {
                continue;
            }
            let source = detail_obj
                .get("format")
                .and_then(Value::as_str)
                .filter(|format| !format.is_empty())
                .map(str::to_string);
            let id = detail_obj
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string);
            let content = (detail_type == "reasoning.text")
                .then(|| detail_obj.get("text").and_then(Value::as_str))
                .flatten()
                .map(str::to_string);
            let summary = (detail_type == "reasoning.summary")
                .then(|| detail_obj.get("summary").and_then(Value::as_str))
                .flatten()
                .map(str::to_string);
            let encrypted = (detail_type == "reasoning.encrypted")
                .then(|| detail_obj.get("data"))
                .flatten()
                .filter(|value| !value.is_null())
                .cloned();
            let mut raw_detail = detail_obj.clone();
            raw_detail.retain(|key, _| !is_internal_extra_key(key));
            let mut extra_body = HashMap::new();
            extra_body.insert(
                CHAT_REASONING_DETAIL_EXTRA_KEY.to_string(),
                Value::Object(raw_detail),
            );
            parts.push(Part::Reasoning {
                id,
                content,
                encrypted,
                summary,
                source,
                extra_body,
            });
        }
        if !details.is_empty() {
            return;
        }
    }

    let scalar = msg_obj
        .get("reasoning")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|value| (value, CHAT_REASONING_SURFACE_REASONING))
        .or_else(|| {
            msg_obj
                .get("reasoning_content")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| (value, CHAT_REASONING_SURFACE_REASONING_CONTENT))
        });
    if let Some((content, surface)) = scalar {
        parts.push(Part::Reasoning {
            id: None,
            content: Some(content.to_string()),
            encrypted: None,
            summary: None,
            source: None,
            extra_body: HashMap::from([(
                CHAT_REASONING_SURFACE_EXTRA_KEY.to_string(),
                Value::String(surface.to_string()),
            )]),
        });
    }
    if let Some(opaque) = msg_obj
        .get("reasoning_opaque")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        parts.push(Part::Reasoning {
            id: None,
            content: None,
            encrypted: Some(Value::String(opaque.to_string())),
            summary: None,
            source: None,
            extra_body: HashMap::new(),
        });
    }
}

fn tool_choice_from_value(mut v: Value) -> ToolChoice {
    remove_untrusted_internal_keys(&mut v);
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
                let extra = split_extra(schema_obj, &["name", "schema", "description", "strict"]);
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
    serde_json::from_value::<OpenAiChatUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| Usage {
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: split_extra(obj, &[]),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::Node;
    use crate::urp::internal_legacy_bridge::{Item, nodes_to_items};
    use serde_json::json;

    fn output_parts(nodes: &[Node]) -> Vec<Part> {
        nodes_to_items(nodes)
            .into_iter()
            .find_map(|item| match item {
                Item::Message { parts, .. } => Some(parts),
                Item::ToolResult { .. } => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn chat_usage_preserves_nested_unknown_details() {
        let response = json!({
            "id": "chatcmpl_usage_details",
            "object": "chat.completion",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "ok" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 8,
                "prompt_tokens_details": {
                    "cached_tokens": 3,
                    "vendor_prompt_detail": { "kind": "warm" },
                    "_monoize_spoofed_prompt": true
                },
                "completion_tokens_details": {
                    "reasoning_tokens": 5,
                    "vendor_completion_detail": [1, 2],
                    "_monoize_spoofed_completion": true
                },
                "vendor_usage_counter": 9,
                "_monoize_spoofed_usage": true
            }
        });

        let usage = decode_response(&response)
            .expect("decode chat response")
            .usage
            .expect("chat usage");
        assert_eq!(
            usage
                .input_details
                .expect("input details")
                .cache_read_tokens,
            3
        );
        assert_eq!(
            usage
                .output_details
                .expect("output details")
                .reasoning_tokens,
            5
        );
        assert_eq!(
            usage.extra_body["prompt_tokens_details"],
            json!({ "vendor_prompt_detail": { "kind": "warm" } })
        );
        assert_eq!(
            usage.extra_body["completion_tokens_details"],
            json!({ "vendor_completion_detail": [1, 2] })
        );
        assert_eq!(usage.extra_body["vendor_usage_counter"], json!(9));
        assert!(!usage.extra_body.contains_key("vendor_prompt_detail"));
        assert!(!usage.extra_body.contains_key("vendor_completion_detail"));
        assert!(!usage.extra_body.contains_key("_monoize_spoofed_usage"));
    }

    #[test]
    fn chat_usage_fallback_rejects_reserved_wire_keys() {
        let usage = parse_usage_from_chat(
            json!({
                "prompt_tokens": 1,
                "completion_tokens": 2,
                "prompt_tokens_details": "invalid",
                "vendor_usage_counter": 3,
                "_monoize_spoofed_usage": true
            })
            .as_object()
            .expect("usage object"),
        );
        assert_eq!(usage.extra_body["vendor_usage_counter"], json!(3));
        assert!(!usage.extra_body.contains_key("_monoize_spoofed_usage"));
    }

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
                    "reasoning_details": [
                        {
                            "type": "reasoning.text",
                            "text": "new_reasoning",
                            "format": "openrouter"
                        },
                        {
                            "type": "reasoning.encrypted",
                            "data": "new_sig",
                            "format": "openrouter"
                        }
                    ],
                    "reasoning_content": "legacy_reasoning",
                    "reasoning_opaque": "legacy_sig"
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let parts = output_parts(&decoded.output);
        let reasoning = parts
            .iter()
            .filter_map(|part| match part {
                Part::Reasoning { .. } => Some(part),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(reasoning.len(), 2);
        assert!(matches!(
            reasoning[0],
            Part::Reasoning {
                content: Some(content),
                encrypted: None,
                extra_body,
                ..
            } if content == "new_reasoning"
                && extra_body.get(CHAT_REASONING_DETAIL_EXTRA_KEY)
                    == Some(&value["choices"][0]["message"]["reasoning_details"][0])
        ));
        assert!(matches!(
            reasoning[1],
            Part::Reasoning {
                content: None,
                encrypted: Some(Value::String(sig)),
                extra_body,
                ..
            } if sig == "new_sig"
                && extra_body.get(CHAT_REASONING_DETAIL_EXTRA_KEY)
                    == Some(&value["choices"][0]["message"]["reasoning_details"][1])
        ));
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
        let parts = output_parts(&decoded.output);
        assert!(parts.iter().any(|part| matches!(
            part,
            Part::Reasoning {
                content: Some(content),
                encrypted: None,
                ..
            } if content == "legacy_reasoning"
        )));
        assert!(parts.iter().any(|part| matches!(
            part,
            Part::Reasoning {
                content: None,
                encrypted: Some(Value::String(sig)),
                ..
            } if sig == "legacy_sig"
        )));
    }

    #[test]
    fn decode_response_accepts_real_upstream_gpt5_reasoning_payload_shape() {
        let value = json!({
            "id": "resp_real_shape",
            "object": "chat.completion",
            "created": 1773667800i64,
            "model": "gpt-5.4-2026-03-05",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "One valid combination is 8 packs of pencils and 4 packs of pens.",
                    "reasoning": "plain reasoning",
                    "reasoning_content": "plain reasoning",
                    "reasoning_details": [
                        {
                            "type": "reasoning.text",
                            "text": "plain reasoning"
                        },
                        {
                            "type": "reasoning.encrypted",
                            "data": "opaque_sig_payload"
                        }
                    ],
                    "reasoning_opaque": "opaque_sig_payload"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 52,
                "completion_tokens": 287,
                "total_tokens": 339,
                "prompt_tokens_details": { "cached_tokens": 0 },
                "completion_tokens_details": { "reasoning_tokens": 210 }
            }
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let parts = output_parts(&decoded.output);
        assert!(parts.iter().any(|part| matches!(
            part,
            Part::Reasoning {
                content: Some(content),
                encrypted: None,
                ..
            } if content == "plain reasoning"
        )));
        assert!(parts.iter().any(|part| matches!(
            part,
            Part::Reasoning {
                content: None,
                encrypted: Some(Value::String(sig)),
                ..
            } if sig == "opaque_sig_payload"
        )));
    }

    #[test]
    fn decode_response_accepts_content_array_tool_call_blocks() {
        let value = json!({
            "id": "chatcmpl_test",
            "model": "m",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "before tool" },
                        { "type": "tool_call", "id": "call_1", "name": "lookup", "arguments": { "q": 1 } }
                    ]
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let parts = output_parts(&decoded.output);

        assert!(parts.iter().any(|part| {
            matches!(part, Part::Text { content, .. } if content == "before tool")
        }));
        assert!(parts.iter().any(|part| {
            matches!(
                part,
                Part::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } if call_id == "call_1" && name == "lookup" && arguments == "{\"q\":1}"
            )
        }));
    }

    #[test]
    fn decode_response_accepts_content_array_tool_use_blocks() {
        let value = json!({
            "id": "chatcmpl_test",
            "model": "m",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "before tool" },
                        { "type": "tool_use", "id": "call_1", "name": "lookup", "input": { "q": 1 } }
                    ]
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let parts = output_parts(&decoded.output);

        assert!(parts.iter().any(|part| {
            matches!(part, Part::Text { content, .. } if content == "before tool")
        }));
        assert!(parts.iter().any(|part| {
            matches!(
                part,
                Part::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } if call_id == "call_1" && name == "lookup" && arguments == "{\"q\":1}"
            )
        }));
    }

    #[test]
    fn decode_response_rejects_top_level_openrouter_error() {
        let error = decode_response(&json!({
            "error": {
                "message": "provider exhausted",
                "code": 503,
                "type": "upstream_error"
            }
        }))
        .expect_err("top-level error must not decode as a successful response");

        assert!(error.contains("provider exhausted"), "{error}");
        assert!(error.contains("503"), "{error}");
    }

    #[test]
    fn decode_response_rejects_choice_error() {
        let error = decode_response(&json!({
            "id": "chatcmpl_error",
            "model": "openrouter/model",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "error",
                "native_finish_reason": "error",
                "error": { "message": "mid-generation failure", "code": 502 }
            }]
        }))
        .expect_err("choice error must not decode as a successful response");

        assert!(error.contains("mid-generation failure"), "{error}");
        assert!(error.contains("502"), "{error}");
    }

    #[test]
    fn decode_response_preserves_unknown_native_finish_reason_and_choice_fields() {
        let decoded = decode_response(&json!({
            "id": "chatcmpl_deepseek",
            "model": "deepseek-v4",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "partial" },
                "finish_reason": "insufficient_system_resource",
                "native_finish_reason": "insufficient_system_resource",
                "provider_marker": "deepseek"
            }]
        }))
        .expect("resource finish reason is a terminal response, not a parse error");

        assert_eq!(decoded.finish_reason, Some(FinishReason::Other));
        assert_eq!(
            decoded
                .extra_body
                .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
                .and_then(Value::as_str),
            Some("insufficient_system_resource")
        );
        assert_eq!(
            decoded
                .extra_body
                .get(CHAT_CHOICE_EXTRA_BODY_KEY)
                .and_then(Value::as_object)
                .and_then(|extra| extra.get("provider_marker")),
            Some(&json!("deepseek"))
        );
    }

    #[test]
    fn chat_file_and_audio_parts_decode_to_typed_media() {
        let decoded = decode_request(&json!({
            "model": "gpt-4o-audio-preview",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "file", "file": { "file_id": "file_openai_1" } },
                    {
                        "type": "file",
                        "file": { "file_data": "ZmlsZQ==", "filename": "note.txt" }
                    },
                    {
                        "type": "input_audio",
                        "input_audio": { "data": "YXVkaW8=", "format": "mp3" }
                    }
                ]
            }]
        }))
        .expect("chat request decodes");

        assert!(matches!(
            &decoded.input[0],
            Node::File {
                source: crate::urp::FileSource::FileId { file_id },
                extra_body,
                ..
            } if file_id == "file_openai_1"
                && extra_body.get(crate::urp::FILE_ID_ORIGIN_EXTRA_KEY)
                    == Some(&json!(crate::urp::FILE_ID_ORIGIN_OPENAI))
        ));
        assert!(matches!(
            &decoded.input[1],
            Node::File {
                source: crate::urp::FileSource::Base64 {
                    filename: Some(filename),
                    media_type,
                    data,
                },
                ..
            } if filename == "note.txt"
                && media_type == "application/octet-stream"
                && data == "ZmlsZQ=="
        ));
        assert!(matches!(
            &decoded.input[2],
            Node::Audio {
                source: crate::urp::AudioSource::Base64 { media_type, data },
                ..
            } if media_type == "audio/mpeg" && data == "YXVkaW8="
        ));
    }

    #[test]
    fn chat_request_rejects_multiple_choices() {
        let error = decode_request(&json!({
            "model": "gpt-5",
            "messages": [{ "role": "user", "content": "hello" }],
            "n": 2
        }))
        .expect_err("URP cannot represent multiple candidates");

        assert_eq!(error, "Chat Completions n must be the integer 1");
    }

    #[test]
    fn chat_request_decodes_typed_stop_shape_and_verbosity() {
        let scalar = decode_request(&json!({
            "model": "gpt-5.4",
            "messages": [{ "role": "user", "content": "hello" }],
            "stop": "END",
            "verbosity": "low"
        }))
        .expect("decode scalar stop");
        assert_eq!(scalar.stop, Some(StopControl::Single("END".to_string())));
        assert_eq!(scalar.verbosity.as_deref(), Some("low"));

        let multiple = decode_request(&json!({
            "model": "gpt-5.4",
            "messages": [{ "role": "user", "content": "hello" }],
            "stop": ["ONE", "TWO"]
        }))
        .expect("decode array stop");
        assert_eq!(
            multiple.stop,
            Some(StopControl::Multiple(vec![
                "ONE".to_string(),
                "TWO".to_string()
            ]))
        );
    }

    #[test]
    fn unknown_typed_text_block_remains_provider_item_and_round_trips() {
        let native_block = json!({
            "type": "vendor_text",
            "text": "must stay opaque",
            "vendor": { "revision": 2 }
        });
        let decoded = decode_request(&json!({
            "model": "openrouter/model",
            "messages": [{
                "role": "user",
                "content": [native_block.clone()]
            }]
        }))
        .expect("decode unknown Chat block");

        assert!(matches!(
            &decoded.input[0],
            Node::ProviderItem {
                origin_protocol: ProviderProtocol::ChatCompletion,
                item_type,
                body,
                ..
            } if item_type == "vendor_text" && body == &native_block
        ));
        let encoded = crate::urp::encode::openai_chat::encode_request(&decoded, "openrouter/model");
        assert_eq!(encoded["messages"][0]["content"][0], native_block);
    }

    #[test]
    fn chat_audio_only_response_replays_message_audio_and_null_content() {
        let audio = json!({
            "id": "audio_1",
            "data": "YXVkaW8=",
            "expires_at": 1_900_000_000,
            "transcript": "hello",
            "vendor": { "codec": "pcm16" }
        });
        let decoded = decode_response(&json!({
            "id": "chatcmpl_audio",
            "model": "gpt-4o-audio-preview",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "audio": audio.clone()
                },
                "finish_reason": "stop"
            }]
        }))
        .expect("decode audio-only response");
        assert!(matches!(
            &decoded.output[0],
            Node::ProviderItem {
                origin_protocol: ProviderProtocol::ChatCompletion,
                item_type,
                body,
                extra_body,
                ..
            } if item_type == "audio"
                && body == &audio
                && extra_body.get(crate::urp::CHAT_MESSAGE_AUDIO_EXTRA_KEY)
                    == Some(&Value::Bool(true))
        ));

        let encoded =
            crate::urp::encode::openai_chat::encode_response(&decoded, "gpt-4o-audio-preview");
        assert_eq!(encoded["choices"][0]["message"]["audio"], audio);
        assert_eq!(encoded["choices"][0]["message"]["content"], Value::Null);
    }

    #[test]
    fn chat_tool_choice_rejects_recursive_internal_key_spoofing() {
        let decoded = decode_request(&json!({
            "model": "gpt-5.4",
            "messages": [{ "role": "user", "content": "lookup" }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "parameters": { "type": "object" }
                }
            }],
            "tool_choice": {
                "type": "allowed_tools",
                "_monoize_outer_spoof": true,
                "allowed_tools": {
                    "mode": "required",
                    "_monoize_wrapper_spoof": true,
                    "tools": [{
                        "type": "function",
                        "_monoize_selector_spoof": true,
                        "function": {
                            "name": "lookup",
                            "_monoize_nested_spoof": true
                        }
                    }]
                }
            }
        }))
        .expect("decode modern Chat selector");

        let canonical = crate::urp::encode::tool_choice_to_value(
            decoded.tool_choice.as_ref().expect("tool choice"),
        );
        assert_eq!(
            canonical,
            json!({
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [{
                        "type": "function",
                        "function": { "name": "lookup" }
                    }]
                }
            })
        );

        let chat = crate::urp::encode::openai_chat::encode_request(&decoded, "gpt-5.4");
        let responses = crate::urp::encode::openai_responses::encode_request(&decoded, "gpt-5.4");
        assert!(!chat["tool_choice"].to_string().contains("_monoize_"));
        assert!(!responses["tool_choice"].to_string().contains("_monoize_"));
    }

    #[test]
    fn legacy_chat_function_choice_provenance_rejects_recursive_internal_key_spoofing() {
        let mut decoded = decode_request(&json!({
            "model": "gpt-4-0613",
            "messages": [{ "role": "user", "content": "lookup" }],
            "functions": [{
                "name": "lookup",
                "parameters": { "type": "object" }
            }],
            "function_call": {
                "name": "lookup",
                "_monoize_outer_spoof": true,
                "vendor_selector": {
                    "keep": 1,
                    "_monoize_nested_spoof": true
                }
            }
        }))
        .expect("decode deprecated Chat selector");

        let raw = decoded
            .extra_body
            .get_mut(CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY)
            .expect("legacy selector provenance");
        assert_eq!(
            raw,
            &json!({ "name": "lookup", "vendor_selector": { "keep": 1 } })
        );
        raw.as_object_mut().expect("legacy selector object").insert(
            "encoder_probe".to_string(),
            json!({ "keep": 2, "_monoize_encoder_spoof": true }),
        );

        let encoded = crate::urp::encode::openai_chat::encode_request(&decoded, "gpt-4-0613");
        assert_eq!(
            encoded["function_call"],
            json!({
                "name": "lookup",
                "vendor_selector": { "keep": 1 },
                "encoder_probe": { "keep": 2 }
            })
        );
        assert!(!encoded["function_call"].to_string().contains("_monoize_"));
        assert!(encoded.get("tool_choice").is_none());
    }

    #[test]
    fn deprecated_function_call_and_function_result_round_trip_as_legacy_messages() {
        let decoded = decode_request(&json!({
            "model": "gpt-4-0613",
            "messages": [
                { "role": "user", "content": "weather" },
                {
                    "role": "assistant",
                    "content": null,
                    "function_call": {
                        "name": "lookup",
                        "arguments": "{\"city\":\"Taipei\"}"
                    }
                },
                {
                    "role": "function",
                    "name": "lookup",
                    "content": "sunny"
                }
            ]
        }))
        .expect("decode deprecated function lifecycle");

        assert!(decoded.input.iter().any(|node| matches!(
            node,
            Node::ToolCall { call_id, name, arguments, extra_body, .. }
                if call_id == "legacy_function:lookup"
                    && name == "lookup"
                    && arguments == "{\"city\":\"Taipei\"}"
                    && extra_body.get(CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                        == Some(&Value::Bool(true))
        )));
        assert!(decoded.input.iter().any(|node| matches!(
            node,
            Node::ToolResult { call_id, content, extra_body, .. }
                if call_id == "legacy_function:lookup"
                    && matches!(&content[0], ToolResultContent::Text { text, .. } if text == "sunny")
                    && extra_body.get(CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY)
                        == Some(&json!("lookup"))
        )));

        let encoded = crate::urp::encode::openai_chat::encode_request(&decoded, "gpt-4-0613");
        assert_eq!(
            encoded["messages"][1]["function_call"],
            json!({ "name": "lookup", "arguments": "{\"city\":\"Taipei\"}" })
        );
        assert!(encoded["messages"][1].get("tool_calls").is_none());
        assert_eq!(encoded["messages"][2]["role"], json!("function"));
        assert_eq!(encoded["messages"][2]["name"], json!("lookup"));
        assert_eq!(encoded["messages"][2]["content"], json!("sunny"));
        assert!(encoded["messages"][2].get("tool_call_id").is_none());

        let response = decode_response(&json!({
            "id": "chatcmpl_legacy",
            "model": "gpt-4-0613",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "function_call": {
                        "name": "lookup",
                        "arguments": "{\"city\":\"Taipei\"}"
                    }
                },
                "finish_reason": "function_call"
            }]
        }))
        .expect("decode deprecated function response");
        let encoded_response =
            crate::urp::encode::openai_chat::encode_response(&response, "gpt-4-0613");
        assert_eq!(
            encoded_response["choices"][0]["message"]["function_call"],
            json!({ "name": "lookup", "arguments": "{\"city\":\"Taipei\"}" })
        );
        assert_eq!(
            encoded_response["choices"][0]["finish_reason"],
            json!("function_call")
        );
        assert_eq!(
            encoded_response["choices"][0]["message"]["content"],
            Value::Null
        );
        assert!(
            encoded_response["choices"][0]["message"]
                .get("tool_calls")
                .is_none()
        );
    }
}
