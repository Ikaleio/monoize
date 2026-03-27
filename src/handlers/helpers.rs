use super::*;
use crate::transforms::split_sse_frames::DEFAULT_MAX_FRAME_LENGTH;

#[allow(clippy::result_large_err)]
pub(super) fn decode_urp_request(
    protocol: DownstreamProtocol,
    known: Value,
    extra: Map<String, Value>,
) -> AppResult<urp::UrpRequest> {
    let merged = merge_known_and_extra(known, extra);
    let decoded = match protocol {
        DownstreamProtocol::Responses => urp::decode::openai_responses::decode_request(&merged),
        DownstreamProtocol::ChatCompletions => urp::decode::openai_chat::decode_request(&merged),
        DownstreamProtocol::AnthropicMessages => urp::decode::anthropic::decode_request(&merged),
    };
    decoded.map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))
}

pub(super) fn merge_known_and_extra(known: Value, extra: Map<String, Value>) -> Value {
    let mut obj = known.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        obj.insert(k, v);
    }
    Value::Object(obj)
}

pub(super) fn resolve_max_multiplier(
    req: &urp::UrpRequest,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<f64> {
    let ceiling = auth.max_multiplier;
    let requested =
        read_max_multiplier_from_extra(req).or_else(|| parse_max_multiplier_header(headers));

    match (ceiling, requested) {
        (Some(c), Some(r)) => Some(r.min(c)),
        (Some(c), None) => Some(c),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

pub(super) fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
        })
}

/// Reject the request if the API key has an IP whitelist and the client IP is not in it.
#[allow(clippy::result_large_err)]
pub(super) fn check_ip_whitelist(
    auth: &crate::auth::AuthResult,
    headers: &HeaderMap,
) -> AppResult<()> {
    if auth.ip_whitelist.is_empty() {
        return Ok(());
    }
    let client_ip = extract_client_ip(headers).unwrap_or_default();
    if client_ip.is_empty()
        || !auth
            .ip_whitelist
            .iter()
            .any(|allowed| allowed == &client_ip)
    {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "ip_not_allowed",
            "client IP is not in the API key whitelist",
        ));
    }
    Ok(())
}

pub(super) fn extract_request_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct UserImageRequestMetrics {
    pub image_parts: usize,
    pub base64_parts: usize,
    pub url_parts: usize,
    pub base64_chars: usize,
    pub estimated_decoded_bytes: usize,
}

pub(super) fn summarize_user_image_request_metrics(
    req: &urp::UrpRequest,
) -> UserImageRequestMetrics {
    let mut metrics = UserImageRequestMetrics::default();
    for item in &req.inputs {
        let urp::Item::Message { role, parts, .. } = item else {
            continue;
        };
        if *role != urp::Role::User {
            continue;
        }
        for part in parts {
            let urp::Part::Image { source, .. } = part else {
                continue;
            };
            metrics.image_parts += 1;
            match source {
                urp::ImageSource::Base64 { data, .. } => {
                    metrics.base64_parts += 1;
                    metrics.base64_chars += data.len();
                    metrics.estimated_decoded_bytes += estimate_base64_decoded_bytes(data);
                }
                urp::ImageSource::Url { url, .. } => {
                    if let Some(data) = extract_base64_data_url_payload(url) {
                        metrics.base64_parts += 1;
                        metrics.base64_chars += data.len();
                        metrics.estimated_decoded_bytes += estimate_base64_decoded_bytes(data);
                    } else {
                        metrics.url_parts += 1;
                    }
                }
            }
        }
    }
    metrics
}

pub(super) fn encoded_json_size_bytes(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn log_outgoing_request_shape(
    request_id: Option<&str>,
    downstream_model: &str,
    upstream_model: &str,
    provider_type: ProviderType,
    stream: bool,
    upstream_path: &str,
    upstream_body: &Value,
    req: &urp::UrpRequest,
) {
    let image_metrics = summarize_user_image_request_metrics(req);
    tracing::info!(
        request_id = request_id.unwrap_or(""),
        downstream_model = %downstream_model,
        upstream_model = %upstream_model,
        provider_type = ?provider_type,
        stream,
        upstream_path = %upstream_path,
        upstream_json_bytes = encoded_json_size_bytes(upstream_body),
        user_image_parts = image_metrics.image_parts,
        user_image_base64_parts = image_metrics.base64_parts,
        user_image_url_parts = image_metrics.url_parts,
        user_image_base64_chars = image_metrics.base64_chars,
        user_image_estimated_decoded_bytes = image_metrics.estimated_decoded_bytes,
        "forwarding request shape"
    );
}

fn estimate_base64_decoded_bytes(data: &str) -> usize {
    let trimmed = data.trim_end_matches('=');
    (trimmed.len() / 4) * 3
        + match trimmed.len() % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        }
}

fn extract_base64_data_url_payload(url: &str) -> Option<&str> {
    let payload = url.strip_prefix("data:")?;
    let (meta, data) = payload.split_once(',')?;
    if !meta.ends_with(";base64") || data.is_empty() {
        return None;
    }
    Some(data)
}

pub(super) fn read_max_multiplier_from_extra(req: &urp::UrpRequest) -> Option<f64> {
    req.extra_body
        .get("max_multiplier")
        .and_then(|v| {
            v.as_f64().or_else(|| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|n| n.is_finite())
            })
        })
        .filter(|n| *n > 0.0)
}

pub(super) fn inject_monoize_context(auth: &crate::auth::AuthResult, req: &mut urp::UrpRequest) {
    if let Some(username) = &auth.username {
        req.extra_body
            .insert("__monoize_username".to_string(), json!(username.clone()));
    }
}

pub(super) fn strip_monoize_context(req: &mut urp::UrpRequest) {
    req.extra_body.remove("__monoize_username");
}

pub(super) async fn apply_transform_rules_request(
    state: &AppState,
    req: &mut urp::UrpRequest,
    rules: &[TransformRuleConfig],
    match_model: &str,
) -> AppResult<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let mut states = transforms::build_states_for_rules(rules, state.transform_registry.as_ref())
        .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_init_failed",
            e.to_string(),
        )
    })?;
    let context = transforms::TransformRuntimeContext {
        image_transform_cache: state.image_transform_cache.clone(),
        http_client: state.http.clone(),
    };
    transforms::apply_transforms(
        transforms::UrpData::Request(req),
        rules,
        &mut states,
        match_model,
        Phase::Request,
        &context,
        state.transform_registry.as_ref(),
    )
    .await
    .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_apply_failed",
            e.to_string(),
        )
    })
}

pub(super) async fn apply_transform_rules_response(
    state: &AppState,
    resp: &mut urp::UrpResponse,
    rules: &[TransformRuleConfig],
    model: &str,
) -> AppResult<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let mut states = transforms::build_states_for_rules(rules, state.transform_registry.as_ref())
        .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_init_failed",
            e.to_string(),
        )
    })?;
    let context = transforms::TransformRuntimeContext {
        image_transform_cache: state.image_transform_cache.clone(),
        http_client: state.http.clone(),
    };
    transforms::apply_transforms(
        transforms::UrpData::Response(resp),
        rules,
        &mut states,
        model,
        Phase::Response,
        &context,
        state.transform_registry.as_ref(),
    )
    .await
    .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_apply_failed",
            e.to_string(),
        )
    })
}

pub(super) async fn transform_urp_stream(
    state: &AppState,
    mut rx: mpsc::Receiver<urp::UrpStreamEvent>,
    tx: mpsc::Sender<urp::UrpStreamEvent>,
    provider_rules: &[TransformRuleConfig],
    auth_rules: &[TransformRuleConfig],
    model: &str,
) -> AppResult<()> {
    let mut provider_states =
        transforms::build_states_for_rules(provider_rules, state.transform_registry.as_ref())
            .map_err(|e| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "transform_init_failed",
                    e.to_string(),
                )
            })?;
    let mut auth_states =
        transforms::build_states_for_rules(auth_rules, state.transform_registry.as_ref()).map_err(
            |e| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "transform_init_failed",
                    e.to_string(),
                )
            },
        )?;
    let context = transforms::TransformRuntimeContext {
        image_transform_cache: state.image_transform_cache.clone(),
        http_client: state.http.clone(),
    };

    while let Some(event) = rx.recv().await {
        let provider_events = transforms::apply_stream_transforms(
            event,
            provider_rules,
            &mut provider_states,
            model,
            Phase::Response,
            &context,
            state.transform_registry.as_ref(),
        )
        .await
        .map_err(|e| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transform_apply_failed",
                e.to_string(),
            )
        })?;

        for provider_event in provider_events {
            let auth_events = transforms::apply_stream_transforms(
                provider_event,
                auth_rules,
                &mut auth_states,
                model,
                Phase::Response,
                &context,
                state.transform_registry.as_ref(),
            )
            .await
            .map_err(|e| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "transform_apply_failed",
                    e.to_string(),
                )
            })?;

            for auth_event in auth_events {
                tx.send(auth_event).await.map_err(|_| {
                    AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "stream_transform_failed",
                        "failed to forward transformed stream event",
                    )
                })?;
            }
        }
    }

    Ok(())
}

#[allow(clippy::result_large_err)]
pub(crate) fn typed_request_to_legacy(
    req: &urp::UrpRequest,
    max_multiplier: Option<f64>,
) -> AppResult<UrpRequest> {
    let encoded = urp::encode::openai_responses::encode_request(req, &req.model);
    let mut extra = Map::new();
    if let Some(limit) = max_multiplier {
        extra.insert("max_multiplier".to_string(), Value::from(limit));
    }
    parse_urp_request(&encoded, extra)
}

pub(super) fn build_routing_stub(req: &urp::UrpRequest, max_multiplier: Option<f64>) -> UrpRequest {
    UrpRequest {
        model: req.model.clone(),
        max_multiplier,
    }
}

pub(super) fn build_embeddings_routing_stub(
    model: &str,
    max_multiplier: Option<f64>,
) -> UrpRequest {
    UrpRequest {
        model: model.to_string(),
        max_multiplier,
    }
}

pub(super) fn is_valid_embeddings_input(input: &Value) -> bool {
    if input.as_str().is_some() {
        return true;
    }
    input
        .as_array()
        .is_some_and(|arr| arr.iter().all(|item| item.as_str().is_some()))
}

pub(super) fn read_max_multiplier_from_embeddings_body(body: &Value) -> Option<f64> {
    body.as_object()
        .and_then(|obj| obj.get("max_multiplier"))
        .and_then(|v| {
            v.as_f64().or_else(|| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|n| n.is_finite())
            })
        })
        .filter(|n| *n > 0.0)
}

pub(super) fn resolve_max_multiplier_for_embeddings(
    body: &Value,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<f64> {
    let ceiling = auth.max_multiplier;
    let requested = read_max_multiplier_from_embeddings_body(body)
        .or_else(|| parse_max_multiplier_header(headers));

    match (ceiling, requested) {
        (Some(c), Some(r)) => Some(r.min(c)),
        (Some(c), None) => Some(c),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

pub(super) fn effective_sse_max_frame_length(
    provider_rules: &[TransformRuleConfig],
    auth_rules: &[TransformRuleConfig],
    model: &str,
) -> Option<usize> {
    resolve_sse_max_frame_length_from_rules(provider_rules, model)
        .or_else(|| resolve_sse_max_frame_length_from_rules(auth_rules, model))
}

fn resolve_sse_max_frame_length_from_rules(
    rules: &[TransformRuleConfig],
    model: &str,
) -> Option<usize> {
    rules
        .iter()
        .find(|rule| {
            rule.enabled
                && rule.phase == Phase::Response
                && rule.transform == "split_sse_frames"
                && match &rule.models {
                    None => true,
                    Some(patterns) => patterns
                        .iter()
                        .any(|pattern| model_glob_match(pattern, model)),
                }
        })
        .map(|rule| {
            rule.config
                .get("max_frame_length")
                .and_then(|v| v.as_u64())
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_MAX_FRAME_LENGTH)
        })
}

pub(super) fn requires_buffered_response_stream(
    provider_rules: &[TransformRuleConfig],
    auth_rules: &[TransformRuleConfig],
    model: &str,
    downstream: DownstreamProtocol,
) -> bool {
    provider_rules
        .iter()
        .chain(auth_rules.iter())
        .filter(|rule| rule.enabled && rule.phase == Phase::Response)
        .filter(|rule| match &rule.models {
            None => true,
            Some(patterns) => patterns
                .iter()
                .any(|pattern| model_glob_match(pattern, model)),
        })
        .any(|rule| {
            rule.transform == "assistant_markdown_images_to_output"
                && !matches!(downstream, DownstreamProtocol::Responses)
        })
}

pub(super) fn model_glob_match(pattern: &str, model: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    regex::Regex::new(&regex)
        .map(|re| re.is_match(model))
        .unwrap_or(false)
}

/// Default upstream extra_body field whitelists per provider type.
///
/// Fields that the URP request decoder already extracts into typed struct
/// fields (model, stream, temperature, etc.) are NOT in extra_body at all;
/// these lists cover only the keys that remain in `UrpRequest.extra_body`
/// and are safe to forward to the given upstream API.
const EXTRA_WHITELIST_CHAT_COMPLETION: &[&str] = &[
    "frequency_penalty",
    "logit_bias",
    "logprobs",
    "top_logprobs",
    "max_completion_tokens",
    "max_tokens",
    "metadata",
    "presence_penalty",
    "seed",
    "stop",
    "stream_options",
    "parallel_tool_calls",
    "debug",
    "image_config",
    "modalities",
    "cache_control",
    "top_k",
    "top_a",
    "min_p",
    "repetition_penalty",
    "prediction",
    "route",
    "structured_outputs",
    "verbosity",
    // OpenRouter / third-party extension fields
    "models",
    "provider",
    "plugins",
    "session_id",
    "trace",
];

const EXTRA_WHITELIST_RESPONSES: &[&str] = &[
    "background",
    "context_management",
    "conversation",
    "include",
    "instructions",
    "metadata",
    "max_tool_calls",
    "parallel_tool_calls",
    "previous_response_id",
    "prompt",
    "prompt_cache_key",
    "prompt_cache_retention",
    "safety_identifier",
    "service_tier",
    "store",
    "text",
    "top_logprobs",
    "truncation",
];

const EXTRA_WHITELIST_ANTHROPIC: &[&str] = &[
    "max_tokens",
    "metadata",
    "output_config",
    "service_tier",
    "stop_sequences",
    "top_k",
    "inference_geo",
];

const EXTRA_WHITELIST_GEMINI: &[&str] = &[
    "generationConfig",
    "safetySettings",
    "cachedContent",
    "labels",
];

fn default_extra_whitelist(provider_type: ProviderType) -> &'static [&'static str] {
    match provider_type {
        ProviderType::ChatCompletion => EXTRA_WHITELIST_CHAT_COMPLETION,
        ProviderType::Responses => EXTRA_WHITELIST_RESPONSES,
        ProviderType::Messages => EXTRA_WHITELIST_ANTHROPIC,
        ProviderType::Gemini => EXTRA_WHITELIST_GEMINI,
        ProviderType::Group => &[],
    }
}

/// Filter `req.extra_body` to only contain fields allowed by the upstream
/// provider type's whitelist, optionally extended by a provider-level override.
///
/// If `provider_override` contains `"*"`, all fields pass through unfiltered.
pub(super) fn filter_extra_body_for_provider(
    req: &mut urp::UrpRequest,
    provider_type: ProviderType,
    provider_override: &Option<Vec<String>>,
) {
    if let Some(overrides) = provider_override {
        if overrides.iter().any(|s| s == "*") {
            return;
        }
    }

    let defaults = default_extra_whitelist(provider_type);
    let override_set: HashSet<&str> = provider_override
        .as_ref()
        .map(|v| v.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    req.extra_body
        .retain(|k, _| defaults.contains(&k.as_str()) || override_set.contains(k.as_str()));
}



#[cfg(test)]
mod tests {
    use super::*;

    fn response_rule(transform: &str) -> TransformRuleConfig {
        TransformRuleConfig {
            transform: transform.to_string(),
            enabled: true,
            models: None,
            phase: Phase::Response,
            config: json!({}),
        }
    }

    #[test]
    fn assistant_markdown_images_to_output_stays_passthrough_for_responses() {
        assert!(!requires_buffered_response_stream(
            &[response_rule("assistant_markdown_images_to_output")],
            &[],
            "gpt-5-mini",
            DownstreamProtocol::Responses,
        ));
    }

    #[test]
    fn assistant_markdown_images_to_output_still_buffers_for_chat_and_messages() {
        assert!(requires_buffered_response_stream(
            &[response_rule("assistant_markdown_images_to_output")],
            &[],
            "gpt-5-mini",
            DownstreamProtocol::ChatCompletions,
        ));
        assert!(requires_buffered_response_stream(
            &[response_rule("assistant_markdown_images_to_output")],
            &[],
            "gpt-5-mini",
            DownstreamProtocol::AnthropicMessages,
        ));
    }
}
