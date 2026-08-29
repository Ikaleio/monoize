use crate::urp::decode::{
    deserialize_u64ish_default, is_internal_extra_key, parse_file_node_from_obj,
    parse_file_source_from_obj, parse_image_node_from_obj, parse_image_source_from_obj,
    parse_tool_definition, remove_untrusted_internal_keys, retain_wire_extra_fields, split_extra,
    value_to_text, value_to_u64,
};
use crate::urp::{
    FILE_ID_ORIGIN_EXTRA_KEY, FILE_ID_ORIGIN_MESSAGES, FileSource, FinishReason, ImageSource,
    InputDetails, JsonSchemaDefinition, MESSAGES_OUTPUT_CONFIG_EXTRA_KEY,
    MESSAGES_THINKING_CONFIG_EXTRA_KEY, Node, OrdinaryRole, OutputDetails, ProviderProtocol,
    REASONING_DOWNSTREAM_ONLY_PRESENTATION_EXTRA_KEY, REASONING_KIND_EXTRA_KEY,
    REASONING_KIND_REDACTED_THINKING, ReasoningConfig, ResponseFormat, StopControl, ToolChoice,
    ToolResultContent, UrpRequest, UrpResponse, Usage, unwrap_reasoning_signature_sigil,
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
    let mut extra_body = split_extra(bobj, &["type", "thinking", "signature"]);
    // Mark as downstream-only when we have a summary but no encrypted content to pass back.
    // When signature IS present (even without mz sigil), it's encrypted reasoning that can
    // be round-tripped — not a mere presentation artifact.
    if thinking.is_some() && id.is_none() && encrypted.is_none() {
        extra_body.insert(
            REASONING_DOWNSTREAM_ONLY_PRESENTATION_EXTRA_KEY.to_string(),
            Value::Bool(true),
        );
    }
    Some(Node::Reasoning {
        id,
        content: None,
        encrypted,
        summary: thinking,
        source: None,
        extra_body,
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

fn effort_from_anthropic_budget(budget: u64) -> Option<String> {
    match budget {
        0 => None,
        1..=1024 => Some("low".to_string()),
        1025..=4096 => Some("medium".to_string()),
        4097..=16384 => Some("high".to_string()),
        _ => Some("xhigh".to_string()),
    }
}

fn decode_anthropic_reasoning_config(obj: &Map<String, Value>) -> Option<ReasoningConfig> {
    let mut thinking = obj.get("thinking").and_then(Value::as_object).cloned();
    let mut output_config = obj.get("output_config").and_then(Value::as_object).cloned();
    if let Some(thinking) = thinking.as_mut() {
        thinking.retain(|key, _| !is_internal_extra_key(key));
    }
    if let Some(output_config) = output_config.as_mut() {
        output_config.retain(|key, _| !is_internal_extra_key(key));
    }
    if thinking.is_none() && output_config.is_none() {
        return None;
    }

    let effort = output_config
        .as_ref()
        .and_then(|config| config.get("effort"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            let thinking = thinking.as_ref()?;
            match thinking.get("type").and_then(Value::as_str) {
                Some("disabled") => Some("none".to_string()),
                Some("enabled") => thinking
                    .get("budget_tokens")
                    .and_then(Value::as_u64)
                    .and_then(effort_from_anthropic_budget),
                _ => None,
            }
        });

    let mut extra_body = HashMap::new();
    if let Some(thinking) = thinking {
        extra_body.insert(
            MESSAGES_THINKING_CONFIG_EXTRA_KEY.to_string(),
            Value::Object(thinking),
        );
    }
    if let Some(output_config) = output_config {
        extra_body.insert(
            MESSAGES_OUTPUT_CONFIG_EXTRA_KEY.to_string(),
            Value::Object(output_config),
        );
    }
    Some(ReasoningConfig { effort, extra_body })
}

fn decode_anthropic_response_format(obj: &Map<String, Value>) -> Option<ResponseFormat> {
    let format = obj
        .get("output_config")?
        .as_object()?
        .get("format")?
        .as_object()?;
    if format.get("type").and_then(Value::as_str) != Some("json_schema") {
        return None;
    }

    Some(ResponseFormat::JsonSchema {
        json_schema: JsonSchemaDefinition {
            // Messages has no schema-name field. OpenAI-compatible targets require one.
            name: "response".to_string(),
            description: None,
            schema: format.get("schema")?.clone(),
            strict: None,
            extra_body: HashMap::new(),
        },
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
    #[serde(default)]
    cache_creation: AnthropicCacheCreationUsage,
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

#[derive(Debug, Default, Deserialize)]
struct AnthropicCacheCreationUsage {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    ephemeral_5m_input_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    ephemeral_1h_input_tokens: u64,
}

impl From<AnthropicUsage> for Usage {
    fn from(mut value: AnthropicUsage) -> Self {
        if let Some(output_details) = value
            .extra
            .get_mut("output_tokens_details")
            .and_then(Value::as_object_mut)
        {
            output_details.retain(|key, _| !is_internal_extra_key(key));
        }
        retain_wire_extra_fields(&mut value.extra);
        let native_reasoning_tokens = value
            .extra
            .get_mut("output_tokens_details")
            .and_then(Value::as_object_mut)
            .and_then(|details| details.remove("thinking_tokens"))
            .and_then(|value| value_to_u64(&value));
        let output_tokens_details_is_empty = value
            .extra
            .get("output_tokens_details")
            .and_then(Value::as_object)
            .is_some_and(Map::is_empty);
        if output_tokens_details_is_empty {
            value.extra.remove("output_tokens_details");
        }
        let reasoning_tokens = native_reasoning_tokens.unwrap_or(value.reasoning_tokens);

        let input_details = if value.cache_read_input_tokens > 0
            || value.cache_creation_input_tokens > 0
            || value.tool_prompt_tokens > 0
        {
            Some(InputDetails {
                standard_tokens: 0,
                cache_read_tokens: value.cache_read_input_tokens,
                cache_read_modality_breakdown: None,
                cache_creation_tokens: value.cache_creation_input_tokens,
                cache_creation_5m_tokens: value.cache_creation.ephemeral_5m_input_tokens,
                cache_creation_1h_tokens: value.cache_creation.ephemeral_1h_input_tokens,
                tool_prompt_tokens: value.tool_prompt_tokens,
                modality_breakdown: None,
            })
        } else {
            None
        };

        let output_details = if reasoning_tokens > 0
            || value.accepted_prediction_tokens > 0
            || value.rejected_prediction_tokens > 0
        {
            Some(OutputDetails {
                standard_tokens: 0,
                reasoning_tokens,
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
                        input_nodes.push(provider_item_from_messages_block(
                            bobj,
                            OrdinaryRole::System,
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
                            tool_type: crate::urp::ToolCallType::Function,
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
                            tool_type: crate::urp::ToolCallType::Function,
                            call_id,
                            is_error,
                            content: decode_tool_result_content(bobj.get("content")),
                            extra_body: split_extra(
                                bobj,
                                &["type", "tool_use_id", "is_error", "content"],
                            ),
                        });
                    }
                    "image" => {
                        if let Some(node) = parse_image_node_from_obj(bobj, base_role) {
                            message_nodes.push(node);
                        }
                    }
                    "document" | "file" => {
                        if let Some(node) = parse_file_node_from_obj(bobj, base_role) {
                            message_nodes.push(node);
                        }
                    }
                    _ => {
                        message_nodes.push(provider_item_from_messages_block(bobj, base_role));
                    }
                }
            }
        }

        if !msg_extra_body.is_empty() && !message_nodes.is_empty() {
            input_nodes.push(Node::NextDownstreamEnvelopeExtra {
                extra_body: msg_extra_body,
            });
        }
        input_nodes.extend(message_nodes);
    }

    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(parse_tool_definition)
            .collect::<Vec<_>>()
    });

    let reasoning = decode_anthropic_reasoning_config(obj);

    let raw_tool_choice = obj.get("tool_choice").cloned();
    let parallel_tool_calls = obj
        .get("parallel_tool_calls")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            raw_tool_choice
                .as_ref()
                .and_then(tool_choice_disable_parallel)
                .map(|disabled| !disabled)
        });

    let mut extra_body = split_extra(
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
            "parallel_tool_calls",
            "stop_sequences",
            "metadata",
        ],
    );
    if let Some(mut metadata) = obj.get("metadata").and_then(Value::as_object).cloned() {
        metadata.remove("user_id");
        if !metadata.is_empty() {
            extra_body.insert("metadata".to_string(), Value::Object(metadata));
        }
    }

    Ok(UrpRequest {
        model,
        input: input_nodes,
        stream: obj.get("stream").and_then(|v| v.as_bool()),
        temperature: obj.get("temperature").and_then(|v| v.as_f64()),
        top_p: obj.get("top_p").and_then(|v| v.as_f64()),
        max_output_tokens: obj.get("max_tokens").and_then(|v| v.as_u64()),
        reasoning,
        tools,
        tool_choice: raw_tool_choice.map(tool_choice_from_messages_value),
        parallel_tool_calls,
        stop: obj
            .get("stop_sequences")
            .and_then(Value::as_array)
            .and_then(|stops| {
                stops
                    .iter()
                    .map(Value::as_str)
                    .map(|stop| stop.map(str::to_string))
                    .collect::<Option<Vec<_>>>()
            })
            .map(StopControl::Multiple),
        verbosity: None,
        response_format: decode_anthropic_response_format(obj),
        user: obj
            .get("metadata")
            .and_then(|v| v.get("user_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        extra_body,
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
                        tool_type: crate::urp::ToolCallType::Function,
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(bobj, &["type", "id", "name", "input"]),
                    }]
                }
                "image" => parse_image_node_from_obj(bobj, OrdinaryRole::Assistant)
                    .into_iter()
                    .collect(),
                "document" | "file" => parse_file_node_from_obj(bobj, OrdinaryRole::Assistant)
                    .into_iter()
                    .collect(),
                _ => {
                    vec![provider_item_from_messages_block(
                        bobj,
                        OrdinaryRole::Assistant,
                    )]
                }
            };

            output_nodes.extend(decoded_nodes);
        }
    }

    let finish_reason = match obj.get("stop_reason").and_then(|v| v.as_str()) {
        Some("end_turn" | "stop_sequence") => Some(FinishReason::Stop),
        Some("max_tokens") => Some(FinishReason::Length),
        Some("tool_use") => Some(FinishReason::ToolCalls),
        Some("refusal") => Some(FinishReason::ContentFilter),
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
        extra_body: split_extra(obj, &["id", "type", "role", "model", "content", "usage"]),
    })
}

fn provider_item_from_messages_block(block: &Map<String, Value>, role: OrdinaryRole) -> Node {
    let item_type = block
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Node::ProviderItem {
        id: block
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        origin_protocol: ProviderProtocol::Messages,
        role,
        item_type,
        body: Value::Object(block.clone()),
        extra_body: HashMap::new(),
    }
}

fn tool_choice_from_messages_value(mut v: Value) -> ToolChoice {
    remove_untrusted_internal_keys(&mut v);
    if let Some(obj) = v.as_object() {
        let disable_parallel = obj
            .get("disable_parallel_tool_use")
            .and_then(|x| x.as_bool());
        match obj.get("type").and_then(|x| x.as_str()) {
            Some("auto") => {
                if let Some(disable) = disable_parallel {
                    return ToolChoice::Specific(serde_json::json!({
                        "type": "auto",
                        "disable_parallel_tool_use": disable
                    }));
                }
                return ToolChoice::Mode("auto".to_string());
            }
            Some("any") => {
                if let Some(disable) = disable_parallel {
                    return ToolChoice::Specific(serde_json::json!({
                        "type": "required",
                        "disable_parallel_tool_use": disable
                    }));
                }
                return ToolChoice::Mode("required".to_string());
            }
            Some("none") => return ToolChoice::Mode("none".to_string()),
            Some("tool") => {
                if let Some(name) = obj.get("name").and_then(|x| x.as_str()) {
                    let mut choice = serde_json::json!({
                        "type": "function",
                        "function": { "name": name }
                    });
                    if let Some(disable) = disable_parallel {
                        choice["disable_parallel_tool_use"] = Value::Bool(disable);
                    }
                    return ToolChoice::Specific(choice);
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

fn tool_choice_disable_parallel(v: &Value) -> Option<bool> {
    let obj = v.as_object()?;
    match obj.get("type").and_then(|x| x.as_str()) {
        Some("auto" | "any" | "tool") => obj
            .get("disable_parallel_tool_use")
            .and_then(|x| x.as_bool()),
        _ => None,
    }
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
                extra_body: HashMap::new(),
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
        blocks.push(ToolResultContent::Text {
            text,
            extra_body: HashMap::new(),
        });
    }
    blocks
}

fn decode_tool_result_content_block(block: &Value, content: &mut Vec<ToolResultContent>) {
    if let Some(text) = block.as_str() {
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text: text.to_string(),
                extra_body: HashMap::new(),
            });
        }
        return;
    }
    let Some(obj) = block.as_object() else {
        let text = value_to_text(block);
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text,
                extra_body: HashMap::new(),
            });
        }
        return;
    };

    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "text" => {
            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                content.push(ToolResultContent::Text {
                    text: text.to_string(),
                    extra_body: split_extra(obj, &["type", "text"]),
                });
            }
        }
        _ => {
            if let Some(source) = parse_image_source_from_obj(obj) {
                let mut extra_body = split_extra(obj, &["type", "source"]);
                if matches!(source, ImageSource::FileId { .. }) {
                    extra_body.insert(
                        FILE_ID_ORIGIN_EXTRA_KEY.to_string(),
                        Value::String(FILE_ID_ORIGIN_MESSAGES.to_string()),
                    );
                }
                content.push(ToolResultContent::Image { source, extra_body });
                return;
            }
            if let Some(source) = parse_file_source_from_obj(obj) {
                let mut extra_body = split_extra(obj, &["type", "source"]);
                if matches!(source, FileSource::FileId { .. }) {
                    extra_body.insert(
                        FILE_ID_ORIGIN_EXTRA_KEY.to_string(),
                        Value::String(FILE_ID_ORIGIN_MESSAGES.to_string()),
                    );
                }
                content.push(ToolResultContent::File { source, extra_body });
                return;
            }
            content.push(ToolResultContent::ProviderItem {
                origin_protocol: ProviderProtocol::Messages,
                item_type: obj
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
                body: block.clone(),
                extra_body: HashMap::new(),
            });
        }
    }
}
