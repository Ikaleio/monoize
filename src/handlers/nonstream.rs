use super::*;
use std::collections::HashSet;

pub(crate) fn strip_orphaned_tool_calls(req: &mut urp::UrpRequest) {
    let answered: HashSet<String> = req.input
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

pub(super) async fn execute_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<f64>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
) -> AppResult<(urp::UrpResponse, String)> {
    let started_at = std::time::Instant::now();
    let requested_model = req.model.clone();
    let transform_match_model =
        normalized_logical_model_for_matching(state, &requested_model).await;
    // Preserve the originally-decoded URP request so each per-attempt iteration
    // can re-derive the transformed request from a pristine base. This matters
    // because cross-family strip runs BEFORE all transforms per-attempt
    // (auto_cache_* etc. must observe the stripped request so their cache
    // breakpoints actually survive into the upstream encoding).
    let original_req = req.clone();
    inject_monoize_context(auth, &mut req);
    apply_transform_rules_request(state, &mut req, &auth.transforms, &transform_match_model)
        .await?;
    strip_monoize_context(&mut req);
    resolve_model_suffix(state, &mut req).await;
    let logical_model = req.model.clone();
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let attempts =
        build_monoize_attempts(state, &routing_stub, auth.effective_groups.clone()).await?;
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
    let mut execution_state = AttemptExecutionState::default();
    for attempt in attempts {
        execution_state.enter_provider(&attempt.provider_id);
        if !execution_state.provider_budget_remaining(&attempt) {
            continue;
        }

        let max_channel_attempts = (attempt.channel_max_retries + 1).max(1) as usize;
        for channel_attempt in 0..max_channel_attempts {
            if !execution_state.provider_budget_remaining(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt();
            // Clone from the pristine original request (pre-transforms) so
            // that the cross-family strip can run BEFORE both api-key and
            // provider transforms. This guarantees that transforms which
            // inject upstream-specific part-level metadata (e.g.
            // `auto_cache_system`, `auto_cache_tool_use`) survive into the
            // encoded upstream request even when the downstream and upstream
            // protocol families differ.
            let mut req_attempt = original_req.clone();
            if attempt.strip_cross_protocol_nested_extra
                && !downstream.is_same_family(attempt.provider_type)
            {
                urp::strip_nested_extra_body(&mut req_attempt.input);
            }
            inject_monoize_context(auth, &mut req_attempt);
            apply_transform_rules_request(
                state,
                &mut req_attempt,
                &auth.transforms,
                &transform_match_model,
            )
            .await?;
            strip_monoize_context(&mut req_attempt);
            req_attempt.model = attempt.upstream_model.clone();
            apply_transform_rules_request(
                state,
                &mut req_attempt,
                &attempt.provider_transforms,
                &transform_match_model,
            )
            .await?;

            let upstream_body = encode_request_for_provider(&mut req_attempt, &attempt)?;
            let provider = build_channel_provider_config(&attempt);
            let path = upstream_path_for_model(
                attempt.provider_type,
                &req_attempt.model,
                req_attempt.stream.unwrap_or(false),
            );
            log_outgoing_request_shape(
                request_id.as_deref(),
                &logical_model,
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
                        &logical_model,
                        false,
                        request_id.as_deref(),
                        request_ip.as_deref(),
                        started_at,
                    )
                    .await;
                    mark_channel_success(state, &attempt).await;
                    let mut resp =
                        decode_response_from_provider(attempt.provider_type, &value, &req_attempt.model)?;
                    apply_transform_rules_response(
                        state,
                        &mut resp,
                        &attempt.provider_transforms,
                        &req.model,
                    )
                    .await?;
                    apply_transform_rules_response(state, &mut resp, &auth.transforms, &req.model)
                        .await?;
                    if attempt.provider_type == ProviderType::OpenaiImage
                        && !matches!(downstream, DownstreamProtocol::Responses)
                    {
                        convert_assistant_images_to_markdown(&mut resp);
                    }
                    let charge =
                        maybe_charge_response(state, auth, &attempt, &logical_model, &resp).await?;
                    spawn_request_log(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        resp.usage.clone(),
                        charge.charge_nano_usd,
                        charge.billing_breakdown,
                        false,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        attempt.channel_id.clone(),
                        None,
                        None,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                    );
                    return Ok((resp, logical_model.clone()));
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
                            &logical_model,
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
                            attempt_number,
                            provider_id: attempt.provider_id.clone(),
                            channel_id: attempt.channel_id.clone(),
                            error: app_err.message.clone(),
                        });
                        mark_channel_retryable_failure(state, &attempt, retryable_failure_class)
                            .await;
                        last_failed_attempt = Some(attempt.clone());
                        if !is_attempt_channel_healthy(state, &attempt).await {
                            break;
                        }
                        if execution_state.provider_budget_remaining(&attempt) {
                            if channel_attempt + 1 < max_channel_attempts {
                                maybe_sleep_before_channel_retry(&attempt).await;
                            }
                            continue;
                        }
                        break;
                    }
                    spawn_request_log_error(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
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
    }
    let final_err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        build_exhausted_error_message(&logical_model, &tried_providers),
    );
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            state,
            auth,
            &attempt,
            &logical_model,
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
            &logical_model,
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
    let (resp, logical_model) = execute_nonstream_typed(
        state,
        auth,
        req,
        max_multiplier,
        downstream,
        request_id,
        request_ip,
    )
    .await?;
    Ok(encode_response_for_downstream(
        downstream,
        &resp,
        &logical_model,
    ))
}

#[allow(clippy::result_large_err)]
pub(super) fn encode_request_for_provider(
    req: &mut urp::UrpRequest,
    attempt: &MonoizeAttempt,
) -> AppResult<Value> {
    filter_extra_body_for_provider(req, attempt.provider_type, &attempt.extra_fields_whitelist);
    strip_orphaned_tool_calls(req);
    let model = req.model.clone();
    let value = match attempt.provider_type {
        ProviderType::Responses => {
            urp::encode::openai_responses::encode_request(req, &model)
        }
        ProviderType::ChatCompletion => urp::encode::openai_chat::encode_request(req, &model),
        ProviderType::Messages => urp::encode::anthropic::encode_request(req, &model),
        ProviderType::Gemini => urp::encode::gemini::encode_request(req, &model),
        ProviderType::OpenaiImage => urp::encode::openai_image::encode_request(req, &model),
        ProviderType::Replicate => urp::encode::replicate::encode_request(req, &model),
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

#[allow(clippy::result_large_err)]
pub(super) fn decode_response_from_provider(
    provider_type: ProviderType,
    value: &Value,
    model: &str,
) -> AppResult<urp::UrpResponse> {
    let decoded = match provider_type {
        ProviderType::Responses => {
            urp::decode::openai_responses::decode_response(value)
        }
        ProviderType::ChatCompletion => urp::decode::openai_chat::decode_response(value),
        ProviderType::Messages => urp::decode::anthropic::decode_response(value),
        ProviderType::Gemini => urp::decode::gemini::decode_response(value),
        ProviderType::OpenaiImage => urp::decode::openai_image::decode_response(value, model),
        ProviderType::Replicate => urp::decode::replicate::decode_response(value),
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
