use crate::urp::decode::{
    deserialize_u64ish_default, is_internal_extra_key, parse_audio_node_from_obj,
    parse_file_node_from_obj, parse_image_node_from_obj, parse_tool_definition,
    remove_untrusted_internal_keys, retain_wire_extra_fields, split_extra, value_to_text,
    value_to_u64,
};
use crate::urp::{
    FinishReason, InputDetails, JsonSchemaDefinition, MESSAGES_OUTPUT_CONFIG_EXTRA_KEY,
    MESSAGES_THINKING_CONFIG_EXTRA_KEY, Node, OrdinaryRole, OutputDetails, ProviderProtocol,
    ReasoningConfig, ResponseFormat, StopControl, ToolChoice, ToolResultContent, UrpRequest,
    UrpResponse, Usage, unwrap_reasoning_signature_sigil,
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
    let extra_body = split_extra(bobj, &["type", "thinking", "signature"]);
    let metadata = crate::urp::ReasoningMetadata {
        downstream_only: thinking.is_some() && id.is_none() && encrypted.is_none(),
        summary_as_thinking: true,
        ..Default::default()
    };
    Some(Node::Reasoning {
        metadata,
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
    let extra_body = split_extra(bobj, &["type", "data"]);
    Some(Node::Reasoning {
        metadata: crate::urp::ReasoningMetadata {
            redacted: true,
            ..Default::default()
        },
        id,
        content: None,
        encrypted: Some(encrypted),
        summary: None,
        source: None,
        extra_body,
    })
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
        .map(str::to_string);

    let mode = thinking
        .as_ref()
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let budget_tokens = thinking
        .as_ref()
        .and_then(|v| v.get("budget_tokens"))
        .and_then(Value::as_u64);
    let display = thinking
        .as_ref()
        .and_then(|v| v.get("display"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut extra_body = HashMap::new();
    if let Some(mut thinking) = thinking {
        thinking.retain(|key, _| !matches!(key.as_str(), "type" | "budget_tokens" | "display"));
        extra_body.insert(
            MESSAGES_THINKING_CONFIG_EXTRA_KEY.to_string(),
            Value::Object(thinking),
        );
    }
    if let Some(mut output_config) = output_config {
        output_config.remove("effort");
        if output_config
            .get("format")
            .and_then(|v| v.get("type"))
            .and_then(Value::as_str)
            == Some("json_schema")
        {
            output_config.remove("format");
        }
        extra_body.insert(
            MESSAGES_OUTPUT_CONFIG_EXTRA_KEY.to_string(),
            Value::Object(output_config),
        );
    }
    Some(ReasoningConfig {
        effort,
        mode,
        budget_tokens,
        display,
        extra_body,
        ..Default::default()
    })
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
            extra_body: split_extra(format, &["type", "schema"]),
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
                tool_prompt_modality_breakdown: None,
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
            iterations: crate::urp::usage::decode_messages_iterations(
                value.extra.remove("iterations").as_ref(),
            ),
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
    Node::Text {
        logprobs: None,
        signature: None,
        citations: crate::urp::citations::decode(
            extra_body
                .remove("citations")
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default(),
            crate::urp::ProviderProtocol::Messages,
        ),
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
        input_nodes.extend(decode_content_nodes(system, OrdinaryRole::System)?);
    }

    for raw_msg in obj
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing messages".to_string())?
    {
        let msg_obj = raw_msg
            .as_object()
            .ok_or_else(|| "Messages message must be an object".to_string())?;
        let base_role = ordinary_role_from_messages_role(
            msg_obj
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user"),
        );

        let msg_extra_body = split_extra(msg_obj, &["role", "content"]);
        let message_nodes = match msg_obj.get("content") {
            Some(content) => decode_content_nodes(content, base_role)?,
            None => Vec::new(),
        };

        if !msg_extra_body.is_empty() && !message_nodes.is_empty() {
            input_nodes.push(Node::NextDownstreamEnvelopeExtra {
                extra_body: msg_extra_body,
            });
        }
        input_nodes.extend(message_nodes);
    }

    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|value| {
                let mut tool = parse_tool_definition(value)?;
                if tool.function.is_none() && tool.custom.is_none() {
                    tool.origin_protocol = Some(ProviderProtocol::Messages);
                    if tool.config.is_none() {
                        tool.config = Some(Value::Object(
                            std::mem::take(&mut tool.extra_body).into_iter().collect(),
                        ));
                    }
                }
                Some(tool)
            })
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

    crate::urp::tool_signature::restore_request_call_signatures(&mut input_nodes);
    crate::urp::sampling::strip_request_extras(
        &mut extra_body,
        crate::urp::ProviderProtocol::Messages,
    );
    Ok(UrpRequest {
        image_generation: None,
        sampling: crate::urp::sampling::request_config(
            obj,
            crate::urp::ProviderProtocol::Messages,
        ),
        logprobs: None,
        context: Default::default(),
        instructions_format: None,
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
    if obj.get("type").and_then(Value::as_str) == Some("error") {
        return Err(obj
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("Messages API returned an error")
            .to_string());
    }

    let output_nodes = match obj.get("content") {
        Some(content) => decode_content_nodes(content, OrdinaryRole::Assistant)?,
        None => Vec::new(),
    };

    let finish_reason = match obj.get("stop_reason").and_then(|v| v.as_str()) {
        Some("end_turn" | "stop_sequence") => Some(FinishReason::Stop),
        Some("max_tokens") => Some(FinishReason::Length),
        Some("model_context_window_exceeded") => Some(FinishReason::ContextLimit),
        Some("pause_turn") => Some(FinishReason::Paused),
        Some("compaction") => Some(FinishReason::Compaction),
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
        outcome: None,
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

fn decode_content_nodes(content: &Value, role: OrdinaryRole) -> Result<Vec<Node>, String> {
    if let Some(blocks) = content.as_array() {
        let mut nodes = Vec::new();
        for block in blocks {
            nodes.extend(decode_content_nodes(block, role)?);
        }
        return Ok(nodes);
    }
    Ok(decode_content_block(content, role)?.into_iter().collect())
}

pub(crate) fn decode_content_block(
    block: &Value,
    role: OrdinaryRole,
) -> Result<Option<Node>, String> {
    if let Some(text) = block.as_str() {
        return Ok(Some(Node::text(role, text)));
    }
    let Some(obj) = block.as_object() else {
        return Ok(None);
    };
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(if role == OrdinaryRole::System {
            "text"
        } else {
            ""
        });
    let node = match kind {
        "text" | "input_text" | "output_text" => {
            obj.get("text").and_then(Value::as_str).map(|text| {
                text_node_with_phase(
                    role,
                    text,
                    obj.get("phase").and_then(Value::as_str),
                    split_extra(obj, &["type", "text", "phase"]),
                )
            })
        }
        "thinking" => decode_anthropic_thinking_block(obj),
        "redacted_thinking" => decode_anthropic_redacted_thinking_block(obj),
        "image" | "image_url" | "input_image" | "output_image" => Some(
            parse_image_node_from_obj(obj, role)
                .ok_or("Messages image source is unsupported or malformed")?,
        ),
        "document" | "file" | "input_file" | "output_file" => Some(
            parse_file_node_from_obj(obj, role)
                .ok_or("Messages document source is unsupported or malformed")?,
        ),
        "audio" | "input_audio" | "output_audio" => Some(
            parse_audio_node_from_obj(obj, role)
                .ok_or("Messages audio source is unsupported or malformed")?,
        ),
        "tool_use" => Some(Node::ToolCall {
            namespace: obj
                .get("toolset_name")
                .and_then(Value::as_str)
                .map(str::to_string),
            signature: None,
            id: obj.get("id").and_then(Value::as_str).map(str::to_string),
            tool_type: crate::urp::ToolCallType::Function,
            call_id: obj
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name: obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            arguments: serde_json::to_string(obj.get("input").unwrap_or(&Value::Null))
                .unwrap_or_else(|_| "{}".into()),
            extra_body: split_extra(obj, &["type", "id", "name", "input", "toolset_name"]),
        }),
        "tool_result" => Some(Node::ToolResult {
            signature: None,
            namespace: obj
                .get("toolset_name")
                .and_then(Value::as_str)
                .map(str::to_string),
            name: None,
            id: None,
            tool_type: crate::urp::ToolCallType::Function,
            call_id: obj
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            is_error: obj
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            content: decode_tool_result_content(obj.get("content"))?,
            extra_body: split_extra(
                obj,
                &["type", "tool_use_id", "is_error", "content", "toolset_name"],
            ),
        }),
        _ => Some(provider_item_from_messages_block(obj, role)),
    };
    Ok(node)
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

fn tool_choice_from_messages_value(mut value: Value) -> ToolChoice {
    remove_untrusted_internal_keys(&mut value);
    if let Some(obj) = value.as_object_mut() {
        obj.remove("disable_parallel_tool_use");
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let normalized = match kind.as_str() {
            "any" => "required",
            "tool" => "function",
            other => other,
        };
        if matches!(kind.as_str(), "auto" | "any" | "none") && obj.len() == 1 {
            return ToolChoice::Mode(normalized.to_string());
        }
        if kind == "tool"
            && let Some(name) = obj.remove("name")
        {
            obj.insert("function".into(), serde_json::json!({"name":name}));
        }
        obj.insert("type".into(), Value::String(normalized.to_string()));
    }
    match value {
        Value::String(mode) => ToolChoice::Mode(mode),
        value => ToolChoice::Specific(value),
    }
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

fn decode_tool_result_content(content: Option<&Value>) -> Result<Vec<ToolResultContent>, String> {
    let mut blocks = Vec::new();
    let Some(content) = content else {
        return Ok(blocks);
    };
    if let Some(items) = content.as_array() {
        for block in items {
            decode_tool_result_content_block(block, &mut blocks)?;
        }
    } else {
        decode_tool_result_content_block(content, &mut blocks)?;
    }
    Ok(blocks)
}

fn decode_tool_result_content_block(
    block: &Value,
    content: &mut Vec<ToolResultContent>,
) -> Result<(), String> {
    if let Some(blocks) = block.as_array() {
        for block in blocks {
            decode_tool_result_content_block(block, content)?;
        }
        return Ok(());
    }
    let Some(obj) = block.as_object() else {
        let text = value_to_text(block);
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text,
                extra_body: HashMap::new(),
            });
        }
        return Ok(());
    };
    match obj.get("type").and_then(Value::as_str).unwrap_or("") {
        "text" | "input_text" | "output_text" => {
            if let Some(text) = obj.get("text").and_then(Value::as_str) {
                content.push(ToolResultContent::Text {
                    text: text.to_string(),
                    extra_body: split_extra(obj, &["type", "text"]),
                });
            }
        }
        "image" | "image_url" | "input_image" | "output_image" => {
            let Some(Node::Image {
                source,
                metadata,
                extra_body,
                ..
            }) = parse_image_node_from_obj(obj, OrdinaryRole::User)
            else {
                return Err("Messages tool-result image source is unsupported or malformed".into());
            };
            content.push(ToolResultContent::Image {
                source,
                metadata,
                extra_body,
            });
        }
        "document" | "file" | "input_file" | "output_file" => {
            let Some(Node::File {
                source,
                metadata,
                extra_body,
                ..
            }) = parse_file_node_from_obj(obj, OrdinaryRole::User)
            else {
                return Err(
                    "Messages tool-result document source is unsupported or malformed".into(),
                );
            };
            content.push(ToolResultContent::File {
                source,
                metadata,
                extra_body,
            });
        }
        "audio" | "input_audio" | "output_audio" => {
            let Some(Node::Audio {
                source,
                metadata,
                extra_body,
                ..
            }) = parse_audio_node_from_obj(obj, OrdinaryRole::User)
            else {
                return Err("Messages tool-result audio source is unsupported or malformed".into());
            };
            let source = match source {
                crate::urp::AudioSource::Base64 { media_type, data } => {
                    crate::urp::FileSource::Base64 { media_type, data }
                }
                crate::urp::AudioSource::Url { url } => crate::urp::FileSource::Url { url },
            };
            content.push(ToolResultContent::File {
                source,
                metadata,
                extra_body,
            });
        }
        _ => {
            content.push(ToolResultContent::ProviderItem {
                origin_protocol: ProviderProtocol::Messages,
                item_type: obj
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                body: block.clone(),
                extra_body: HashMap::new(),
            });
        }
    }
    Ok(())
}

pub(crate) fn decode_usage(value: &Value) -> Option<Usage> {
    serde_json::from_value::<AnthropicUsage>(value.clone())
        .ok()
        .map(Usage::from)
}
