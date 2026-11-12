//! Responses request bodies and the request echo of synthesized responses.

use super::image_generation::prepare_image_generation_request;
use super::input_items::{
    encode_input_items, encode_instruction_items, is_retained_instruction_node,
};
use super::media::validate_stable_responses_audio;
use super::tools::encode_tools;
use crate::urp::cache_breakpoint::{CacheWireTarget, materialize_wire_markers};
use crate::urp::encode::{merge_extra, tool_choice_to_responses_value};
use crate::urp::{
    InstructionsFormat, Node, ProviderProtocol, ReasoningConfig, ResponseFormat, UrpRequest,
    is_internal_key,
};
use serde_json::{Map, Value, json};

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    encode_request_checked(req, upstream_model)
        .unwrap_or_else(|error| crate::urp::media::error_body(&error))
}

pub fn encode_request_checked(req: &UrpRequest, upstream_model: &str) -> Result<Value, String> {
    let mut prepared = crate::urp::media::prepare_request(req, ProviderProtocol::Responses)?;
    prepare_image_generation_request(&mut prepared)?;
    validate_stable_responses_audio(&prepared.input)?;
    materialize_wire_markers(&mut prepared, CacheWireTarget::Responses, upstream_model);
    Ok(encode_request_prepared(&prepared, upstream_model))
}

/// STR3t: the effective downstream request controls echoed on a synthesized Responses object.
pub(crate) fn response_request_echo(mut req: UrpRequest) -> Value {
    req.input.retain(is_retained_instruction_node);
    let model = req.model.clone();
    materialize_wire_markers(&mut req, CacheWireTarget::Responses, &model);
    let encoded = encode_request_prepared(&req, &req.model);
    let mut echo = Map::new();
    for key in [
        "instructions",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "text",
        "temperature",
        "top_p",
        "top_logprobs",
        "reasoning",
        "max_output_tokens",
        "max_tool_calls",
        "metadata",
        "truncation",
        "service_tier",
        "prompt_cache_key",
        "safety_identifier",
        "user",
        "background",
    ] {
        if let Some(value) = encoded.get(key) {
            echo.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(echo)
}

fn encode_request_prepared(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut instructions = req
        .instructions_format
        .map(|format| retained_instructions(&req.input, format));
    let input_items = encode_input_items(
        &req.input,
        req.instructions_format.is_some(),
        &mut instructions,
    );

    let mut body = json!({
        "model": upstream_model,
        "input": Value::Array(input_items),
    });
    let obj = body.as_object_mut().expect("responses request object");

    if let Some(instructions) = instructions {
        obj.insert("instructions".to_string(), instructions);
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
    if let Some(max) = req.max_output_tokens {
        obj.insert("max_output_tokens".to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning
        && let Some(reasoning) = encode_reasoning_config(reasoning)
    {
        obj.insert("reasoning".to_string(), reasoning);
    }
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(choice) = &req.tool_choice {
        obj.insert(
            "tool_choice".to_string(),
            tool_choice_to_responses_value(choice),
        );
    }
    if let Some(parallel) = req.parallel_tool_calls {
        obj.insert("parallel_tool_calls".to_string(), Value::Bool(parallel));
    }
    if let Some(user) = &req.user {
        obj.insert("user".to_string(), Value::String(user.clone()));
    }
    if let Some(format) = &req.response_format {
        apply_response_format(obj, format);
    }
    if let Some(verbosity) = &req.verbosity {
        let text = obj.entry("text".to_string()).or_insert_with(|| json!({}));
        if !text.is_object() {
            *text = json!({});
        }
        text.as_object_mut()
            .expect("responses text object")
            .insert("verbosity".to_string(), Value::String(verbosity.clone()));
    }
    merge_text_config(obj, req.extra_body.get("text"));
    merge_extra(obj, &req.extra_body);
    crate::urp::logprobs::encode_request(&mut body, &req.logprobs, ProviderProtocol::Responses);
    crate::urp::sampling::encode_request(&mut body, &req.sampling, ProviderProtocol::Responses);
    body
}

/// The `instructions` member restored from retained instruction nodes in their native format.
fn retained_instructions(input: &[Node], format: InstructionsFormat) -> Value {
    match format {
        InstructionsFormat::Null if !input.iter().any(is_retained_instruction_node) => Value::Null,
        InstructionsFormat::Items => Value::Array(encode_instruction_items(input)),
        _ => Value::String(
            input
                .iter()
                .filter_map(|node| match node {
                    Node::Text { content, .. } if is_retained_instruction_node(node) => {
                        Some(content.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    }
}

fn encode_reasoning_config(reasoning: &ReasoningConfig) -> Option<Value> {
    let mut obj = Map::new();
    if reasoning.disabled() {
        obj.insert("effort".to_string(), json!("none"));
    } else if let Some(effort) = &reasoning.effort {
        obj.insert("effort".to_string(), Value::String(effort.clone()));
    }
    if let Some(summary) = &reasoning.summary {
        obj.insert("summary".to_string(), json!(summary));
    }
    for (key, value) in &reasoning.extra_body {
        if !ReasoningConfig::is_control(key) && !is_internal_key(key) {
            obj.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    (!obj.is_empty()).then_some(Value::Object(obj))
}

fn apply_response_format(obj: &mut Map<String, Value>, format: &ResponseFormat) {
    let format = match format {
        ResponseFormat::Text => json!({ "type": "text" }),
        ResponseFormat::JsonObject => json!({ "type": "json_object" }),
        ResponseFormat::JsonSchema { json_schema } => {
            let mut format = Map::new();
            format.insert("type".to_string(), json!("json_schema"));
            format.insert("name".to_string(), json!(json_schema.name));
            format.insert("schema".to_string(), json_schema.schema.clone());
            if let Some(description) = &json_schema.description {
                format.insert("description".to_string(), json!(description));
            }
            if let Some(strict) = json_schema.strict {
                format.insert("strict".to_string(), json!(strict));
            }
            merge_extra(&mut format, &json_schema.extra_body);
            Value::Object(format)
        }
    };
    obj.insert("text".to_string(), json!({ "format": format }));
}

/// Keeps unknown members of the source `text` object; generated `format` and `verbosity` win.
fn merge_text_config(obj: &mut Map<String, Value>, raw_text: Option<&Value>) {
    let Some(raw_text) = raw_text.and_then(Value::as_object) else {
        return;
    };
    let mut merged = raw_text.clone();
    merged
        .retain(|key, _| !is_internal_key(key) && !matches!(key.as_str(), "format" | "verbosity"));
    if let Some(generated) = obj.get("text").and_then(Value::as_object) {
        for (key, value) in generated {
            merged.insert(key.clone(), value.clone());
        }
    }
    obj.insert("text".to_string(), Value::Object(merged));
}
