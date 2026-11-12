use super::*;
use std::collections::HashSet;

fn strip_orphaned_tool_calls(req: &mut urp::UrpRequest) {
    let answered: HashSet<String> = req
        .input
        .iter()
        .filter_map(|node| match node {
            urp::Node::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    req.input.retain_mut(|node| match node {
        urp::Node::ToolCall { call_id, .. } => answered.contains(&*call_id),
        urp::Node::NextDownstreamEnvelopeExtra { .. } => true,
        _ => true,
    });
}

fn has_responses_state_reference(req: &urp::UrpRequest) -> bool {
    ["previous_response_id", "conversation"]
        .into_iter()
        .any(|key| {
            req.extra_body
                .get(key)
                .is_some_and(|value| !value.is_null())
        })
}

pub(in crate::handlers) fn encode_request_for_provider(
    req: &mut urp::UrpRequest,
    attempt: &MonoizeAttempt,
    downstream: DownstreamProtocol,
) -> AppResult<Value> {
    validate_media_request_route(req, attempt)?;
    if matches!(downstream, DownstreamProtocol::Responses)
        && attempt.provider_type != ProviderType::Responses
    {
        req.extra_body.remove("store");
        req.extra_body.remove("conversation");
        req.extra_body.remove("previous_response_id");
    }
    if responses_history::is_managed(req) {
        req.extra_body.remove("previous_response_id");
        if attempt.provider_type == ProviderType::Responses {
            req.extra_body
                .insert("store".to_string(), Value::Bool(false));
        } else {
            req.extra_body.remove("store");
        }
    }
    filter_extra_body_for_provider(
        req,
        attempt.provider_type,
        downstream,
        &attempt.extra_fields_whitelist,
    );
    if attempt.provider_type == ProviderType::Responses
        && !matches!(downstream, DownstreamProtocol::Responses)
    {
        apply_cross_family_responses_controls(req);
    }
    filter_tools_for_provider(req, attempt.provider_type, downstream);
    if attempt.provider_type == ProviderType::Responses
        && req.image_generation.is_some()
        && (attempt.logical_model.starts_with("gpt-image-")
            || attempt.logical_model == "chatgpt-image-latest")
    {
        for tool in req
            .tools
            .iter_mut()
            .flatten()
            .filter(|tool| tool.tool_type == "image_generation")
        {
            if !tool.extra_body.contains_key("model")
                && let Some(config) = tool.config.get_or_insert_with(|| json!({})).as_object_mut()
            {
                config
                    .entry("model")
                    .or_insert_with(|| json!(attempt.logical_model));
            }
        }
    }
    let stateful_same_responses = matches!(downstream, DownstreamProtocol::Responses)
        && attempt.provider_type == ProviderType::Responses
        && has_responses_state_reference(req);
    if !stateful_same_responses {
        strip_orphaned_tool_calls(req);
    }
    let model = req.model.clone();
    let value = match attempt.provider_type {
        ProviderType::Responses => urp::encode::openai_responses::encode_request_checked(
            req, &model,
        )
        .map_err(|message| AppError::new(StatusCode::BAD_REQUEST, "unsupported_media", message))?,
        ProviderType::ChatCompletion => {
            urp::encode::openai_chat::encode_request_checked(req, &model, attempt.chat_dialect)
                .map_err(|error| {
                    AppError::new(StatusCode::BAD_REQUEST, error.code, error.message)
                })?
        }
        ProviderType::Messages => urp::encode::anthropic::encode_request_checked(req, &model)
            .map_err(|message| {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
            })?,
        ProviderType::Gemini => {
            urp::encode::gemini::encode_request_checked(req, &model).map_err(|message| {
                AppError::new(StatusCode::BAD_REQUEST, "unsupported_media", message)
            })?
        }
        ProviderType::OpenaiImage => urp::encode::openai_image::encode_request_checked(req, &model)
            .map_err(|message| {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
            })?,
        ProviderType::OpenrouterImage => urp::encode::openrouter_image::encode_request(req, &model)
            .map_err(|message| {
                AppError::new(StatusCode::BAD_REQUEST, "unsupported_media", message)
            })?,
        ProviderType::Replicate => urp::encode::replicate::encode_request(req, &model),
        ProviderType::Group => {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "provider_type_not_supported",
                "group is virtual",
            ));
        }
        ProviderType::SystemOne | ProviderType::OpenaiDecisions => {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "provider_type_not_supported",
                "decision channels serve only decision endpoints",
            ));
        }
    };
    Ok(value)
}

pub(in crate::handlers) fn decode_response_from_provider(
    provider_type: ProviderType,
    value: &Value,
    model: &str,
    mask_sensitive_info: bool,
    request: &urp::UrpRequest,
) -> AppResult<urp::UrpResponse> {
    if provider_type == ProviderType::ChatCompletion
        && let Some(error) = embedded_chat_completion_error(value)
    {
        return Err(embedded_chat_completion_error_to_app(
            error,
            mask_sensitive_info,
        ));
    }
    if provider_type == ProviderType::ChatCompletion
        && chat_completion_finish_reason_is_error(value)
    {
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_chat_error",
            "upstream Chat Completions response terminated with finish_reason=error",
        )
        .with_type("server_error"));
    }
    let mut decoded = match provider_type {
        ProviderType::Responses => urp::decode::openai_responses::decode_response(value),
        ProviderType::ChatCompletion => urp::decode::openai_chat::decode_response(value),
        ProviderType::Messages => urp::decode::anthropic::decode_response(value),
        ProviderType::Gemini => urp::decode::gemini::decode_response(value),
        ProviderType::OpenaiImage | ProviderType::OpenrouterImage => {
            urp::decode::openai_image::decode_response(value, model)
        }
        ProviderType::Replicate => urp::decode::replicate::decode_response(value),
        ProviderType::Group => Err("provider_type group is virtual".to_string()),
        ProviderType::SystemOne | ProviderType::OpenaiDecisions => {
            Err("decision channels serve only decision endpoints".to_string())
        }
    }
    .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "invalid_upstream_response", e))?;
    if matches!(provider_type, ProviderType::Messages | ProviderType::Gemini) {
        restore_messages_custom_tool_calls(request, &mut decoded);
    }
    if provider_type == ProviderType::ChatCompletion
        && let Some(mime) = request
            .extra_body
            .get("audio")
            .and_then(|v| v.get("format"))
            .and_then(Value::as_str)
            .and_then(urp::media::audio_mime_for_format)
    {
        for node in &mut decoded.output {
            if let urp::Node::Audio {
                source: urp::AudioSource::Base64 { media_type, .. },
                ..
            } = node
                && media_type == "audio/unknown"
            {
                *media_type = mime.into();
            }
        }
    }
    let aliases = tool_namespace_aliases(request);
    for node in &mut decoded.output {
        restore_tool_namespace_node(node, &aliases);
    }
    Ok(decoded)
}

fn embedded_chat_completion_error(value: &Value) -> Option<&Value> {
    value
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            value
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("error"))
                .filter(|error| !error.is_null())
        })
}

fn chat_completion_finish_reason_is_error(value: &Value) -> bool {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        == Some("error")
}

fn embedded_chat_completion_error_to_app(error: &Value, mask_sensitive_info: bool) -> AppError {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.is_empty())
        .unwrap_or("upstream Chat Completions response terminated with an error");
    let metadata = error.get("metadata").and_then(Value::as_object);
    let upstream_code = error.get("code").and_then(json_scalar_string).or_else(|| {
        metadata
            .and_then(|metadata| metadata.get("provider_code"))
            .and_then(json_scalar_string)
    });
    let upstream_status = error
        .get("code")
        .and_then(Value::as_u64)
        .filter(|status| (400..=599).contains(status))
        .and_then(|status| StatusCode::from_u16(status as u16).ok());
    let upstream_type = error.get("type").and_then(json_scalar_string).or_else(|| {
        metadata
            .and_then(|metadata| metadata.get("error_type"))
            .and_then(json_scalar_string)
    });
    let upstream_param = error
        .get("param")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    // SAN-4: the embedded message is upstream-controlled free text — the
    // client sees the masked form while the raw text stays admin-readable
    // via `internal_message`. SAN-CFG5 item 1: masking off disables `MASK`.
    AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_chat_error",
        crate::error_sanitize::maybe_mask_sensitive_text(message, mask_sensitive_info),
    )
    .with_internal_message(crate::error_sanitize::truncate_error_detail(message))
    .with_type("server_error")
    .with_upstream_error(
        upstream_status,
        upstream_code,
        upstream_type,
        upstream_param,
    )
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

/// STR3t: request controls a synthesized Responses object must echo on this attempt.
pub(in crate::handlers) fn responses_request_echo_for_attempt(
    req: &urp::UrpRequest,
    attempt: &MonoizeAttempt,
    downstream: DownstreamProtocol,
) -> Option<Value> {
    (matches!(downstream, DownstreamProtocol::Responses)
        && attempt.provider_type != ProviderType::Responses)
        .then(|| {
            let mut effective = req.clone();
            // The echo speaks Responses, so a mapped upstream web search projects back.
            map_web_search_tools_for_provider(&mut effective, ProviderType::Responses);
            urp::encode::openai_responses::response_request_echo(effective)
        })
}

pub(in crate::handlers) fn encode_response_for_downstream(
    downstream: DownstreamProtocol,
    resp: &urp::UrpResponse,
    logical_model: &str,
) -> AppResult<Value> {
    match downstream {
        DownstreamProtocol::Responses => {
            urp::encode::openai_responses::encode_response_checked(resp, logical_model)
        }
        DownstreamProtocol::ChatCompletions => {
            urp::encode::openai_chat::encode_response_checked(resp, logical_model)
        }
        DownstreamProtocol::AnthropicMessages => {
            urp::encode::anthropic::encode_response_checked(resp, logical_model)
        }
    }
    .map_err(|message| AppError::new(StatusCode::BAD_GATEWAY, "unsupported_output_media", message))
}
