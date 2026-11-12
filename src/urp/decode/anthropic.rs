mod content;
mod usage;

pub(crate) use content::decode_content_block;
pub(crate) use usage::decode_usage;

use crate::urp::decode::{parse_tool_definition, remove_untrusted_internal_keys, split_extra};
use crate::urp::encode::anthropic::messages_finish_reason;
use crate::urp::{
    FinishReason, JsonSchemaDefinition, MESSAGES_OUTPUT_CONFIG_EXTRA_KEY,
    MESSAGES_SYSTEM_ENVELOPE_EXTRA_KEY, MESSAGES_THINKING_CONFIG_EXTRA_KEY, Node, OrdinaryRole,
    ProviderProtocol, ReasoningConfig, ResponseFormat, StopControl, ToolChoice, UrpRequest,
    UrpResponse, is_internal_key,
};
use content::decode_content_nodes;
use serde_json::{Map, Value};
use std::collections::HashMap;

fn decode_anthropic_reasoning_config(obj: &Map<String, Value>) -> Option<ReasoningConfig> {
    let mut thinking = obj.get("thinking").and_then(Value::as_object).cloned();
    let mut output_config = obj.get("output_config").and_then(Value::as_object).cloned();
    if let Some(thinking) = thinking.as_mut() {
        thinking.retain(|key, _| !is_internal_key(key));
    }
    if let Some(output_config) = output_config.as_mut() {
        output_config.retain(|key, _| !is_internal_key(key));
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
            // Messages structured outputs always enforce the schema.
            strict: Some(true),
            extra_body: split_extra(format, &["type", "schema"]),
        },
    })
}

fn ordinary_role_from_messages_role(role: Option<&Value>) -> Result<OrdinaryRole, String> {
    match role.and_then(Value::as_str) {
        Some("user") => Ok(OrdinaryRole::User),
        Some("assistant") => Ok(OrdinaryRole::Assistant),
        Some("system") => Ok(OrdinaryRole::System),
        Some("developer") => Ok(OrdinaryRole::Developer),
        Some(other) => Err(format!(
            "messages: Unexpected role \"{other}\". Allowed roles are \"user\", \"assistant\" or \"system\""
        )),
        None => Err("messages: each message requires a string role".to_string()),
    }
}

fn decode_stop_sequences(value: Option<&Value>) -> Result<Option<StopControl>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let invalid = || "stop_sequences: Input should be a valid array of strings".to_string();
    let stops = value
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|stop| stop.as_str().map(str::to_string).ok_or_else(invalid))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(StopControl::Multiple(stops)))
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let mut request = decode_native_request(value)?;
    crate::urp::cache_breakpoint::decode_request_markers(&mut request, ProviderProtocol::Messages);
    Ok(request)
}

fn decode_input_nodes(obj: &Map<String, Value>) -> Result<Vec<Node>, String> {
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
        let base_role = ordinary_role_from_messages_role(msg_obj.get("role"))?;

        let mut msg_extra_body = split_extra(msg_obj, &["role", "content"]);
        let message_nodes = match msg_obj.get("content") {
            Some(content) => decode_content_nodes(content, base_role)?,
            None => Vec::new(),
        };

        if base_role == OrdinaryRole::System {
            msg_extra_body.insert(MESSAGES_SYSTEM_ENVELOPE_EXTRA_KEY.into(), Value::Bool(true));
        }
        if base_role == OrdinaryRole::System
            || (!msg_extra_body.is_empty() && !message_nodes.is_empty())
        {
            input_nodes.push(Node::NextDownstreamEnvelopeExtra {
                extra_body: msg_extra_body,
            });
        }
        input_nodes.extend(message_nodes);
    }
    Ok(input_nodes)
}

fn decode_native_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "messages request must be object".to_string())?;
    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();
    let mut input_nodes = decode_input_nodes(obj)?;

    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|value| {
                let mut tool = parse_tool_definition(value)?;
                if tool.function.is_some() {
                    tool.origin_protocol = Some(ProviderProtocol::Messages);
                }
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
    crate::urp::sampling::strip_request_extras(&mut extra_body, ProviderProtocol::Messages);
    Ok(UrpRequest {
        image_generation: None,
        sampling: crate::urp::sampling::request_config(obj, ProviderProtocol::Messages),
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
        stop: decode_stop_sequences(obj.get("stop_sequences"))?,
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

    let finish_reason = obj
        .get("stop_reason")
        .and_then(Value::as_str)
        .and_then(messages_finish_reason)
        .unwrap_or(FinishReason::Other);

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
        finish_reason: Some(finish_reason),
        usage: obj.get("usage").and_then(decode_usage),
        extra_body: split_extra(obj, &["id", "type", "role", "model", "content", "usage"]),
    })
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
