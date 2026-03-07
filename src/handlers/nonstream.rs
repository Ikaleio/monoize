use super::*;

pub(super) async fn execute_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<f64>,
    request_id: Option<String>,
    request_ip: Option<String>,
) -> AppResult<(urp::UrpResponse, String)> {
    let started_at = std::time::Instant::now();
    inject_monoize_context(auth, &mut req);
    apply_transform_rules_request(state, &mut req, &auth.transforms).await?;
    strip_monoize_context(&mut req);
    resolve_model_suffix(state, &mut req).await;
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let attempts = build_monoize_attempts(state, &routing_stub).await?;
    insert_pending_request_log(
        state,
        auth,
        &req.model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await;
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    for attempt in attempts {
        let mut req_attempt = req.clone();
        req_attempt.model = attempt.upstream_model.clone();
        apply_transform_rules_request(state, &mut req_attempt, &attempt.provider_transforms).await?;

        let upstream_body = encode_request_for_provider(&req_attempt, &attempt)?;
        let provider = build_channel_provider_config(&attempt);
        let path = upstream_path_for_model(
            attempt.provider_type,
            &req_attempt.model,
            req_attempt.stream.unwrap_or(false),
        );
        log_outgoing_request_shape(
            request_id.as_deref(),
            &req.model,
            &req_attempt.model,
            attempt.provider_type,
            req_attempt.stream.unwrap_or(false),
            &path,
            &upstream_body,
            &req_attempt,
        );
        let call = upstream::call_upstream_with_timeout_and_headers(
            client_http(state),
            &provider,
            &attempt.api_key,
            &path,
            &upstream_body,
            attempt.request_timeout_ms,
            provider_extra_headers(attempt.provider_type),
        )
        .await;
        match call {
            Ok(value) => {
                update_pending_channel_info(
                    state,
                    auth,
                    &attempt,
                    &req.model,
                    false,
                    request_id.as_deref(),
                    request_ip.as_deref(),
                    started_at,
                )
                .await;
                mark_channel_success(state, &attempt).await;
                let mut resp = decode_response_from_provider(attempt.provider_type, &value)?;
                apply_transform_rules_response(
                    state,
                    &mut resp,
                    &attempt.provider_transforms,
                    &req.model,
                )
                .await?;
                apply_transform_rules_response(state, &mut resp, &auth.transforms, &req.model)
                    .await?;
                let charge =
                    maybe_charge_response(state, auth, &attempt, &req.model, &resp).await?;
                spawn_request_log(
                    state,
                    auth,
                    &attempt,
                    &req.model,
                    resp.usage.clone(),
                    charge.charge_nano_usd,
                    charge.billing_breakdown,
                    false,
                    started_at,
                    request_id.clone(),
                    request_ip.clone(),
                    attempt.channel_id.clone(),
                    None,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                );
                return Ok((resp, req.model.clone()));
            }
            Err(err) => {
                let non_retryable = is_non_retryable_client_error(&err);
                let retryable = is_retryable_error(&err);
                let retryable_failure_class = classify_retryable_failure(&err);
                let app_err = upstream_error_to_app(err);
                if non_retryable {
                    spawn_request_log_error(
                        state,
                        auth,
                        &attempt,
                        &req.model,
                        false,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        &app_err,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                    );
                    return Err(app_err);
                }
                if retryable {
                    tried_providers.push(TriedProvider {
                        provider_id: attempt.provider_id.clone(),
                        channel_id: attempt.channel_id.clone(),
                        error: app_err.message.clone(),
                    });
                    mark_channel_retryable_failure(state, &attempt, retryable_failure_class).await;
                    last_failed_attempt = Some(attempt.clone());
                    continue;
                }
                spawn_request_log_error(
                    state,
                    auth,
                    &attempt,
                    &req.model,
                    false,
                    started_at,
                    request_id.clone(),
                    request_ip.clone(),
                    &app_err,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                );
                return Err(app_err);
            }
        }
    }
    let final_err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        build_exhausted_error_message(&req.model, &tried_providers),
    );
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            state,
            auth,
            &attempt,
            &req.model,
            false,
            started_at,
            request_id,
            request_ip,
            &final_err,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
        );
    } else {
        spawn_request_log_error_no_attempt(
            state,
            auth,
            &req.model,
            false,
            started_at,
            request_id,
            request_ip,
            &final_err,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
        );
    }
    Err(final_err)
}

pub(super) async fn forward_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: urp::UrpRequest,
    max_multiplier: Option<f64>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
) -> AppResult<Value> {
    let (resp, logical_model) =
        execute_nonstream_typed(state, auth, req, max_multiplier, request_id, request_ip).await?;
    Ok(encode_response_for_downstream(
        downstream,
        &resp,
        &logical_model,
    ))
}

pub(super) fn encode_request_for_provider(
    req: &urp::UrpRequest,
    attempt: &MonoizeAttempt,
) -> AppResult<Value> {
    let value = match attempt.provider_type {
        ProviderType::Responses => urp::encode::openai_responses::encode_request(req, &req.model),
        ProviderType::ChatCompletion => urp::encode::openai_chat::encode_request(req, &req.model),
        ProviderType::Messages => urp::encode::anthropic::encode_request(req, &req.model),
        ProviderType::Gemini => urp::encode::gemini::encode_request(req, &req.model),
        ProviderType::Grok => urp::encode::grok::encode_request(req, &req.model),
        ProviderType::Group => {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "provider_type_not_supported",
                "group is virtual",
            ));
        }
    };
    Ok(value)
}

pub(super) fn decode_response_from_provider(
    provider_type: ProviderType,
    value: &Value,
) -> AppResult<urp::UrpResponse> {
    let decoded = match provider_type {
        ProviderType::Responses => urp::decode::openai_responses::decode_response(value),
        ProviderType::ChatCompletion => urp::decode::openai_chat::decode_response(value),
        ProviderType::Messages => urp::decode::anthropic::decode_response(value),
        ProviderType::Gemini => urp::decode::gemini::decode_response(value),
        ProviderType::Grok => urp::decode::grok::decode_response(value),
        ProviderType::Group => Err("provider_type group is virtual".to_string()),
    };
    decoded.map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "invalid_upstream_response", e))
}

pub(super) fn encode_response_for_downstream(
    downstream: DownstreamProtocol,
    resp: &urp::UrpResponse,
    logical_model: &str,
) -> Value {
    match downstream {
        DownstreamProtocol::Responses => {
            urp::encode::openai_responses::encode_response(resp, logical_model)
        }
        DownstreamProtocol::ChatCompletions => {
            urp::encode::openai_chat::encode_response(resp, logical_model)
        }
        DownstreamProtocol::AnthropicMessages => {
            urp::encode::anthropic::encode_response(resp, logical_model)
        }
    }
}
