use super::*;

pub(in crate::handlers) fn decode_urp_request(
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

pub(in crate::handlers) fn merge_known_and_extra(known: Value, extra: Map<String, Value>) -> Value {
    let mut obj = known.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        obj.insert(k, v);
    }
    Value::Object(obj)
}

pub(in crate::handlers) fn resolve_max_multiplier(
    req: &urp::UrpRequest,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<Multiplier> {
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

pub(in crate::handlers) fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    crate::client_ip::canonical_client_ip_from_headers(headers).map(|address| address.to_string())
}

/// Reject the request if the API key has an IP whitelist and the client IP is not in it.
pub(in crate::handlers) fn check_ip_whitelist(
    auth: &crate::auth::AuthResult,
    headers: &HeaderMap,
) -> AppResult<()> {
    if auth.ip_whitelist.is_empty() {
        return Ok(());
    }
    let client_ip = crate::client_ip::canonical_client_ip_from_headers(headers);
    let allowed = client_ip.is_some_and(|client_ip| {
        auth.ip_whitelist.iter().any(|entry| {
            entry
                .parse::<std::net::IpAddr>()
                .is_ok_and(|allowed| allowed == client_ip)
                || entry
                    .parse::<ipnet::IpNet>()
                    .is_ok_and(|network| network.contains(&client_ip))
        })
    });
    if !allowed {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "ip_not_allowed",
            "client IP is not in the API key whitelist",
        ));
    }
    Ok(())
}

pub(in crate::handlers) fn extract_request_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// CM-AFF-1a: read the client-supplied session-affinity value. Underscore and
/// hyphenated aliases are both accepted because some reverse proxies drop
/// header names that contain `_`. OpenCode clients send `x-opencode-session`.
/// Values are sanitized per the shared affinity sanitizer; an empty result
/// means "absent".
pub(in crate::handlers) fn extract_client_session_id(headers: &HeaderMap) -> Option<String> {
    for name in [
        "session_id",
        "session-id",
        "x-session-id",
        "x-opencode-session",
        "x-session-affinity",
    ] {
        if let Some(raw) = headers.get(name).and_then(|value| value.to_str().ok()) {
            let sanitized = crate::handlers::routing::sanitize_session_affinity(raw);
            if !sanitized.is_empty() {
                return Some(sanitized);
            }
        }
    }
    None
}

pub(in crate::handlers) fn read_max_multiplier_from_extra(
    req: &urp::UrpRequest,
) -> Option<Multiplier> {
    req.extra_body
        .get("max_multiplier")
        .and_then(Value::as_str)
        .and_then(parse_positive_multiplier)
}

pub(in crate::handlers) fn inject_monoize_context(
    auth: &crate::auth::AuthResult,
    req: &mut urp::UrpRequest,
) {
    req.context.username = auth.username.clone();
    req.context.api_key_id = auth.api_key_id.clone();
}

pub(in crate::handlers) fn strip_monoize_context(req: &mut urp::UrpRequest) {
    req.context.username = None;
    req.context.api_key_id = None;
}

pub(crate) fn typed_request_to_legacy(
    req: &urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
) -> AppResult<UrpRequest> {
    let mut legacy = build_routing_stub(req, max_multiplier);
    legacy.messages_custom_tool_names = messages_custom_bridge_names(req);
    Ok(legacy)
}

pub(in crate::handlers) fn build_routing_stub(
    req: &urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
) -> UrpRequest {
    UrpRequest {
        audio_output_format: req
            .extra_body
            .get("audio")
            .and_then(|v| v.get("format"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        model: req.model.clone(),
        max_multiplier,
        server_tool_usage_classes: server_tool_usage_classes(req.tools.as_deref()),
        messages_custom_tool_names: HashSet::new(),
        affinity_explicit: stable_affinity_field(req),
        affinity_prefix_hash: affinity_prefix_hash(req),
    }
}

pub(in crate::handlers) fn build_embeddings_routing_stub(
    model: &str,
    max_multiplier: Option<Multiplier>,
) -> UrpRequest {
    UrpRequest {
        audio_output_format: None,
        model: model.to_string(),
        max_multiplier,
        server_tool_usage_classes: Vec::new(),
        messages_custom_tool_names: HashSet::new(),
        affinity_explicit: None,
        affinity_prefix_hash: short_xxh3_hex(model),
    }
}

pub(in crate::handlers) fn is_valid_embeddings_input(input: &Value) -> bool {
    if input.as_str().is_some() {
        return true;
    }
    input
        .as_array()
        .is_some_and(|arr| arr.iter().all(|item| item.as_str().is_some()))
}

pub(in crate::handlers) fn read_max_multiplier_from_embeddings_body(
    body: &Value,
) -> Option<Multiplier> {
    body.as_object()
        .and_then(|obj| obj.get("max_multiplier"))
        .and_then(Value::as_str)
        .and_then(parse_positive_multiplier)
}

pub(in crate::handlers) fn resolve_max_multiplier_for_embeddings(
    body: &Value,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<Multiplier> {
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

/// PR4c.2z and RC4h: controls for chat-shaped clients replaying statelessly through Responses.
pub(in crate::handlers) fn apply_cross_family_responses_controls(req: &mut urp::UrpRequest) {
    req.extra_body
        .entry("include".to_string())
        .or_insert_with(|| json!(["reasoning.encrypted_content"]));
    req.extra_body
        .entry("store".to_string())
        .or_insert(Value::Bool(false));
    let Some(reasoning) = req.reasoning.as_mut() else {
        return;
    };
    let Some(mode) = reasoning.mode.as_deref() else {
        return;
    };
    if !matches!(mode, "enabled" | "adaptive") {
        return;
    }
    if reasoning.effort.is_none()
        && let Some(budget) = reasoning.budget_tokens
    {
        reasoning.effort = Some(
            match budget {
                0..=1024 => "low",
                1025..=4096 => "medium",
                4097..=16384 => "high",
                _ => "xhigh",
            }
            .to_string(),
        );
    }
    if reasoning.summary.is_none() && reasoning.display.as_deref() != Some("omitted") {
        reasoning.summary = Some("auto".to_string());
    }
}

#[cfg(test)]
mod routing_media_context_tests {
    use super::*;

    fn assert_context(request: &urp::UrpRequest, audio_format: Option<&str>) {
        let before = serde_json::to_value(request).unwrap();
        assert!(
            urp::encode::openai_responses::encode_request_checked(request, &request.model).is_err()
        );
        let context = typed_request_to_legacy(request, Some("1.5".parse().unwrap())).unwrap();
        assert_eq!(context.model, request.model);
        assert_eq!(context.audio_output_format.as_deref(), audio_format);
        assert_eq!(context.max_multiplier.unwrap().to_string(), "1.5");
        assert_eq!(serde_json::to_value(request).unwrap(), before);
    }

    #[test]
    fn chat_audio_context_does_not_require_a_responses_media_carrier() {
        for stream in [false, true] {
            let request = urp::decode::openai_chat::decode_request(&json!({
                "model":"chat-audio", "stream":stream, "audio":{"format":"pcm16", "voice":"alloy"},
                "messages":[{"role":"user", "content":[
                    {"type":"input_audio", "input_audio":{"data":"YQ==", "format":"wav"}}
                ]}]
            }))
            .unwrap();
            assert_context(&request, Some("pcm16"));
        }
    }

    #[test]
    fn gemini_private_file_context_does_not_require_a_responses_media_carrier() {
        for stream in [false, true] {
            let mut request = urp::decode::gemini::decode_request(&json!({
                "model":"gemini-files", "contents":[{"role":"user", "parts":[
                    {"fileData":{"mimeType":"application/pdf", "fileUri":"https://generativelanguage.googleapis.com/v1beta/files/source-file"}}
                ]}]
            })).unwrap();
            request.stream = Some(stream);
            assert_context(&request, None);
        }
    }

    #[test]
    fn gemini_audio_context_does_not_require_a_responses_media_carrier() {
        for stream in [false, true] {
            let mut request = urp::decode::gemini::decode_request(&json!({
                "model":"gemini-audio", "contents":[{"role":"user", "parts":[
                    {"inlineData":{"mimeType":"audio/wav", "data":"YQ=="}}
                ]}]
            }))
            .unwrap();
            request.stream = Some(stream);
            assert_context(&request, None);
        }
    }
}
