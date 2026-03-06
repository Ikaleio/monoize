use super::*;

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
pub(super) fn check_ip_whitelist(
    auth: &crate::auth::AuthResult,
    headers: &HeaderMap,
) -> AppResult<()> {
    if auth.ip_whitelist.is_empty() {
        return Ok(());
    }
    let client_ip = extract_client_ip(headers).unwrap_or_default();
    if client_ip.is_empty() || !auth.ip_whitelist.iter().any(|allowed| allowed == &client_ip) {
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
        req.extra_body.insert("__monoize_username".to_string(), json!(username.clone()));
    }
}

pub(super) fn strip_monoize_context(req: &mut urp::UrpRequest) {
    req.extra_body.remove("__monoize_username");
}

pub(super) async fn apply_transform_rules_request(
    state: &AppState,
    req: &mut urp::UrpRequest,
    rules: &[TransformRuleConfig],
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
    let model = req.model.clone();
    let context = transforms::TransformRuntimeContext {
        image_transform_cache: state.image_transform_cache.clone(),
    };
    transforms::apply_transforms(
        transforms::UrpData::Request(req),
        rules,
        &mut states,
        &model,
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

pub(super) fn typed_request_to_legacy(
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

pub(super) fn build_embeddings_routing_stub(model: &str, max_multiplier: Option<f64>) -> UrpRequest {
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

pub(super) fn has_enabled_response_rules(rules: &[TransformRuleConfig], model: &str) -> bool {
    rules
        .iter()
        .filter(|rule| rule.enabled && rule.phase == Phase::Response)
        .any(|rule| match &rule.models {
            None => true,
            Some(patterns) => patterns
                .iter()
                .any(|pattern| model_glob_match(pattern, model)),
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

pub(super) fn ensure_stream_usage_requested(req: &mut urp::UrpRequest, provider_type: ProviderType) {
    if req.stream != Some(true) || provider_type != ProviderType::ChatCompletion {
        return;
    }
    match req.extra_body.get_mut("stream_options") {
        Some(Value::Object(stream_options)) => {
            stream_options
                .entry("include_usage".to_string())
                .or_insert(Value::Bool(true));
        }
        Some(_) => {}
        None => {
            req.extra_body
                .insert("stream_options".to_string(), json!({"include_usage": true}));
        }
    }
}
