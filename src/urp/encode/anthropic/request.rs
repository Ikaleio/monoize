use super::blocks::validate_complete_tool_inputs;
use super::media::messages_media_error_body;
use super::messages::encode_conversation;
use super::model::messages_default_max_tokens;
use super::thinking::{encode_thinking_controls, validate_generated_thinking_controls};
use super::tools::{encode_tool_choice, encode_tools, prepare_schema_custom_tools};
use crate::urp::cache_breakpoint::{CacheWireTarget, materialize_wire_markers};
use crate::urp::encode::merge_extra;
use crate::urp::{
    MESSAGES_OUTPUT_CONFIG_EXTRA_KEY, MESSAGES_THINKING_CONFIG_EXTRA_KEY, ProviderProtocol,
    ResponseFormat, StopControl, UrpRequest,
};
use serde_json::{Map, Value, json};

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    match prepare_request(req, upstream_model) {
        Ok(prepared) => encode_prepared_request(&prepared, upstream_model),
        Err(message) => messages_media_error_body(&message),
    }
}

/// Encodes one Messages request and rejects provider-invalid thinking controls that Monoize
/// generated before the caller can dispatch the body upstream. Explicit client Messages
/// controls are forwarded unvalidated (RC4d, RC4f).
pub fn encode_request_checked(req: &UrpRequest, upstream_model: &str) -> Result<Value, String> {
    let prepared = prepare_request(req, upstream_model)?;
    validate_complete_tool_inputs(&prepared.input)?;
    let body = encode_prepared_request(&prepared, upstream_model);
    let explicit = |key: &str| {
        prepared
            .reasoning
            .as_ref()
            .is_some_and(|reasoning| reasoning.extra_body.contains_key(key))
    };
    if let Value::Object(obj) = &body {
        validate_generated_thinking_controls(
            obj,
            upstream_model,
            explicit(MESSAGES_THINKING_CONFIG_EXTRA_KEY),
            explicit(MESSAGES_OUTPUT_CONFIG_EXTRA_KEY),
        )?;
    }
    Ok(body)
}

fn prepare_request(req: &UrpRequest, upstream_model: &str) -> Result<UrpRequest, String> {
    let mut prepared = crate::urp::media::prepare_request(req, ProviderProtocol::Messages)?;
    prepare_schema_custom_tools(&mut prepared)?;
    materialize_wire_markers(&mut prepared, CacheWireTarget::Messages, upstream_model);
    Ok(prepared)
}

fn encode_prepared_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let (system, messages) = encode_conversation(&req.input);
    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(upstream_model));
    obj.insert("messages".to_string(), Value::Array(messages));
    obj.insert(
        "max_tokens".to_string(),
        json!(
            req.max_output_tokens
                .unwrap_or_else(|| messages_default_max_tokens(upstream_model))
        ),
    );
    if !system.is_empty() {
        obj.insert("system".to_string(), Value::Array(system));
    }
    if let Some(stream) = req.stream {
        obj.insert("stream".to_string(), Value::Bool(stream));
    }
    if let Some(temp) = req.temperature {
        obj.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        obj.insert("top_p".to_string(), Value::from(top_p));
    }
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(choice) = &req.tool_choice {
        obj.insert(
            "tool_choice".to_string(),
            encode_tool_choice(choice, req.parallel_tool_calls),
        );
    } else if req.parallel_tool_calls == Some(false)
        && req.tools.as_ref().is_some_and(|tools| !tools.is_empty())
    {
        obj.insert(
            "tool_choice".to_string(),
            json!({ "type": "auto", "disable_parallel_tool_use": true }),
        );
    }
    if let Some(reasoning) = &req.reasoning {
        encode_thinking_controls(&mut obj, reasoning, upstream_model);
    }
    if let Some(stop) = &req.stop {
        let stops = match stop {
            StopControl::Single(stop) => vec![Value::String(stop.clone())],
            StopControl::Multiple(stops) => stops
                .iter()
                .map(|stop| Value::String(stop.clone()))
                .collect(),
        };
        obj.insert("stop_sequences".to_string(), Value::Array(stops));
    }
    merge_extra(&mut obj, &req.extra_body);
    if let Some(user) = &req.user {
        let user_id = Value::String(user.clone());
        match obj.get_mut("metadata") {
            Some(Value::Object(metadata)) => {
                metadata.insert("user_id".to_string(), user_id);
            }
            _ => {
                obj.insert(
                    "metadata".to_string(),
                    Value::Object(Map::from_iter([("user_id".to_string(), user_id)])),
                );
            }
        }
    }
    if let Some(format) = &req.response_format {
        apply_response_format(&mut obj, format);
    }
    let mut body = Value::Object(obj);
    crate::urp::sampling::encode_request(&mut body, &req.sampling, ProviderProtocol::Messages);
    body
}

fn apply_response_format(obj: &mut Map<String, Value>, format: &ResponseFormat) {
    let output_config = match obj.remove("output_config") {
        Some(Value::Object(output_config)) => Some(output_config),
        _ => None,
    };
    match format {
        ResponseFormat::JsonSchema { json_schema } => {
            let mut output_config = output_config.unwrap_or_default();
            let mut messages_format = match output_config.remove("format") {
                Some(Value::Object(format)) => format,
                _ => Map::new(),
            };
            messages_format.insert("type".to_string(), json!("json_schema"));
            messages_format.insert("schema".to_string(), json_schema.schema.clone());
            merge_extra(&mut messages_format, &json_schema.extra_body);
            output_config.insert("format".to_string(), Value::Object(messages_format));
            obj.insert("output_config".to_string(), Value::Object(output_config));
        }
        ResponseFormat::Text | ResponseFormat::JsonObject => {
            let Some(mut output_config) = output_config else {
                return;
            };
            output_config.remove("format");
            if !output_config.is_empty() {
                obj.insert("output_config".to_string(), Value::Object(output_config));
            }
        }
    }
}
