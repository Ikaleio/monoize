use super::*;

pub async fn count_messages_tokens(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let request_id = messages_wire_request_id(&canonical_request_id(&headers));
    let result = async {
        let auth = auth_tenant(&headers, &state).await?;
        let crate::payload::JsonBody(body) =
            crate::payload::JsonBody::from_request(request, &state).await?;
        let downstream = DownstreamProtocol::AnthropicMessages;
        let mut req = decode_urp_body(downstream, &body)?;
        if req.model.trim().is_empty() {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "missing model",
            ));
        }
        req.stream = Some(false);
        req.context.anthropic_client_headers = anthropic_client_headers(&headers);
        forward_count(&state, &headers, &auth, downstream, req).await
    }
    .await;
    let mut response =
        result.unwrap_or_else(|err| anthropic_error_response(err, Some(&request_id)));
    insert_messages_request_id_header(&mut response, &request_id);
    response
}

pub(super) async fn forward_count(
    state: &AppState,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
    downstream: DownstreamProtocol,
    mut req: urp::UrpRequest,
) -> AppResult<Response> {
    apply_configured_model_redirects_to_model(state, &mut req.model, auth).await;
    ensure_model_allowed(auth, &req.model)?;
    let max_multiplier = resolve_max_multiplier(&req, headers, auth);
    resolve_model_suffix(state, &mut req).await?;
    let routing = build_routing_stub(&req, max_multiplier);
    let mut attempts = build_count_attempts(state, &routing, auth).await?;
    attach_client_session_id(
        &mut attempts,
        extract_client_session_id(headers),
        Some(&req),
    );
    bind_media_request_routes(&mut req, &mut attempts)?;
    let mut attempts = attempts.into_iter();
    let mut attempt = attempts.next().ok_or_else(|| {
        AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!(
                "token counting is not available for model {}",
                routing.model
            ),
        )
    })?;
    let mask = state.monoize_runtime.read().await.mask_sensitive_info;
    let mut original = Some(req);
    loop {
        let req = attempt_request_from_original(&mut original, attempts.len() == 0);
        match count_attempt(state, auth, downstream, req, &attempt, mask).await {
            Ok(body) => return Ok(Json(body).into_response()),
            Err(err) => match attempts.next() {
                Some(next) => attempt = next,
                None => return Err(err),
            },
        }
    }
}

async fn count_attempt(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    downstream: DownstreamProtocol,
    mut req: urp::UrpRequest,
    attempt: &MonoizeAttempt,
    mask: bool,
) -> AppResult<Value> {
    shape_request_for_attempt(&mut req, attempt, auth, downstream)?;
    let mut body = encode_request_for_provider(&mut req, attempt, downstream)?;
    let gemini = attempt.provider_type == ProviderType::Gemini;
    let count_request = body
        .as_object_mut()
        .expect("request encoders emit an object");
    let path = if gemini {
        count_request.retain(|key, _| {
            matches!(
                key.as_str(),
                "contents"
                    | "tools"
                    | "toolConfig"
                    | "safetySettings"
                    | "systemInstruction"
                    | "generationConfig"
                    | "cachedContent"
                    | "serviceTier"
                    | "continuationToken"
                    | "labels"
                    | "store"
            )
        });
        let model = gemini_upstream_model(&attempt.upstream_model);
        count_request.insert("model".to_owned(), Value::String(format!("models/{model}")));
        body = Value::Object(Map::from_iter([(
            "generateContentRequest".to_owned(),
            body,
        )]));
        gemini_model_path(model, "countTokens")
    } else {
        count_request.retain(|key, _| {
            matches!(
                key.as_str(),
                "model"
                    | "messages"
                    | "system"
                    | "tools"
                    | "tool_choice"
                    | "thinking"
                    | "output_config"
                    | "cache_control"
                    | "speed"
                    | "context_management"
                    | "mcp_servers"
            )
        });
        "/v1/messages/count_tokens".to_owned()
    };
    let extra_headers = attempt_extra_headers(attempt, &body);
    let provider = build_channel_provider_config(attempt);
    let http = client_http_for_attempt(state, attempt)?;
    let encoded = upstream::JsonBody::new(&body);
    drop(body);
    let mut value = upstream::call_upstream_with_timeout_and_headers(
        &http,
        &provider,
        &attempt.api_key,
        &path,
        encoded,
        attempt.request_timeout_ms,
        &extra_headers,
    )
    .await
    .map_err(|err| upstream_error_to_app(err, mask))?;
    let field = if gemini {
        "totalTokens"
    } else {
        "input_tokens"
    };
    let count = value.get(field).and_then(Value::as_u64).ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "invalid_upstream_response",
            "upstream response has no valid token count",
        )
    })?;
    if matches!(downstream, DownstreamProtocol::Gemini) {
        let mut body = json!({"totalTokens": count});
        if gemini {
            let members = value
                .as_object_mut()
                .expect("count response is a JSON object");
            for key in [
                "cachedContentTokenCount",
                "promptTokensDetails",
                "cacheTokensDetails",
            ] {
                if let Some(value) = members.remove(key) {
                    body[key] = value;
                }
            }
        }
        Ok(body)
    } else {
        Ok(json!({"input_tokens": count}))
    }
}
