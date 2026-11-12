use super::messages::encode_messages;
use super::tools::{
    encode_legacy_function_choice, encode_legacy_functions, encode_tools, tool_choice_names_any,
};
use super::{ChatDialect, ChatEncodeError, merge_chat_wire_extra};
use crate::urp::encode::tool_choice_to_chat_value;
use crate::urp::{
    CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY, CHAT_MAX_TOKENS_FIELD_EXTRA_KEY,
    CHAT_REASONING_CONFIG_EXTRA_KEY, CHAT_THINKING_CONFIG_EXTRA_KEY, Node, OrdinaryRole,
    ProviderProtocol, ReasoningConfig, ResponseFormat, StopControl, UrpRequest, is_internal_key,
};
use serde_json::{Map, Value, json};

pub fn encode_request(req: &UrpRequest, upstream_model: &str, dialect: ChatDialect) -> Value {
    encode_request_checked(req, upstream_model, dialect)
        .unwrap_or_else(|error| crate::urp::media::error_body(&error.message))
}

pub fn encode_request_checked(
    req: &UrpRequest,
    upstream_model: &str,
    dialect: ChatDialect,
) -> Result<Value, ChatEncodeError> {
    let mut prepared = crate::urp::media::prepare_request(req, ProviderProtocol::ChatCompletion)
        .map_err(|message| ChatEncodeError {
            code: "unsupported_media",
            message,
        })?;
    crate::urp::cache_breakpoint::materialize_wire_markers(
        &mut prepared,
        crate::urp::cache_breakpoint::CacheWireTarget::Chat(dialect),
        upstream_model,
    );
    // PC2.11: an empty assistant text carries nothing and must not split the assistant turn.
    prepared.input.retain(|node| !is_empty_assistant_text(node));
    encode_prepared_request(&prepared, upstream_model, dialect)
}

fn is_empty_assistant_text(node: &Node) -> bool {
    matches!(node, Node::Text {
        role: OrdinaryRole::Assistant,
        content,
        citations,
        extra_body,
        ..
    } if content.is_empty()
        && citations.is_empty()
        && extra_body.keys().all(|key| is_internal_key(key)))
}

fn encode_prepared_request(
    req: &UrpRequest,
    upstream_model: &str,
    dialect: ChatDialect,
) -> Result<Value, ChatEncodeError> {
    let mut body = json!({
        "model": upstream_model,
        "messages": encode_messages(&req.input, dialect),
    });

    let obj = body.as_object_mut().expect("chat request object");
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
        let client_field = req
            .extra_body
            .get(CHAT_MAX_TOKENS_FIELD_EXTRA_KEY)
            .and_then(Value::as_str)
            .filter(|field| matches!(*field, "max_tokens" | "max_completion_tokens"));
        let key = match (dialect, client_field) {
            (ChatDialect::DeepSeek, _) => "max_tokens",
            (_, Some(field)) => field,
            (ChatDialect::OpenRouter | ChatDialect::Vercel, None) => "max_tokens",
            (ChatDialect::OpenAi | ChatDialect::Generic, None) => "max_completion_tokens",
        };
        obj.insert(key.to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning {
        encode_reasoning_controls(obj, reasoning, dialect);
    }

    encode_tool_controls(obj, req, dialect)?;
    if let Some(stop) = &req.stop {
        let value = match stop {
            StopControl::Single(stop) => Value::String(stop.clone()),
            StopControl::Multiple(stops) => Value::Array(
                stops
                    .iter()
                    .map(|stop| Value::String(stop.clone()))
                    .collect(),
            ),
        };
        obj.insert("stop".to_string(), value);
    }
    if let Some(verbosity) = &req.verbosity {
        obj.insert("verbosity".to_string(), Value::String(verbosity.clone()));
    }
    if let Some(format) = &req.response_format {
        obj.insert(
            "response_format".to_string(),
            encode_response_format(format),
        );
    }
    if let Some(user) = &req.user {
        obj.insert("user".to_string(), Value::String(user.clone()));
    }

    merge_chat_wire_extra(obj, &req.extra_body);
    if dialect != ChatDialect::Vercel {
        obj.remove("providerOptions");
    }

    if req.stream == Some(true) {
        match obj.get_mut("stream_options") {
            Some(Value::Object(options)) => {
                options.insert("include_usage".to_string(), Value::Bool(true));
            }
            _ => {
                obj.insert("stream_options".to_string(), json!({"include_usage": true}));
            }
        }
    }

    crate::urp::logprobs::encode_request(
        &mut body,
        &req.logprobs,
        ProviderProtocol::ChatCompletion,
    );
    crate::urp::sampling::encode_request(
        &mut body,
        &req.sampling,
        ProviderProtocol::ChatCompletion,
    );
    Ok(body)
}

/// Tool controls accompany encoded tools; without a tool list they pass through as given.
fn encode_tool_controls(
    obj: &mut Map<String, Value>,
    req: &UrpRequest,
    dialect: ChatDialect,
) -> Result<(), ChatEncodeError> {
    let mut dropped_tool_names = Vec::new();
    let mut emit_tool_controls = true;
    if let Some(tools) = &req.tools {
        let (modern_tools, dropped) = encode_tools(tools, dialect)?;
        dropped_tool_names = dropped;
        let legacy_functions = encode_legacy_functions(tools);
        if !modern_tools.is_empty() {
            obj.insert("tools".to_string(), Value::Array(modern_tools));
        }
        if !legacy_functions.is_empty() {
            obj.insert("functions".to_string(), Value::Array(legacy_functions));
        }
        emit_tool_controls = obj.contains_key("tools") || obj.contains_key("functions");
    }
    if emit_tool_controls {
        if let Some(tc) = &req.tool_choice {
            if let Some(raw_legacy_choice) =
                req.extra_body.get(CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY)
            {
                if let Some(choice) = encode_legacy_function_choice(tc, raw_legacy_choice) {
                    obj.insert("function_call".to_string(), choice);
                }
            } else if tool_choice_names_any(tc, &dropped_tool_names) {
                obj.insert("tool_choice".to_string(), json!("auto"));
            } else {
                obj.insert("tool_choice".to_string(), tool_choice_to_chat_value(tc));
            }
        }
        if let Some(parallel) = req.parallel_tool_calls {
            obj.insert("parallel_tool_calls".to_string(), Value::Bool(parallel));
        }
    }
    Ok(())
}

/// RC4 for Chat targets, by dialect.
fn encode_reasoning_controls(
    obj: &mut Map<String, Value>,
    reasoning: &ReasoningConfig,
    dialect: ChatDialect,
) {
    let native_reasoning = reasoning
        .extra_body
        .get(CHAT_REASONING_CONFIG_EXTRA_KEY)
        .and_then(Value::as_object)
        .filter(|_| dialect.reasoning_details());
    let native_thinking = reasoning
        .extra_body
        .get(CHAT_THINKING_CONFIG_EXTRA_KEY)
        .and_then(Value::as_object);
    let effort = if reasoning.disabled() {
        Some("none")
    } else {
        reasoning.effort.as_deref()
    }
    .map(chat_wire_effort);
    // A native `thinking` object owns mode and budget.
    let budget = reasoning
        .budget_tokens
        .filter(|_| native_thinking.is_none());
    let mode = reasoning
        .mode
        .as_deref()
        .filter(|_| native_thinking.is_none());

    if let Some(native) = native_reasoning {
        let mut controls = native.clone();
        controls.retain(|key, _| {
            !matches!(
                key.as_str(),
                "effort" | "summary" | "max_tokens" | "enabled"
            )
        });
        if let Some(effort) = effort {
            controls.insert("effort".into(), json!(effort));
        } else if let Some(budget) = budget {
            controls.insert("max_tokens".into(), json!(budget));
        }
        if let Some(summary) = &reasoning.summary {
            controls.insert("summary".into(), json!(summary));
        }
        if let Some(mode) = mode {
            controls.insert("enabled".into(), json!(mode != "disabled"));
        }
        obj.insert("reasoning".into(), Value::Object(controls));
    } else if let Some(effort) = effort {
        if dialect == ChatDialect::DeepSeek && effort == "none" {
            // DeepSeek toggles thinking only via `thinking.type`; `reasoning_effort` takes
            // low/high/max.
            if native_thinking.is_none() {
                obj.insert("thinking".into(), json!({ "type": "disabled" }));
            }
        } else {
            obj.insert("reasoning_effort".into(), json!(effort));
        }
    } else if matches!(dialect, ChatDialect::OpenRouter | ChatDialect::Vercel) {
        if let Some(budget) = budget {
            obj.insert("reasoning".into(), json!({ "max_tokens": budget }));
        } else if mode.is_some_and(|mode| mode != "disabled") {
            obj.insert("reasoning".into(), json!({ "enabled": true }));
        }
    }

    // OpenAI Chat defines no `thinking` parameter.
    if let Some(native) = native_thinking.filter(|_| dialect != ChatDialect::OpenAi) {
        let mut thinking = native.clone();
        thinking.retain(|key, _| !matches!(key.as_str(), "type" | "budget_tokens" | "display"));
        if reasoning.disabled() {
            thinking.insert("type".into(), json!("disabled"));
        } else if let Some(mode) = &reasoning.mode {
            thinking.insert("type".into(), json!(mode));
        }
        if let Some(budget) = reasoning.budget_tokens {
            thinking.insert("budget_tokens".into(), json!(budget));
        }
        if let Some(display) = &reasoning.display {
            thinking.insert("display".into(), json!(display));
        }
        obj.insert("thinking".into(), Value::Object(thinking));
    }
}

fn chat_wire_effort(effort: &str) -> &str {
    if effort == "minimum" {
        "minimal"
    } else {
        effort
    }
}

fn encode_response_format(format: &ResponseFormat) -> Value {
    match format {
        ResponseFormat::Text => json!({ "type": "text" }),
        ResponseFormat::JsonObject => json!({ "type": "json_object" }),
        ResponseFormat::JsonSchema { json_schema } => {
            let mut schema_obj = Map::new();
            schema_obj.insert("name".to_string(), Value::String(json_schema.name.clone()));
            schema_obj.insert("schema".to_string(), json_schema.schema.clone());
            if let Some(desc) = &json_schema.description {
                schema_obj.insert("description".to_string(), Value::String(desc.clone()));
            }
            if let Some(strict) = json_schema.strict {
                schema_obj.insert("strict".to_string(), Value::Bool(strict));
            }
            merge_chat_wire_extra(&mut schema_obj, &json_schema.extra_body);
            json!({
                "type": "json_schema",
                "json_schema": Value::Object(schema_obj),
            })
        }
    }
}
