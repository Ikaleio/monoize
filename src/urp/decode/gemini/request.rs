//! Gemini `GenerateContentRequest` decoding (GEM-3, GEM-4, PG4).

use super::config::{GenerationControls, strip_typed_generation_config, uses_text_response_format};
use super::parts::{content_parts, decode_part};
use super::proto_names::normalize_proto_json_names;
use super::tools::{decode_tool_choice, decode_tools, tool_config_extra};
use crate::urp::decode::split_extra;
use crate::urp::tool_signature::restore_request_call_signatures;
use crate::urp::{
    GEMINI_CONTENT_EXTRA_KEY, GEMINI_SYSTEM_EXTRA_KEY, GEMINI_TEXT_FORMAT_KEY, Node, OrdinaryRole,
    UrpRequest,
};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let value = normalize_proto_json_names(value);
    let obj = value
        .as_object()
        .ok_or_else(|| "gemini request must be object".to_string())?;

    let mut input = Vec::new();
    if let Some(system) = obj.get("systemInstruction") {
        decode_system_instruction(system, &mut input)?;
    }
    let mut resolved_results = HashSet::new();
    for content in obj
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        decode_content(content, &mut input, &mut resolved_results)?;
    }
    restore_request_call_signatures(&mut input);

    let generation_config = obj.get("generationConfig").and_then(Value::as_object);
    let generation = GenerationControls::decode(generation_config);
    let mut extra_body = split_extra(
        obj,
        &[
            "model",
            "contents",
            "systemInstruction",
            "tools",
            "toolConfig",
            "stream",
            "streamGenerateContent",
        ],
    );
    if generation_config.is_some_and(uses_text_response_format) {
        extra_body.insert(GEMINI_TEXT_FORMAT_KEY.into(), json!(true));
    }
    if let Some(config) = extra_body
        .get_mut("generationConfig")
        .and_then(Value::as_object_mut)
    {
        strip_typed_generation_config(config);
    }
    let tool_config = obj.get("toolConfig").and_then(Value::as_object);
    if let Some(extra) = tool_config
        .map(tool_config_extra)
        .filter(|extra| !extra.is_empty())
    {
        extra_body.insert(
            "toolConfig".into(),
            Value::Object(extra.into_iter().collect()),
        );
    }

    Ok(UrpRequest {
        image_generation: None,
        sampling: generation.sampling,
        logprobs: generation.logprobs,
        context: Default::default(),
        instructions_format: None,
        model: obj
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        input,
        stream: obj
            .get("stream")
            .and_then(Value::as_bool)
            .or_else(|| obj.get("streamGenerateContent").and_then(Value::as_bool)),
        temperature: generation.temperature,
        top_p: generation.top_p,
        max_output_tokens: generation.max_output_tokens,
        reasoning: generation.reasoning,
        tools: obj
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| decode_tools(tools)),
        tool_choice: tool_config
            .and_then(|config| config.get("functionCallingConfig"))
            .and_then(decode_tool_choice),
        parallel_tool_calls: None,
        stop: generation.stop,
        verbosity: None,
        response_format: generation.response_format,
        user: None,
        extra_body,
    })
}

/// System instruction Parts become system-role nodes. Unknown `systemInstruction` members of a
/// Content-shaped instruction stay on its first node.
fn decode_system_instruction(system: &Value, input: &mut Vec<Node>) -> Result<(), String> {
    let first = input.len();
    for part in content_parts(system.get("parts").unwrap_or(system)) {
        input.push(decode_part(part, OrdinaryRole::System)?);
    }
    if let Some(node) = input.get_mut(first)
        && let Some(system) = system
            .as_object()
            .filter(|system| system.contains_key("parts"))
    {
        let extra = split_extra(system, &["parts"]);
        if !extra.is_empty() {
            node.extra_body_mut().insert(
                GEMINI_SYSTEM_EXTRA_KEY.into(),
                Value::Object(extra.into_iter().collect()),
            );
        }
    }
    Ok(())
}

/// Decodes one Content. Its first node always carries [`GEMINI_CONTENT_EXTRA_KEY`], even when
/// empty: the encoder starts a new Content at that marker, which keeps native Content
/// boundaries.
fn decode_content(
    content: &Map<String, Value>,
    input: &mut Vec<Node>,
    resolved_results: &mut HashSet<String>,
) -> Result<(), String> {
    let first = input.len();
    let role = match content.get("role").and_then(Value::as_str) {
        Some("model" | "assistant") => OrdinaryRole::Assistant,
        Some("system") => OrdinaryRole::System,
        Some("developer") => OrdinaryRole::Developer,
        _ => OrdinaryRole::User,
    };
    for part in content.get("parts").map(content_parts).unwrap_or_default() {
        let mut node = decode_part(part, role)?;
        if let Node::ToolResult {
            id, call_id, name, ..
        } = &mut node
        {
            if id.is_none()
                && let Some(matching) = name
                    .as_deref()
                    .and_then(|name| unresolved_call_id(input, resolved_results, name))
            {
                *call_id = matching;
            }
            resolved_results.insert(call_id.clone());
        }
        input.push(node);
    }
    if let Some(node) = input.get_mut(first) {
        node.extra_body_mut().insert(
            GEMINI_CONTENT_EXTRA_KEY.into(),
            Value::Object(
                split_extra(content, &["role", "parts"])
                    .into_iter()
                    .collect(),
            ),
        );
    }
    Ok(())
}

/// PG4d: a `functionResponse` without `id` answers the earliest call of the same name that no
/// earlier result answered, so parallel calls of one function keep their correlation.
fn unresolved_call_id(input: &[Node], resolved: &HashSet<String>, name: &str) -> Option<String> {
    input.iter().find_map(|node| match node {
        Node::ToolCall {
            call_id,
            name: call_name,
            ..
        } if call_name == name && !resolved.contains(call_id) => Some(call_id.clone()),
        _ => None,
    })
}
